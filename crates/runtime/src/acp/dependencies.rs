//! User-owned adapter installations. Never alter the user's npm prefix or PATH.
use super::catalog::{command_available, AcpAgentDef};
use anyhow::{anyhow, bail, Context, Result};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, SystemTime},
};
use tokio::sync::Mutex;

use crate::disk_cleanup::{self, LiveProcesses};

/// Managed adapters, each with a `<id>.active.json` record.
const MANAGED: [&str; 2] = ["codex", "claude"];
/// An installation younger than this may still be written by an install in
/// another Supervisor sharing this home (an install times out after 300 s).
const INSTALL_GRACE: Duration = Duration::from_secs(60 * 60);
/// Per-install npm cache inside the staged prefix, removed once it succeeds.
const STAGE_CACHE: &str = ".npm-cache";

/// Serializes installs and cleanup within this Supervisor.
fn install_lock() -> &'static Mutex<()> {
    static INSTALL: OnceLock<Mutex<()>> = OnceLock::new();
    INSTALL.get_or_init(|| Mutex::new(()))
}

pub fn prefix() -> PathBuf {
    crate::config::home_dir().join(".local/share/remote-codex/adapters")
}

pub fn bin_dir() -> PathBuf {
    if cfg!(windows) {
        prefix()
    } else {
        prefix().join("bin")
    }
}

pub fn package(id: &str) -> Option<&'static str> {
    match id {
        "codex" => Some("@agentclientprotocol/codex-acp"),
        "claude" => Some("@agentclientprotocol/claude-agent-acp"),
        _ => None,
    }
}

pub fn install_command(id: &str) -> Option<String> {
    Some(command_line(&install_args(id).ok()?))
}

