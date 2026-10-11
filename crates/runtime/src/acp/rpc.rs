use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::timeout;

pub struct Pending {
    pub tx: oneshot::Sender<Result<Value>>,
}

#[derive(Default)]
struct RpcState {
    pending: HashMap<i64, Pending>,
    closed_reason: Option<String>,
}

struct PendingGuard {
    id: i64,
    state: Arc<Mutex<RpcState>>,
    armed: bool,
}

impl PendingGuard {
    async fn remove(&mut self) {
        self.state.lock().await.pending.remove(&self.id);
        self.armed = false;
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(mut state) = self.state.try_lock() {
            state.pending.remove(&self.id);
            return;
        }
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let state = self.state.clone();
            let id = self.id;
            handle.spawn(async move {
                state.lock().await.pending.remove(&id);
            });
        }
    }
}

/// One ACP stdio process. stdin is locked only for the write, so multiple
/// sessions on the same process can request/notify concurrently.
pub struct AcpProcess {
    pub(crate) cli_env: Vec<(String, String)>,
    pub(crate) id: String,
    pub(crate) started_at: String,
    stdin: Mutex<ChildStdin>,
    child: Arc<Mutex<Child>>,
    state: Arc<Mutex<RpcState>>,
    next_id: AtomicI64,
}

pub(crate) struct ParsedCommand {
    pub program: String,
    pub args: Vec<String>,
}

pub(crate) fn parse_spawn_command(command: &str) -> Result<ParsedCommand> {
    let parts =
        shell_words::split(command).with_context(|| format!("parse ACP command `{command}`"))?;
    let mut program = parts
        .first()
        .filter(|program| !program.is_empty())
        .cloned()
        .ok_or_else(|| anyhow!("empty ACP command"))?;

    if let Some(path) = super::catalog::resolve_executable(&shell_words::quote(&program)) {
        program = path.to_string_lossy().into_owned();
    }

    // Keep argv structured, including .cmd/.bat paths. Rust's Windows Command
    // handles batch dispatch and cmd escaping itself. Passing a prequoted /C
    // string through .args instead applies CRT escaping (\") to cmd syntax.
    Ok(ParsedCommand {
        program,
        args: parts.into_iter().skip(1).collect(),
    })
}

impl AcpProcess {
    pub async fn connection_open(&self) -> bool {
        let open = self.state.lock().await.closed_reason.is_none();
        open && !self.exited().await.unwrap_or(true)
    }

    pub async fn shutdown(&self) -> Result<()> {
        close_rpc_state(&self.state, "Harness restarted".into()).await;
        let mut child = self.child.lock().await;
        if child.try_wait()?.is_none() {
            terminate_tree(&child);
            if child.try_wait()?.is_none() {
                child.kill().await?;
            }
        }
        Ok(())
    }

