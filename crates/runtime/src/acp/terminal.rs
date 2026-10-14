use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::watch;
use uuid::Uuid;

const DEFAULT_OUTPUT_BYTE_LIMIT: usize = 1024 * 1024;

struct LiveTerminal {
    /// The harness process that created it.
    owner: String,
    output: Arc<Mutex<Vec<u8>>>,
    output_byte_limit: usize,
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
        for key in [
            "TREER_AGENT_ID",
            "TREER_WORKLOAD_CREDENTIAL",
            "TREER_AGENT_SERVER_URL",
            "TREER_WORKSPACE_ID",
            "TREER_SERVER_ID",
        ] {
            cmd.env_remove(key);
        }
        cmd.args(&parsed_args)
            .current_dir(&cwd)
            .envs(env.iter().cloned())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|err| anyhow!("failed to spawn ACP terminal command `{command}`: {err}"))?;
        let child_id = child.id().unwrap_or(0);
        let limit = output_byte_limit
            .unwrap_or(DEFAULT_OUTPUT_BYTE_LIMIT)
            .max(1);
        let output = Arc::new(Mutex::new(Vec::new()));
        if let Some(mut stdout) = child.stdout.take() {
            let output = output.clone();
            tokio::spawn(async move { collect_output(&mut stdout, &output).await });
        }
        if let Some(mut stderr) = child.stderr.take() {
            let output = output.clone();
            tokio::spawn(async move { collect_output(&mut stderr, &output).await });
        }
        let (exit_tx, exit_rx) = watch::channel(None);
        tokio::spawn(async move {
            let status = match child.wait().await {
                Ok(status) => exit_status_value(status),
                Err(err) => json!({ "exitCode": null, "signal": null, "error": err.to_string() }),
            };
            let _ = exit_tx.send(Some(status));
        });
        self.inner.lock().unwrap().insert(
            id.clone(),
            LiveTerminal {
                owner: owner.into(),
                output,
                output_byte_limit: limit,
                child_id,
                exit_rx,
            },
        );
        Ok(id)
    }

    pub fn output(&self, id: &str) -> Result<Value> {
        let hub = self.inner.lock().unwrap();
        let term = hub.get(id).ok_or_else(|| anyhow!("terminal not found"))?;
        let complete = term.output.lock().unwrap();
        let (retained, truncated) = retained_output(&complete, term.output_byte_limit);
        let exit_status = term.exit_rx.borrow().clone();
        Ok(json!({
            "output": String::from_utf8_lossy(retained),
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
            terminate_process(term.child_id)?;
        }
        Ok(())
    }

    pub fn release(&self, id: &str) {
        if let Some(term) = self.inner.lock().unwrap().remove(id) {
            if term.exit_rx.borrow().is_none() && term.child_id > 0 {
                let _ = terminate_process(term.child_id);
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

async fn collect_output<R: AsyncRead + Unpin>(reader: &mut R, output: &Arc<Mutex<Vec<u8>>>) {
    let mut buf = vec![0u8; 4096];
    while let Ok(n) = reader.read(&mut buf).await {
        if n == 0 {
            break;
        }
        output.lock().unwrap().extend_from_slice(&buf[..n]);
    }
}

fn retained_output(output: &[u8], limit: usize) -> (&[u8], bool) {
    if output.len() <= limit {
        return (output, false);
    }
    let mut start = output.len() - limit;
    while start < output.len() && (output[start] & 0xc0) == 0x80 {
        start += 1;
    }
    (&output[start..], true)
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
