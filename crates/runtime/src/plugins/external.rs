//! Third-party plugins installed from a git repository.
//!
//! A plugin is a repository with `pockymoe-plugin.json` at its root. Each action
//! runs the manifest's `entry` argv as a child process in the plugin directory:
//! one request JSON on stdin, one JSON value on stdout. Manifest `schedules` run
//! actions at device scope, independent of any thread being busy.
//!
//! Plugins are trusted local code running as the Supervisor's OS user, like a
//! VS Code extension. `permissions` gate what Pockymoe hands the process (its
//! CLI credential), not what the process can do on the machine.
use std::{
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, ensure, Context, Result};
use async_trait::async_trait;
use pockymoe_protocol::PluginDto;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use super::{BuiltinPlugin, PluginActionContext};
use crate::Supervisor;

pub const MANIFEST_FILE: &str = "pockymoe-plugin.json";
const MAX_MANIFEST: u64 = 262144;
const MAX_STDOUT: usize = 262144;
const MAX_STDERR: usize = 65536;
const MIN_SCHEDULE_SECONDS: u64 = 10;
const RUN_HISTORY: usize = 20;
const GIT_TIMEOUT: Duration = Duration::from_secs(300);
/// Reserved for plugins that ship with Pockymoe.
const RESERVED_PREFIXES: &[&str] = &["pockymoe.", "remote-codex."];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Permission {
    /// `POCKYMOE_URL`/`POCKYMOE_TOKEN`: the device owner's CLI credential, which
    /// creates and messages threads, registers hooks and runs `modelComplete`
    /// through the saved upstream connections.
    PockymoeCli,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestAction {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Schedule {
    pub action: String,
    pub every_seconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// argv; a first element containing `/` is relative to the plugin directory.
    pub entry: Vec<String>,
    /// Run once in the plugin directory after install and update (e.g. `uv sync`).
    #[serde(default)]
    pub setup: Vec<String>,
    #[serde(default)]
    pub actions: Vec<ManifestAction>,
    #[serde(default)]
    pub schedules: Vec<Schedule>,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default = "empty_object")]
    pub default_settings: Value,
}

fn default_timeout() -> u64 {
    60
}
fn empty_object() -> Value {
    json!({})
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

impl Manifest {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 1, "unsupported schemaVersion");
        ensure!(
            !self.id.is_empty()
                && self.id.len() <= 128
                && self.id.contains('.')
                && !self.id.starts_with('.')
                && self
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')),
            "plugin id must be a dotted name such as owner.plugin"
        );
        ensure!(
            !RESERVED_PREFIXES.iter().any(|p| self.id.starts_with(p)),
            "plugin id prefix is reserved for built-in plugins"
        );
        ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 80,
            "name must be 1..80 bytes"
        );
        ensure!(
            !self.version.is_empty() && self.version.len() <= 64,
            "version must be 1..64 bytes"
        );
        ensure!(self.description.len() <= 1024, "description exceeds 1 KiB");
        for (field, argv) in [("entry", &self.entry), ("setup", &self.setup)] {
            ensure!(
                argv.len() <= 32
                    && argv.iter().all(|a| a.len() <= 4096 && !a.contains('\0'))
                    && argv.first().is_none_or(|a| !a.is_empty()),
                "invalid {field}"
            );
        }
        ensure!(!self.entry.is_empty(), "entry is required");
        ensure!(self.actions.len() <= 32, "at most 32 actions");
        let mut names = HashSet::new();
        for action in &self.actions {
            ensure!(valid_name(&action.name), "invalid action name");
            ensure!(
                names.insert(&action.name),
                "duplicate action {}",
                action.name
            );
            ensure!(
                (1..=300).contains(&action.timeout_seconds),
                "action timeoutSeconds must be 1..300"
            );
        }
        ensure!(self.schedules.len() <= 8, "at most 8 schedules");
        for schedule in &self.schedules {
            ensure!(
                names.contains(&schedule.action),
                "schedule names unknown action {}",
                schedule.action
            );
            ensure!(
                (MIN_SCHEDULE_SECONDS..=31536000).contains(&schedule.every_seconds),
                "schedule everySeconds must be {MIN_SCHEDULE_SECONDS}..31536000"
            );
        }
        ensure!(
            self.default_settings.is_object()
                && serde_json::to_vec(&self.default_settings)?.len() <= 65536,
            "defaultSettings must be an object up to 64 KiB"
        );
        Ok(())
    }

    pub fn read(dir: &Path) -> Result<Self> {
        let file = std::fs::File::open(dir.join(MANIFEST_FILE))
            .with_context(|| format!("{MANIFEST_FILE} not found at the repository root"))?;
        let mut text = String::new();
        file.take(MAX_MANIFEST + 1).read_to_string(&mut text)?;
        ensure!(
            text.len() as u64 <= MAX_MANIFEST,
            "manifest exceeds 256 KiB"
        );
        let manifest: Self = serde_json::from_str(&text).context("invalid plugin manifest")?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn timeout(&self, action: &str) -> Option<Duration> {
        self.actions
            .iter()
            .find(|a| a.name == action)
            .map(|a| Duration::from_secs(a.timeout_seconds))
    }
}

