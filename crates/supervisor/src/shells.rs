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
    /// Private modes (`CSI ? N h/l`) in effect at the end of `data`, in the
    /// order they were first set. Full-screen programs such as Codex set the
    /// alternate screen and mouse tracking once at startup and then only
    /// repaint, so those switches leave the window after enough output.
    modes: Vec<(u16, bool)>,
    /// The tail of the last chunk when it ended inside a mode sequence.
    pending: String,
    /// Whether the start of the output has been dropped.
    trimmed: bool,
}

/// Modes a terminal rebuilt from the replay must know: the alternate screen,
/// mouse tracking and encodings, alternate scroll, application cursor keys,
/// bracketed paste, focus reports and cursor visibility. Without them a
/// reattached viewer of a full-screen program shows its repaints in the
/// normal buffer and the wheel never reaches the program.
const REPLAYED_MODES: [u16; 14] = [
    1, 25, 47, 1047, 1049, 1000, 1002, 1003, 1004, 1005, 1006, 1007, 1015, 2004,
];

impl ReplayBuffer {
    fn push(&mut self, chunk: &str) -> u64 {
        self.track_modes(chunk);
        self.data.push_str(chunk);
        if self.data.len() > REPLAY_LIMIT {
            let mut cut = self.data.len() - REPLAY_LIMIT;
            while !self.data.is_char_boundary(cut) {
                cut += 1;
            }
            // Start at a line or at an escape sequence, never inside one:
            // repaint-only output has no newlines for long stretches.
            if let Some(next) = self.data[cut..].find(['\n', '\x1b']) {
                cut += next;
                if self.data[cut..].starts_with('\n') {
                    cut += 1;
                }
            }
            self.data.drain(..cut);
            self.trimmed = true;
        }
        self.seq += 1;
        self.seq
    }

    fn track_modes(&mut self, chunk: &str) {
        let text = std::mem::take(&mut self.pending) + chunk;
        let bytes = text.as_bytes();
        let mut i = 0;
        while let Some(found) = text[i..].find("\x1b[?") {
            let start = i + found;
            let mut end = start + 3;
            while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b';') {
                end += 1;
            }
            if end == bytes.len() {
                // Cut across chunks: finish it with the next one (bounded).
                if end - start <= 64 {
                    self.pending = text[start..].to_string();
                }
                return;
            }
            let on = match bytes[end] {
                b'h' => Some(true),
                b'l' => Some(false),
                _ => None,
            };
            if let Some(on) = on {
                for mode in text[start + 3..end]
                    .split(';')
                    .filter_map(|m| m.parse::<u16>().ok())
                {
                    if !REPLAYED_MODES.contains(&mode) {
                        continue;
                    }
                    match self.modes.iter_mut().find(|(m, _)| *m == mode) {
                        Some(entry) => entry.1 = on,
                        None => self.modes.push((mode, on)),
                    }
                }
            }
            i = end + 1;
        }
    }

    /// What a new viewer replays: once the start has been dropped, the modes in
    /// effect come first, so the rebuilt terminal is in the program's state.
    fn snapshot(&self) -> String {
        if !self.trimmed {
            return self.data.clone();
        }
        let preamble: String = self
            .modes
            .iter()
            .filter(|(_, on)| *on)
            .map(|(mode, _)| format!("\x1b[?{mode}h"))
            .collect();
        preamble + &self.data
    }
}

