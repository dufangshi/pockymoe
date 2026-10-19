use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Result};
use pockymoe_protocol::now_rfc3339;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Bytes kept per shell so a reattaching viewer (reload, reconnect, a second
/// pane) sees the recent screen instead of a blank terminal.
const REPLAY_LIMIT: usize = 256 * 1024;

#[derive(Clone)]
pub struct ShellOutput {
    pub shell_id: String,
    pub thread_id: String,
    pub data: String,
    /// Per-shell order. Attach replays everything up to a sequence number and
    /// skips broadcast chunks at or below it, so no output is shown twice.
    pub seq: u64,
    pub exited: bool,
}

#[derive(Default)]
struct ReplayBuffer {
    data: String,
    seq: u64,
}

impl ReplayBuffer {
    fn push(&mut self, chunk: &str) -> u64 {
        self.data.push_str(chunk);
        if self.data.len() > REPLAY_LIMIT {
            let mut cut = self.data.len() - REPLAY_LIMIT;
            while !self.data.is_char_boundary(cut) {
                cut += 1;
            }
            // Prefer a line start so replay does not begin inside an escape sequence.
            if let Some(newline) = self.data[cut..].find('\n') {
                cut += newline + 1;
            }
            self.data.drain(..cut);
        }
        self.seq += 1;
        self.seq
    }
}

struct LiveShell {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    #[allow(dead_code)]
    events: broadcast::Sender<String>,
    thread_id: String,
    workspace_id: String,
    cwd: String,
    tmux_session_name: String,
    created_at: String,
    metadata: Arc<Mutex<ShellMetadata>>,
    replay: Arc<Mutex<ReplayBuffer>>,
}

struct ShellMetadata {
    label: Option<String>,
    cols: u16,
    rows: u16,
    updated_at: String,
    last_activity_at: Option<String>,
}

struct HubState {
    shells: HashMap<String, LiveShell>,
    all: broadcast::Sender<ShellOutput>,
}

pub struct ShellHub {
    inner: Arc<Mutex<HubState>>,
}

impl Default for ShellHub {
    fn default() -> Self {
        let (all, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Mutex::new(HubState {
                shells: HashMap::new(),
                all,
            })),
        }
    }
}

pub fn hub() -> &'static ShellHub {
    static HUB: std::sync::OnceLock<ShellHub> = std::sync::OnceLock::new();
    HUB.get_or_init(ShellHub::default)
}