    pub async fn spawn(
        command: &str,
        cwd: &str,
        extra_env: &[(&str, String)],
    ) -> Result<(
        Self,
        mpsc::UnboundedReceiver<Value>,
        mpsc::UnboundedReceiver<(i64, String, Value)>,
    )> {
        let parsed = parse_spawn_command(command)?;
        let mut cmd = Command::new(&parsed.program);
        #[cfg(unix)]
        cmd.process_group(0);
        crate::child_process::hide_tokio(&mut cmd);
        cmd.env("PATH", super::catalog::child_path());
        cmd.args(&parsed.args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (key, value) in extra_env {
            cmd.env(key, value);
        }
        let cli_env = crate::interaction::launch_env();
        cmd.envs(cli_env.iter().cloned());
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn ACP `{command}`"))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("missing stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("missing stdout"))?;
        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            tracing::debug!(target: "acp.stderr", "{}", String::from_utf8_lossy(&buf[..n]));
                        }
                    }
                }
            });
        }
        let state = Arc::new(Mutex::new(RpcState::default()));
        let (updates_tx, updates_rx) = mpsc::unbounded_channel();
        let (req_tx, req_rx) = mpsc::unbounded_channel();
        let reader_state = state.clone();
        tokio::spawn(async move {
            let reason = match read_loop(stdout, reader_state.clone(), updates_tx, req_tx).await {
                Ok(()) => "ACP stdout closed".to_string(),
                Err(err) => {
                    tracing::warn!(error = %err, "ACP reader exited");
                    format!("ACP reader failed: {err}")
                }
            };
            close_rpc_state(&reader_state, reason).await;
        });
        let child = Arc::new(Mutex::new(child));
        spawn_exit_monitor(child.clone(), state.clone());
        Ok((
            Self {
                cli_env,
                id: uuid::Uuid::new_v4().to_string(),
                started_at: pockymoe_protocol::now_rfc3339(),
                stdin: Mutex::new(stdin),
                child,
                state,
                next_id: AtomicI64::new(1),
            },
            updates_rx,
            req_rx,
        ))
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        self.request_with_timeout(method, params, timeout_for_method(method))
            .await
    }

    /// Writes the request before returning its owned response future. The active
    /// turn can then own completion without blocking an input-delivery endpoint.
    pub async fn begin_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<futures_util::future::BoxFuture<'static, Result<Value>>> {
        self.begin_request_with_timeout(method, params, timeout_for_method(method))
            .await
    }

    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        wait: Option<Duration>,
    ) -> Result<Value> {
        self.begin_request_with_timeout(method, params, wait)
            .await?
            .await
    }

    async fn begin_request_with_timeout(
        &self,
        method: &str,
        params: Value,
        wait: Option<Duration>,
    ) -> Result<futures_util::future::BoxFuture<'static, Result<Value>>> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self.state.lock().await;
            if let Some(reason) = &state.closed_reason {
                return Err(anyhow!(reason.clone()));
            }
            state.pending.insert(id, Pending { tx });
        }
        let mut guard = PendingGuard {
            id,
            state: self.state.clone(),
            armed: true,
        };
        {
            let mut stdin = self.stdin.lock().await;
            if let Err(err) = write_message(
                &mut stdin,
                &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
            )
            .await
            {
                guard.remove().await;
                return Err(err);
            }
        }
        let method = method.to_owned();
        Ok(Box::pin(async move {
            let result = if let Some(wait) = wait {
                match timeout(wait, rx).await {
                    Ok(result) => result,
                    Err(_) => {
                        guard.remove().await;
                        return Err(anyhow!("ACP request timeout ({method})"));
                    }
                }
            } else {
                rx.await
            };
            guard.disarm();
            result.map_err(|_| anyhow!("ACP request dropped"))?
        }))
    }

    pub async fn respond(&self, id: i64, result: Value) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        write_message(
            &mut stdin,
            &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        )
        .await
    }

    pub async fn respond_error(&self, id: i64, message: &str) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        write_message(
            &mut stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32000, "message": message }
            }),
        )
        .await
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        write_message(
            &mut stdin,
            &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
        )
        .await
    }

    pub async fn exited(&self) -> Result<bool> {
        Ok(self.child.lock().await.try_wait()?.is_some())
    }
}

async fn write_message(stdin: &mut ChildStdin, value: &Value) -> Result<()> {
    let mut body = serde_json::to_vec(value)?;
    body.push(b'\n');
    stdin.write_all(&body).await?;
    stdin.flush().await?;
    Ok(())
}

async fn read_loop(
    stdout: tokio::process::ChildStdout,
    state: Arc<Mutex<RpcState>>,
    updates: mpsc::UnboundedSender<Value>,
    requests: mpsc::UnboundedSender<(i64, String, Value)>,
) -> Result<()> {
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf).await?;
        if n == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&buf);
        let msg = if line.to_ascii_lowercase().starts_with("content-length:") {
            let len: usize = line
                .split(':')
                .nth(1)
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            let mut rest = String::new();
            loop {
                rest.clear();
                reader.read_line(&mut rest).await?;
                if rest.trim().is_empty() {
                    break;
                }
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).await?;
            serde_json::from_slice::<Value>(&body)?
        } else {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            serde_json::from_str::<Value>(trimmed)?
        };
        dispatch(msg, &state, &updates, &requests).await;
    }
    Ok(())
}

