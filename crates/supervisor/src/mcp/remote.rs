//! The stdio --device option uses the existing CLI peer trust boundary.
use super::tools::Grant;
use anyhow::{bail, ensure, Result};
use pockymoe_runtime::Supervisor;
use serde_json::{json, Value};
use std::sync::Arc;
pub fn supported(name: &str, access: &str) -> bool {
    matches!(
        name,
        "pockymoe_devices"
            | "pockymoe_workspaces"
            | "pockymoe_threads"
            | "pockymoe_agents"
            | "pockymoe_models"
            | "pockymoe_thread_read"
            | "pockymoe_thread_launch"
            | "pockymoe_thread_wait"
    ) || (name == "pockymoe_thread_send" && access == "full")
}
async fn request(state: &Arc<Supervisor>, mut value: Value, device: &str) -> Result<Value> {
    value["deviceId"] = json!(device);
    Box::pin(crate::peer_send::intercept(state, None, &value))
        .await?
        .ok_or_else(|| anyhow::anyhow!("device did not handle request"))
}
pub async fn call(
    state: &Arc<Supervisor>,
    grant: &Grant,
    name: &str,
    args: &Value,
    device: &str,
) -> Result<Value> {
    ensure!(
        supported(name, &grant.access),
        "tool unavailable through the cross-device CLI boundary"
    );
    let op = match name {
        "pockymoe_devices" => "devices",
        "pockymoe_workspaces" => "workspaces",
        "pockymoe_threads" => "list",
        "pockymoe_agents" => "backends",
        "pockymoe_models" => "models",
        "pockymoe_thread_read" => "transcript",
        "pockymoe_thread_launch" => "create",
        "pockymoe_thread_send" => "send",
        "pockymoe_thread_wait" => "status",
        _ => bail!("unsupported remote tool"),
    };
    let mut input = args.clone();
    input["operation"] = json!(op);
    if name == "pockymoe_thread_launch" {
        input["model"] = args.get("model").cloned().unwrap_or(json!("default"));
        input["approvalMode"] = args
            .get("approvalMode")
            .cloned()
            .unwrap_or(json!("guarded"));
        grant.check(
            name,
            args["workspaceId"].as_str(),
            input["approvalMode"].as_str(),
        )?;
        let created = request(state, input, device).await?;
        if let Some(prompt) = args["prompt"].as_str() {
            request(state,json!({"operation":"send","threadId":created["threadId"],"text":prompt,"delivery":"queue"}),device).await?;
        }
        return Ok(json!({"thread":created}));
    }
    if name == "pockymoe_thread_read" {
        input["view"] = json!(if args["turnId"].is_string() {
            "turn"
        } else {
            "summary"
        });
        input["limit"] = json!(args["limit"].as_u64().unwrap_or(20).min(20));
    }
    if name == "pockymoe_thread_wait" {
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_secs(
                args["timeoutSeconds"].as_u64().unwrap_or(60).min(600),
            );
        loop {
            let status = request(state, input.clone(), device).await?;
            if (status["status"] != "running" && status["queuedCount"].as_u64().unwrap_or(0) == 0)
                || status["waitingForInput"] == true
                || tokio::time::Instant::now() >= deadline
            {
                let transcript=request(state,json!({"operation":"transcript","threadId":args["threadId"],"view":"summary","limit":1}),device).await?;
                return Ok(json!({"status":status,"thread":transcript}));
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
    Ok(json!({"result":request(state,input,device).await?}))
}