fn command_line(args: &[String]) -> String {
    if cfg!(windows) {
        // npm.cmd is dispatched through cmd.exe; POSIX single quotes would
        // become literal characters in --prefix and --cache paths.
        args.iter()
            .map(|arg| format!("\"{}\"", arg.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        shell_words::join(args)
    }
}

pub fn managed_bin_dirs() -> Vec<PathBuf> {
    ["codex", "claude"]
        .iter()
        .filter_map(|id| {
            let file = prefix().join(format!("{id}.active.json"));
            let directory: PathBuf = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
            // Only a completed, user-owned versioned installation can enter PATH.
            (directory.starts_with(prefix().join("installations")) && directory.is_dir()).then_some(
                if cfg!(windows) {
                    directory.clone()
                } else {
                    directory.join("bin")
                },
            )
        })
        .collect()
}
fn install_args(id: &str) -> Result<Vec<String>> {
    args_for(id, &prefix(), &prefix().join("cache"))
}
fn args_for(id: &str, target: &Path, cache: &Path) -> Result<Vec<String>> {
    let package = package(id).ok_or_else(|| anyhow!("No managed adapter for {id}"))?;
    Ok(vec![
        "npm".into(),
        "install".into(),
        "--global".into(),
        "--prefix".into(),
        target.to_string_lossy().into_owned(),
        "--cache".into(),
        cache.to_string_lossy().into_owned(),
        format!("{package}@latest"),
        "--registry=https://registry.npmjs.org".into(),
        "--fetch-retries=3".into(),
        "--fetch-retry-mintimeout=1000".into(),
        "--fetch-retry-maxtimeout=10000".into(),
        "--no-audit".into(),
        "--no-fund".into(),
    ])
}

pub async fn ensure(def: &AcpAgentDef, update: bool) -> Result<()> {
    if def.transport != "adapter" {
        return Ok(());
    }
    if !command_available(&def.base_command) {
        bail!("Install the base agent first: {}", def.base_command);
    }
    let _guard = install_lock().lock().await;
    if !update && command_available(&def.server_command) {
        return Ok(());
    }
    static FAILURES: OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, (std::time::Instant, String)>>,
    > = OnceLock::new();
    let failures = FAILURES.get_or_init(Default::default);
    let key = prefix().join(&def.id);
    if !update {
        if let Some((at, reason)) = failures.lock().unwrap().get(&key) {
            if at.elapsed() < Duration::from_secs(60) {
                bail!("{reason}; automatic installation retries after 60 seconds. Use Settings > Harnesses > ACP adapter > Install to retry now.");
            }
        }
    }
    let result = install(def).await.map_err(|error| anyhow!(
        "ACP dependency unavailable: {} (package {}). {error:#}. Repair in Settings > Harnesses > ACP adapter",
        def.server_command, package(&def.id).unwrap_or("unknown")));
    match &result {
        Ok(()) => {
            failures.lock().unwrap().remove(&key);
        }
        Err(error) => {
            failures
                .lock()
                .unwrap()
                .insert(key, (std::time::Instant::now(), error.to_string()));
        }
    }
    result
}

async fn install(def: &AcpAgentDef) -> Result<()> {
    crate::node_runtime::ensure().await?;
    let root = prefix().join("installations");
    tokio::fs::create_dir_all(&root)
        .await
        .context("Create private ACP adapter directory")?;
    // A fresh prefix avoids npm ENOTEMPTY rename leftovers and preserves the
    // working adapter until a complete replacement has been verified.
    let stage = tempfile::Builder::new()
        .prefix(&format!("{}-", def.id))
        .tempdir_in(&root)?;
    // A private cache per install: the shared one only grew (each install of
    // `@latest` fetched new tarballs) and was not read again.
    let args = args_for(&def.id, stage.path(), &stage.path().join(STAGE_CACHE))?;
    let parsed = super::rpc::parse_spawn_command(&shell_words::join(&args))?;
    let mut command = tokio::process::Command::new(parsed.program);
    crate::child_process::hide_tokio(&mut command);
    command
        .args(parsed.args)
        .env("PATH", super::catalog::child_path())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let result = tokio::time::timeout(Duration::from_secs(300), command.output()).await
        .context("ACP adapter installation timed out; the previous adapter was preserved. Retry Install in Settings")??;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let detail: String = stderr
            .chars()
            .rev()
            .take(4000)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        bail!("Install {} failed ({}): {}. The previous adapter was preserved; retry Install in Settings > Harnesses > ACP adapter", def.display_name, result.status, detail.trim());
    }
    let bin = if cfg!(windows) {
        stage.path().to_path_buf()
    } else {
        stage.path().join("bin")
    };
    let exe = super::catalog::command_program(&def.server_command)
        .context("Invalid adapter executable")?;
    let executable = which::which_in(&exe, Some(bin.as_os_str()), stage.path())
        .ok()
        .filter(|path| path.starts_with(stage.path()))
        .context("Installation completed but the staged ACP adapter is not executable")?;
    ensure_staged_target(&executable, stage.path())?;
    if let Err(error) = std::fs::remove_dir_all(stage.path().join(STAGE_CACHE)) {
        tracing::debug!(%error, "staged npm cache not removed");
    }
    let directory = stage.keep();
    let record = prefix().join(format!("{}.active.json", def.id));
    let next = record.with_extension(format!("{}.next", uuid::Uuid::new_v4()));
    let activated = (|| -> Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&next)?;
        file.write_all(&serde_json::to_vec(&directory)?)?;
        file.sync_all()?;
        std::fs::rename(&next, &record)?;
        Ok(())
    })();
    if activated.is_err() {
        let _ = std::fs::remove_file(next);
        let _ = std::fs::remove_dir_all(&directory);
        return activated;
    }
    // The caller holds the install lock, so no install of ours races this.
    let _ = tokio::task::spawn_blocking(|| {
        cleanup_blocking(
            &prefix(),
            LiveProcesses::collect().as_ref(),
            SystemTime::now(),
        )
    })
    .await;
    Ok(())
}

