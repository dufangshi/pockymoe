//! Trusted, statically linked feature plugins. No manifest code loading.
use std::sync::Arc;

use anyhow::{bail, ensure, Context, Result};
use async_trait::async_trait;
use pockymoe_protocol::PluginDto;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::Supervisor;

pub mod content_quality;

pub const TERMINAL_PLUGIN_ID: &str = "remote-codex.terminal";
pub const DSH_PLUGIN_ID: &str = "remote-codex.deepseek-harness";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSettings {
    pub revision: u64,
    pub values: Value,
}

/// A full per-thread override. Persist even an empty override as a CAS tombstone.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadPluginOverride {
    pub revision: u64,
    pub enabled: Option<bool>,
    pub values: Option<Value>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectivePluginSettings {
    pub enabled: bool,
    pub values: Value,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPluginSettings {
    pub revision: u64,
    pub global_revision: u64,
    pub enabled: Option<bool>,
    pub values: Option<Value>,
    pub effective: EffectivePluginSettings,
    #[serde(skip)]
    global_enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPluginDto {
    #[serde(flatten)]
    pub plugin: PluginDto,
    pub global_enabled: bool,
    pub enabled_override: Option<bool>,
    pub settings_overridden: bool,
}

fn validate_settings(plugin: &dyn BuiltinPlugin, values: &Value) -> Result<()> {
    ensure!(
        values.is_object() && serde_json::to_vec(values)?.len() <= 65536,
        "plugin settings must be an object up to 64 KiB"
    );
    fn has_credentials(value: &Value) -> bool {
        match value {
            Value::Object(object) => object.iter().any(|(key, value)| {
                let key: String = key
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect();
                matches!(
                    key.as_str(),
                    "apikey"
                        | "accesstoken"
                        | "refreshtoken"
                        | "token"
                        | "password"
                        | "secret"
                        | "clientsecret"
                        | "authorization"
                        | "credential"
                        | "credentials"
                        | "privatekey"
                ) || has_credentials(value)
            }),
            Value::Array(values) => values.iter().any(has_credentials),
            _ => false,
        }
    }
    ensure!(
        !has_credentials(values),
        "plugin settings must not contain credentials; use a private connection reference"
    );
    plugin.validate_settings(values)
}

/// One accepted hook invocation. Settings are snapshotted before execution.
pub struct PluginActionContext {
    pub supervisor: Arc<Supervisor>,
    pub run_id: String,
    pub thread_id: String,
    pub settings: PluginSettings,
    pub input: Value,
    pub event: Value,
}

#[async_trait]
pub trait BuiltinPlugin: Send + Sync {
    fn id(&self) -> &'static str;
    fn descriptor(&self, supervisor: &Supervisor) -> PluginDto;
    fn enabled_by_default(&self) -> bool {
        true
    }
    fn default_settings(&self) -> Value {
        json!({})
    }
    /// Override with typed deserialization/validation when declaring settings.
    fn validate_settings(&self, values: &Value) -> Result<()> {
        ensure!(values == &json!({}), "plugin declares no settings");
        Ok(())
    }
    fn actions(&self) -> &'static [&'static str] {
        &[]
    }
    /// Pure hook definition: called inside turn settlement, without I/O or model calls.
    fn turn_automation(
        &self,
        _thread: &str,
        _settings: &Value,
    ) -> Result<Option<pockymoe_protocol::AutomationDefinition>> {
        Ok(None)
    }
    fn validate_action(&self, action: &str, _input: &Value) -> Result<()> {
        ensure!(self.actions().contains(&action), "plugin action not found");
        Ok(())
    }
    async fn invoke(&self, _action: &str, _context: PluginActionContext) -> Result<Value> {
        bail!("plugin action not found")
    }
}

pub(crate) struct PluginHost {
    entries: Vec<Arc<dyn BuiltinPlugin>>,
}

impl Default for PluginHost {
    fn default() -> Self {
        Self {
            entries: vec![
                Arc::new(TerminalPlugin),
                Arc::new(DeepSeekPlugin),
                Arc::new(content_quality::ContentQualityPlugin),
            ],
        }
    }
}

impl PluginHost {
    pub(crate) fn sync_turn_automations(
        &self,
        conn: &rusqlite::Connection,
        thread: &str,
        now: &str,
        available: &std::collections::HashMap<String, bool>,
        only_plugin: Option<&str>,
    ) -> Result<()> {
        for plugin in self
            .entries
            .iter()
            .filter(|p| only_plugin.is_none_or(|id| id == p.id()))
        {
            let key = format!("builtin-plugin:{}", plugin.id());
            let existing: Option<(String, String, Option<String>)> = conn
                .query_row(
                    "SELECT id,state,error FROM automations WHERE thread_id=?1 AND request_id=?2",
                    params![thread, key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let settings = self.thread_settings(
                conn,
                thread,
                plugin.id(),
                available.get(plugin.id()).copied().unwrap_or(false),
            )?;
            let definition = if settings.effective.enabled {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    plugin.turn_automation(thread, &settings.effective.values)
                }))
                .map_err(|_| anyhow::anyhow!("plugin turn hook panicked"))??
            } else {
                None
            };
            let cursor: i64 = conn.query_row(
                "SELECT coalesce(max(sequence),0) FROM automation_events",
                [],
                |r| r.get(0),
            )?;
            match (existing, definition) {
                (None, Some(definition)) => {
                    conn.execute("INSERT INTO automations(id,thread_id,definition_json,state,event_cursor,created_at,updated_at,request_id,request_definition) VALUES(?1,?2,?3,'enabled',?4,?5,?5,?6,?3)", params![uuid::Uuid::new_v4().to_string(),thread,serde_json::to_string(&definition)?,cursor,now,key])?;
                }
                (Some((id, state, error)), Some(definition)) if state != "cancelled" => {
                    let resume =
                        state == "paused" && error.as_deref() == Some("pluginSettingsDisabled");
                    conn.execute("UPDATE automations SET definition_json=?2,updated_at=?3,state=CASE WHEN ?4 THEN 'enabled' ELSE state END,error=CASE WHEN ?4 THEN NULL ELSE error END,event_cursor=CASE WHEN ?4 THEN ?5 ELSE event_cursor END WHERE id=?1", params![id,serde_json::to_string(&definition)?,now,resume,cursor])?;
                }
                (Some((id, state, _)), None) if state == "enabled" => {
                    conn.execute("UPDATE automations SET state='paused',updated_at=?2,error='pluginSettingsDisabled' WHERE id=?1", params![id,now])?;
                    conn.execute("UPDATE automation_runs SET state='cancelled',completed_at=?2,error='pluginSettingsDisabled' WHERE automation_id=?1 AND state='due'", params![id,now])?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    // Resolve descriptors before acquiring db: third-party descriptors may query Supervisor.
    pub(crate) fn availability(
        &self,
        supervisor: &Supervisor,
    ) -> std::collections::HashMap<String, bool> {
        self.entries
            .iter()
            .map(|p| {
                (
                    p.id().to_string(),
                    p.descriptor(supervisor).available != Some(false),
                )
            })
            .collect()
    }

    pub(crate) fn thread_settings(
        &self,
        conn: &rusqlite::Connection,
        thread: &str,
        id: &str,
        available: bool,
    ) -> Result<ThreadPluginSettings> {
        let plugin = self.get(id)?;
        let read = |key: String| -> Result<Option<String>> {
            Ok(conn
                .query_row("SELECT value FROM kv WHERE key=?1", [key], |r| r.get(0))
                .optional()?)
        };
        let global = read(format!("plugin:{id}:settings"))?
            .map(|raw| serde_json::from_str::<PluginSettings>(&raw))
            .transpose()?
            .unwrap_or_else(|| PluginSettings {
                revision: 0,
                values: plugin.default_settings(),
            });
        let overlay = read(format!("thread:{thread}:plugin:{id}:settings"))?
            .map(|raw| serde_json::from_str::<ThreadPluginOverride>(&raw))
            .transpose()?
            .unwrap_or_default();
        let global_enabled = read(format!("plugin:{id}:enabled"))?
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(plugin.enabled_by_default());
        Ok(ThreadPluginSettings {
            revision: overlay.revision,
            global_revision: global.revision,
            enabled: overlay.enabled,
            values: overlay.values.clone(),
            global_enabled: available && global_enabled,
            effective: EffectivePluginSettings {
                enabled: available && global_enabled && overlay.enabled.unwrap_or(true),
                values: overlay.values.unwrap_or(global.values),
            },
        })
    }

    pub(crate) fn sync_plugin_automations(
        &self,
        conn: &rusqlite::Connection,
        id: &str,
        now: &str,
        available: &std::collections::HashMap<String, bool>,
    ) -> Result<()> {
        let threads = {
            let mut q = conn.prepare("SELECT DISTINCT a.thread_id FROM automations a JOIN threads t ON t.id=a.thread_id WHERE a.request_id=?1 AND t.closed_at IS NULL")?;
            let rows = q
                .query_map([format!("builtin-plugin:{id}")], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for thread in threads {
            self.sync_turn_automations(conn, &thread, now, available, Some(id))?;
        }
        Ok(())
    }

    pub fn register(&mut self, plugin: Arc<dyn BuiltinPlugin>) -> Result<()> {
        let id = plugin.id();
        ensure!(
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')),
            "invalid plugin id"
        );
        ensure!(
            !self.entries.iter().any(|p| p.id() == id),
            "plugin already registered: {id}"
        );
        plugin.validate_settings(&plugin.default_settings())?;
        self.entries.push(plugin);
        Ok(())
    }
    pub fn get(&self, id: &str) -> Result<Arc<dyn BuiltinPlugin>> {
        self.entries
            .iter()
            .find(|p| p.id() == id)
            .cloned()
            .context("plugin not found")
    }
}

impl Supervisor {
    pub fn submit_plugin_action(
        &self,
        thread: &str,
        plugin: &str,
        action: &str,
        input: Value,
        timeout_seconds: u64,
        request: &str,
    ) -> Result<Value> {
        self.validate_thread_plugin_action(thread, plugin, action, &input, timeout_seconds)?;
        ensure!(
            !request.is_empty() && request.len() <= 96,
            "clientRequestId must be 1..96 bytes"
        );
        let action = pockymoe_protocol::AutomationAction::PluginAction {
            plugin_id: plugin.into(),
            action: action.into(),
            input,
            timeout_seconds,
        };
        let request = format!("plugin-action:{request}");
        let existing: Option<String> = self.db.with(|c| {
            Ok(c.query_row(
                "SELECT request_definition FROM automations WHERE thread_id=?1 AND request_id=?2",
                params![thread, request],
                |r| r.get(0),
            )
            .optional()?)
        })?;
        let definition = if let Some(raw) = existing {
            let definition: pockymoe_protocol::AutomationDefinition = serde_json::from_str(&raw)?;
            ensure!(
                serde_json::to_value(&definition.action)? == serde_json::to_value(&action)?,
                "conflict: clientRequestId reused with different action"
            );
            definition
        } else {
            serde_json::from_value(
                json!({"name":format!("{} action", self.get_plugin(plugin)?.name),"trigger":{"kind":"at","at":pockymoe_protocol::now_rfc3339()},"action":action}),
            )?
        };
        self.automation_create(thread, definition, Some(&request))
    }

    pub(crate) async fn execute_plugin_run(self: &Arc<Self>, run_id: &str) -> Result<()> {
        use futures_util::FutureExt;
        let (thread, raw, event, receipt): (String, String, String, String) = self.db.with(|c| {
            Ok(c.query_row("SELECT a.thread_id,r.definition_json,r.event_json,r.receipt_json FROM automation_runs r JOIN automations a ON a.id=r.automation_id WHERE r.id=?1 AND r.state='running'", [run_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?)
        })?;
        let definition: pockymoe_protocol::AutomationDefinition = serde_json::from_str(&raw)?;
        let pockymoe_protocol::AutomationAction::PluginAction {
            plugin_id,
            action,
            input,
            timeout_seconds,
        } = definition.action
        else {
            bail!("not a plugin action")
        };
        let mut receipt: Value = serde_json::from_str(&receipt)?;
        let result: Result<Value> = async {
            // The running claim is the scope authorization boundary. Disabling a
            // thread cancels due work; already claimed invocations may finish.
            self.validate_plugin_action_input(&plugin_id, &action, &input, timeout_seconds)?;
            let settings: PluginSettings = serde_json::from_value(receipt["settings"].clone())?;
            let plugin = self.plugins.get(&plugin_id)?;
            plugin.validate_settings(&settings.values)?;
            let context = PluginActionContext {
                supervisor: self.clone(),
                run_id: run_id.into(),
                thread_id: thread,
                settings,
                input,
                event: serde_json::from_str(&event)?,
            };
            let invocation =
                std::panic::AssertUnwindSafe(plugin.invoke(&action, context)).catch_unwind();
            match tokio::time::timeout(std::time::Duration::from_secs(timeout_seconds), invocation)
                .await
            {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => bail!("plugin action panicked"),
                Err(_) => bail!("plugin action timed out; external effects may be incomplete"),
            }
        }
        .await;
        let (status, error) = match result {
            Ok(output) if serde_json::to_vec(&output)?.len() <= 262144 => {
                receipt["output"] = output;
                ("completed", None)
            }
            Ok(_) => ("failed", Some("plugin output exceeds 256 KiB".to_string())),
            Err(error) => ("failed", Some(error.to_string())),
        };
        self.db.with(|c| {
            c.execute("UPDATE automation_runs SET state=?2,completed_at=?3,receipt_json=?4,error=?5 WHERE id=?1 AND state='running'", params![run_id,status,pockymoe_protocol::now_rfc3339(),receipt.to_string(),error])?;
            Ok(())
        })
    }

    /// Register compiled modules before wrapping the Supervisor in Arc or serving it.
    pub fn with_builtin_plugins(mut self, plugins: Vec<Arc<dyn BuiltinPlugin>>) -> Result<Self> {
        for plugin in plugins {
            self.plugins.register(plugin)?;
        }
        Ok(self)
    }

    pub fn list_plugins(&self) -> Vec<PluginDto> {
        self.plugins
            .entries
            .iter()
            .map(|p| self.plugin_descriptor(p.as_ref()))
            .collect()
    }

    fn plugin_descriptor(&self, plugin: &dyn BuiltinPlugin) -> PluginDto {
        let mut dto = plugin.descriptor(self);
        dto.id = plugin.id().into();
        dto.source = Some("builtin".into());
        dto.enabled = dto.available != Some(false)
            && self.plugin_enabled(plugin.id(), plugin.enabled_by_default());
        if !dto.capabilities.is_object() {
            dto.capabilities = json!({});
        }
        if let Some(caps) = dto.capabilities.as_object_mut() {
            caps.insert("actions".into(), json!(plugin.actions()));
            caps.insert("scopes".into(), json!(["device", "thread"]));
        }
        dto
    }

    pub fn get_plugin(&self, id: &str) -> Result<PluginDto> {
        Ok(self.plugin_descriptor(self.plugins.get(id)?.as_ref()))
    }

    pub fn plugin_settings(&self, id: &str) -> Result<PluginSettings> {
        let plugin = self.plugins.get(id)?;
        let saved = self.db.get_kv(&format!("plugin:{id}:settings"))?;
        saved
            .map(|s| serde_json::from_str(&s).map_err(Into::into))
            .unwrap_or_else(|| {
                Ok(PluginSettings {
                    revision: 0,
                    values: plugin.default_settings(),
                })
            })
    }

    /// Full replacement with optimistic concurrency. Settings contain no credentials.
    pub fn save_plugin_settings(
        &self,
        id: &str,
        settings: PluginSettings,
    ) -> Result<PluginSettings> {
        let plugin = self.plugins.get(id)?;
        validate_settings(plugin.as_ref(), &settings.values)?;
        let available = self.plugins.availability(self);
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            let key = format!("plugin:{id}:settings");
            let previous: Option<String> = tx.query_row("SELECT value FROM kv WHERE key=?1", [&key], |r| r.get(0)).optional()?;
            let revision = previous.map(|s| serde_json::from_str::<PluginSettings>(&s)).transpose()?.map_or(0, |s| s.revision);
            ensure!(revision == settings.revision, "conflict: plugin settings changed; reload before saving");
            let next = PluginSettings { revision: revision.checked_add(1).context("plugin settings revision exhausted")?, values: settings.values };
            tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, serde_json::to_string(&next)?])?;
            self.plugins.sync_plugin_automations(&tx, id, &pockymoe_protocol::now_rfc3339(), &available)?;
            tx.commit()?;
            Ok(next)
        })
    }

    pub fn list_thread_plugins(&self, thread: &str) -> Result<Vec<ThreadPluginDto>> {
        self.get_thread(thread)?;
        let descriptors = self.list_plugins();
        self.db.with(|conn| {
            descriptors
                .into_iter()
                .map(|mut plugin| {
                    let settings = self.plugins.thread_settings(
                        conn,
                        thread,
                        &plugin.id,
                        plugin.available != Some(false),
                    )?;
                    plugin.enabled = settings.effective.enabled;
                    Ok(ThreadPluginDto {
                        plugin,
                        global_enabled: settings.global_enabled,
                        enabled_override: settings.enabled,
                        settings_overridden: settings.values.is_some(),
                    })
                })
                .collect()
        })
    }

    pub fn thread_plugin_settings(&self, thread: &str, id: &str) -> Result<ThreadPluginSettings> {
        self.get_thread(thread)?;
        let available = self.get_plugin(id)?.available != Some(false);
        self.db
            .with(|conn| self.plugins.thread_settings(conn, thread, id, available))
    }

    pub fn save_thread_plugin_settings(
        &self,
        thread: &str,
        id: &str,
        input: ThreadPluginOverride,
    ) -> Result<ThreadPluginSettings> {
        self.get_thread(thread)?;
        let plugin = self.plugins.get(id)?;
        if let Some(values) = &input.values {
            validate_settings(plugin.as_ref(), values)?;
        }
        let available = self.plugins.availability(self);
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM threads WHERE id=?1)", [thread], |r| r.get(0))?;
            ensure!(exists, "thread not found");
            let previous = self.plugins.thread_settings(&tx, thread, id, available[id])?;
            ensure!(previous.revision == input.revision, "conflict: thread plugin settings changed; reload before saving");
            let next = ThreadPluginOverride { revision: input.revision.checked_add(1).context("plugin settings revision exhausted")?, ..input };
            tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![format!("thread:{thread}:plugin:{id}:settings"), serde_json::to_string(&next)?])?;
            let snapshot = self.plugins.thread_settings(&tx, thread, id, available[id])?;
            let now = pockymoe_protocol::now_rfc3339();
            if !snapshot.effective.enabled {
                Self::cancel_pending_plugin_actions(&tx, Some(thread), id, &now)?;
            }
            self.plugins.sync_turn_automations(&tx, thread, &now, &available, Some(id))?;
            tx.commit()?;
            Ok(snapshot)
        })
    }

    pub(crate) fn validate_thread_plugin_action(
        &self,
        thread: &str,
        id: &str,
        action: &str,
        input: &Value,
        timeout: u64,
    ) -> Result<()> {
        ensure!(
            self.thread_plugin_settings(thread, id)?.effective.enabled,
            "plugin is not enabled or unavailable for this thread"
        );
        self.validate_plugin_action_input(id, action, input, timeout)
    }

    fn validate_plugin_action_input(
        &self,
        id: &str,
        action: &str,
        input: &Value,
        timeout: u64,
    ) -> Result<()> {
        let plugin = self.plugins.get(id)?;
        ensure!(
            (1..=300).contains(&timeout),
            "timeoutSeconds must be 1..300"
        );
        ensure!(
            serde_json::to_vec(input)?.len() <= 65536,
            "plugin input exceeds 64 KiB"
        );
        ensure!(
            plugin.actions().contains(&action),
            "plugin action not found"
        );
        plugin.validate_action(action, input)
    }
}