async fn dispatch(
    msg: Value,
    state: &Arc<Mutex<RpcState>>,
    updates: &mpsc::UnboundedSender<Value>,
    requests: &mpsc::UnboundedSender<(i64, String, Value)>,
) {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    if let Some(id) = msg.get("id").and_then(Value::as_i64) {
        if !method.is_empty() {
            let _ = requests.send((
                id,
                method.to_string(),
                msg.get("params").cloned().unwrap_or(json!({})),
            ));
            return;
        }
        if let Some(pending) = state.lock().await.pending.remove(&id) {
            if let Some(error) = msg.get("error") {
                let _ = pending.tx.send(Err(anyhow!("{error}")));
            } else {
                let _ = pending
                    .tx
                    .send(Ok(msg.get("result").cloned().unwrap_or(Value::Null)));
            }
        }
        return;
    }
    if method == "session/update" || method.starts_with('_') {
        let mut params = msg.get("params").cloned().unwrap_or(json!({}));
        let Some(object) = params.as_object_mut() else {
            return;
        };
        object.insert("_remoteMethod".into(), json!(method));
        let _ = updates.send(params);
    }
}

async fn close_rpc_state(state: &Arc<Mutex<RpcState>>, reason: String) {
    let pending = {
        let mut state = state.lock().await;
        if state.closed_reason.is_none() {
            state.closed_reason = Some(reason.clone());
        }
        std::mem::take(&mut state.pending)
    };
    for (_, pending) in pending {
        let _ = pending.tx.send(Err(anyhow!(reason.clone())));
    }
}

fn spawn_exit_monitor(child: Arc<Mutex<Child>>, state: Arc<Mutex<RpcState>>) {
    tokio::spawn(async move {
        loop {
            let status = {
                let mut child = child.lock().await;
                child.try_wait()
            };
            match status {
                Ok(Some(status)) => {
                    // Let the reader dispatch any response already buffered before the exit.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    close_rpc_state(&state, format!("ACP process exited ({status})")).await;
                    break;
                }
                Ok(None) => tokio::time::sleep(Duration::from_millis(100)).await,
                Err(err) => {
                    close_rpc_state(&state, format!("failed to inspect ACP process: {err}")).await;
                    break;
                }
            }
        }
    });
}

fn terminate_tree(child: &Child) {
    let Some(pid) = child.id() else { return };
    #[cfg(unix)]
    // Each ACP process is started in its own process group. This also closes
    // an adapter-owned app-server; no other harness or desktop app is targeted.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGTERM);
    }
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("taskkill.exe");
        crate::child_process::hide_std(&mut cmd);
        let _ = cmd
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

impl Drop for AcpProcess {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.try_lock() {
            terminate_tree(&child);
            let _ = child.start_kill();
        } else if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let child = self.child.clone();
            handle.spawn(async move {
                let mut child = child.lock().await;
                terminate_tree(&child);
                let _ = child.start_kill();
            });
        }
    }
}

