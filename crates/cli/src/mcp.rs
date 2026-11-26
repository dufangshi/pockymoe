//! Newline JSON-RPC bridge. stdout is exclusively MCP protocol messages.
use crate::threads::{Client, Connection};
use anyhow::Result;
use serde_json::{json, Value};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::RwLock;

/// How long a call waits for a restarting Supervisor before it fails.
const RECONNECT: Duration = Duration::from_secs(30);

/// The Supervisor connection, rebuilt from the saved `cli.json` when the
/// Supervisor restarts: it issues a new CLI token each time it starts.
struct Link {
    connection: Connection,
    client: RwLock<Arc<Client>>,
}

/// Failures where the request never ran: the Supervisor was down, or it
/// restarted and no longer accepts the token this process read at start.
fn not_delivered(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<reqwest::Error>()
        .is_some_and(reqwest::Error::is_connect)
        || error.to_string().starts_with("HTTP 401")
}

/// The connection broke after the request was sent, as when the Supervisor
/// restarts during a long wait: the call may or may not have run.
fn interrupted(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<reqwest::Error>()
        .is_some_and(|e| !e.is_timeout() && (e.is_request() || e.is_body() || e.is_decode()))
}

impl Link {
    /// `repeatable` calls (reads) are sent again after an interruption too.
    async fn request(&self, input: Value, wait_seconds: u64, repeatable: bool) -> Result<Value> {
        let retry =
            |error: &anyhow::Error| not_delivered(error) || (repeatable && interrupted(error));
        let client = self.client.read().await.clone();
        let error = match client.request_for(input.clone(), wait_seconds).await {
            Err(error) if retry(&error) => error,
            done => return done,
        };
        let deadline = tokio::time::Instant::now() + RECONNECT;
        let mut last = error;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let Ok(fresh) = Client::new(self.connection.clone()) else {
                continue;
            };
            match fresh.request_for(input.clone(), wait_seconds).await {
                Err(error) if retry(&error) => last = error,
                done => {
                    *self.client.write().await = Arc::new(fresh);
                    return done;
                }
            }
        }
        Err(last)
    }
}

/// A short reason for the client, without the request body.
fn failure(id: &Value, error: &anyhow::Error) -> Value {
    let text = error.to_string();
    let reason = text
        .split_once(": ")
        .and_then(|(_, body)| serde_json::from_str::<Value>(body).ok())
        .and_then(|body| body["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| {
            if not_delivered(error) {
                "the Pockymoe Supervisor is not running or its CLI connection changed".into()
            } else if interrupted(error) {
                "the connection to the Supervisor broke during this call (it may have restarted); the call may have run, so check before repeating it".into()
            } else {
                text
            }
        });
    let reason: String = reason.chars().take(500).collect();
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":format!("Supervisor request failed: {reason}")}})
}

fn remember_read_only(reply: &Value, names: &mut HashSet<String>) {
    for tool in reply["result"]["tools"].as_array().into_iter().flatten() {
        if tool["annotations"]["readOnlyHint"] == true {
            if let Some(name) = tool["name"].as_str() {
                names.insert(name.to_owned());
            }
        }
    }
}

pub async fn serve(
    connection: Connection,
    device: Option<String>,
    access: String,
    workspaces: Vec<String>,
) -> Result<()> {
    let link = Arc::new(Link {
        client: RwLock::new(Arc::new(Client::new(connection.clone())?)),
        connection,
    });
    let workspaces = (!workspaces.is_empty()).then_some(workspaces);
    // Read-only tools, learnt from the tools/list replies this bridge relays.
    let read_only = Arc::new(std::sync::RwLock::new(HashSet::<String>::new()));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let output = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    let mut tasks = tokio::task::JoinSet::new();
    let mut pending = std::collections::HashMap::<String, tokio::task::AbortHandle>::new();
    let mut initialized = false;
    let session_id = uuid::Uuid::new_v4().to_string();
    let envelope = |request: &Value| json!({"operation":"outsideMcp","deviceId":device,"access":access,"workspaces":workspaces,"sessionId":session_id,"request":request});
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                let mut out = output.lock().await;
                out.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32700,\"message\":\"Parse error\"}}\n").await?;
                out.flush().await?;
                continue;
            }
        };
        if request["method"] == "notifications/cancelled" {
            if let Some(handle) = pending.remove(&request["params"]["requestId"].to_string()) {
                handle.abort();
                let _ = link.request(envelope(&request), 0, false).await;
            }
            continue;
        }
        if request["method"] == "initialize" {
            initialized = true;
        }
        if request.get("id").is_none() {
            continue;
        }
        if !initialized {
            let mut out = output.lock().await;
            out.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":"Initialize first"}})).as_bytes()).await?;
            out.flush().await?;
            continue;
        }
        let key = request["id"].to_string();
        let link = link.clone();
        let output = output.clone();
        let input = envelope(&request);
        let method = request["method"].as_str().unwrap_or("");
        let repeatable = matches!(method, "initialize" | "ping" | "tools/list")
            || (method == "tools/call"
                && request["params"]["name"]
                    .as_str()
                    .is_some_and(|name| read_only.read().unwrap().contains(name)));
        let lists_tools = method == "tools/list";
        let read_only = read_only.clone();
        let handle = tasks.spawn(async move {
            let id = request["id"].clone();
            let response = match link.request(input, 600, repeatable).await {
                Ok(v) => v,
                Err(error) => failure(&id, &error),
            };
            if lists_tools {
                remember_read_only(&response, &mut read_only.write().unwrap());
            }
            let mut out = output.lock().await;
            out.write_all(format!("{response}\n").as_bytes()).await?;
            out.flush().await?;
            Ok::<_, std::io::Error>(())
        });
        pending.insert(key, handle);
        while tasks.try_join_next().is_some() {}
        pending.retain(|_, h| !h.is_finished());
    }
    tasks.abort_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_names_the_supervisor_reason_and_never_the_request() {
        let error = anyhow::anyhow!(
            "HTTP 400 Bad Request: {}",
            json!({"code":"bad","message":"outside MCP requires the owner CLI credential"})
        );
        let reply = failure(&json!(7), &error);
        assert_eq!(reply["id"], 7);
        assert_eq!(
            reply["error"]["message"],
            "Supervisor request failed: outside MCP requires the owner CLI credential"
        );
        assert!(not_delivered(&anyhow::anyhow!(
            "HTTP 401 Unauthorized: {{}}"
        )));
        assert!(!not_delivered(&error));
    }

    #[test]
    fn only_tools_listed_as_read_only_are_repeated() {
        let mut names = HashSet::new();
        remember_read_only(
            &json!({"result":{"tools":[
                {"name":"pockymoe_thread_wait","annotations":{"readOnlyHint":true}},
                {"name":"pockymoe_thread_send","annotations":{"readOnlyHint":false}},
                {"name":"other"}
            ]}}),
            &mut names,
        );
        assert_eq!(names, HashSet::from(["pockymoe_thread_wait".to_owned()]));
    }
}
