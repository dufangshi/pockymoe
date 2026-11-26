//! Newline JSON-RPC bridge. stdout is exclusively MCP protocol messages.
use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub async fn serve(
    client: crate::threads::Client,
    device: Option<String>,
    access: String,
) -> Result<()> {
    let client = std::sync::Arc::new(client);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let output = std::sync::Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    let mut tasks = tokio::task::JoinSet::new();
    let mut pending = std::collections::HashMap::<String, tokio::task::AbortHandle>::new();
    let mut initialized = false;
    let session_id = uuid::Uuid::new_v4().to_string();
    while let Some(line) = lines.next_line().await? {
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                let mut out = output.lock().await;
                out.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32700,\"message\":\"Parse error\"}}\n").await?;
                continue;
            }
        };
        if request["method"] == "notifications/cancelled" {
            if let Some(handle) = pending.remove(&request["params"]["requestId"].to_string()) {
                handle.abort();
                let _=client.request(json!({"operation":"outsideMcp","deviceId":device,"access":access,"sessionId":session_id,"request":request})).await;
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
            continue;
        }
        let key = request["id"].to_string();
        let client = client.clone();
        let output = output.clone();
        let device = device.clone();
        let access = access.clone();
        let session_id = session_id.clone();
        let handle=tasks.spawn(async move {
            let id=request["id"].clone();
            let result=client.request_for(json!({"operation":"outsideMcp","deviceId":device,"access":access,"sessionId":session_id,"request":request}),600).await;
            let response=match result {Ok(v)=>v,Err(_)=>json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":"Supervisor request failed"}})};
            let mut out=output.lock().await;out.write_all(format!("{response}\n").as_bytes()).await?;out.flush().await?;Ok::<_,std::io::Error>(())
        });
        pending.insert(key, handle);
        while tasks.try_join_next().is_some() {}
        pending.retain(|_, h| !h.is_finished());
    }
    tasks.abort_all();
    Ok(())
}
