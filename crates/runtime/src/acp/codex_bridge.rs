//! Narrow control bridge to the app-server already owned by codex-acp.
//! ACP owns prompt/resume/cancel; model catalog compatibility is applied in transit.
//! The private control pipe also synchronizes native permission defaults before ACP prompts.
//! Fork history is copied in a short-lived app-server so it cannot retain a writer.
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::{collections::HashSet, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::Mutex,
};

pub(super) struct CodexBridge {
    command: String,
    listener: TcpListener,
    token: String,
    stream: Mutex<Option<BufReader<TcpStream>>>,
}

impl CodexBridge {
    pub async fn new(
        command: &str,
        env: &mut Vec<(&'static str, String)>,
        policy: &super::modes::ProductSessionPolicy,
        upstream_directory: Option<&std::path::Path>,
    ) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let token = uuid::Uuid::new_v4().to_string();
        env.retain(|(key, _)| *key != "CODEX_PATH");
        env.push((
            "CODEX_PATH",
            std::env::current_exe()?.to_string_lossy().into_owned(),
        ));
        env.push((
            "POCKYMOE_APP_SERVER_BRIDGE",
            json!({
                "upstreamDirectory": upstream_directory,
                "address": listener.local_addr()?.to_string(), "token": token, "command": command, "policy": super::codex_permissions::native_policy(policy)
            })
            .to_string(),
        ));
        Ok(Self {
            command: command.to_string(),
            listener,
            token,
            stream: Mutex::new(None),
        })
    }

    pub async fn set_policy(
        &self,
        session: &str,
        policy: &super::modes::ProductSessionPolicy,
    ) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let mut guard = self.stream.lock().await;
            if guard.is_none() {
                loop {
                    let (stream, _) = self.listener.accept().await?;
                    let mut reader = BufReader::new(stream);
                    let mut hello = String::new();
                    if matches!(tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut hello)).await, Ok(Ok(_)))
                        && hello.trim() == self.token {
                        *guard = Some(reader);
                        break;
                    }
                }
            }
            let native = super::codex_permissions::native_policy(policy);
            request(guard.as_mut().unwrap(), "thread/settings/update", json!({
                "threadId":session, "sandboxPolicy":native["sandboxPolicy"],
                "approvalPolicy":native["approvalPolicy"], "approvalsReviewer":native["approvalsReviewer"]
            })).await?;
            Ok(())
        }).await.context("Codex permission synchronization timed out")?
    }

    pub async fn fork(
        &self,
        source: &str,
        rollback_count: u32,
        cwd: &str,
        policy: &super::modes::ProductSessionPolicy,
    ) -> Result<String> {
        tokio::time::timeout(Duration::from_secs(60), async {
            // thread/fork loads a writer in the process that handles it. Using the
            // parent's app-server strands that writer when ACP later loads the fork
            // with its own CLI identity. A dedicated process reads the persisted
            // source without resuming it, then exits before the fork is exposed.
            let (process, _, _) = super::rpc::AcpProcess::spawn(
                &format!("{} app-server", self.command), cwd, &[],
            ).await?;
            let result = async {
                process.request("initialize", json!({
                    "clientInfo":{"name":"pockymoe-fork","version":env!("CARGO_PKG_VERSION")},
                    "capabilities":{"experimentalApi":true}
                })).await?;
                process.notify("initialized", json!({})).await?;
                let native = super::codex_permissions::native_policy(policy);
                // The bridge only needs the new thread ID. The inherited turns can
                // exceed the transport limit when included in the response.
                let mut params = json!({"threadId": source, "persistExtendedHistory": true,
                    "excludeTurns": true, "deferGoalContinuation":true, "cwd":cwd,
                    "sandbox":native["sandbox"], "approvalPolicy":native["approvalPolicy"],
                    "approvalsReviewer":native["approvalsReviewer"]});
                if rollback_count > 0 {
                    let mut remaining = rollback_count as usize;
                    let mut cursor = Value::Null;
                    loop {
                        let turns = process.request("thread/turns/list", json!({"threadId":source,"sortDirection":"desc","limit":100,"itemsView":"summary","cursor":cursor})).await?;
                        let data = turns["data"].as_array().context("Codex returned no turn list")?;
                        if let Some(turn) = data.get(remaining) {
                            params["lastTurnId"] = turn.get("id").filter(|id| id.is_string()).context("Codex turn id missing")?.clone();
                            break;
                        }
                        remaining = remaining.saturating_sub(data.len());
                        let next = turns.get("nextCursor").filter(|value| value.is_string()).context("The selected Codex turn was not found")?;
                        if next == &cursor { bail!("Codex turn pagination did not advance"); }
                        cursor = next.clone();
                    }
                }
                let result = process.request("thread/fork", params).await?;
                let id = result.pointer("/thread/id").and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("Codex fork returned no thread id"))?.to_string();
                Ok(id)
            }.await;
            process.shutdown().await.context("release Codex fork writer")?;
            result
        }).await.context("Codex fork bridge timed out")?
    }
}