pub fn plugins_dir(database: &Path) -> PathBuf {
    database.with_extension("plugins")
}
pub fn data_dir(database: &Path, id: &str) -> PathBuf {
    database.with_extension("plugin-data").join(id)
}

pub struct ExternalPlugin {
    pub manifest: Manifest,
    pub dir: PathBuf,
    // The plugin trait hands out 'static names; each load leaks these few strings once.
    id: &'static str,
    actions: &'static [&'static str],
}

impl ExternalPlugin {
    pub fn load(dir: &Path) -> Result<Self> {
        let manifest = Manifest::read(dir)?;
        ensure!(
            dir.file_name().and_then(|n| n.to_str()) == Some(manifest.id.as_str()),
            "plugin directory does not match its manifest id"
        );
        let id: &'static str = Box::leak(manifest.id.clone().into_boxed_str());
        let actions: Vec<&'static str> = manifest
            .actions
            .iter()
            .map(|a| &*Box::leak(a.name.clone().into_boxed_str()))
            .collect();
        Ok(Self {
            dir: dir.to_path_buf(),
            id,
            actions: Box::leak(actions.into_boxed_slice()),
            manifest,
        })
    }

    fn has(&self, permission: Permission) -> bool {
        self.manifest.permissions.contains(&permission)
    }