/// Remove adapter installations that no `*.active.json` names and no running
/// process uses, and the shared npm cache older runtimes filled. Runs after
/// each install and from the Supervisor's periodic disk cleanup.
pub async fn cleanup_installations() {
    let _guard = install_lock().lock().await;
    let _ = tokio::task::spawn_blocking(|| {
        cleanup_blocking(
            &prefix(),
            LiveProcesses::collect().as_ref(),
            SystemTime::now(),
        )
    })
    .await;
}

/// Installation directory names that `*.active.json` records point at, or
/// `None` when a record exists but cannot be read (then nothing is removed).
fn active_installations(prefix: &Path) -> Option<BTreeSet<String>> {
    let root = prefix.join("installations");
    let canonical_root = std::fs::canonicalize(&root).ok();
    let mut active = BTreeSet::new();
    for id in MANAGED {
        let bytes = match std::fs::read(prefix.join(format!("{id}.active.json"))) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        let directory: PathBuf = serde_json::from_slice(&bytes).ok()?;
        let Some(parent) = directory.parent() else {
            continue;
        };
        // A record outside installations never enters PATH (managed_bin_dirs).
        let inside = parent == root
            || canonical_root.as_deref().is_some_and(|canonical| {
                std::fs::canonicalize(parent).is_ok_and(|parent| parent == canonical)
            });
        if let (true, Some(name)) = (inside, directory.file_name()) {
            active.insert(name.to_string_lossy().into_owned());
        }
    }
    Some(active)
}

