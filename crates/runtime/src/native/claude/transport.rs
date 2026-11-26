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
}
impl Process {
    pub async fn spawn(
        command: &str,
        cwd: &str,
        args: &[String],
        clean_oauth: bool,
    ) -> Result<Arc<Self>> {
        // Until a native environment broker exists, fail closed rather than launch
        // host Claude with an isolated thread's cwd or expose host subscriptions.
        anyhow::ensure!(
            crate::execution::current().is_none(),
            "unsupported_environment_native_claude: select ACP for isolated execution"
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
        let mut child = crate::execution::host_process::spawn(&mut cmd, &id, true, false).await?;
        let pid = child.id();
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
        let mut child = self.child.lock().await;
        // Kill only this owned process group, never a name-based process search.
        #[cfg(unix)]
        if let Some(pid) = child.id() {
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
        #[cfg(unix)]
        if let Ok(child) = self.child.try_lock() {
            if let Some(pid) = child.id() {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
        }
    }
}