    /// Run one action as a child process and return its stdout JSON.
    pub(crate) async fn run(
        &self,
        supervisor: &Supervisor,
        action: &str,
        request: Value,
    ) -> Result<Value> {
        let timeout = self
            .manifest
            .timeout(action)
            .context("plugin action not found")?;
        let data = data_dir(&supervisor.config.database_url, self.id);
        std::fs::create_dir_all(&data)?;
        restrict(&data);
        let context = supervisor.interaction.context.read().unwrap().clone();
        let mut path = std::ffi::OsString::new();
        if let Some(bin) = context.as_ref().and_then(|c| c.bin_dir.as_ref()) {
            path.push(bin);
            path.push(if cfg!(windows) { ";" } else { ":" });
        }
        path.push(std::env::var_os("PATH").unwrap_or_default());

        let program = resolve_program(&self.dir, &self.manifest.entry[0]);
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(&self.manifest.entry[1..])
            .current_dir(&self.dir)
            .env_clear()
            .envs(crate::service::automation::inherited_command_env())
            .env("PATH", path)
            .env("POCKYMOE_PLUGIN_ID", self.id)
            .env("POCKYMOE_PLUGIN_DIR", &self.dir)
            .env("POCKYMOE_PLUGIN_DATA_DIR", &data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if self.has(Permission::PockymoeCli) {
            if let Some(context) = &context {
                cmd.env("POCKYMOE_URL", &context.url)
                    .env("POCKYMOE_TOKEN", &context.token);
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.as_std_mut().process_group(0);
        }
        crate::child_process::hide_tokio(&mut cmd);
        let mut child = cmd.spawn().context("plugin entry could not start")?;
        let pid = child.id();
        let mut stdin = child.stdin.take().context("plugin stdin")?;
        let stdout = child.stdout.take().context("plugin stdout")?;
        let stderr = child.stderr.take().context("plugin stderr")?;
        let body = serde_json::to_vec(&request)?;
        let exchange = async {
            // A plugin that ignores stdin must not stall the run.
            let write = async {
                let _ = stdin.write_all(&body).await;
                drop(stdin);
            };
            let ((), out, err, status) = tokio::join!(
                write,
                read_capped(stdout, MAX_STDOUT),
                read_capped(stderr, MAX_STDERR),
                child.wait()
            );
            Ok::<_, anyhow::Error>((out?, err?, status?))
        };
        let (out, err, status) = match tokio::time::timeout(timeout, exchange).await {
            Ok(result) => result?,
            Err(_) => {
                if let Some(pid) = pid {
                    crate::service::automation::kill_command_tree(pid).await;
                }
                bail!("plugin action timed out; external effects may be incomplete")
            }
        };
        if !status.success() {
            let tail: String = String::from_utf8_lossy(&err)
                .chars()
                .rev()
                .take(2000)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            bail!("plugin exited with {status}: {}", tail.trim());
        }
        ensure!(out.len() <= MAX_STDOUT, "plugin output exceeds 256 KiB");
        let text = String::from_utf8(out).context("plugin output is not UTF-8")?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(text.trim()).context("plugin output is not one JSON value")
    }
}

fn resolve_program(dir: &Path, program: &str) -> PathBuf {
    let path = Path::new(program);
    if path.is_relative() && program.contains(['/', '\\']) {
        dir.join(path)
    } else {
        path.to_path_buf()
    }
}

/// Read up to `cap` bytes and keep draining, so a chatty child never blocks.
async fn read_capped(mut reader: impl AsyncRead + Unpin, cap: usize) -> Result<Vec<u8>> {
    let mut kept = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            return Ok(kept);
        }
        let room = (cap + 1).saturating_sub(kept.len());
        kept.extend_from_slice(&buf[..n.min(room)]);
    }
}

fn restrict(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[async_trait]
impl BuiltinPlugin for ExternalPlugin {
    fn id(&self) -> &'static str {
        self.id
    }
    fn source(&self) -> &'static str {
        "external"
    }
    fn descriptor(&self, s: &Supervisor) -> PluginDto {
        let install =
            s.db.get_kv(&format!("plugin:{}:install", self.id))
                .ok()
                .flatten()
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
        PluginDto {
            id: self.id.into(),
            name: self.manifest.name.clone(),
            version: self.manifest.version.clone(),
            description: self.manifest.description.clone(),
            remote_codex: String::new(),
            capabilities: json!({
                "artifactTypes": [],
                "timelineRenderers": [],
                "threadPanels": [],
                "permissions": self.manifest.permissions,
                "schedules": self.manifest.schedules,
                "actionDetails": self.manifest.actions,
                "install": install,
            }),
            enabled: false,
            source: Some("external".into()),
            available: Some(true),
            unavailable_reason_code: None,
            unavailable_reason: None,
            default_enabled: None,
        }
    }
    fn enabled_by_default(&self) -> bool {
        // Installing never runs anything; the owner enables it after reading its permissions.
        false
    }
    fn default_settings(&self) -> Value {
        self.manifest.default_settings.clone()
    }
    fn validate_settings(&self, _values: &Value) -> Result<()> {
        // The host already bounds size and rejects credentials; the plugin validates the rest.
        Ok(())
    }
    fn actions(&self) -> &'static [&'static str] {
        self.actions
    }
    async fn invoke(&self, action: &str, context: PluginActionContext) -> Result<Value> {
        let request = json!({
            "action": action,
            "scope": "thread",
            "threadId": context.thread_id,
            "runId": context.run_id,
            "input": context.input,
            "event": context.event,
            "settings": context.settings.values,
        });
        self.run(&context.supervisor, action, request).await
    }
}