async fn request(stream: &mut BufReader<TcpStream>, method: &str, params: Value) -> Result<Value> {
    let id = format!("pockymoe-fork-{}", uuid::Uuid::new_v4());
    let message = json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params});
    stream
        .get_mut()
        .write_all(format!("{message}\n").as_bytes())
        .await?;
    let mut line = String::new();
    if stream.read_line(&mut line).await? == 0 {
        bail!("Codex fork bridge closed");
    }
    let response: Value = serde_json::from_str(&line)?;
    if response["id"] != id {
        bail!("Codex fork bridge response mismatch");
    }
    if let Some(error) = response.get("error") {
        bail!("Codex fork: {error}");
    }
    Ok(response["result"].clone())
}

/// Hidden CLI entry point used as codex-acp's CODEX_PATH executable.
pub async fn run() -> Result<()> {
    let config: Value = serde_json::from_str(&std::env::var("POCKYMOE_APP_SERVER_BRIDGE")?)?;
    let command = config["command"]
        .as_str()
        .context("missing native Codex command")?;
    let parsed = super::rpc::parse_spawn_command(&format!("{command} app-server"))?;
    let mut command = Command::new(parsed.program);
    crate::child_process::hide_tokio(&mut command);
    let mut child = command
        .args(parsed.args)
        .env_remove("POCKYMOE_APP_SERVER_BRIDGE")
        .env_remove("CODEX_PATH")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let mut native_in = child.stdin.take().context("native stdin")?;
    let mut native_out = BufReader::new(child.stdout.take().context("native stdout")?).lines();
    let mut acp_in = BufReader::new(tokio::io::stdin()).lines();
    let mut acp_out = tokio::io::stdout();
    let stream = TcpStream::connect(config["address"].as_str().context("bridge address")?).await?;
    let (read, mut control_out) = stream.into_split();
    control_out
        .write_all(format!("{}\n", config["token"].as_str().context("bridge token")?).as_bytes())
        .await?;
    let mut control_in = BufReader::new(read).lines();
    let mut pending = HashSet::new();
    let mut models = super::codex_models::ModelCatalogBridge::default();
    let upstreams = config["upstreamDirectory"]
        .as_str()
        .map(|directory| super::upstream_models::UpstreamModels::new(directory.into()));
    let mut permissions = super::codex_permissions::PermissionBridge::new(config["policy"].clone());
    loop {
        tokio::select! {
            line = acp_in.next_line() => {
                let Some(line) = line? else { break; };
                let mut message: Value = serde_json::from_str(&line)?;
                permissions.request(&mut message);
                if message["method"] == "model/list" && message["params"]["cursor"].is_null() {
                    // Use only the connection applied to this harness, never an
                    // arbitrary saved connection (which may route elsewhere).
                    if let Some(upstreams) = &upstreams {
                        match upstreams.catalog("codex").await {
                            Ok(catalog) => models.set_upstream(catalog.map(|(_, models)| models).unwrap_or_default()),
                            Err(_) => models.set_upstream(Vec::new()),
                        }
                    }
                }
                models.observe_request(&message);
                native_in.write_all(format!("{message}\n").as_bytes()).await?;
            }
            line = control_in.next_line() => {
                let Some(line) = line? else { break; };
                let mut message: Value = serde_json::from_str(&line)?;
                let method = message["method"].as_str().unwrap_or("");
                if !allowed_control(&message) { bail!("unsupported Codex bridge operation"); }
                let id = message["id"].as_str().context("bridge request id")?.to_string();
                pending.insert((id, method.to_string()));
                permissions.request(&mut message);
                native_in.write_all(format!("{message}\n").as_bytes()).await?;
            }
            line = native_out.next_line() => {
                let Some(line) = line? else { break; };
                let mut message: Value = serde_json::from_str(&line)?;
                permissions.response(&message);
                let id = message["id"].as_str().unwrap_or("");
                let owned = pending.iter().find(|(key,_)| key == id).cloned();
                if let Some(key) = owned {
                    pending.remove(&key);
                    control_out.write_all(format!("{line}\n").as_bytes()).await?;
                } else {
                    models.observe_response(&mut message);
                    acp_out.write_all(format!("{message}\n").as_bytes()).await?;
                    acp_out.flush().await?;
                }
            }
        }
    }
    let _ = child.kill().await;
    Ok(())
}

fn allowed_control(message: &Value) -> bool {
    message["method"] == "thread/settings/update"
        && message["params"].as_object().is_some_and(|params| {
            params.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "threadId" | "sandboxPolicy" | "approvalPolicy" | "approvalsReviewer"
                )
            }) && params.get("threadId").is_some_and(Value::is_string)
                && params.contains_key("sandboxPolicy")
        })
}
