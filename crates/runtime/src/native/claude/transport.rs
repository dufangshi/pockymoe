//! The Agent SDK's stream-json protocol, without an SDK or Node intermediary.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, oneshot, Mutex},
};

type Waiters = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;
pub(super) struct Process {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    waiters: Waiters,
    pub events: Mutex<mpsc::Receiver<Value>>,
    pub alive: Arc<AtomicBool>,
    pub id: String,
    pub pid: Option<u32>,
    execution: Option<crate::execution::ExecutionContext>,
    host_guard: Option<crate::execution::host_process::HostProcessContext>,
}
impl Process {
    pub async fn spawn(
        command: &str,
        cwd: &str,
        args: &[String],
        clean_oauth: bool,
    ) -> Result<Arc<Self>> {
        // An isolated thread never launches host Claude: `spawn_isolated` is the
        // only path while an environment context is in scope.
        anyhow::ensure!(
            crate::execution::current().is_none(),
            "unsupported_environment_native_claude: isolated sessions start inside the environment"
        );
        let parsed = crate::acp::rpc::parse_spawn_command(command)?;
        let mut cmd = Command::new(parsed.program);
        cmd.args(parsed.args)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env("PATH", crate::acp::child_path());
        #[cfg(unix)]
        cmd.process_group(0);
        crate::child_process::hide_tokio(&mut cmd);
        if clean_oauth {
            crate::acp::rpc::clear_native_oauth_environment(&mut cmd, "claude");
        }
        cmd.envs(crate::upstreams::launch_environment("claude"));
        for key in crate::child_process::inherited_host_identity() {
            cmd.env_remove(key);
        }
        // A parent CLI's nested-session marker must not attach the new process to
        // the owner's session. Only the thread-scoped gateway identity is passed.
        cmd.env_remove("CLAUDECODE")
            .envs(crate::interaction::launch_env());
        let id = uuid::Uuid::new_v4().to_string();
        let child = crate::execution::host_process::spawn(&mut cmd, &id, true, false).await?;
        // With a Host guardian the receipt is the durable proof that every
        // writer stopped (required before a thread migrates into an environment).
        let guard = crate::execution::host_process::enabled_context();
        Self::attach(child, id, None, guard)
    }

    /// Starts `claude` through the environment worker, like the ACP transport:
    /// the worker clears the inherited environment, applies owner-approved
    /// native credential checks and owns the process domain inside the container.
    pub async fn spawn_isolated(
        context: &crate::execution::ExecutionContext,
        args: &[String],
        cwd: &str,
        env: Vec<(String, String)>,
    ) -> Result<Arc<Self>> {
        let id = uuid::Uuid::new_v4().to_string();
        let mut argv = vec!["claude".to_string()];
        argv.extend(args.iter().cloned());
        let child = context
            .spawn(&crate::execution::RunRequest {
                id: id.clone(),
                argv,
                cwd: cwd.into(),
                env,
                stdin_closed: false,
            })
            .await?;
        Self::attach(child, id, Some(context.clone()), None)
    }

    fn attach(
        mut child: Child,
        id: String,
        execution: Option<crate::execution::ExecutionContext>,
        host_guard: Option<crate::execution::host_process::HostProcessContext>,
    ) -> Result<Arc<Self>> {
        // A container process has no host PID; the docker client's PID is not
        // the harness and must never receive a process-group signal.
        let pid = if execution.is_some() {
            None
        } else {
            child.id()
        };
        let stdin = child.stdin.take().context("Claude stdin missing")?;
        let stdout = child.stdout.take().context("Claude stdout missing")?;
        let stderr = child.stderr.take().context("Claude stderr missing")?;
        // Drain stderr without logging it: SDK errors can include provider secrets.
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut BufReader::new(stderr), &mut tokio::io::sink()).await;
        });
        let waiters: Waiters = Default::default();
        let pending = waiters.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let live = alive.clone();
        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut bytes = Vec::new();
            loop {
                bytes.clear();
                // Bound each frame, including an unterminated or malicious line.
                use tokio::io::AsyncReadExt;
                match (&mut reader)
                    .take(32 * 1024 * 1024 + 1)
                    .read_until(b'\n', &mut bytes)
                    .await
                {
                    Ok(0) | Err(_) => break,
                    Ok(_) if bytes.len() > 32 * 1024 * 1024 => break,
                    Ok(_) => {}
                }
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                    break;
                };
                if value["type"] == "control_response" {
                    if let Some(id) = value["response"]["request_id"].as_str() {
                        if let Some(waiter) = pending.lock().await.remove(id) {
                            let _ = waiter.send(value["response"].clone());
                        }
                    }
                } else if tx.send(value).await.is_err() {
                    break;
                }
            }
            live.store(false, Ordering::SeqCst);
            pending.lock().await.clear();
        });
        Ok(Arc::new(Self {
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            waiters,
            events: Mutex::new(rx),
            alive,
            id,
            pid,
            execution,
            host_guard,
        }))
    }
    pub async fn write(&self, value: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&value)?;
        bytes.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        tokio::time::timeout(Duration::from_secs(15), async {
            stdin.write_all(&bytes).await?;
            stdin.flush().await
        })
        .await
        .context("Claude stdin timeout")??;
        Ok(())
    }
    pub async fn request(&self, request: Value) -> Result<Value> {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().await.insert(id.clone(), tx);
        let result = async {
            self.write(json!({"type":"control_request", "request_id":id, "request":request}))
                .await?;
            let response = tokio::time::timeout(Duration::from_secs(45), rx)
                .await
                .context("Claude control timeout")?
                .context("Claude control connection closed")?;
            if response["subtype"] == "error" {
                bail!("Claude rejected control request");
            }
            Ok(response["response"].clone())
        }
        .await;
        self.waiters.lock().await.remove(&id);
        result
    }
    pub async fn stop(&self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Some(context) = &self.execution {
            // The worker owns the container process domain; this ends it there
            // (and fences the environment if quiescence cannot be proved).
            let _ = context.kill(&self.id).await;
        }
        let mut child = self.child.lock().await;
        if let Some(guard) = &self.host_guard {
            // SIGTERM to the guardian: it ends the harness's whole tree and
            // records the stop. A process-group SIGKILL would kill the guardian
            // first and leave no proof (and orphan the harness).
            if guard.quiesce_process(&self.id).await.is_ok() {
                let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
                return;
            }
        }
        // Kill only this owned process group, never a name-based process search.
        #[cfg(unix)]
        if let Some(pid) = self.pid.and(child.id()) {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        // Dropping stdin closes the guardian transport; it reaps the harness
        // tree and writes its receipt, so it must not receive SIGKILL.
        if self.host_guard.is_some() {
            return;
        }
        #[cfg(unix)]
        if let Ok(child) = self.child.try_lock() {
            if let Some(pid) = self.pid.and(child.id()) {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
        }
    }
}
