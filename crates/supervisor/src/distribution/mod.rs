//! Native, user-owned runtime distribution. GitHub releases are authoritative.
mod releases;
mod service;
mod setup;
mod update;

pub use setup::{run_device, setup, SetupOptions};
pub use update::{maintenance_cli, run_worker, updater};

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
fn install_root() -> PathBuf {
    home().join(".local/share/remote-codex/native")
}
fn config_path() -> PathBuf {
    std::env::var_os("POCKYMOE_RELAY_SUPERVISOR_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".remote-codex/relay-supervisor.json"))
}
fn read<T: DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("Read {}", path.display()))?,
    )
    .with_context(|| format!("Parse {}", path.display()))
}
/// `relay-supervisor.json` is shared with Windows Device Managers and with
/// installations from before the rename, which read only `REMOTE_CODEX_*` keys:
/// accept either name (the new one wins) and write the old one.
fn read_device_config(path: &Path) -> Result<serde_json::Value> {
    let mut saved: serde_json::Value = read(path)?;
    if let Some(object) = saved.as_object_mut() {
        let legacy: Vec<String> = object
            .keys()
            .filter(|key| key.starts_with(LEGACY_PREFIX))
            .cloned()
            .collect();
        for key in legacy {
            let value = object.remove(&key).unwrap_or_default();
            let current = format!("{CURRENT_PREFIX}{}", &key[LEGACY_PREFIX.len()..]);
            object.entry(current).or_insert(value);
        }
    }
    Ok(saved)
}
fn write_device_config(path: &Path, saved: &serde_json::Value) -> Result<()> {
    let mut legacy = saved.clone();
    if let Some(object) = legacy.as_object_mut() {
        *object = std::mem::take(object)
            .into_iter()
            .map(|(key, value)| match key.strip_prefix(CURRENT_PREFIX) {
                Some(rest) => (format!("{LEGACY_PREFIX}{rest}"), value),
                None => (key, value),
            })
            .collect();
    }
    write(path, &legacy)
}
const LEGACY_PREFIX: &str = "REMOTE_CODEX_";
const CURRENT_PREFIX: &str = "POCKYMOE_";
/// Agent CLIs live in directories that shell startup files add (nvm,
/// ~/.local/bin, ~/.grok/bin, ...). Service managers start the Supervisor with
/// a minimal PATH, so merge in the PATH of the user's login shell; without it
/// harness detection and launches miss installed agents.
pub fn user_path() -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    #[cfg(unix)]
    {
        merge_paths(login_shell_path().as_deref(), &current, &home())
    }
    #[cfg(not(unix))]
    {
        current
    }
}
#[cfg(unix)]
fn login_shell_path() -> Option<String> {
    const MARK: &str = "__POCKYMOE_PATH__";
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(passwd_shell)
        // macOS keeps users in Directory Services, not /etc/passwd.
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh"
            } else {
                "/bin/sh"
            }
            .into()
        });
    let mut child = std::process::Command::new(shell)
        .args(["-ilc", &format!("printf '{MARK}%s{MARK}' \"$PATH\"")])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    while child.try_wait().ok()?.is_none() {
        if started.elapsed() > std::time::Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    let start = out.find(MARK)? + MARK.len();
    let end = start + out[start..].find(MARK)?;
    Some(out[start..end].to_string())
}
#[cfg(unix)]
fn passwd_shell() -> Option<String> {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()?;
    std::fs::read_to_string("/etc/passwd")
        .ok()?
        .lines()
        .find(|line| line.split(':').next() == Some(user.as_str()))
        .and_then(|line| line.split(':').nth(6))
        .map(str::to_string)
        .filter(|shell| !shell.is_empty())
}
/// Shell entries first, in the user's order, then the rest of the current
/// PATH; common user bin directories are added when the shell gave nothing.
fn merge_paths(shell: Option<&str>, current: &str, home: &Path) -> String {
    let fallback = [".local/bin", "bin", ".cargo/bin", ".bun/bin"]
        .map(|dir| home.join(dir))
        .into_iter()
        .filter(|dir| dir.is_dir())
        .map(|dir| dir.to_string_lossy().into_owned());
    let mut seen = std::collections::BTreeSet::new();
    shell
        .unwrap_or_default()
        .split(':')
        .map(str::to_string)
        .chain(current.split(':').map(str::to_string))
        .chain(fallback.filter(|_| shell.is_none()))
        .filter(|entry| !entry.is_empty() && seen.insert(entry.clone()))
        .collect::<Vec<_>>()
        .join(":")
}
fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn write<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_bytes(path, &serde_json::to_vec_pretty(value)?)
}
fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    private_dir(path.parent().context("Missing parent directory")?)?;
    let next = path.with_extension(format!("{}.next", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&next)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&next, path)?;
    Ok(())
}
fn executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = path;
    Ok(())
}
fn binary_name() -> &'static str {
    if cfg!(windows) {
        "remote-codex.exe"
    } else {
        "remote-codex"
    }
}

#[cfg(test)]
mod device_config_tests {
    use super::*;

    #[test]
    fn device_config_reads_either_key_name_and_writes_the_legacy_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay-supervisor.json");
        std::fs::write(
            &path,
            r#"{"REMOTE_CODEX_RELAY_AGENT_TOKEN":"old","POCKYMOE_RELAY_SERVER_URL":"wss://new","REMOTE_CODEX_RELAY_SERVER_URL":"wss://old","other":1}"#,
        )
        .unwrap();
        let saved = read_device_config(&path).unwrap();
        assert_eq!(
            saved,
            serde_json::json!({"POCKYMOE_RELAY_AGENT_TOKEN":"old","POCKYMOE_RELAY_SERVER_URL":"wss://new","other":1})
        );
        write_device_config(&path, &saved).unwrap();
        let written: serde_json::Value = read(&path).unwrap();
        assert_eq!(
            written,
            serde_json::json!({"REMOTE_CODEX_RELAY_AGENT_TOKEN":"old","REMOTE_CODEX_RELAY_SERVER_URL":"wss://new","other":1})
        );
    }
}

#[cfg(all(test, unix))]
mod path_tests {
    use super::*;

    #[test]
    fn shell_path_comes_first_and_duplicates_are_dropped() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".local/bin")).unwrap();
        assert_eq!(
            merge_paths(Some("/nvm/bin:/usr/bin:"), "/usr/bin:/bin", home.path()),
            "/nvm/bin:/usr/bin:/bin"
        );
        let local = home.path().join(".local/bin");
        assert_eq!(
            merge_paths(None, "/usr/bin", home.path()),
            format!("/usr/bin:{}", local.display())
        );
    }
}
