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
/// Why a tool is missing here, and where it does work.
pub(super) fn unavailable(name: &str, access: &str) -> anyhow::Error {
    if name == "pockymoe_thread_send" && access != "full" {
        return anyhow::anyhow!(
            "sending to a thread on another device needs full access: the other device does not report the thread's approval mode"
        );
    }
    anyhow::anyhow!(
        "{name} is not available through --device: another device allows listing, reading, launching and waiting (and sending with full access). Run `pockymoe mcp serve` on that device for the rest"
    )
}
/// The remote thread's workspace, for the grant's workspace limit.
async fn thread_workspace(state: &Arc<Supervisor>, thread: &Value, device: &str) -> Result<String> {
    let status = request(
        state,
        json!({"operation":"status","threadId":thread}),
        device,
    )
    .await?;
    status["workspaceId"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("the other device did not report the thread's workspace"))
}
/// A first prompt sent over the peer boundary arrives framed as a message;
/// name the thread after what was asked instead.
pub(super) fn title(args: &Value) -> Option<String> {
    args["title"].as_str().map(str::to_owned).or_else(|| {
        let line = args["prompt"]
            .as_str()?
            .lines()
            .find(|l| !l.trim().is_empty())?;
        Some(line.trim().chars().take(60).collect())
    })
}
pub async fn call(
    state: &Arc<Supervisor>,
    grant: &Grant,
    name: &str,
    args: &Value,
    device: &str,
) -> Result<Value> {
    if !supported(name, &grant.access) {
        return Err(unavailable(name, &grant.access));
    }
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
    if let Some(workspace) = args["workspaceId"].as_str() {
        grant.check(name, Some(workspace), None)?;
    }
    if !args["threadId"].is_null() && grant.workspaces.is_some() {
        let workspace = thread_workspace(state, &args["threadId"], device).await?;
        grant.check(name, Some(&workspace), None)?;
    }
    let mut input = args.clone();
    input["operation"] = json!(op);
    // Results take the same shape as on this device, so a client reads one
    // contract whichever device it reaches.
    match name {
        "pockymoe_thread_launch" => {
            let mode = args["approvalMode"].as_str().unwrap_or("guarded");
            grant.check(name, args["workspaceId"].as_str(), Some(mode))?;
            input["model"] = args.get("model").cloned().unwrap_or(json!("default"));
            input["approvalMode"] = json!(mode);
            input["provider"] = json!("acp");
            input["title"] = json!(title(args));
            input.as_object_mut().unwrap().remove("prompt");
            let mut created = request(state, input, device).await?;
            if let Some(prompt) = args["prompt"].as_str() {
                request(state,json!({"operation":"send","threadId":created["threadId"],"text":prompt,"delivery":"queue"}),device).await?;
            }
            created["id"] = created["threadId"].clone();
            created["deviceId"] = json!(device);
            Ok(json!({"thread":created}))
        }
        "pockymoe_thread_read" => {
            input["view"] = json!(if args["turnId"].is_string() {
                "turn"
            } else {
                args["view"].as_str().unwrap_or("summary")
            });
            input["limit"] = json!(args["limit"].as_u64().unwrap_or(20).min(20));
            Ok(json!({"transcript":request(state,input,device).await?}))
        }
        "pockymoe_thread_wait" => {
            let deadline = tokio::time::Instant::now()
                + std::time::Duration::from_secs(
                    args["timeoutSeconds"].as_u64().unwrap_or(60).min(600),
                );
            loop {
                let status = request(state, input.clone(), device).await?;
                if (status["status"] != "running"
                    && status["queuedCount"].as_u64().unwrap_or(0) == 0)
                    || status["waitingForInput"] == true
                    || tokio::time::Instant::now() >= deadline
                {
                    let transcript=request(state,json!({"operation":"transcript","threadId":args["threadId"],"view":"summary","limit":1}),device).await?;
                    return Ok(json!({"status":status,"transcript":transcript}));
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
        "pockymoe_workspaces" => {
            let mut list = request(state, input, device).await?;
            if let Some(rows) = list.as_array_mut() {
                rows.retain(|w| {
                    w["id"]
                        .as_str()
                        .is_some_and(|id| grant.allows_workspace(id))
                });
            }
            Ok(json!({"workspaces":list}))
        }
        "pockymoe_threads" => {
            // Filter here: the workspace limit and title search are this grant's.
            input.as_object_mut().unwrap().remove("query");
            input["limit"] = json!(100);
            let mut result = request(state, input, device).await?;
            if let Some(rows) = result["threads"].as_array_mut() {
                let query = args["query"].as_str().unwrap_or("").to_lowercase();
                rows.retain(|t| {
                    t["workspaceId"]
                        .as_str()
                        .is_some_and(|id| grant.allows_workspace(id))
                        && t["title"]
                            .as_str()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&query)
                });
                rows.truncate(args["limit"].as_u64().unwrap_or(30).clamp(1, 100) as usize);
            }
            Ok(json!({"threads":result["threads"]}))
        }
        "pockymoe_agents" => Ok(json!({"agents":request(state,input,device).await?})),
        "pockymoe_models" => Ok(json!({"models":request(state,input,device).await?})),
        "pockymoe_devices" => {
            let result = request(state, input, device).await?;
            Ok(json!({"deviceId":device,"peers":result}))
        }
        _ => {
            ensure!(name == "pockymoe_thread_send", "unsupported remote tool");
            request(state, input, device).await
        }
    }
}
