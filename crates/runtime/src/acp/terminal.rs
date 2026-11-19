use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::watch;
use uuid::Uuid;

const DEFAULT_OUTPUT_BYTE_LIMIT: usize = 1024 * 1024;
/// After the command exits, how long its exit status waits for stdout/stderr to
/// reach EOF. Descendants that inherited the pipes can keep them open; their
/// later output still lands in the buffer, but the exit is not held back.
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

struct LiveTerminal {
    execution: Option<crate::execution::ExecutionContext>,
    host_guard: Option<crate::execution::host_process::HostProcessContext>,
    process_id: String,
    /// The harness process that created it.
    owner: String,
    output: Arc<Mutex<OutputBuffer>>,
    child_id: u32,
    exit_rx: watch::Receiver<Option<Value>>,
}

#[derive(Default)]
pub struct AgentTerminals {
    inner: Mutex<HashMap<String, LiveTerminal>>,
}

impl AgentTerminals {
    pub async fn create(
        &self,
        owner: &str,
        command: &str,
        args: &[String],
        cwd: PathBuf,
        env: &[(String, String)],
        output_byte_limit: Option<usize>,
    ) -> Result<String> {
        let (executable, parsed_args) = if args.is_empty() {
            parse_command_line(command)?
        } else {
            (command.to_string(), args.to_vec())
        };
        let id = Uuid::new_v4().to_string();
        let mut cmd = Command::new(&executable);
        crate::child_process::hide_tokio(&mut cmd);
        // ACP terminals are spawned by the hub, not the harness. Discard any
        // identity inherited by the hub before applying the owning process env.
        for key in crate::child_process::inherited_host_identity() {
            cmd.env_remove(key);
        }
        cmd.args(&parsed_args)
            .current_dir(&cwd)
            .envs(env.iter().cloned())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let execution = crate::execution::current();
        let host_guard = execution
            .is_none()
            .then(crate::execution::host_process::enabled_context)
            .flatten();
        let process_id = Uuid::new_v4().to_string();
        let mut child = if let Some(context) = &execution {
            let mut argv = vec![executable];
            argv.extend(parsed_args);
            let mut worker_env = context.launch_env();
            worker_env.extend(env.iter().cloned().filter(|(k, _)| {
                !matches!(
                    k.as_str(),
                    "PATH"
                        | "POCKYMOE_URL"
                        | "REMOTE_CODEX_URL"
                        | "POCKYMOE_TOKEN"
                        | "REMOTE_CODEX_TOKEN"
                        | "POCKYMOE_THREAD_ID"
                        | "REMOTE_CODEX_THREAD_ID"
                )
            }));
            context
                .spawn(&crate::execution::RunRequest {
                    id: process_id.clone(),
                    argv,
                    cwd: cwd.to_string_lossy().into(),
                    env: worker_env,
                    stdin_closed: true,
                })
                .await?
        } else {
            crate::execution::host_process::spawn(&mut cmd, &process_id, true, true)
                .await
                .map_err(|err| anyhow!("failed to spawn ACP terminal command `{command}`: {err}"))?
        };
        // Keep the worker transport's stdin open until the finite command exits.
        let transport_stdin = child.stdin.take();
        let child_id = child.id().unwrap_or(0);
        let limit = output_byte_limit
            .unwrap_or(DEFAULT_OUTPUT_BYTE_LIMIT)
            .clamp(1, DEFAULT_OUTPUT_BYTE_LIMIT);
        let output = Arc::new(Mutex::new(OutputBuffer::new(limit)));
        let mut readers = Vec::new();
        if let Some(mut stdout) = child.stdout.take() {
            let output = output.clone();
            readers.push(tokio::spawn(async move {
                collect_output(&mut stdout, &output).await
            }));
        }
        if let Some(mut stderr) = child.stderr.take() {
            let output = output.clone();
            readers.push(tokio::spawn(async move {
                collect_output(&mut stderr, &output).await
            }));
        }
        let (exit_tx, exit_rx) = watch::channel(None);
        tokio::spawn(async move {
            let _keep_transport = transport_stdin;
            let status = match child.wait().await {
                Ok(status) => exit_status_value(status),
                Err(err) => json!({ "exitCode": null, "signal": null, "error": err.to_string() }),
            };
            // A caller that sees the exit reads the output next and releases the
            // terminal, so the last lines must already be in the buffer.
            drain_readers(readers, OUTPUT_DRAIN_TIMEOUT).await;
            let _ = exit_tx.send(Some(status));
        });
        self.inner.lock().unwrap().insert(
            id.clone(),
            LiveTerminal {
                execution,
                host_guard,
                process_id,
                owner: owner.into(),
                output,
                child_id,
                exit_rx,
            },
        );
        Ok(id)
    }