fn descriptor(
    s: &Supervisor,
    id: &str,
    name: &str,
    description: &str,
    panel: Value,
    available: bool,
) -> PluginDto {
    PluginDto {
        id: id.into(),
        name: name.into(),
        version: s.config.app_version.clone(),
        description: description.into(),
        remote_codex: format!(">={}", s.config.app_version),
        capabilities: json!({"artifactTypes":[],"timelineRenderers":[],"threadPanels":[panel]}),
        enabled: false,
        source: Some("builtin".into()),
        available: Some(available),
        unavailable_reason_code: (!available).then(|| {
            if id == TERMINAL_PLUGIN_ID {
                "unsupported_platform"
            } else {
                "harness_missing"
            }
            .into()
        }),
        unavailable_reason: (!available).then(|| {
            if id == TERMINAL_PLUGIN_ID {
                "Terminal is unavailable on this platform."
            } else {
                "Install DeepSeek Harness (dsh) on this device to use this plugin."
            }
            .into()
        }),
    }
}

struct TerminalPlugin;
#[async_trait]
impl BuiltinPlugin for TerminalPlugin {
    fn id(&self) -> &'static str {
        TERMINAL_PLUGIN_ID
    }
    fn descriptor(&self, s: &Supervisor) -> PluginDto {
        descriptor(
            s,
            self.id(),
            "Terminal",
            "Per-thread PTY terminal.",
            json!({"id":"terminal","label":"Terminal","kind":"terminal","artifactTypes":[]}),
            cfg!(unix),
        )
    }
}

struct DeepSeekPlugin;
#[async_trait]
impl BuiltinPlugin for DeepSeekPlugin {
    fn id(&self) -> &'static str {
        DSH_PLUGIN_ID
    }
    fn descriptor(&self, s: &Supervisor) -> PluginDto {
        let available = crate::acp::builtin_agents(None)
            .iter()
            .find(|a| a.id == "deepseek")
            .is_some_and(|a| crate::acp::command_available(&a.base_command));
        descriptor(s, self.id(), "DeepSeek Harness", "Run modes, DSH plugins, profile settings, commands and the native DSH console for DeepSeek Harness threads.", json!({"id":"deepseek-harness","label":"DeepSeek Harness","kind":"harness:deepseek","artifactTypes":[]}), available)
    }
}
