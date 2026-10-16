use super::http::{map_err, ApiErr};
use anyhow::Context;
use axum::{extract::State, Extension, Json};
use pockymoe_protocol::{CreateThreadInput, Provider};
use pockymoe_runtime::{
    interaction::{clamp_wait, list_roles, load_role, AgentOptions, SendInput, TranscriptQuery},
    Supervisor,
};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Roles are looked up from the caller's workspace, or the default root.
fn caller_cwd(state: &Supervisor, from: Option<&str>) -> anyhow::Result<String> {
    Ok(match from {
        Some(from) => {
            state
                .get_workspace(&state.get_thread(from)?.workspace_id)?
                .abs_path
        }
        None => state.config.workspace_root.to_string_lossy().into_owned(),
    })
}

pub(crate) async fn command(
    State(state): State<Arc<Supervisor>>,
    Extension(caller): Extension<crate::auth::CliCaller>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    let input = crate::treer::messages::prepare_send(&state, caller.0.as_deref(), input)
        .map_err(map_err)?;
    if let Some(reply) = crate::peer_send::intercept(&state, caller.0.as_deref(), &input)
        .await
        .map_err(map_err)?
    {
        return Ok(Json(reply));
    }
    Ok(Json(run(&state, input, caller.0).await.map_err(map_err)?))
}