struct LiveShell {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    #[allow(dead_code)]
    events: broadcast::Sender<String>,
    thread_id: String,
    execution: Option<pockymoe_runtime::execution::ExecutionContext>,
    host_receipt: Option<std::path::PathBuf>,
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
        let id = Uuid::new_v4().to_string();
        let execution = pockymoe_runtime::execution::current();
        let guardian = if execution.is_none() {
            pockymoe_runtime::execution::host_process::terminal(&id)?
        } else {
            None
        };
        let mut cmd = if let Some(context) = &execution {
            let env = context.checked()?;
            let docker = context.std_docker()?;
            let mut cmd = CommandBuilder::new(docker.get_program());
            cmd.args(docker.get_args());
            cmd.env_clear();
            cmd.env("PATH", "/usr/local/bin:/usr/bin:/bin");
            cmd.args(["exec", "-it", "-w", cwd]);
            for (key, value) in context.launch_env() {
                cmd.args(["-e", &format!("{key}={value}")]);
            }
            cmd.arg(
                env.container_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("environment container missing"))?,
            );
            cmd.args([
                "/opt/pockymoe/bin/pockymoe",
                "environment-worker",
                "shell",
                &id,
            ]);
            cmd
        } else if let Some((exe, path)) = &guardian {
            let mut command = CommandBuilder::new(exe);
            command.args(["environment-worker", "host-shell", &id, &default_shell()]);
            command.env("POCKYMOE_HOST_PROCESS_RECEIPT", path);
            command
        } else {
            CommandBuilder::new(default_shell())
        };
        // The owner's shell, but not the Supervisor's own credentials.
        for key in pockymoe_runtime::supervisor_secret_env() {
            cmd.env_remove(key);
        }
        if execution.is_none() {
            cmd.cwd(cwd);
            terminal_env(&mut cmd, |name| std::env::var_os(name));
            // `codex`, `claude` and their Node right after a first install.
            cmd.env("PATH", pockymoe_runtime::acp::child_path());
        }
        let child = pair.slave.spawn_command(cmd)?;
        let host_receipt = guardian.map(|(_, path)| path);
        if let Some(path) = &host_receipt {
            pockymoe_runtime::execution::host_process::register_terminal(
                path,
                child
                    .process_id()
                    .ok_or_else(|| anyhow!("Host terminal PID missing"))?,
            )?;
        }
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (tx, _) = broadcast::channel(64);
        let events = tx.clone();

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
        let output_execution = execution.clone();
        let read_output = move || {
            let mut buf = [0u8; 4096];
            let mut pending = Vec::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if output_execution
                            .as_ref()
                            .is_some_and(|context| context.checked().is_err())
                        {
                            break;
                        }
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
                if let Some(context) = &shell.execution {
                    let _ = context.kill_sync(&output_id);
                }
                if let Some(path) = &shell.host_receipt {
                    let _ = pockymoe_runtime::execution::host_process::stop_terminal(path);
                }
                let mut child = shell.child.lock().unwrap();
                if child.try_wait().ok().flatten().is_none() {
                    if shell.host_receipt.is_none() {
                        let _ = child.kill();
                    }
                }
                let _ = child.wait();
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
                execution,
                host_receipt,
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
        if let Some(context) = &shell.execution {
            context.checked()?;
        }
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
        if let Some(context) = &shell.execution {
            context.checked()?;
        }
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
        // Remove the handle even if its generation has already been fenced. Only
        // cleanup may use the captured old container; no new input is admitted.
        let shell = self
            .inner
            .lock()
            .unwrap()
            .shells
            .remove(id)
            .ok_or_else(|| anyhow!("shell not found"))?;
        let cleanup = shell
            .execution
            .as_ref()
            .map(|context| context.kill_sync(id));
        if let Some(path) = &shell.host_receipt {
            pockymoe_runtime::execution::host_process::stop_terminal(path)?;
        }
        let mut child = shell.child.lock().unwrap();
        if child.try_wait()?.is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
        let mut terminated = Self::shell_value(&shell, id);
        terminated["status"] = json!("exited");
        terminated["updatedAt"] = json!(now_rfc3339());
        if let Some(result) = cleanup {
            result?;
        }
        Ok(terminated)
    }

    pub fn terminate_thread(&self, thread: &str) {
        let _ = self.terminate_thread_checked(thread);
    }

    pub fn terminate_thread_checked(&self, thread: &str) -> Result<()> {
        let ids = self
            .inner
            .lock()
            .unwrap()
            .shells
            .iter()
            .filter(|(_, s)| s.thread_id == thread)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let mut failed = false;
        for id in ids {
            failed |= self.terminate(&id).is_err();
        }
        anyhow::ensure!(!failed, "thread terminal cleanup could not be confirmed");
        Ok(())
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
        let hub = self.inner.lock().unwrap();
        let shell = hub
            .shells
            .get(id)
            .ok_or_else(|| anyhow!("shell not found"))?;
        if let Some(context) = &shell.execution {
            context.checked()?;
        }
        let replay = shell.replay.lock().unwrap();
        Ok((replay.snapshot(), replay.seq))
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
        if let Some(context) = &shell.execution {
            context.checked()?;
        }
        Ok(Self::shell_value(shell, id))
    }

    /// Whether a command, not the shell's prompt, owns the terminal now; `None`
    /// when this cannot be told (a container shell).
    pub fn command_running(&self, id: &str) -> Option<bool> {
        let hub = self.inner.lock().unwrap();
        hub.shells.get(id).and_then(LiveShell::command_running)
    }

    fn shell_value(shell: &LiveShell, id: &str) -> Value {
        let command_running = shell.command_running();
        let metadata = shell.metadata.lock().unwrap();
        json!({
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
            "rows": metadata.rows,
            "commandRunning": command_running
        })
    }
}

