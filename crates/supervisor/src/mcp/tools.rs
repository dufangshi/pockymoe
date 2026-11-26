use anyhow::{bail, ensure, Context, Result};
use pockymoe_runtime::Supervisor;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Grant {
    pub device: Option<String>,
    pub client: String,
    pub access: String,
    pub workspaces: Option<Vec<String>>,
}
impl Grant {
    pub fn allows_workspace(&self, workspace: &str) -> bool {
        self.workspaces
            .as_ref()
            .is_none_or(|w| w.iter().any(|w| w == workspace))
    }
    pub fn check(&self, name: &str, workspace: Option<&str>, approval: Option<&str>) -> Result<()> {
        let level = level(name).context("unknown tool")?;
        ensure!(
            matches!(self.access.as_str(), "read" | "supervised" | "full"),
            "invalid access level"
        );
        ensure!(level == 0 || self.access != "read", "read-only grant");
        ensure!(level < 2 || self.access == "full", "full access required");
        if let Some(w) = workspace {
            ensure!(self.allows_workspace(w), "workspace outside grant");
        }
        if level > 0 && self.access == "supervised" {
            ensure!(
                approval.is_none_or(|m| m == "guarded"),
                "supervised access requires guarded approval mode"
            );
        }
        Ok(())
    }
}
fn level(name: &str) -> Option<u8> {
    Some(match name {
        "pockymoe_devices"
        | "pockymoe_workspaces"
        | "pockymoe_agents"
        | "pockymoe_models"
        | "pockymoe_threads"
        | "pockymoe_thread_read"
        | "pockymoe_thread_wait"
        | "pockymoe_git_status"
        | "pockymoe_git_diff"
        | "pockymoe_git_turn_diff"
        | "pockymoe_events" => 0,
        "pockymoe_thread_launch" | "pockymoe_thread_send" | "pockymoe_thread_stop" => 1,
        "pockymoe_thread_answer" | "pockymoe_thread_switch" => 2,
        _ => return None,
    })
}
pub fn catalog() -> Value {
    let s = || json!({"type":"string"});
    let defs = vec![
        ("devices", "Local device identity", json!({}), vec![]),
        ("workspaces", "List allowed workspaces", json!({}), vec![]),
        ("agents", "List harnesses", json!({}), vec![]),
        (
            "models",
            "List models for a workspace",
            json!({"workspaceId":s(),"agentId":s()}),
            vec!["workspaceId"],
        ),
        (
            "threads",
            "Search allowed threads by title",
            json!({"workspaceId":s(),"query":s(),"limit":{"type":"integer","minimum":1,"maximum":100}}),
            vec![],
        ),
        (
            "thread_read",
            "Read a thread, transcript page, or turn",
            json!({"threadId":s(),"turnId":s(),"beforeTurnId":s(),"view":{"type":"string","enum":["summary","overview","turn"]},"limit":{"type":"integer","minimum":1,"maximum":100}}),
            vec!["threadId"],
        ),
        (
            "thread_wait",
            "Wait until idle or input is needed; returns status and transcript",
            json!({"threadId":s(),"timeoutSeconds":{"type":"integer","minimum":0,"maximum":600}}),
            vec!["threadId"],
        ),
        (
            "thread_launch",
            "Create a thread and optionally queue its first prompt",
            json!({"workspaceId":s(),"title":s(),"agentId":s(),"model":s(),"prompt":s(),"approvalMode":{"type":"string","enum":["guarded","yolo"]}}),
            vec!["workspaceId"],
        ),
        (
            "thread_send",
            "Send inbox mail, queue a prompt, or steer",
            json!({"threadId":s(),"text":s(),"delivery":{"type":"string","enum":["inbox","queue","steer"]},"subject":s(),"interruptReason":s()}),
            vec!["threadId", "text"],
        ),
        (
            "thread_switch",
            "Switch an idle thread to another harness, preserving its history (full access)",
            json!({"threadId":s(),"provider":{"type":"string","enum":["acp","codex","claude","opencode"]},"agentId":s(),"model":s(),"reasoningEffort":s(),"preview":{"type":"boolean"}}),
            vec!["threadId", "agentId"],
        ),
        (
            "thread_stop",
            "Stop a thread",
            json!({"threadId":s(),"reason":s()}),
            vec!["threadId"],
        ),
        (
            "thread_answer",
            "Answer a pending approval (full access)",
            json!({"threadId":s(),"requestId":s(),"allow":{"type":"boolean"},"answer":s()}),
            vec!["threadId", "requestId", "allow"],
        ),
        (
            "git_status",
            "Working copy Git status",
            json!({"threadId":s(),"workingCopyId":s()}),
            vec!["threadId"],
        ),
        (
            "git_diff",
            "Working copy Git diff",
            json!({"threadId":s(),"workingCopyId":s(),"area":{"type":"string","enum":["worktree","index","branch","commit"]},"reference":s(),"path":s()}),
            vec!["threadId"],
        ),
        (
            "git_turn_diff",
            "Diff the before/after checkpoints of one turn",
            json!({"threadId":s(),"turnId":s(),"workingCopyId":s(),"path":s(),"ignoreWhitespace":{"type":"boolean"}}),
            vec!["threadId", "turnId"],
        ),
        (
            "events",
            "Poll persisted turn, input-needed and PR milestones with a cursor",
            json!({"cursor":s()}),
            vec![],
        ),
    ];
    json!({"tools":defs.into_iter().map(|(n,d,p,r)| {let name=format!("pockymoe_{n}"); let l=level(&name).unwrap(); json!({"name":name,"description":d,"inputSchema":{"type":"object","properties":p,"required":r,"additionalProperties":false},"outputSchema":{"type":"object","additionalProperties":true},"annotations":{"readOnlyHint":l==0,"destructiveHint":l>0,"openWorldHint":l>0}})}).collect::<Vec<_>>()})
}
pub async fn call(
    state: &Arc<Supervisor>,
    grant: &Grant,
    name: &str,
    args: &Value,
) -> Result<Value> {
    grant.check(name, None, None)?;
    ensure!(args.is_object(), "arguments must be an object");
    let catalog = catalog();
    let spec = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .context("unknown tool")?;
    let schema = &spec["inputSchema"];
    for required in schema["required"].as_array().unwrap() {
        ensure!(
            args.get(required.as_str().unwrap()).is_some(),
            "missing required argument: {required}"
        );
    }
    for (key, value) in args.as_object().unwrap() {
        let property = &schema["properties"][key];
        ensure!(!property.is_null(), "unknown argument: {key}");
        ensure!(
            match property["type"].as_str().unwrap_or("") {
                "string" => value.is_string(),
                "integer" => value.as_u64().is_some(),
                "boolean" => value.is_boolean(),
                _ => false,
            },
            "invalid argument type: {key}"
        );
        if let Some(choices) = property["enum"].as_array() {
            ensure!(choices.contains(value), "invalid argument value: {key}");
        }
        if let Some(n) = value.as_u64() {
            ensure!(
                property["minimum"].as_u64().is_none_or(|min| n >= min)
                    && property["maximum"].as_u64().is_none_or(|max| n <= max),
                "argument out of range: {key}"
            );
        }
    }

    if let Some(device) = &grant.device {
        return super::remote::call(state, grant, name, args, device).await;
    }
    let thread = args["threadId"]
        .as_str()
        .map(|id| state.get_thread(id))
        .transpose()?;
    if let Some(t) = &thread {
        grant.check(name, Some(&t.workspace_id), Some(&t.approval_mode))?;
    }
    if let Some(w) = args["workspaceId"].as_str() {
        grant.check(name, Some(w), None)?;
    }
    let id = || args["threadId"].as_str().context("threadId required");
    match name {
        "pockymoe_devices" => Ok(json!({"deviceId":state.db.host_id,"name":state.config.app_name})),
        "pockymoe_workspaces" => Ok(
            json!({"workspaces":state.list_workspaces()?.into_iter().filter(|w|grant.allows_workspace(&w.id)).collect::<Vec<_>>()}),
        ),
        "pockymoe_agents" => {
            Ok(json!({"agents":state.list_agents(pockymoe_protocol::Provider::Acp).await?}))
        }
        "pockymoe_models" => {
            let w = args["workspaceId"]
                .as_str()
                .context("workspaceId required")?;
            let cwd = state.get_workspace(w)?.abs_path;
            Ok(
                json!({"models":state.list_models(pockymoe_protocol::Provider::Acp,args["agentId"].as_str(),Some(&cwd)).await?}),
            )
        }
        "pockymoe_events" => {
            super::events::poll(state, grant, args["cursor"].as_str().unwrap_or(""))
        }
        "pockymoe_threads" => {
            let query = args["query"].as_str().unwrap_or("").to_lowercase();
            let mut rows = state.list_threads(args["workspaceId"].as_str(), true)?;
            rows.retain(|t| {
                grant.allows_workspace(&t.workspace_id) && t.title.to_lowercase().contains(&query)
            });
            rows.truncate(args["limit"].as_u64().unwrap_or(30).clamp(1, 100) as usize);
            Ok(json!({"threads":rows}))
        }
        "pockymoe_thread_read" => {
            let id = id()?;
            if args["view"].is_string() || args["beforeTurnId"].is_string() {
                let mut query = args.clone();
                query.as_object_mut().unwrap().remove("threadId");
                query["limit"] = json!(args["limit"].as_u64().unwrap_or(10).min(20));
                return Ok(
                    json!({"transcript":state.transcript(id,&serde_json::from_value(query)?)?}),
                );
            }
            if let Some(turn) = args["turnId"].as_str() {
                Ok(json!({"turn":state.get_thread_turn_detail(id,turn).await?}))
            } else {
                Ok(
                    json!({"thread":state.get_thread_detail(id,Some(args["limit"].as_u64().unwrap_or(20).clamp(1,100) as u32)).await?}),
                )
            }
        }
        "pockymoe_thread_wait" => {
            let id = id()?;
            let deadline = tokio::time::Instant::now()
                + std::time::Duration::from_secs(
                    args["timeoutSeconds"].as_u64().unwrap_or(60).min(600),
                );
            loop {
                let t = state.get_thread(id)?;
                let status = serde_json::to_value(&t.status)?;
                let control = state.interaction_status(id).await?;
                if (status != "running"
                    && status != "recovering"
                    && control["queuedCount"].as_u64().unwrap_or(0) == 0)
                    || control["waitingForInput"] == true
                    || tokio::time::Instant::now() >= deadline
                {
                    return Ok(
                        json!({"status":control,"thread":state.get_thread_detail(id,Some(1)).await?}),
                    );
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
        "pockymoe_thread_launch" => {
            let w = args["workspaceId"]
                .as_str()
                .context("workspaceId required")?;
            let mode = args["approvalMode"].as_str().unwrap_or("guarded");
            grant.check(name, Some(w), Some(mode))?;
            ensure!(matches!(mode, "guarded" | "yolo"), "invalid approvalMode");
            let t=state.create_thread(serde_json::from_value(json!({"workspaceId":w,"title":args["title"],"provider":"acp","agentId":args["agentId"],"model":args["model"].as_str().unwrap_or("default"),"approvalMode":mode}))?).await?;
            if let Some(prompt) = args["prompt"].as_str() {
                state.send_to_thread(
                    &t.id,
                    serde_json::from_value(json!({"text":prompt,"delivery":"queue"}))?,
                )?;
            }
            Ok(json!({"thread":t}))
        }
        "pockymoe_thread_send" => {
            let mut input = args.clone();
            input["operation"] = json!("send");
            let d = input["delivery"].as_str().unwrap_or("inbox");
            ensure!(matches!(d, "inbox" | "queue" | "steer"), "invalid delivery");
            Box::pin(crate::interaction::run(state, input, None)).await
        }
        "pockymoe_thread_switch" => {
            let target: pockymoe_runtime::service::HarnessSwitchInput = serde_json::from_value(
                json!({"provider":args["provider"].as_str().unwrap_or("acp"),"agentId":args["agentId"],"model":args["model"],"reasoningEffort":args["reasoningEffort"]}),
            )?;
            if args["preview"] == true {
                Ok(json!({"preview":state.preview_harness_switch(id()?,&target)?}))
            } else {
                Ok(json!({"thread":state.switch_thread_harness(id()?,target).await?}))
            }
        }
        "pockymoe_thread_stop" => Ok(json!({"thread":state.interrupt(id()?).await?})),
        "pockymoe_thread_answer" => Ok(
            json!({"thread":state.respond_request(id()?,args["requestId"].as_str().context("requestId required")?,args["allow"].as_bool().context("allow required")?,args["answer"].as_str()).await?}),
        ),
        "pockymoe_git_turn_diff" => state.checkpoint_diff(
            id()?,
            args["workingCopyId"].as_str(),
            args["turnId"].as_str().context("turnId required")?,
            args["path"].as_str(),
            args["ignoreWhitespace"] == true,
        ),
        "pockymoe_git_status" => state.worktree_git_status(id()?, args["workingCopyId"].as_str()),
        "pockymoe_git_diff" => state.worktree_git_diff(
            id()?,
            args["workingCopyId"].as_str(),
            args["area"].as_str().unwrap_or("worktree"),
            args["reference"].as_str(),
            args["path"].as_str(),
        ),
        _ => bail!("unknown tool"),
    }
}
