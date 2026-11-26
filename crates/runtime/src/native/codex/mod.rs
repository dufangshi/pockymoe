//! Direct Codex app-server runtime. The shared JSON-RPC process owns stdio and cleanup.
mod mapping;
mod wire;
use crate::{
    acp::{rpc::AcpProcess, ProductSessionPolicy, TurnMapper},
    actor::*,
};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use pockymoe_protocol::*;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
struct Session {
    process: Arc<AcpProcess>,
    updates: Mutex<tokio::sync::mpsc::Receiver<Value>>,
    requests: Mutex<tokio::sync::mpsc::Receiver<(i64, String, Value)>>,
    settings: RwLock<SessionSettings>,
    cwd: String,
    loaded: RwLock<LoadedSession>,
    active: RwLock<Option<String>>,
}
struct Pending {
    process: Arc<AcpProcess>,
    rpc_id: i64,
    method: String,
    thread: String,
    dto: ThreadActionRequestDto,
}
pub struct CodexRuntime {
    provider: Provider,
    command: Option<String>,
    fallback: SharedRuntime,
    upstreams: PathBuf,
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    pending: Mutex<HashMap<String, Pending>>,
    events: broadcast::Sender<SessionLifecycle>,
}
impl CodexRuntime {
    pub fn new(provider: Provider, fallback: SharedRuntime, upstreams: PathBuf) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            provider,
            command: None,
            fallback,
            upstreams,
            sessions: RwLock::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            events,
        }
    }
    fn session(&self, id: &str) -> Result<Arc<Session>> {
        self.sessions
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("Native Codex session is not loaded"))
    }
    async fn spawn(
        &self,
        cwd: &str,
    ) -> Result<(
        Arc<AcpProcess>,
        tokio::sync::mpsc::Receiver<Value>,
        tokio::sync::mpsc::Receiver<(i64, String, Value)>,
    )> {
        let isolated = crate::execution::current().is_some();
        let command = if isolated {
            "codex".into()
        } else {
            self.command
                .clone()
                .or_else(|| std::env::var("CODEX_COMMAND").ok())
                .unwrap_or_else(|| "codex".into())
        };
        let env = if isolated {
            crate::execution::isolated_harness_environment("codex")?
        } else if crate::upstreams::native::oauth_selected(&self.upstreams, "codex") {
            vec![(crate::upstreams::native::CLEAN_OAUTH_ENV, "codex".into())]
        } else {
            vec![]
        };
        let (process, updates, requests) =
            AcpProcess::spawn(&format!("{command} app-server"), cwd, &env).await?;
        let process = Arc::new(process);
        let initialized = process.request("initialize",json!({"clientInfo":{"name":"pockymoe","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await;
        if let Err(e) = initialized {
            let _ = process.shutdown().await;
            return Err(e);
        }
        process.notify("initialized", json!({})).await?;
        Ok((process, updates, requests))
    }
    async fn load(
        &self,
        method: &str,
        raw: Option<&str>,
        cwd: &str,
        settings: SessionSettings,
    ) -> Result<StartSessionResult> {
        let (process, updates, requests) = self.spawn(cwd).await?;
        let policy = policy(&settings);
        let mut params = json!({"cwd":cwd,"model":settings.model,"approvalPolicy":policy["approvalPolicy"],"sandbox":policy["sandbox"],"persistExtendedHistory":true});
        if let Some(raw) = raw {
            params["threadId"] = json!(raw);
        }
        let response = match process.request(method, params).await {
            Ok(v) => v,
            Err(e) => {
                let _ = process.shutdown().await;
                return Err(e);
            }
        };
        let raw = response["thread"]["id"]
            .as_str()
            .context("Codex thread id missing")?;
        let id = crate::import_id::scoped_session_id("codex", raw);
        let now = chrono::Utc::now();
        let intent = load_intent();
        let loaded = LoadedSession {
            session_id: id.clone(),
            thread_id: intent.as_ref().map(|i| i.thread_id.clone()),
            process_id: process.id.clone(),
            root_pid: process.pid,
            load_reason: intent
                .as_ref()
                .map(|i| i.reason)
                .unwrap_or(HarnessLoadReason::Turn),
            requested_at: intent.as_ref().map(|i| i.requested_at).unwrap_or(now),
            loaded_at: now,
            last_activity_at: now,
            prompted: false,
            baseline: vec![],
            busy: vec![],
        };
        let _ = self.events.send(SessionLifecycle::Loaded {
            session_id: id.clone(),
            thread_id: loaded.thread_id.clone(),
            reason: loaded.load_reason,
            requested_at: loaded.requested_at,
        });
        self.sessions.write().unwrap().insert(
            id.clone(),
            Arc::new(Session {
                process,
                updates: Mutex::new(updates),
                requests: Mutex::new(requests),
                settings: RwLock::new(settings.clone()),
                cwd: cwd.into(),
                loaded: RwLock::new(loaded),
                active: RwLock::new(None),
            }),
        );
        Ok(StartSessionResult {
            provider_session_id: id,
            model: response["model"]
                .as_str()
                .map(str::to_owned)
                .or(settings.model),
            reasoning_effort: settings.effort,
        })
    }
}
fn policy(settings: &SessionSettings) -> Value {
    crate::acp::codex_permissions::native_policy(&ProductSessionPolicy {
        approval_mode: settings.approval_mode.clone(),
        sandbox_mode: settings.sandbox_mode.clone(),
        collaboration_mode: settings.collaboration_mode.clone(),
    })
}
fn event(bus: &EventBus, input: &StartTurnInput, kind: &str, mut payload: Value) {
    payload["turnId"] = json!(input.turn_id);
    bus.emit(ThreadEventEnvelope {
        event_type: kind.into(),
        thread_id: input.thread_id.clone(),
        timestamp: now_rfc3339(),
        payload,
    });
}
#[async_trait]
impl AgentRuntime for CodexRuntime {
    fn provider(&self) -> Provider {
        self.provider
    }
    fn descriptor(&self) -> AgentBackendDto {
        self.fallback.descriptor()
    }
    async fn start(&self) -> Result<()> {
        Ok(())
    }
    async fn list_agents(&self) -> Result<Vec<ModelOptionDto>> {
        self.fallback.list_agents().await
    }
    async fn capabilities(&self, agent: Option<&str>) -> Result<AgentCapabilitySnapshotDto> {
        self.fallback.capabilities(agent).await
    }
    fn negotiated_caps(&self, agent: Option<&str>) -> AgentProviderCapabilitiesDto {
        self.fallback.negotiated_caps(agent)
    }
    async fn list_models(
        &self,
        _agent: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<Vec<ModelOptionDto>> {
        let (process, _, _) = self.spawn(cwd.unwrap_or(".")).await?;
        let result=async {
            let mut models=vec![]; let mut cursor=Value::Null;
            let mut bridge=crate::acp::codex_models::ModelCatalogBridge::default();
            if let Some((_,catalog))=crate::acp::upstream_models::UpstreamModels::new(self.upstreams.clone()).catalog("codex").await? {bridge.set_upstream(catalog);}
            loop {let request=json!({"id":1,"method":"model/list","params":{"cursor":cursor}});bridge.observe_request(&request);let mut response=json!({"id":1,"result":process.request("model/list",request["params"].clone()).await?});bridge.observe_response(&mut response);
                for model in response["result"]["data"].as_array().into_iter().flatten() { models.push(serde_json::from_value(json!({"id":model["id"],"model":model["model"],"displayName":model["displayName"],"description":model["description"],"isDefault":model["isDefault"],"hidden":model["hidden"].as_bool().unwrap_or(false),"supportedReasoningEfforts":model["supportedReasoningEfforts"],"defaultReasoningEffort":model["defaultReasoningEffort"]}))?); }
                cursor=response["result"]["nextCursor"].clone();if cursor.is_null(){break;}
            } Ok(models)
        }.await;
        let _ = process.shutdown().await;
        result
    }
    async fn start_session(&self, input: StartSessionInput) -> Result<StartSessionResult> {
        self.load(
            "thread/start",
            None,
            &input.cwd,
            SessionSettings {
                model: Some(input.model),
                effort: input.reasoning_effort,
                approval_mode: Some(input.approval_mode),
                sandbox_mode: input.sandbox_mode,
                ..Default::default()
            },
        )
        .await
    }
    async fn resume_session(
        &self,
        id: &str,
        cwd: Option<&str>,
        settings: SessionSettings,
    ) -> Result<StartSessionResult> {
        if let Ok(session) = self.session(id) {
            if session.process.connection_open().await {
                return Ok(StartSessionResult {
                    provider_session_id: id.into(),
                    model: settings.model,
                    reasoning_effort: settings.effort,
                });
            }
            self.release_session(id).await?;
        }
        self.load(
            "thread/resume",
            Some(crate::import_id::raw_session_id(id)),
            cwd.unwrap_or("."),
            settings,
        )
        .await
    }
    async fn start_turn(
        &self,
        input: StartTurnInput,
        bus: EventBus,
        cancel: CancellationToken,
    ) -> Result<Vec<ThreadHistoryItemDto>> {
        let session = self.session(&input.provider_session_id)?;
        let mut updates = session
            .updates
            .try_lock()
            .map_err(|_| anyhow!("conflict: session busy"))?;
        let mut requests = session.requests.lock().await;
        self.apply_session_settings(
            &input.provider_session_id,
            SessionSettings {
                model: input.model.clone(),
                effort: input.reasoning_effort.clone(),
                approval_mode: input.approval_mode.clone(),
                sandbox_mode: input.sandbox_mode.clone(),
                collaboration_mode: input.collaboration_mode.clone(),
                performance_mode: input.performance_mode,
            },
        )
        .await?;
        let settings = session.settings.read().unwrap().clone();
        let policy = policy(&settings);
        let blocks = crate::acp::prompt::build_prompt_blocks(
            &input.prompt,
            std::path::Path::new(&session.cwd),
            true,
            &input.images,
        )?;
        let content:Vec<Value>=blocks.into_iter().map(|v|match v["type"].as_str(){Some("image")=>json!({"type":"image","url":format!("data:{};base64,{}",v["mimeType"].as_str().unwrap_or("image/png"),v["data"].as_str().unwrap_or(""))}),Some("text")=>json!({"type":"text","text":v["text"]}),_=>json!({"type":"text","text":format!("{}",v["uri"].as_str().unwrap_or(""))})}).collect();
        let mut params = json!({"threadId":crate::import_id::raw_session_id(&input.provider_session_id),"input":content,"model":settings.model,"effort":settings.effort,"approvalPolicy":policy["approvalPolicy"],"sandboxPolicy":policy["sandboxPolicy"]});
        if let Some(mode) = settings.collaboration_mode {
            params["collaborationMode"] = json!({"mode":mode,"settings":{"model":settings.model,"reasoning_effort":settings.effort,"developer_instructions":null}});
        }
        let response = session.process.request("turn/start", params).await?;
        let turn = response["turn"]["id"]
            .as_str()
            .context("Codex turn id missing")?
            .to_owned();
        *session.active.write().unwrap() = Some(turn.clone());
        session.loaded.write().unwrap().prompted = true;
        let mut mapper = TurnMapper::new(&input.turn_id);
        let mut interrupted = false;
        let result=async {loop {tokio::select! {
            _=cancel.cancelled(), if !interrupted => {interrupted=true;self.interrupt(&input.provider_session_id,&turn).await?;},
            request=requests.recv()=>{let Some((rpc_id,method,params))=request else {return Err(ExecutionUncertain("app-server request stream closed".into()).into());};
                let id=format!("native-codex:{}:{rpc_id}",session.process.id);
                let dto=ThreadActionRequestDto{id:id.clone(),kind:if method.contains("requestUserInput") {"requestUserInput"}else{"permission"}.into(),title:params["command"].as_str().unwrap_or(&method).into(),description:params["reason"].as_str().map(str::to_owned),turn_id:Some(input.turn_id.clone()),item_id:params["itemId"].as_str().map(str::to_owned),created_at:now_rfc3339(),questions:params["questions"].as_array().cloned().unwrap_or_default()};
                self.pending.lock().await.insert(id,Pending{process:session.process.clone(),rpc_id,method,thread:input.thread_id.clone(),dto:dto.clone()});event(&bus,&input,"thread.request.created",json!({"request":dto}));
            },
            update=updates.recv()=>{let Some(update)=update else {return Err(ExecutionUncertain("app-server notification stream closed".into()).into());};
                let update=json!({"method":update["_remoteMethod"],"params":update});
                let params=&update["params"];if params["threadId"].as_str().is_some_and(|id|id!=crate::import_id::raw_session_id(&input.provider_session_id)){continue;}
                if update["method"]=="turn/completed" && params["turn"]["id"]==turn {
                    if params["turn"]["status"]=="failed" {bail!("Codex turn failed: {}",params["turn"]["error"]);}
                    interrupted |= params["turn"]["status"]=="interrupted";break;
                }
                for mapped in mapping::apply(&mut mapper,&update) {for item in mapped.items {event(&bus,&input,"thread.item.started",json!({"item":item}));}for (id,delta,sequence) in mapped.deltas {event(&bus,&input,"thread.output.delta",json!({"itemId":id,"delta":delta,"sequence":sequence}));}if let Some(usage)=mapped.usage {event(&bus,&input,"runtime.usage.updated",json!({"usage":usage}));}}
            }
        }} Ok(())}.await;
        *session.active.write().unwrap() = None;
        session.loaded.write().unwrap().last_activity_at = chrono::Utc::now();
        self.pending
            .lock()
            .await
            .retain(|_, p| p.process.id != session.process.id);
        result?;
        Ok(mapper.finish(interrupted))
    }
    async fn interrupt(&self, id: &str, turn: &str) -> Result<()> {
        self.session(id)?
            .process
            .request(
                "turn/interrupt",
                serde_json::to_value(wire::TurnInterruptParams {
                    thread_id: crate::import_id::raw_session_id(id).into(),
                    turn_id: turn.into(),
                })?,
            )
            .await?;
        Ok(())
    }
    async fn send_input(&self, id: &str, _turn: &str, prompt: &str) -> Result<()> {
        let s = self.session(id)?;
        let turn = s.active.read().unwrap().clone().context("No active turn")?;
        s.process.request("turn/steer",json!({"threadId":crate::import_id::raw_session_id(id),"expectedTurnId":turn,"input":[{"type":"text","text":prompt}]})).await?;
        Ok(())
    }
    async fn apply_session_settings(&self, id: &str, settings: SessionSettings) -> Result<()> {
        let s = self.session(id)?;
        let mut current = s.settings.write().unwrap();
        macro_rules! merge {($($key:ident),*)=>{$(if settings.$key.is_some(){current.$key=settings.$key;})*}}
        merge!(
            model,
            effort,
            sandbox_mode,
            approval_mode,
            collaboration_mode,
            performance_mode
        );
        Ok(())
    }
    async fn respond_permission(&self, id: &str, allow: bool, answer: Option<&str>) -> Result<()> {
        let p = self
            .pending
            .lock()
            .await
            .remove(id)
            .context("Native Codex request not found")?;
        let response = if p.method.contains("requestUserInput") {
            json!({"answers":serde_json::from_str::<Value>(answer.unwrap_or("{}"))?})
        } else if p.method.contains("elicitation") {
            json!({"action":if allow{"accept"}else{"decline"},"content":answer.and_then(|v|serde_json::from_str::<Value>(v).ok())})
        } else {
            json!({"decision":if allow{"accept"}else{"decline"}})
        };
        p.process.respond(p.rpc_id, response).await
    }
    async fn pending_requests(&self, thread: &str) -> Vec<ThreadActionRequestDto> {
        self.pending
            .lock()
            .await
            .values()
            .filter(|p| p.thread == thread)
            .map(|p| p.dto.clone())
            .collect()
    }
    async fn compact_session(&self, id: &str, _thread: &str, _bus: EventBus) -> Result<()> {
        self.session(id)?
            .process
            .request(
                "thread/compact/start",
                serde_json::to_value(wire::ThreadCompactStartParams {
                    thread_id: crate::import_id::raw_session_id(id).into(),
                })?,
            )
            .await?;
        Ok(())
    }
    async fn fork_session(&self, id: &str) -> Result<StartSessionResult> {
        let s = self.session(id)?;
        let settings = s.settings.read().unwrap().clone();
        self.load(
            "thread/fork",
            Some(crate::import_id::raw_session_id(id)),
            &s.cwd,
            settings,
        )
        .await
    }
    async fn fork_session_at(&self, id: &str, point: &ForkPoint) -> Result<ForkResult> {
        let fork = self.fork_session(id).await?;
        if point.rollback_count > 0 {
            self.rewind_session(&fork.provider_session_id, point.rollback_count)
                .await?;
        }
        Ok(ForkResult {
            session: fork,
            release_source: false,
        })
    }
    async fn rewind_session(&self, id: &str, count: u32) -> Result<()> {
        let s = self.session(id)?;
        if s.active.read().unwrap().is_some() {
            bail!("conflict: turn running");
        }
        let raw = crate::import_id::raw_session_id(id);
        let mut remaining = count;
        let mut cursor = Value::Null;
        let mut visited = std::collections::HashSet::new();
        let mut before = None;
        while remaining > 0 {
            anyhow::ensure!(
                visited.insert(cursor.to_string()),
                "Codex history cursor repeated"
            );
            let page=s.process.request("thread/turns/list",json!({"threadId":raw,"cursor":cursor,"limit":remaining.min(100),"sortDirection":"desc","itemsView":"summary"})).await?;
            for turn in page["data"]
                .as_array()
                .context("Codex turn history missing")?
            {
                before = turn["id"].as_str().map(str::to_owned);
                remaining -= 1;
                if remaining == 0 {
                    break;
                }
            }
            cursor = page["nextCursor"].clone();
            if cursor.is_null() {
                break;
            }
        }
        if let Some(before) = before {
            s.process
                .request(
                    "thread/revert",
                    serde_json::to_value(wire::ThreadRevertParams {
                        thread_id: raw.into(),
                        before_turn_id: before,
                    })?,
                )
                .await?;
        }

        Ok(())
    }
    fn session_loaded(&self, id: &str) -> bool {
        self.sessions.read().unwrap().contains_key(id)
    }
    async fn loaded_sessions(&self) -> Vec<LoadedSession> {
        self.sessions
            .read()
            .unwrap()
            .values()
            .map(|s| {
                let mut loaded = s.loaded.read().unwrap().clone();
                if s.active.read().unwrap().is_some() {
                    loaded.busy.push(HarnessBusyReason::Turn);
                }
                loaded
            })
            .collect()
    }
    fn lifecycle(&self) -> Option<broadcast::Receiver<SessionLifecycle>> {
        Some(self.events.subscribe())
    }
    async fn release_session(&self, id: &str) -> Result<()> {
        let s = self.session(id)?;
        if s.active.read().unwrap().is_some() {
            return Err(TurnRunning.into());
        }
        s.process.terminate(Duration::from_secs(2)).await?;
        self.sessions.write().unwrap().remove(id);
        Ok(())
    }
    async fn release_loaded(
        &self,
        id: &str,
        _scope: ReleaseScope,
        process: Option<&str>,
    ) -> Result<Vec<LoadedSession>> {
        let s = self.session(id)?;
        if process.is_some_and(|p| p != s.process.id) {
            bail!("conflict: process changed");
        }
        let loaded = s.loaded.read().unwrap().clone();
        self.release_session(id).await?;
        Ok(vec![loaded])
    }
    async fn session_instance_id(&self, id: &str) -> Option<String> {
        self.session(id).ok().map(|s| s.process.id.clone())
    }
    async fn session_instance_started_at(&self, id: &str) -> Option<String> {
        self.session(id).ok().map(|s| s.process.started_at.clone())
    }
    async fn execution_state(&self, id: &str) -> ExecutionState {
        let Ok(s) = self.session(id) else {
            return ExecutionState::Unknown;
        };
        if !s.process.connection_open().await {
            return ExecutionState::Unknown;
        }
        let active = s.active.read().unwrap().clone();
        match active {
            Some(turn_id) => ExecutionState::Running { turn_id },
            None => ExecutionState::Idle,
        }
    }
    async fn get_goal(&self, id: &str) -> Result<Option<GoalState>> {
        let response = self
            .session(id)?
            .process
            .request(
                "thread/goal/get",
                serde_json::to_value(wire::ThreadGoalGetParams {
                    thread_id: crate::import_id::raw_session_id(id).into(),
                })?,
            )
            .await?;
        Ok(goal(&response["goal"]))
    }
    async fn set_goal(
        &self,
        id: &str,
        objective: Option<String>,
        status: Option<String>,
    ) -> Result<Option<GoalState>> {
        let response = self
            .session(id)?
            .process
            .request(
                "thread/goal/set",
                serde_json::to_value(wire::ThreadGoalSetParams {
                    thread_id: crate::import_id::raw_session_id(id).into(),
                    objective,
                    status: status.map(Value::String),
                    origin: Some(json!("user")),
                    token_budget: None,
                })?,
            )
            .await?;
        Ok(goal(&response["goal"]))
    }
    async fn restart(&self, _agent: &str) -> Result<usize> {
        let sessions: Vec<_> = self.sessions.read().unwrap().keys().cloned().collect();
        for id in &sessions {
            if self.session(id)?.active.read().unwrap().is_some() {
                return Err(TurnRunning.into());
            }
        }
        for id in &sessions {
            self.release_session(id).await?;
        }
        Ok(sessions.len())
    }
    async fn touch_session(&self, id: &str) {
        if let Ok(session) = self.session(id) {
            session.loaded.write().unwrap().last_activity_at = chrono::Utc::now();
        }
    }
    async fn list_import_sessions(&self, agent: Option<&str>) -> Result<Vec<ImportSessionMeta>> {
        self.fallback.list_import_sessions(agent).await
    }
}

fn goal(value: &Value) -> Option<GoalState> {
    if value.is_null() {
        return None;
    }
    Some(GoalState {
        objective: value["objective"].as_str()?.into(),
        status: value["status"].as_str().unwrap_or("active").into(),
        tokens_used: value["tokensUsed"]
            .as_u64()
            .unwrap_or(0)
            .min(u32::MAX as u64) as u32,
        time_used_seconds: value["timeUsedSeconds"]
            .as_u64()
            .unwrap_or(0)
            .min(u32::MAX as u64) as u32,
    })
}

#[cfg(test)]
mod tests;