impl LiveShell {
    /// A job-control shell hands the terminal's foreground process group to the
    /// command it runs and takes it back at the prompt. The prompt belongs to the
    /// spawned shell, or, under the host guardian, to the shell the guardian
    /// started as its child.
    #[cfg(not(unix))]
    fn command_running(&self) -> Option<bool> {
        None
    }

    #[cfg(unix)]
    fn command_running(&self) -> Option<bool> {
        if self.execution.is_some() {
            // `docker exec` owns the local terminal; the container's shell is
            // out of reach, so the caller must assume a command may be running.
            return None;
        }
        let foreground = self.master.lock().unwrap().process_group_leader()?;
        let child = i32::try_from(self.child.lock().unwrap().process_id()?).ok()?;
        if foreground == child {
            return Some(false);
        }
        if self.host_receipt.is_some() {
            return Some(parent_pid(foreground)? != child);
        }
        Some(true)
    }
}

/// The parent of a process, from `/proc` (the host guardian is Linux-only).
#[cfg(unix)]
fn parent_pid(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (comm) state ppid ...`; the command name may itself contain `)`.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
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
        for key in pockymoe_runtime::supervisor_secret_env() {
            cmd.env_remove(key);
        }
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
        for key in pockymoe_runtime::supervisor_secret_env() {
            cmd.env_remove(key);
        }
        cmd.env("LANG", "zh_CN.UTF-8");
        terminal_env(&mut cmd, |name| {
            (name == "LANG").then(|| OsString::from("zh_CN.UTF-8"))
        });
        assert_eq!(cmd.get_env("LANG"), Some(OsStr::new("zh_CN.UTF-8")));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod command_running_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for(hub: &ShellHub, id: &str, expected: bool) -> Option<bool> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let running = hub.command_running(id);
            if running == Some(expected) || Instant::now() > deadline {
                return running;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn a_running_command_is_told_apart_from_the_prompt() {
        let hub = ShellHub::default();
        let cwd = std::env::temp_dir();
        let (id, _) = hub
            .create("thread", "workspace", cwd.to_str().unwrap(), 80, 24, None)
            .unwrap();
        assert_eq!(wait_for(&hub, &id, false), Some(false));
        hub.write(&id, "sleep 5\n").unwrap();
        assert_eq!(wait_for(&hub, &id, true), Some(true));
        assert_eq!(hub.get(&id).unwrap()["commandRunning"], true);
        hub.write(&id, "\u{3}").unwrap();
        assert_eq!(wait_for(&hub, &id, false), Some(false));
        hub.terminate(&id).unwrap();
    }
}

#[cfg(test)]
mod replay_tests {
    use super::*;

    #[test]
    fn a_trimmed_replay_restores_the_modes_a_full_screen_program_set_at_startup() {
        let mut replay = ReplayBuffer::default();
        // Codex: alternate screen, mouse tracking, SGR encoding, bracketed paste;
        // the last sequence is split across two chunks.
        replay.push("$ codex\r\n\x1b[?2004h\x1b[?1049h\x1b[?1007l\x1b[?1000h\x1b[?10");
        replay.push("02h\x1b[?1003h\x1b[?1006h\x1b[?25l");
        assert_eq!(
            replay.snapshot(),
            replay.data,
            "nothing dropped, nothing added"
        );
        // Then it only repaints, past the window, without newlines.
        let frame = format!("\x1b[3;1H\x1b[Kredraw {}", "x".repeat(60));
        for _ in 0..(REPLAY_LIMIT / frame.len() + 50) {
            replay.push(&frame);
        }
        let snapshot = replay.snapshot();
        assert!(
            snapshot
                .starts_with("\x1b[?2004h\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h"),
            "{:?}",
            &snapshot[..80]
        );
        assert!(
            !snapshot.contains("\x1b[?1007h") && !snapshot.contains("\x1b[?25h"),
            "modes turned off stay off"
        );
        // The kept output starts at an escape sequence, not inside one.
        let body = &snapshot[snapshot.find("\x1b[3;1H").unwrap()..];
        assert_eq!(body.len(), replay.data.len());
        assert!(replay.data.starts_with('\x1b'), "{:?}", &replay.data[..20]);
        assert!(replay.data.len() <= REPLAY_LIMIT);

        // Leaving the program turns them off again.
        replay.push("\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?1049l\x1b[?25h$ ");
        let snapshot = replay.snapshot();
        assert!(
            snapshot.starts_with("\x1b[?2004h\x1b[?25h"),
            "{:?}",
            &snapshot[..40]
        );
        assert!(!snapshot[..40].contains("1049h"));
    }
}