    pub fn ensure_owner(&self, id: &str, owner: &str) -> Result<()> {
        let hub = self.inner.lock().unwrap();
        let term = hub.get(id).ok_or_else(|| anyhow!("terminal not found"))?;
        anyhow::ensure!(term.owner == owner, "foreign ACP terminal");
        if let Some(context) = &term.execution {
            context.checked()?;
        }
        Ok(())
    }
    pub fn output(&self, id: &str) -> Result<Value> {
        let hub = self.inner.lock().unwrap();
        let term = hub.get(id).ok_or_else(|| anyhow!("terminal not found"))?;
        let (retained, truncated) = term.output.lock().unwrap().snapshot();
        let exit_status = term.exit_rx.borrow().clone();
        Ok(json!({
            "output": String::from_utf8_lossy(&retained),
            "truncated": truncated,
            "exitStatus": exit_status
        }))
    }

    pub async fn wait_for_exit(&self, id: &str) -> Result<Value> {
        let mut exit_rx = self
            .inner
            .lock()
            .unwrap()
            .get(id)
            .ok_or_else(|| anyhow!("terminal not found"))?
            .exit_rx
            .clone();
        loop {
            if let Some(status) = exit_rx.borrow().clone() {
                return Ok(status);
            }
            exit_rx
                .changed()
                .await
                .map_err(|_| anyhow!("terminal exit status unavailable"))?;
        }
    }

    pub fn kill(&self, id: &str) -> Result<()> {
        let hub = self.inner.lock().unwrap();
        let term = hub.get(id).ok_or_else(|| anyhow!("terminal not found"))?;
        if term.exit_rx.borrow().is_none() && term.child_id > 0 {
            if let Some(context) = &term.execution {
                let context = context.clone();
                let id = term.process_id.clone();
                tokio::spawn(async move {
                    let _ = context.kill(&id).await;
                });
            } else if let Some(context) = &term.host_guard {
                context.stop_process_sync(&term.process_id)?;
            } else {
                terminate_process(term.child_id)?;
            }
        }
        Ok(())
    }

    pub fn release(&self, id: &str) {
        if let Some(term) = self.inner.lock().unwrap().remove(id) {
            if term.exit_rx.borrow().is_none() && term.child_id > 0 {
                if let Some(context) = term.execution {
                    let id = term.process_id;
                    tokio::spawn(async move {
                        let _ = context.kill(&id).await;
                    });
                } else if let Some(context) = term.host_guard {
                    let _ = context.stop_process_sync(&term.process_id);
                } else {
                    let _ = terminate_process(term.child_id);
                }
            }
        }
    }

    /// Commands a harness process started that are still running.
    pub fn running(&self, owner: &str) -> usize {
        self.inner
            .lock()
            .unwrap()
            .values()
            .filter(|term| term.owner == owner && term.exit_rx.borrow().is_none())
            .count()
    }

    /// Stop and forget every terminal of a released harness process.
    pub fn release_owner(&self, owner: &str) {
        let ids: Vec<_> = self
            .inner
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, term)| term.owner == owner)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.release(&id);
        }
    }
}

/// The last `limit` bytes of a terminal's combined output. Dropping the oldest
/// bytes costs as much as the bytes dropped, not a copy of the whole window.
struct OutputBuffer {
    bytes: VecDeque<u8>,
    limit: usize,
    truncated: bool,
}