impl ShellHub {
    pub fn create(
        &self,
        thread_id: &str,
        workspace_id: &str,
        cwd: &str,
        cols: u16,
        rows: u16,
        label: Option<String>,
    ) -> Result<(String, Value)> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = CommandBuilder::new(default_shell());
        cmd.cwd(cwd);
        terminal_env(&mut cmd, |name| std::env::var_os(name));
        let child = pair.slave.spawn_command(cmd)?;
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (tx, _) = broadcast::channel(64);
        let events = tx.clone();
        let id = Uuid::new_v4().to_string();
        let all = {
            let hub = self.inner.lock().unwrap();
            hub.all.clone()
        };
        let hub_state = self.inner.clone();
        let output_id = id.clone();
        let output_thread = thread_id.to_string();
        let replay = Arc::new(Mutex::new(ReplayBuffer::default()));
        let output_replay = replay.clone();
        let now = now_rfc3339();
        let metadata = Arc::new(Mutex::new(ShellMetadata {
            label: label.and_then(|label| {
                let label = label.trim();
                (!label.is_empty()).then(|| label.to_string())
            }),
            cols,
            rows,
            updated_at: now.clone(),
            last_activity_at: None,
        }));
        let output_metadata = metadata.clone();
        let read_output = move || {
            let mut buf = [0u8; 4096];
            let mut pending = Vec::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        // Keep an incomplete UTF-8 sequence for the next read instead
                        // of replacing it, so split CJK/emoji output stays intact.
                        pending.extend_from_slice(&buf[..n]);
                        let valid = match std::str::from_utf8(&pending) {
                            Ok(_) => pending.len(),
                            Err(error) if error.error_len().is_none() => error.valid_up_to(),
                            Err(_) => pending.len(),
                        };
                        if valid == 0 {
                            continue;
                        }
                        let data = String::from_utf8_lossy(&pending[..valid]).into_owned();
                        pending.drain(..valid);
                        let activity_at = now_rfc3339();
                        let mut metadata = output_metadata.lock().unwrap();
                        metadata.updated_at = activity_at.clone();
                        metadata.last_activity_at = Some(activity_at);
                        drop(metadata);
                        let _ = events.send(data.clone());
                        // Sequence and broadcast under the replay lock; attach snapshots
                        // under the same lock, which makes the split exact.
                        let mut replay = output_replay.lock().unwrap();
                        let seq = replay.push(&data);
                        let _ = all.send(ShellOutput {
                            shell_id: output_id.clone(),
                            thread_id: output_thread.clone(),
                            data,
                            seq,
                            exited: false,
                        });
                    }
                }
            }
            // The process ended (`exit`, a crash or terminate). Drop the entry so it
            // no longer lists as running, reap it, and tell attached viewers.
            let removed = hub_state.lock().unwrap().shells.remove(&output_id);
            if let Some(shell) = removed {
                let _ = shell.child.lock().unwrap().wait();
            }
            let seq = output_replay.lock().unwrap().seq + 1;
            let _ = all.send(ShellOutput {
                shell_id: output_id,
                thread_id: output_thread,
                data: String::new(),
                seq,
                exited: true,
            });
        };
        self.inner.lock().unwrap().shells.insert(
            id.clone(),
            LiveShell {
                writer: Mutex::new(writer),
                master: Mutex::new(pair.master),
                child: Mutex::new(child),
                events: tx,
                thread_id: thread_id.into(),
                workspace_id: workspace_id.into(),
                cwd: cwd.into(),
                tmux_session_name: format!("pockymoe-pty-{id}"),
                created_at: now,
                metadata,
                replay,
            },
        );
        // Start reading only once the entry exists, so an immediate exit removes it.
        thread::spawn(read_output);
        Ok((id.clone(), self.shell_json(&id)?))
    }

    pub fn write(&self, id: &str, data: &str) -> Result<()> {
        let hub = self.inner.lock().unwrap();
        let shell = hub
            .shells
            .get(id)
            .ok_or_else(|| anyhow!("shell not found"))?;
        shell.writer.lock().unwrap().write_all(data.as_bytes())?;
        let now = now_rfc3339();
        let mut metadata = shell.metadata.lock().unwrap();
        metadata.updated_at = now.clone();
        metadata.last_activity_at = Some(now);
        Ok(())
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<()> {
        let hub = self.inner.lock().unwrap();
        let shell = hub
            .shells
            .get(id)
            .ok_or_else(|| anyhow!("shell not found"))?;
        shell.master.lock().unwrap().resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut metadata = shell.metadata.lock().unwrap();
        metadata.cols = cols;
        metadata.rows = rows;
        metadata.updated_at = now_rfc3339();
        Ok(())
    }

    pub fn update_label(&self, id: &str, label: Option<String>) -> Result<Value> {
        let hub = self.inner.lock().unwrap();
        let shell = hub
            .shells
            .get(id)
            .ok_or_else(|| anyhow!("shell not found"))?;
        let mut metadata = shell.metadata.lock().unwrap();
        metadata.label = label.and_then(|label| {
            let label = label.trim();
            (!label.is_empty()).then(|| label.to_string())
        });
        metadata.updated_at = now_rfc3339();
        drop(metadata);
        self.shell_json_locked(&hub, id)
    }

    pub fn terminate(&self, id: &str) -> Result<Value> {
        let mut hub = self.inner.lock().unwrap();
        let terminated = {
            let shell = hub
                .shells
                .get(id)
                .ok_or_else(|| anyhow!("shell not found"))?;
            let mut child = shell.child.lock().unwrap();
            child.kill()?;
            child.wait()?;
            let mut value = self.shell_json_locked(&hub, id)?;
            value["status"] = json!("exited");
            value["updatedAt"] = json!(now_rfc3339());
            value
        };
        hub.shells.remove(id);
        Ok(terminated)
    }

    pub fn subscribe_all(&self) -> broadcast::Receiver<ShellOutput> {
        self.inner.lock().unwrap().all.subscribe()
    }

    pub fn list_for_thread(&self, thread_id: &str) -> Vec<Value> {
        let hub = self.inner.lock().unwrap();
        hub.shells
            .iter()
            .filter(|(_, shell)| shell.thread_id == thread_id)
            .filter_map(|(id, _)| self.shell_json_locked(&hub, id).ok())
            .collect()
    }

    pub(crate) fn get(&self, id: &str) -> Result<Value> {
        self.shell_json(id)
    }

    /// Recent output for a new viewer and the last sequence it covers.
    pub(crate) fn replay(&self, id: &str) -> Result<(String, u64)> {
        let replay = {
            let hub = self.inner.lock().unwrap();
            hub.shells
                .get(id)
                .ok_or_else(|| anyhow!("shell not found"))?
                .replay
                .clone()
        };
        let replay = replay.lock().unwrap();
        Ok((replay.data.clone(), replay.seq))
    }

    fn shell_json(&self, id: &str) -> Result<Value> {
        let hub = self.inner.lock().unwrap();
        self.shell_json_locked(&hub, id)
    }

    fn shell_json_locked(&self, hub: &HubState, id: &str) -> Result<Value> {
        let shell = hub
            .shells
            .get(id)
            .ok_or_else(|| anyhow!("shell not found"))?;
        let metadata = shell.metadata.lock().unwrap();
        Ok(json!({
            "id": id,
            "threadId": shell.thread_id,
            "workspaceId": shell.workspace_id,
            "label": metadata.label,
            "tmuxSessionName": shell.tmux_session_name,
            "backend": "pty",
            "cwd": shell.cwd,
            "status": "running",
            "attachedViewerId": Value::Null,
            "createdAt": shell.created_at,
            "updatedAt": metadata.updated_at,
            "lastActivityAt": metadata.last_activity_at,
            "cols": metadata.cols,
            "rows": metadata.rows
        }))
    }
}