/// Which installation directories may go: not active, not in use, not a
/// trash directory, and old enough that no other install still writes them.
fn installation_removals(
    entries: &[(String, bool)],
    active: &BTreeSet<String>,
    in_use: impl Fn(&str) -> bool,
) -> Vec<String> {
    entries
        .iter()
        .filter(|(name, stale)| {
            *stale && !name.starts_with('.') && !active.contains(name) && !in_use(name)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

fn cleanup_blocking(prefix: &Path, live: Option<&LiveProcesses>, now: SystemTime) {
    let Some(live) = live else {
        tracing::debug!("process table unreadable; adapter installations kept");
        return;
    };
    let root = prefix.join("installations");
    disk_cleanup::sweep_trash(&root);
    disk_cleanup::sweep_trash(prefix);
    match active_installations(prefix) {
        Some(active) => {
            let entries: Vec<(String, bool)> = disk_cleanup::child_dirs(&root)
                .into_iter()
                .filter_map(|path| {
                    let name = path.file_name()?.to_string_lossy().into_owned();
                    Some((name, disk_cleanup::older_than(&path, INSTALL_GRACE, now)))
                })
                .collect();
            let remove: Vec<PathBuf> =
                installation_removals(&entries, &active, |name| live.uses(&root.join(name)))
                    .into_iter()
                    .map(|name| root.join(name))
                    .collect();
            disk_cleanup::remove_all(&root, &remove, "ACP adapter installation");
        }
        None => tracing::warn!("unreadable adapter activation record; installations kept"),
    }
    // Older runtimes shared one npm cache across installs, and the manual
    // install command still uses it. A running npm names it in its arguments
    // and writes `_logs` when it starts.
    let cache = prefix.join("cache");
    let quiet = ["", "_cacache", "_cacache/tmp", "_logs"]
        .iter()
        .all(|part| {
            let path = cache.join(part);
            !path.exists() || disk_cleanup::older_than(&path, INSTALL_GRACE, now)
        });
    if cache.is_dir() && quiet && !live.uses(&cache) {
        disk_cleanup::remove_all(prefix, &[cache], "shared npm cache");
    }
}
fn ensure_staged_target(executable: &std::path::Path, stage: &std::path::Path) -> Result<()> {
    let resolved = std::fs::canonicalize(executable).context("Resolve staged ACP adapter")?;
    let root = std::fs::canonicalize(stage)?;
    anyhow::ensure!(
        resolved.starts_with(root) && resolved.is_file(),
        "Staged adapter points outside its private installation"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapter_installs_use_a_fresh_prefix_and_network_retries() {
        let args = args_for(
            "codex",
            Path::new("/fresh/install"),
            Path::new("/fresh/install/.npm-cache"),
        )
        .unwrap();
        assert_eq!(
            args[args.iter().position(|a| a == "--prefix").unwrap() + 1],
            "/fresh/install"
        );
        assert_eq!(
            args[args.iter().position(|a| a == "--cache").unwrap() + 1],
            "/fresh/install/.npm-cache"
        );
        assert!(args.iter().any(|a| a == "--fetch-retries=3"));
        assert!(args
            .iter()
            .any(|a| a == "@agentclientprotocol/codex-acp@latest"));
        assert!(args_for("unknown", Path::new("/tmp"), Path::new("/tmp/c")).is_err());
    }
    #[test]
    fn only_inactive_unused_settled_installations_are_selected() {
        let entries = [
            ("claude-new".to_string(), true),
            ("claude-old".to_string(), true),
            ("codex-busy".to_string(), true),
            ("codex-fresh".to_string(), false),
            (".trash-x".to_string(), true),
        ];
        let active = BTreeSet::from(["claude-new".to_string()]);
        assert_eq!(
            installation_removals(&entries, &active, |name| name == "codex-busy"),
            ["claude-old"]
        );
    }
    #[test]
    fn cleanup_keeps_active_and_live_installations_and_stops_on_unreadable_records() {
        let prefix = tempfile::tempdir().unwrap();
        let root = prefix.path().join("installations");
        for name in ["claude-new", "claude-old", "codex-live", "codex-old"] {
            std::fs::create_dir_all(root.join(name).join("bin")).unwrap();
        }
        std::fs::create_dir_all(prefix.path().join("cache/_cacache")).unwrap();
        let record = |id: &str, directory: &Path| {
            std::fs::write(
                prefix.path().join(format!("{id}.active.json")),
                serde_json::to_vec(directory).unwrap(),
            )
            .unwrap()
        };
        record("claude", &root.join("claude-new"));
        // Outside installations: never on PATH, so it protects nothing.
        record("codex", &prefix.path().join("elsewhere"));
        let live = LiveProcesses::from_texts([format!(
            "{}/bin/codex-acp",
            root.join("codex-live").display()
        )]);
        let count = || disk_cleanup::child_dirs(&root).len();
        // Everything is younger than the grace period.
        cleanup_blocking(prefix.path(), Some(&live), SystemTime::now());
        assert_eq!(count(), 4);
        assert!(prefix.path().join("cache").is_dir());
        let later = SystemTime::now() + INSTALL_GRACE * 2;
        // No process table: nothing can be proven unused.
        cleanup_blocking(prefix.path(), None, later);
        assert_eq!(count(), 4);
        std::fs::write(prefix.path().join("claude.active.json"), b"{broken").unwrap();
        cleanup_blocking(prefix.path(), Some(&live), later);
        assert_eq!(count(), 4);

        record("claude", &root.join("claude-new"));
        cleanup_blocking(prefix.path(), Some(&live), later);
        let mut left: Vec<_> = disk_cleanup::child_dirs(&root)
            .into_iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["claude-new", "codex-live"]);
        assert!(!prefix.path().join("cache").exists());
    }
    #[test]
    fn incomplete_or_external_staged_adapter_never_activates() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("adapter");
        std::fs::write(&outside, "fixture").unwrap();
        assert!(ensure_staged_target(&outside, root.path()).is_err());
        assert!(ensure_staged_target(&root.path().join("missing"), root.path()).is_err());
        let inside = root.path().join("adapter");
        std::fs::write(&inside, "fixture").unwrap();
        assert!(ensure_staged_target(&inside, root.path()).is_ok());
    }
}