impl OutputBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            limit: limit.max(1),
            truncated: false,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        if chunk.len() >= self.limit {
            self.truncated |= chunk.len() > self.limit || !self.bytes.is_empty();
            self.bytes.clear();
            self.bytes.extend(&chunk[chunk.len() - self.limit..]);
            return;
        }
        let overflow = (self.bytes.len() + chunk.len()).saturating_sub(self.limit);
        if overflow > 0 {
            self.bytes.drain(..overflow);
            self.truncated = true;
        }
        self.bytes.extend(chunk);
    }

    /// Retained bytes and the public truncated flag. A cut window starts at the
    /// next UTF-8 character boundary rather than with a replacement character.
    fn snapshot(&self) -> (Vec<u8>, bool) {
        let skip = if self.truncated {
            self.bytes
                .iter()
                .take_while(|byte| (**byte & 0xc0) == 0x80)
                .count()
        } else {
            0
        };
        (self.bytes.range(skip..).copied().collect(), self.truncated)
    }
}

async fn collect_output<R: AsyncRead + Unpin>(reader: &mut R, output: &Mutex<OutputBuffer>) {
    let mut buf = vec![0u8; 4096];
    while let Ok(n) = reader.read(&mut buf).await {
        if n == 0 {
            break;
        }
        output.lock().unwrap().push(&buf[..n]);
    }
}

/// Wait for the output readers to reach EOF, up to `limit` in total. Readers
/// still running afterwards keep appending to the buffer.
async fn drain_readers(readers: Vec<tokio::task::JoinHandle<()>>, limit: Duration) {
    let deadline = tokio::time::Instant::now() + limit;
    for reader in readers {
        if tokio::time::timeout_at(deadline, reader).await.is_err() {
            tracing::debug!("ACP terminal output still open after the command exited");
            return;
        }
    }
}

fn parse_command_line(value: &str) -> Result<(String, Vec<String>)> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for ch in value.chars() {
        if escaped {
            token.push(ch);
            escaped = false;
            started = true;
            continue;
        }
        match (quote, ch) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => token.push(ch),
            (Some('"'), '\\') => escaped = true,
            (Some('"'), _) => token.push(ch),
            (Some(_), _) => unreachable!("unsupported quote delimiter"),
            (None, '\'') => {
                quote = Some('\'');
                started = true;
            }
            (None, '"') => {
                quote = Some('"');
                started = true;
            }
            (None, '\\') => {
                escaped = true;
                started = true;
            }
            (None, ch) if ch.is_whitespace() => {
                if started {
                    tokens.push(std::mem::take(&mut token));
                    started = false;
                }
            }
            (None, _) => {
                token.push(ch);
                started = true;
            }
        }
    }
    if escaped || quote.is_some() {
        bail!("unterminated quote or escape in ACP terminal command");
    }
    if started {
        tokens.push(token);
    }
    let executable = tokens
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("empty terminal command"))?;
    Ok((executable, tokens.into_iter().skip(1).collect()))
}

#[cfg(unix)]
fn exit_status_value(status: std::process::ExitStatus) -> Value {
    use std::os::unix::process::ExitStatusExt;
    json!({ "exitCode": status.code(), "signal": status.signal() })
}

#[cfg(not(unix))]
fn exit_status_value(status: std::process::ExitStatus) -> Value {
    json!({ "exitCode": status.code(), "signal": null })
}

#[cfg(unix)]
fn terminate_process(child_id: u32) -> Result<()> {
    let status = std::process::Command::new("kill")
        .args(["-TERM", &child_id.to_string()])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        bail!("failed to terminate process {child_id}")
    }
}