/// One local CLI operation. `peer_api` reuses the read/create subset for requests
/// from other devices, after removing the caller's `fromThreadId`.
pub(crate) async fn run(
    state: &Arc<Supervisor>,
    input: Value,
    caller: Option<String>,
) -> anyhow::Result<Value> {
    crate::host_policy::require_command(state, &input)?;
    let id = input.get("threadId").and_then(Value::as_str).unwrap_or("");
    let from = input["fromThreadId"].as_str();
    let me = || from.context("this command needs the caller's thread identity; pass --from ID");
    let number = || input["number"].as_i64().context("task number required");
    let all = input["all"] == true;
    let wait = clamp_wait(input["timeoutSeconds"].as_u64());
    match input.get("operation").and_then(Value::as_str).unwrap_or("") {
        "previewCreate" | "previewList" | "previewCheck" | "previewStop" => {
            crate::preview_cli::run(state, &input).await
        }
        "automationCreate" | "automationPreview" | "automationList" | "automationShow"
        | "automationRuns" | "automationPause" | "automationResume" | "automationCancel"
        | "commandRun" | "commandShow" => crate::automations::cli(state, &input).await,
        "info" => {
            let identity = crate::peer_link::relay_identity(state);
            Ok(
                json!({"hostMode":state.config.host_mode,"hostCapabilities":state.host_capabilities,"deviceId":state.db.host_id,"relayDeviceId":identity.as_ref().map(|i| &i.device_id),"deviceName":identity.as_ref().map(|i| &i.device_name)}),
            )
        }
        // Recovery for a forgotten local passcode needs the machine owner's own
        // CLI credentials; a thread's scoped token cannot change it.
        "passcodeStatus" | "passcodeOff" => {
            anyhow::ensure!(
                caller.is_none(),
                "Only the computer's owner can change the passcode"
            );
            if input["operation"] == "passcodeOff" {
                crate::auth::set_passcode(state, None)?;
            }
            Ok(json!({"passcode": crate::auth::local_passcode(state).is_some()}))
        }
        "skill" => Ok(json!({
            "version": pockymoe_protocol::APP_VERSION,
            "text": crate::treer::messages::interaction_text(state, pockymoe_protocol::THREAD_INTERACTION_SKILL),
        })),
        "guide" => {
            let topic = input["topic"].as_str().context("guide topic required")?;
            let (_, _, text) = pockymoe_protocol::THREAD_INTERACTION_GUIDES
                .iter()
                .find(|(name, _, _)| *name == topic)
                .context("unknown guide topic")?;
            Ok(json!({"text":crate::treer::messages::interaction_text(state,text)}))
        }
        "list" => {
            let group = input["groupId"].as_str().map(str::to_owned);
            // Drilling into a group, or asking for --all, needs every row; the
            // default listing keeps to lineage roots so a fan-out stays collapsed.
            let include_all =
                group.is_some() || input["includeAgentThreads"].as_bool() == Some(true);
            let mut threads = state.list_threads(
                input.get("workspaceId").and_then(Value::as_str),
                include_all,
            )?;
            if let Some(group) = &group {
                threads.retain(|t| t.root_thread_id.as_deref() == Some(group.as_str()));
            }
            let limit = input["limit"].as_u64().unwrap_or(20).clamp(1, 100) as usize;
            Ok(
                json!({"threads":threads.iter().take(limit).map(|t|json!({"id":t.id,"name":t.agent_name,"title":t.title,"workspaceId":t.workspace_id,"provider":t.provider,"agentId":t.agent_id,"model":t.model,"status":t.status,"updatedAt":t.updated_at,"parentThreadId":t.parent_thread_id,"rootThreadId":t.root_thread_id,"agentThreadCount":t.descendant_count,"closedAt":t.closed_at})).collect::<Vec<_>>(),"totalCount":threads.len()}),
            )
        }
        "show" | "status" => state.interaction_status(id).await,
        "delete" => {
            let caller = caller.ok_or_else(|| anyhow::anyhow!(
                    "forbidden: deletion requires a managed thread credential; reconnect the parent session to obtain one"
                ))?;
            anyhow::ensure!(
                input["fromThreadId"]
                    .as_str()
                    .is_none_or(|from| from == caller),
                "forbidden: --from cannot override the authenticated parent for deletion"
            );
            state.delete_child_thread(&caller, id).await
        }
        "send" => {
            let mut body = serde_json::from_value::<SendInput>(input.clone())?;
            if let Some(caller) = &caller {
                anyhow::ensure!(
                    body.from_thread_id
                        .as_deref()
                        .is_none_or(|from| from == caller),
                    "forbidden: --from cannot override the authenticated caller for peer sends"
                );
                // Omitting attribution must not bypass the peer delivery policy.
                body.from_thread_id = Some(caller.clone());
            }
            let mut receipt = if state.config.treer_enabled {
                let state = state.clone();
                let id = id.to_owned();
                // A Treer prompt can return through this hub's AIS endpoint. Keep
                // the HTTP executor free while the synchronous transport waits.
                tokio::task::spawn_blocking(move || state.send_to_thread(&id, body)).await??
            } else {
                state.send_to_thread(id, body)?
            };
            if receipt["delivery"] == "steer" {
                let pending = receipt["pendingSteerId"].as_str().unwrap();
                match state.steer_pending_prompt(id, pending).await {
                    Ok(_) => receipt["delivery"] = json!("steered"),
                    Err(error) => {
                        // Kept as an explicitly held pending message; the
                        // background continuation worker never runs it.
                        receipt["delivery"] = json!("held");
                        receipt["error"] = json!(error.to_string());
                    }
                }
            }
            Ok(receipt)
        }
        "resolve" => {
            Ok(json!({"threadId":state.resolve_agent(from, input["name"].as_str().unwrap_or(""))?}))
        }
        "tree" => match input["rootThreadId"].as_str() {
            Some(root) => state.agent_tree(root, all).await,
            None => {
                state
                    .lineage_tree(from.context("tree needs a thread; pass --from ID")?, all)
                    .await
            }
        },
        "wait" => {
            state
                .wait_threads(&strings(&input["threadIds"]), input["any"] == true, wait)
                .await
        }
        "wake" => state.register_wake(
            from.context("wake needs the caller's identity")?,
            &strings(&input["threadIds"]),
        ),
        "close" => {
            // Like delete: a managed thread's own credential decides who is closing.
            anyhow::ensure!(
                caller
                    .as_deref()
                    .is_none_or(|c| from.is_none_or(|f| f == c)),
                "forbidden: --from cannot override the authenticated caller for close"
            );
            state
                .close_agent_thread_releasing(
                    caller.as_deref().or(from),
                    id,
                    input["removeWorktree"] == true,
                )
                .await
        }
        "roles" => Ok(list_roles(Path::new(&caller_cwd(state, from)?))),
        "inboxWait" => {
            state
                .inbox_wait(
                    id,
                    &strings(&input["fromThreadIds"]),
                    &strings(&input["kinds"]),
                    input["onlyNew"] == true,
                    wait,
                )
                .await
        }
        "taskAdd" => state.task_add(
            me()?,
            input["title"].as_str().unwrap_or(""),
            input["detail"].as_str(),
            &input["after"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_i64).collect::<Vec<_>>())
                .unwrap_or_default(),
            input["assignThreadId"].as_str(),
        ),
        "taskList" => state.task_list(me()?, all),
        "taskShow" => state.task_show(me()?, number()?),
        "taskClaim" if input["wait"] == true => {
            state
                .task_claim_wait(me()?, input["number"].as_i64(), wait)
                .await
        }
        "taskClaim" => state.task_claim(me()?, input["number"].as_i64()),
        "taskDone" => state.task_done(
            me()?,
            number()?,
            input["result"].as_str(),
            input["failed"] == true,
        ),
        "taskRelease" => state.task_release(me()?, number()?),
        "inbox" => state.inbox_list(id, &input),
        "inboxRead" => state.inbox_read(id, &input),
        "inboxAck" => state.inbox_ack(id, &input),
        "transcript" => {
            state.sync_claude_scheduled_history(id).await?;
            state.transcript(
                id,
                &serde_json::from_value::<TranscriptQuery>(input.clone())?,
            )
        }
        "backends" => Ok(serde_json::to_value(state.backends())?),
        "models" => {
            let provider = serde_json::from_value::<Provider>(
                input.get("provider").cloned().unwrap_or(json!("acp")),
            )?;
            let cwd = if let Some(from) = input["fromThreadId"].as_str() {
                state
                    .get_workspace(&state.get_thread(from)?.workspace_id)?
                    .abs_path
            } else if let Some(workspace) = input["workspaceId"].as_str() {
                state.get_workspace(workspace)?.abs_path
            } else {
                state.config.workspace_root.to_string_lossy().into_owned()
            };
            Ok(serde_json::to_value(
                state
                    .list_models(provider, input["agentId"].as_str(), Some(&cwd))
                    .await?,
            )?)
        }
        "create" => {
            let mut input = input;
            let from = input["fromThreadId"].as_str().map(str::to_owned);
            if let Some(from) = from {
                let caller = state.get_thread(&from)?;
                // Record who asked, so the new thread groups under its lineage root
                // instead of appearing as another top-level entry in the workspace.
                input["parentThreadId"] = json!(from);
                if input.get("workspaceId").is_none() {
                    input["workspaceId"] = json!(caller.workspace_id);
                }
                if input.get("approvalMode").is_none() {
                    input["approvalMode"] = json!(caller.approval_mode);
                }
            }
            let name = input["name"].as_str().map(str::to_owned);
            let role_name = input["role"].as_str().map(str::to_owned);
            let worktree = (input["worktree"] == true || input["worktreeBranch"].is_string())
                .then(|| input["worktreeBranch"].as_str().map(str::to_owned));
            let mut create: CreateThreadInput = serde_json::from_value(input)?;
            let cwd = state.get_workspace(&create.workspace_id)?.abs_path;
            // A role supplies defaults; anything the caller set explicitly wins.
            let role = role_name
                .map(|r| load_role(Path::new(&cwd), &r))
                .transpose()?;
            if let Some(role) = &role {
                if create.model == "default" {
                    if let Some(model) = &role.model {
                        create.model = model.clone();
                    }
                }
                if create.reasoning_effort.is_none() {
                    create.reasoning_effort = role.reasoning_effort.clone();
                }
                if create.agent_id.is_none() {
                    create.agent_id = role.agent.clone();
                }
            }
            let provider = create.provider.unwrap_or_else(|| state.default_provider());
            let models = state
                .list_models(provider, create.agent_id.as_deref(), Some(&cwd))
                .await?;
            let model = models
                .iter()
                .find(|m| m.id == create.model || m.model == create.model);
            anyhow::ensure!(
                create.model == "default" || model.is_some(),
                "model not available; use thread models for this provider/agent"
            );
            if let Some(effort) = create.reasoning_effort.as_deref() {
                anyhow::ensure!(
                    model.is_some_and(|m| m
                        .supported_reasoning_efforts
                        .iter()
                        .any(|e| e.reasoning_effort == effort)),
                    "reasoning effort not available for this model"
                );
            }
            let t = state
                .create_thread_with(
                    create,
                    AgentOptions {
                        name,
                        role,
                        worktree,
                    },
                )
                .await?;
            Ok(
                json!({"threadId":t.id,"name":t.agent_name,"role":t.agent_role,"worktreePath":t.worktree_path,"workspaceId":t.workspace_id,"provider":t.provider,"agentId":t.agent_id,"model":t.model,"reasoningEffort":t.reasoning_effort,"status":t.status}),
            )
        }
        _ => anyhow::bail!("unknown CLI operation"),
    }
}