/// Plugin runs in flight, keyed by plugin id and action: one at a time per action,
/// so a long-polling schedule never blocks a `status` call from the panel.
#[derive(Default)]
pub(crate) struct ExternalRuns {
    running: std::sync::Mutex<HashSet<String>>,
    // Runs of different actions finish concurrently; their history update must not race.
    history: std::sync::Mutex<()>,
    last_tick: std::sync::Mutex<Option<std::time::Instant>>,
}

struct RunGuard<'a>(&'a ExternalRuns, String);
impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.0.running.lock().unwrap().remove(&self.1);
    }
}

impl Supervisor {
    fn external_plugin(&self, id: &str) -> Result<Arc<dyn BuiltinPlugin>> {
        let plugin = self.plugins.get(id)?;
        ensure!(plugin.source() == "external", "not an installed plugin");
        Ok(plugin)
    }

    fn external_module(&self, id: &str) -> Result<ExternalPlugin> {
        self.external_plugin(id)?;
        ExternalPlugin::load(&plugins_dir(&self.config.database_url).join(id))
    }

    /// Register every installed plugin at startup. A broken one is skipped, not fatal.
    pub(crate) fn load_external_plugins(&self) {
        let dir = plugins_dir(&self.config.database_url);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if hidden || !path.is_dir() {
                continue;
            }
            match ExternalPlugin::load(&path) {
                Ok(plugin) => {
                    if let Err(error) = self.plugins.register(Arc::new(plugin)) {
                        tracing::warn!(%error, path = %path.display(), "skipping plugin");
                    }
                }
                Err(error) => tracing::warn!(%error, path = %path.display(), "skipping plugin"),
            }
        }
    }

    /// Clone a git repository into the plugin directory, run its setup and
    /// register it disabled.
    pub async fn install_external_plugin(
        &self,
        source: &str,
        git_ref: Option<&str>,
    ) -> Result<PluginDto> {
        ensure!(
            !source.is_empty() && source.len() <= 2048 && !source.starts_with('-'),
            "invalid plugin source"
        );
        ensure!(
            git_ref.is_none_or(|r| !r.is_empty() && r.len() <= 256 && !r.starts_with('-')),
            "invalid ref"
        );
        let root = plugins_dir(&self.config.database_url);
        std::fs::create_dir_all(&root)?;
        let staging = root.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
        let result = async {
            let mut args = vec!["clone", "--depth", "1"];
            if let Some(r) = git_ref {
                args.extend(["--branch", r]);
            }
            let staging_arg = staging.to_string_lossy().into_owned();
            args.extend(["--", source, &staging_arg]);
            git(&root, &args).await?;
            let manifest = Manifest::read(&staging)?;
            ensure!(
                self.plugins.get(&manifest.id).is_err(),
                "plugin {} is already installed; update it instead",
                manifest.id
            );
            let target = root.join(&manifest.id);
            ensure!(!target.exists(), "plugin directory already exists");
            std::fs::rename(&staging, &target)?;
            if let Err(error) = run_setup(&manifest, &target).await {
                let _ = std::fs::remove_dir_all(&target);
                return Err(error);
            }
            Ok(target)
        }
        .await;
        let target = match result {
            Ok(target) => target,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(error);
            }
        };
        let plugin = ExternalPlugin::load(&target)?;
        let id = plugin.id;
        let commit = git(&target, &["rev-parse", "HEAD"])
            .await
            .unwrap_or_default();
        let now = pockymoe_protocol::now_rfc3339();
        self.db.set_kv(
            &format!("plugin:{id}:install"),
            &json!({"source":source,"ref":git_ref,"commit":commit.trim(),"installedAt":now,"updatedAt":now})
                .to_string(),
        )?;
        self.plugins.register(Arc::new(plugin))?;
        self.get_plugin(id)
    }

    /// Fast-forward an installed plugin to its remote branch and rerun setup.
    pub async fn update_external_plugin(&self, id: &str) -> Result<PluginDto> {
        self.external_plugin(id)?;
        let dir = plugins_dir(&self.config.database_url).join(id);
        git(&dir, &["pull", "--ff-only"]).await?;
        let plugin = ExternalPlugin::load(&dir)?;
        ensure!(plugin.id == id, "an update cannot change the plugin id");
        run_setup(&plugin.manifest, &dir).await?;
        let commit = git(&dir, &["rev-parse", "HEAD"]).await.unwrap_or_default();
        let key = format!("plugin:{id}:install");
        let mut record: Value = self
            .db
            .get_kv(&key)?
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_else(|| json!({}));
        record["commit"] = json!(commit.trim());
        record["updatedAt"] = json!(pockymoe_protocol::now_rfc3339());
        self.db.set_kv(&key, &record.to_string())?;
        self.plugins.replace_external(Arc::new(plugin));
        self.get_plugin(id)
    }

    /// Remove an installed plugin, its settings and pending hooks; data only on `purge`.
    pub fn remove_external_plugin(&self, id: &str, purge_data: bool) -> Result<()> {
        self.external_plugin(id)?;
        self.plugins.remove_external(id)?;
        let now = pockymoe_protocol::now_rfc3339();
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            Self::cancel_pending_plugin_actions(&tx, None, id, &now)?;
            tx.execute(
                "DELETE FROM kv WHERE key LIKE ?1 ESCAPE '\\' OR (key LIKE 'thread:%' AND key LIKE ?2 ESCAPE '\\')",
                rusqlite::params![
                    format!("plugin:{}:%", like_escape(id)),
                    format!("%:plugin:{}:settings", like_escape(id))
                ],
            )?;
            tx.commit()?;
            Ok(())
        })?;
        let _ = std::fs::remove_dir_all(plugins_dir(&self.config.database_url).join(id));
        if purge_data {
            let _ = std::fs::remove_dir_all(data_dir(&self.config.database_url, id));
        }
        Ok(())
    }

    /// Run an action at device scope (manual, schedule, or the Web panel).
    pub async fn run_external_plugin_action(
        &self,
        id: &str,
        action: &str,
        input: Value,
        trigger: &str,
    ) -> Result<Value> {
        let plugin = self.external_plugin(id)?;
        ensure!(
            self.plugin_enabled(id, false),
            "plugin is not enabled on this device"
        );
        ensure!(
            plugin.actions().contains(&action),
            "plugin action not found"
        );
        ensure!(
            serde_json::to_vec(&input)?.len() <= 65536,
            "plugin input exceeds 64 KiB"
        );
        let module = self.external_module(id)?;
        {
            let mut running = self.external_runs.running.lock().unwrap();
            ensure!(
                running.insert(run_key(id, action)),
                "plugin is already running {action}"
            );
        }
        let _guard = RunGuard(&self.external_runs, run_key(id, action));
        let run_id = uuid::Uuid::new_v4().to_string();
        let started = pockymoe_protocol::now_rfc3339();
        let settings = self.plugin_settings(id)?.values;
        let request = json!({
            "action": action,
            "scope": "device",
            "trigger": trigger,
            "runId": run_id,
            "input": input,
            "settings": settings,
        });
        let result = module.run(self, action, request).await;
        let record = json!({
            "id": run_id,
            "action": action,
            "trigger": trigger,
            "startedAt": started,
            "completedAt": pockymoe_protocol::now_rfc3339(),
            "state": if result.is_ok() { "completed" } else { "failed" },
            "error": result.as_ref().err().map(|e| e.to_string()),
        });
        // The settings panel calls `status` on every open; only its failures are history.
        if action == "status" && result.is_ok() {
            return result;
        }
        let key = format!("plugin:{id}:runs");
        let _history = self.external_runs.history.lock().unwrap();
        let mut runs: Vec<Value> = self
            .db
            .get_kv(&key)?
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        runs.insert(0, record);
        runs.truncate(RUN_HISTORY);
        self.db.set_kv(&key, &serde_json::to_string(&runs)?)?;
        result
    }

    pub fn external_plugin_runs(&self, id: &str) -> Result<Value> {
        self.external_plugin(id)?;
        Ok(self
            .db
            .get_kv(&format!("plugin:{id}:runs"))?
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_else(|| json!([])))
    }

    /// Start due manifest schedules for enabled plugins. Called from the worker loop.
    pub fn external_plugin_tick(self: &Arc<Self>) -> Result<()> {
        {
            let mut last = self.external_runs.last_tick.lock().unwrap();
            if last.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
                return Ok(());
            }
            *last = Some(std::time::Instant::now());
        }
        let now = chrono::Utc::now();
        for plugin in self.plugins.snapshot() {
            if plugin.source() != "external" || !self.plugin_enabled(plugin.id(), false) {
                continue;
            }
            let Ok(module) = self.external_module(plugin.id()) else {
                continue;
            };
            for schedule in &module.manifest.schedules {
                let key = format!("plugin:{}:schedule:{}", plugin.id(), schedule.action);
                let due = self
                    .db
                    .get_kv(&key)?
                    .and_then(|at| chrono::DateTime::parse_from_rfc3339(&at).ok())
                    .is_none_or(|at| {
                        (now - at.with_timezone(&chrono::Utc)).num_seconds()
                            >= schedule.every_seconds as i64
                    });
                let busy = self
                    .external_runs
                    .running
                    .lock()
                    .unwrap()
                    .contains(&run_key(plugin.id(), &schedule.action));
                if !due || busy {
                    continue;
                }
                // Recording the start first means a crash skips a tick instead of repeating it.
                self.db.set_kv(&key, &now.to_rfc3339())?;
                let state = self.clone();
                let id = plugin.id().to_string();
                let action = schedule.action.clone();
                tokio::spawn(async move {
                    if let Err(error) = state
                        .run_external_plugin_action(&id, &action, json!({}), "schedule")
                        .await
                    {
                        tracing::warn!(%error, plugin = %id, %action, "scheduled plugin action failed");
                    }
                });
                break;
            }
        }
        Ok(())
    }
}