/// Control RPCs (initialize, session/new, set_mode, …) stay bounded.
/// `session/prompt` is the live agent turn and can run for many minutes, matching
/// the TypeScript ACP adapter which does not time out `session/prompt`.
pub(crate) fn timeout_for_method(method: &str) -> Option<Duration> {
    match method {
        "session/prompt" => None,
        _ => Some(Duration::from_secs(180)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spawn_command_preserves_structured_windows_arguments() {
        let args = [
            r"C:\User Name\node\npm.cmd",
            "install",
            "--prefix",
            r"C:\User Name\O'Brien & tools\",
            "@agentclientprotocol/codex-acp@latest",
        ];
        let parsed = parse_spawn_command(&shell_words::join(args)).unwrap();
        assert_eq!(parsed.program, args[0]);
        assert_eq!(parsed.args, args[1..]);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_batch_harness_preserves_paths_and_reports_update_errors() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("Node Tools & adapter's");
        std::fs::create_dir_all(&folder).unwrap();
        let script = folder.join("npm.cmd");
        std::fs::write(&script, "@echo off\r\necho [%~1]\r\necho \"%~2\"\r\n").unwrap();
        let prefix = folder.join("private prefix").to_string_lossy().into_owned();
        let parsed = parse_spawn_command(&shell_words::join([
            script.to_str().unwrap(),
            "--prefix",
            &prefix,
        ]))
        .unwrap();
        assert_eq!(
            std::path::Path::new(&parsed.program).extension().unwrap(),
            "cmd"
        );
        let output =
            crate::management::output(std::path::Path::new(&parsed.program), &parsed.args, 10)
                .await
                .unwrap();
        assert_eq!(
            output.lines().collect::<Vec<_>>(),
            ["[--prefix]", &format!("\"{prefix}\"")]
        );
        std::fs::write(
            &script,
            "@echo off\r\necho fixture-update-failure 1>&2\r\nexit /b 7\r\n",
        )
        .unwrap();
        let error = crate::management::output(&script, &[], 10)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("fixture-update-failure"));
        // The same path launches ACP/adapter processes, not just update commands.
        std::fs::write(&script, "@echo off\r\nset /p request=\r\necho {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":1}}\r\nset /p request=\r\n").unwrap();
        let (process, _, _) = AcpProcess::spawn(
            &shell_words::quote(script.to_str().unwrap()),
            root.path().to_str().unwrap(),
            &[],
        )
        .await
        .unwrap();
        let result = process
            .request_with_timeout("initialize", json!({}), Some(Duration::from_secs(5)))
            .await
            .unwrap();
        assert_eq!(result["protocolVersion"], 1);
        process.shutdown().await.unwrap();
    }

    fn python_command(script: &std::path::Path) -> String {
        static PYTHON: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        let python = PYTHON.get_or_init(|| {
            ["python3", "python"]
                .into_iter()
                .filter_map(|candidate| which::which(candidate).ok())
                .find(|candidate| {
                    std::process::Command::new(candidate)
                        .args(["-c", "import sys; sys.exit(0)"])
                        .status()
                        .is_ok_and(|status| status.success())
                })
                .expect("Python is required for ACP RPC tests")
        });
        format!(r#""{}" "{}""#, python.display(), script.display())
    }

    #[tokio::test]
    async fn bounded_request_times_out_and_clears_pending() {
        let dir = std::env::temp_dir().join(format!("acp-rpc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("hang.py");
        std::fs::write(&script, "import time\ntime.sleep(30)\n").unwrap();
        let (process, _updates, _requests) =
            AcpProcess::spawn(&python_command(&script), dir.to_str().unwrap(), &[])
                .await
                .expect("spawn hanging process");
        let started = std::time::Instant::now();
        let err = process
            .request_with_timeout("initialize", json!({}), Some(Duration::from_millis(80)))
            .await
            .expect_err("initialize should time out");
        assert!(err.to_string().contains("timeout"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(process.state.lock().await.pending.is_empty());
    }

    #[tokio::test]
    async fn reader_eof_fails_all_pending_requests_and_closes_the_connection() {
        let dir = std::env::temp_dir().join(format!("acp-rpc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("exit_after_request.py");
        std::fs::write(
            &script,
            "import sys\nsys.stdin.readline()\nsys.stdin.readline()\n",
        )
        .unwrap();
        let (process, _updates, _requests) =
            AcpProcess::spawn(&python_command(&script), dir.to_str().unwrap(), &[])
                .await
                .expect("spawn exiting process");

        let (first, second) = timeout(Duration::from_secs(2), async {
            tokio::join!(
                process.request("session/prompt", json!({ "sequence": 1 })),
                process.request("session/prompt", json!({ "sequence": 2 })),
            )
        })
        .await
        .expect("reader EOF should settle every unbounded prompt request");
        for result in [first, second] {
            let error = result.expect_err("reader EOF must fail pending requests");
            assert!(error.to_string().contains("stdout closed"), "{error:#}");
        }
        let state = process.state.lock().await;
        assert!(state.pending.is_empty());
        assert_eq!(state.closed_reason.as_deref(), Some("ACP stdout closed"));
        drop(state);

        let next_error = process
            .request("initialize", json!({}))
            .await
            .expect_err("requests after EOF must fail immediately");
        assert!(next_error.to_string().contains("stdout closed"));
    }

    #[tokio::test]
    async fn dropping_a_request_future_removes_its_pending_entry() {
        let dir = std::env::temp_dir().join(format!("acp-rpc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("hang.py");
        std::fs::write(&script, "import time\ntime.sleep(30)\n").unwrap();
        let (process, _updates, _requests) =
            AcpProcess::spawn(&python_command(&script), dir.to_str().unwrap(), &[])
                .await
                .expect("spawn hanging process");
        let process = Arc::new(process);
        let request_process = process.clone();
        let request =
            tokio::spawn(async move { request_process.request("session/prompt", json!({})).await });

        timeout(Duration::from_secs(2), async {
            loop {
                if process.state.lock().await.pending.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("request should become pending");
        request.abort();
        let _ = request.await;
        timeout(Duration::from_secs(2), async {
            loop {
                if process.state.lock().await.pending.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropping request future should clean its pending entry");
    }
}