#[cfg(windows)]
fn terminate_process(child_id: u32) -> Result<()> {
    let mut command = std::process::Command::new("taskkill");
    command.args(["/PID", &child_id.to_string(), "/T", "/F"]);
    crate::child_process::hide_std(&mut command);
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        bail!("failed to terminate process {child_id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn large_terminal_output_is_bounded_on_the_supervisor() {
        let output = Mutex::new(OutputBuffer::new(1024));
        let input = vec![b'x'; 2 * 1024 * 1024];
        let mut reader = input.as_slice();
        collect_output(&mut reader, &output).await;
        let stored = output.lock().unwrap();
        assert!(stored.bytes.len() <= 1024);
        let (bytes, truncated) = stored.snapshot();
        assert!(truncated);
        assert_eq!(bytes.len(), 1024);
    }

    #[test]
    fn output_ring_keeps_the_newest_bytes_and_reports_truncation() {
        let mut ring = OutputBuffer::new(8);
        ring.push(b"abcd");
        ring.push(b"efgh");
        assert_eq!(ring.snapshot(), (b"abcdefgh".to_vec(), false));
        ring.push(b"ij");
        assert_eq!(ring.snapshot(), (b"cdefghij".to_vec(), true));
        // A chunk larger than the window keeps only its own tail.
        ring.push(b"0123456789");
        assert_eq!(ring.snapshot(), (b"23456789".to_vec(), true));
        // A chunk exactly the window size after nothing else is not a cut.
        let mut exact = OutputBuffer::new(4);
        exact.push(b"wxyz");
        assert_eq!(exact.snapshot(), (b"wxyz".to_vec(), false));
        // Steady 4 KiB arrivals never grow the store past the limit.
        let mut big = OutputBuffer::new(DEFAULT_OUTPUT_BYTE_LIMIT);
        for _ in 0..1024 {
            big.push(&[b'x'; 4096]);
        }
        assert_eq!(big.bytes.len(), DEFAULT_OUTPUT_BYTE_LIMIT);
        assert!(big.snapshot().1);
    }

    #[test]
    fn truncated_output_starts_at_a_character_boundary() {
        let mut ring = OutputBuffer::new(5);
        // "é" is two bytes; the cut lands inside the first one.
        ring.push("aéé".as_bytes());
        ring.push("é".as_bytes());
        let (bytes, truncated) = ring.snapshot();
        assert!(truncated);
        assert_eq!(String::from_utf8(bytes).unwrap(), "éé");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exit_is_reported_after_the_final_output_is_collected() {
        let terminals = AgentTerminals::default();
        let id = terminals
            .create(
                "owner",
                "sh",
                &[
                    "-c".into(),
                    "head -c 300000 /dev/zero | tr '\\0' x; printf err >&2; (sleep 0.3; printf done) &".into(),
                ],
                std::env::temp_dir(),
                &[],
                None,
            )
            .await
            .unwrap();
        let status = terminals.wait_for_exit(&id).await.unwrap();
        assert_eq!(status["exitCode"], 0);
        let output = terminals.output(&id).unwrap();
        let text = output["output"].as_str().unwrap();
        assert_eq!(
            text.len(),
            300_007,
            "{}",
            &text[text.len().saturating_sub(20)..]
        );
        // The last bytes come from a child that writes after the shell exited.
        assert!(text.ends_with("done") && text.contains("err"));
        terminals.release(&id);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_descendant_holding_the_pipe_does_not_hold_back_the_exit() {
        let terminals = AgentTerminals::default();
        let id = terminals
            .create(
                "owner",
                "sh",
                &["-c".into(), "printf ready; sleep 6 &".into()],
                std::env::temp_dir(),
                &[],
                None,
            )
            .await
            .unwrap();
        let started = std::time::Instant::now();
        let status = tokio::time::timeout(Duration::from_secs(10), terminals.wait_for_exit(&id))
            .await
            .expect("exit must not wait for the background sleep")
            .unwrap();
        assert_eq!(status["exitCode"], 0);
        assert!(started.elapsed() < OUTPUT_DRAIN_TIMEOUT + Duration::from_secs(3));
        assert_eq!(terminals.output(&id).unwrap()["output"], "ready");
        terminals.release(&id);
    }
}