fn run_key(id: &str, action: &str) -> String {
    format!("{id}\n{action}")
}

fn like_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

async fn run_setup(manifest: &Manifest, dir: &Path) -> Result<()> {
    if manifest.setup.is_empty() {
        return Ok(());
    }
    let mut cmd = tokio::process::Command::new(resolve_program(dir, &manifest.setup[0]));
    cmd.args(&manifest.setup[1..])
        .current_dir(dir)
        .env_clear()
        .envs(crate::service::automation::inherited_command_env())
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::child_process::hide_tokio(&mut cmd);
    let child = cmd.spawn().context("plugin setup could not start")?;
    let output = tokio::time::timeout(GIT_TIMEOUT, child.wait_with_output())
        .await
        .context("plugin setup timed out")??;
    ensure!(
        output.status.success(),
        "plugin setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .last()
            .unwrap_or("")
    );
    Ok(())
}

async fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(crate::service::automation::inherited_command_env())
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::child_process::hide_tokio(&mut cmd);
    let child = cmd.spawn().context("git is not available")?;
    let output = tokio::time::timeout(GIT_TIMEOUT, child.wait_with_output())
        .await
        .context("git timed out")??;
    ensure!(
        output.status.success(),
        "git {} failed: {}",
        args.first().unwrap_or(&""),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