/// The shell's terminal is xterm.js, whatever terminal (if any) started the
/// Supervisor. A service manager starts it with no TERM and no locale: zsh then
/// has no way to move the cursor left, so a deletion redraws as spaces after the
/// text, and multibyte input breaks. Variables naming the Supervisor's own
/// terminal (an enclosing tmux, Terminal.app) would mislead programs here.
fn terminal_env(cmd: &mut CommandBuilder, get: impl Fn(&str) -> Option<std::ffi::OsString>) {
    if cfg!(windows) {
        return;
    }
    for name in [
        "TMUX",
        "TMUX_PANE",
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "TERM_SESSION_ID",
    ] {
        cmd.env_remove(name);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    let has_locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .any(|name| get(name).is_some_and(|value| !value.is_empty()));
    if !has_locale {
        // macOS ships no C.UTF-8; Linux may lack en_US.UTF-8.
        cmd.env(
            "LANG",
            if cfg!(target_os = "macos") {
                "en_US.UTF-8"
            } else {
                "C.UTF-8"
            },
        );
    }
}

fn default_shell() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    }
}

#[cfg(all(test, not(windows)))]
mod terminal_env_tests {
    use super::*;
    use std::ffi::{OsStr, OsString};

    #[test]
    fn the_shell_gets_an_xterm_and_a_utf8_locale() {
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.env("TERM", "dumb");
        cmd.env("TMUX", "/tmp/tmux-1000/default,1,0");
        terminal_env(&mut cmd, |_| None);
        assert_eq!(cmd.get_env("TERM"), Some(OsStr::new("xterm-256color")));
        assert_eq!(cmd.get_env("COLORTERM"), Some(OsStr::new("truecolor")));
        assert_eq!(cmd.get_env("TMUX"), None);
        assert!(cmd
            .get_env("LANG")
            .is_some_and(|lang| lang.to_string_lossy().ends_with("UTF-8")));
    }

    #[test]
    fn a_locale_the_supervisor_has_is_kept() {
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.env("LANG", "zh_CN.UTF-8");
        terminal_env(&mut cmd, |name| {
            (name == "LANG").then(|| OsString::from("zh_CN.UTF-8"))
        });
        assert_eq!(cmd.get_env("LANG"), Some(OsStr::new("zh_CN.UTF-8")));
    }
}
