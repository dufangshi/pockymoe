use super::http::{map_err, ApiErr};
use anyhow::Context;
use axum::{extract::State, Extension, Json};
use pockymoe_protocol::{CreateThreadInput, Provider, ThreadDto};
use pockymoe_runtime::{
    interaction::{
        clamp_wait, list_roles, load_role, AgentOptions, AgentRequester, SendInput, TranscriptQuery,
    },
    Supervisor,
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

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

/// Threads that are top-level from one workspace's point of view: the lineage
/// roots started there, and agent threads that work there for a conversation
/// started in another workspace. An agent thread folds into its topmost ancestor
/// in its own workspace, so one working where its root or an earlier ancestor
/// also works is not listed again. `threads` must hold every thread, of every
/// workspace, so ancestors can be found; their order is kept.
fn workspace_top_level(threads: Vec<ThreadDto>, workspace: &str) -> Vec<ThreadDto> {
    let by_id: HashMap<&str, &ThreadDto> = threads.iter().map(|t| (t.id.as_str(), t)).collect();
    let folded = |thread: &ThreadDto| {
        let shares_workspace = |id: &str| {
            by_id.get(id).is_some_and(|ancestor| {
                ancestor.id != thread.id && ancestor.workspace_id == thread.workspace_id
            })
        };
        let mut parent = thread.parent_thread_id.as_deref();
        // Lineages are at most a few levels deep; the bound only guards bad data.
        for _ in 0..16 {
            let Some(id) = parent else { break };
            if shares_workspace(id) {
                return true;
            }
            parent = by_id.get(id).and_then(|t| t.parent_thread_id.as_deref());
        }
        // A missing intermediate thread still leaves the root to fold into.
        thread
            .root_thread_id
            .as_deref()
            .is_some_and(shares_workspace)
    };
    let top: HashSet<String> = threads
        .iter()
        .filter(|t| t.workspace_id == workspace && !folded(t))
        .map(|t| t.id.clone())
        .collect();
    threads
        .into_iter()
        .filter(|t| top.contains(&t.id))
        .collect()
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

/// One local CLI operation.
pub(crate) async fn run(
    state: &Arc<Supervisor>,
    input: Value,
    caller: Option<String>,
) -> anyhow::Result<Value> {
    run_as(state, input, caller, false).await
}

/// The read/create subset for requests from other devices, after `peer_api`
/// removed the caller's `fromThreadId`.
pub(crate) async fn run_from_peer(state: &Arc<Supervisor>, input: Value) -> anyhow::Result<Value> {
    run_as(state, input, None, true).await
}

/// Who an operation acts for, for agent isolation. The machine's own credential
/// is the owner; a thread credential is that thread.
pub(crate) fn requester(caller: Option<&str>, peer: bool) -> AgentRequester<'_> {
    match caller {
        _ if peer => AgentRequester::Peer,
        Some(thread) => AgentRequester::Thread(thread),
        None => AgentRequester::Owner,
    }
}

async fn run_as(
    state: &Arc<Supervisor>,
    input: Value,
    caller: Option<String>,
    peer: bool,
) -> anyhow::Result<Value> {
    crate::host_policy::require_command(state, &input)?;
    let requester = requester(caller.as_deref(), peer);
    let reach = |thread: &str| state.ensure_agent_reaches_thread(requester, thread);
    let id = input.get("threadId").and_then(Value::as_str).unwrap_or("");
    let from = input["fromThreadId"].as_str();
    let me = || from.context("this command needs the caller's thread identity; pass --from ID");
    let number = || input["number"].as_i64().context("task number required");
    let all = input["all"] == true;
    let wait = clamp_wait(input["timeoutSeconds"].as_u64());
    match input.get("operation").and_then(Value::as_str).unwrap_or("") {
        "previewCreate" | "previewList" | "previewCheck" | "previewStop" => {
            // Only the authenticated credential names the thread, never `fromThreadId`.
            crate::preview_cli::run(state, &input, caller.as_deref()).await
        }
        "automationCreate" | "automationPreview" | "automationList" | "automationShow"
        | "automationRuns" | "automationPause" | "automationResume" | "automationCancel"
        | "commandRun" | "commandShow" => crate::automations::cli(state, &input, requester).await,
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
            // A workspace's listing also has the agent threads working there for
            // a conversation started elsewhere: there, they are top-level.
            let include_all =
                group.is_some() || input["includeAgentThreads"].as_bool() == Some(true);
            let workspace = input.get("workspaceId").and_then(Value::as_str);
            let mut threads = match workspace {
                Some(workspace) if !include_all => {
                    workspace_top_level(state.list_threads(None, true)?, workspace)
                }
                _ => state.list_threads(workspace, include_all)?,
            };
            if let Some(group) = &group {
                threads.retain(|t| t.root_thread_id.as_deref() == Some(group.as_str()));
            }
            threads.retain(|t| state.agent_reaches_workspace(requester, &t.workspace_id));
            let limit = input["limit"].as_u64().unwrap_or(20).clamp(1, 100) as usize;
            Ok(
                json!({"threads":threads.iter().take(limit).map(|t|json!({"id":t.id,"name":t.agent_name,"title":t.title,"workspaceId":t.workspace_id,"provider":t.provider,"agentId":t.agent_id,"model":t.model,"status":t.status,"updatedAt":t.updated_at,"parentThreadId":t.parent_thread_id,"rootThreadId":t.root_thread_id,"agentThreadCount":t.descendant_count,"closedAt":t.closed_at})).collect::<Vec<_>>(),"totalCount":threads.len()}),
            )
        }
        "show" | "status" => {
            reach(id)?;
            state.interaction_status(id).await
        }
        "delete" => {
            let caller = caller.clone().ok_or_else(|| anyhow::anyhow!(
                    "forbidden: deletion requires a managed thread credential; reconnect the parent session to obtain one"
                ))?;
            anyhow::ensure!(
                input["fromThreadId"]
                    .as_str()
                    .is_none_or(|from| from == caller),
                "forbidden: --from cannot override the authenticated parent for deletion"
            );
            reach(id)?;
            state.delete_child_thread(&caller, id).await
        }
        "send" => {
            reach(id)?;
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
            Some(root) => {
                reach(root)?;
                state.agent_tree(root, all).await
            }
            None => {
                state
                    .lineage_tree(from.context("tree needs a thread; pass --from ID")?, all)
                    .await
            }
        },
        "wait" => {
            let threads = strings(&input["threadIds"]);
            threads.iter().try_for_each(|thread| reach(thread))?;
            state
                .wait_threads(&threads, input["any"] == true, wait)
                .await
        }
        "wake" => {
            let threads = strings(&input["threadIds"]);
            threads.iter().try_for_each(|thread| reach(thread))?;
            state.register_wake(from.context("wake needs the caller's identity")?, &threads)
        }
        "close" => {
            // Like delete: a managed thread's own credential decides who is closing.
            anyhow::ensure!(
                caller
                    .as_deref()
                    .is_none_or(|c| from.is_none_or(|f| f == c)),
                "forbidden: --from cannot override the authenticated caller for close"
            );
            reach(id)?;
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
            reach(id)?;
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
        "taskAdd" => {
            if let Some(assignee) = input["assignThreadId"].as_str() {
                reach(assignee)?;
            }
            state.task_add(
                me()?,
                input["title"].as_str().unwrap_or(""),
                input["detail"].as_str(),
                &input["after"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_i64).collect::<Vec<_>>())
                    .unwrap_or_default(),
                input["assignThreadId"].as_str(),
            )
        }
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
        operation @ ("inbox" | "inboxRead" | "inboxAck") => {
            reach(id)?;
            match operation {
                "inbox" => state.inbox_list(id, &input),
                "inboxRead" => state.inbox_read(id, &input),
                _ => state.inbox_ack(id, &input),
            }
        }
        "transcript" => {
            reach(id)?;
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
                state.ensure_agent_reaches_workspace(requester, workspace)?;
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
            if let Some(workspace) = input["workspaceId"].as_str() {
                state.ensure_agent_reaches_workspace(requester, workspace)?;
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
            // Discovery only knows a model's efforts once a session on it
            // reported them. A model that lists efforts must include this one;
            // one that lists none (unknown) takes it like the UI would, and
            // its harness applies or refuses it when the session starts.
            let mut effort_unconfirmed = false;
            if let Some(effort) = create.reasoning_effort.as_deref() {
                anyhow::ensure!(
                    !effort.is_empty()
                        && effort.len() <= 32
                        && effort
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                    "invalid reasoning effort"
                );
                let known = model
                    .map(|m| m.supported_reasoning_efforts.as_slice())
                    .unwrap_or_default();
                if known.is_empty() {
                    effort_unconfirmed = true;
                } else {
                    anyhow::ensure!(
                        known.iter().any(|e| e.reasoning_effort == effort),
                        "reasoning effort {effort} not available for this model; it offers {}",
                        known
                            .iter()
                            .map(|e| e.reasoning_effort.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
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
            let mut receipt = json!({"threadId":t.id,"name":t.agent_name,"role":t.agent_role,"worktreePath":t.worktree_path,"workspaceId":t.workspace_id,"provider":t.provider,"agentId":t.agent_id,"model":t.model,"reasoningEffort":t.reasoning_effort,"status":t.status});
            if effort_unconfirmed {
                receipt["reasoningEffortUnconfirmed"] = json!(true);
                receipt["note"] = json!("Model discovery lists no efforts for this model yet; the harness applies the effort when the session starts.");
            }
            Ok(receipt)
        }
        _ => anyhow::bail!("unknown CLI operation"),
    }
}

#[cfg(test)]
mod tests {
    use crate::isolation_tests::{cli, setup, thread};
    use pockymoe_protocol::{CreateThreadInput, Provider};
    use pockymoe_runtime::Supervisor;
    use serde_json::{json, Value};

    async fn child(state: &Supervisor, workspace: &str, parent: &str) -> String {
        state
            .create_thread(CreateThreadInput {
                workspace_id: workspace.into(),
                title: None,
                provider: Some(Provider::Codex),
                agent_id: None,
                model: "ios-e2e-stream".into(),
                reasoning_effort: None,
                approval_mode: "guarded".into(),
                parent_thread_id: Some(parent.into()),
            })
            .await
            .unwrap()
            .id
    }

    fn ids(listed: &Value) -> Vec<String> {
        let mut ids: Vec<_> = listed["threads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_owned())
            .collect();
        ids.sort();
        ids
    }

    #[tokio::test]
    async fn an_agent_picks_any_effort_for_a_model_whose_efforts_discovery_does_not_list() {
        let f = setup().await;
        let parent = thread(&f.state, &f.open).await;
        // Discovery has no efforts for the default model: the effort is kept.
        let created = cli(&f, Some(&parent), json!({"operation":"create","workspaceId":f.open,"provider":"codex","model":"default","reasoningEffort":"high"})).await.unwrap();
        assert_eq!(created["reasoningEffort"], "high");
        assert_eq!(created["reasoningEffortUnconfirmed"], true);
        // A model that lists its efforts must include the requested one.
        let listed = cli(&f, Some(&parent), json!({"operation":"create","workspaceId":f.open,"provider":"codex","model":"ios-e2e-stream","reasoningEffort":"high"})).await.unwrap();
        assert_eq!(listed["reasoningEffort"], "high");
        assert!(listed.get("reasoningEffortUnconfirmed").is_none());
        let refused = cli(&f, Some(&parent), json!({"operation":"create","workspaceId":f.open,"provider":"codex","model":"ios-e2e-stream","reasoningEffort":"ultra"})).await.unwrap_err().to_string();
        assert!(refused.contains("low, medium, high"), "{refused}");
        assert!(cli(&f, Some(&parent), json!({"operation":"create","workspaceId":f.open,"provider":"codex","model":"default","reasoningEffort":"high; rm"})).await.is_err());
    }

    fn sorted<const N: usize>(ids: [&String; N]) -> Vec<String> {
        let mut ids: Vec<_> = ids.into_iter().cloned().collect();
        ids.sort();
        ids
    }

    #[tokio::test]
    async fn a_workspace_listing_includes_agent_threads_working_there_for_a_conversation_elsewhere()
    {
        let f = setup().await;
        let (here, there) = (f.open.clone(), f.isolated.clone());
        let root = thread(&f.state, &here).await;
        let other_root = thread(&f.state, &there).await;
        // Started from `here`, working `there`: top-level there.
        let worker = child(&f.state, &there, &root).await;
        // Its own agent thread there folds into it; one back `here` folds into the root.
        let worker_helper = child(&f.state, &there, &worker).await;
        let back_here = child(&f.state, &here, &worker).await;
        let beside = child(&f.state, &here, &root).await;
        let list = |workspace: &str, all: bool| json!({"operation":"list","workspaceId":workspace,"includeAgentThreads":all});

        let there_listed = cli(&f, None, list(&there, false)).await.unwrap();
        assert_eq!(ids(&there_listed), sorted([&other_root, &worker]));
        assert_eq!(there_listed["totalCount"], 2);
        let here_listed = cli(&f, None, list(&here, false)).await.unwrap();
        assert_eq!(ids(&here_listed), sorted([&root]));
        assert_eq!(here_listed["totalCount"], 1);
        // --all and the device-wide listing are unchanged.
        let everything = cli(&f, None, list(&there, true)).await.unwrap();
        assert_eq!(
            ids(&everything),
            sorted([&other_root, &worker, &worker_helper])
        );
        let everything_here = cli(&f, None, list(&here, true)).await.unwrap();
        assert_eq!(ids(&everything_here), sorted([&root, &back_here, &beside]));
        let roots = cli(&f, None, json!({"operation":"list"})).await.unwrap();
        assert_eq!(ids(&roots), sorted([&root, &other_root]));
        assert_eq!(roots["totalCount"], 2);
    }
}
