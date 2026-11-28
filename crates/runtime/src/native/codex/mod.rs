//! Direct Codex app-server runtime. The shared JSON-RPC process owns stdio and cleanup.
mod approvals;
mod commands;
mod mapping;
mod notifications;
mod plan_review;
mod wire;
use crate::{
    acp::{rpc::AcpProcess, ProductSessionPolicy, TurnMapper},
    actor::*,
};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use notifications::Notifications;
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
    updates: Mutex<Notifications>,
    requests: Mutex<tokio::sync::mpsc::Receiver<(i64, String, Value)>>,
    settings: RwLock<SessionSettings>,
    cwd: String,
    loaded: RwLock<LoadedSession>,
    active: RwLock<Option<String>>,
    operation: Mutex<()>,
    subagents: RwLock<Vec<ThreadSubagentDto>>,
    context_delivered: std::sync::atomic::AtomicBool,
    goal_active: std::sync::atomic::AtomicBool,
    context_reader: Option<Arc<std::sync::Mutex<crate::context_inspection::CodexTraceReader>>>,
}
impl Session {
    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let response = self.process.request(method, params);
        tokio::pin!(response);
        // A live turn can read notifications for us. If it ends before the
        // response arrives, take over the receiver when it releases the lock.
        tokio::select! {
            result = &mut response => result,
            mut updates = self.updates.lock() => updates.wait(&self.process, method, response).await,
        }
    }
}
struct PlanReview {
    thread: String,
    dto: ThreadActionRequestDto,
    reply: tokio::sync::oneshot::Sender<bool>,
}
struct Pending {
    process: Arc<AcpProcess>,
    rpc_id: i64,
    method: String,
    thread: String,
    dto: ThreadActionRequestDto,
    params: Value,
    bus: EventBus,
    decisions: Vec<approvals::Decision>,
}
pub struct CodexRuntime {
    provider: Provider,
    command: Option<String>,
    fallback: SharedRuntime,
    upstreams: PathBuf,
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    pending: Mutex<HashMap<String, Pending>>,
    /// "Implement this plan?" after a Plan Mode turn, answered like an approval.
    plan_reviews: Mutex<HashMap<String, PlanReview>>,
    events: broadcast::Sender<SessionLifecycle>,
    inspection: RwLock<Option<Arc<crate::context_inspection::InspectionState>>>,
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
            plan_reviews: Mutex::new(HashMap::new()),
            events,
            inspection: RwLock::new(None),
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
        Notifications,
        tokio::sync::mpsc::Receiver<(i64, String, Value)>,
    )> {
        let isolated = crate::execution::current().is_some();
        let mut env = if isolated {
            crate::execution::isolated_harness_environment("codex")?
        } else if crate::upstreams::native::oauth_selected(&self.upstreams, "codex") {
            vec![(crate::upstreams::native::CLEAN_OAUTH_ENV, "codex".into())]
        } else {
            vec![]
        };
        // An approved native file login runs the image's launcher, which pins
        // ChatGPT/file/openai startup exactly as codex-acp's CODEX_PATH does.
        let native_file = isolated && isolated_native_file(&env);
        let command = if native_file {
            ISOLATED_SUBSCRIPTION_LAUNCHER.into()
        } else if isolated {
            "codex".into()
        } else {
            self.command
                .clone()
                .or_else(|| std::env::var("CODEX_COMMAND").ok())
                .unwrap_or_else(|| "codex".into())
        };
        if let Some(state) = self
            .inspection
            .read()
            .unwrap()
            .as_ref()
            .filter(|s| s.enabled() && !isolated)
        {
            let root = state
                .root
                .join("traces")
                .join(uuid::Uuid::new_v4().to_string());
            std::fs::create_dir_all(&root)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
            }
            env.push((
                "CODEX_ROLLOUT_TRACE_ROOT",
                root.to_string_lossy().into_owned(),
            ));
        }
        // Live turn settings (`turn/settings/update`) are behind a Codex feature
        // that is read when a thread starts; the environment image's older Codex
        // does not have it, so isolated turns take changes from the next turn.
        let launch = if isolated {
            format!("{command} app-server")
        } else {
            format!("{command} app-server -c features.step_model_switching=true -c suppress_unstable_features_warning=true")
        };
        let (process, updates, requests) = AcpProcess::spawn(&launch, cwd, &env).await?;
        let process = Arc::new(process);
        let mut updates = Notifications::new(updates);
        let initialized = updates.request(&process,"initialize",json!({"clientInfo":{"name":"pockymoe","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await
            .and_then(|init| {
                if native_file { verify_isolated_native_file(&init) } else { Ok(()) }
            });
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
        let (process, mut updates, requests) = self.spawn(cwd).await?;
        let policy = policy(&settings);
        let mut params = json!({"cwd":cwd,"model":settings.model,"approvalPolicy":policy["approvalPolicy"],"sandbox":policy["sandbox"],"persistExtendedHistory":true});
        if let Some(raw) = raw {
            params["threadId"] = json!(raw);
            // Resume and fork only need metadata. Codex retains the history
            // in its own store, and a full response can exceed the frame limit.
            params["excludeTurns"] = json!(true);
        }
        let response = match updates.request(&process, method, params).await {
            Ok(v) => v,
            Err(e) => {
                let _ = process.shutdown().await;
                return Err(e);
            }
        };
        let raw = response["thread"]["id"]
            .as_str()
            .context("Codex thread id missing")?;
        let id = match &process.execution {
            Some(context) => format!(
                "codex::env/{}/{}/{raw}",
                context.environment_id, context.generation
            ),
            None => crate::import_id::scoped_session_id("codex", raw),
        };
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
        let context_reader = process.context_trace_root.as_ref().map(|root| {
            Arc::new(std::sync::Mutex::new(
                crate::context_inspection::CodexTraceReader::new(root.clone(), raw.into()),
            ))
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
                operation: Mutex::new(()),
                subagents: RwLock::new(vec![]),
                context_delivered: std::sync::atomic::AtomicBool::new(false),
                goal_active: std::sync::atomic::AtomicBool::new(false),
                context_reader,
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
    fn configure_context_inspection(&self, state: Arc<crate::context_inspection::InspectionState>) {
        *self.inspection.write().unwrap() = Some(state);
    }
    async fn inspect_context(&self, id: &str, _detail: &str) -> Result<Option<Value>> {
        Ok(self
            .inspection
            .read()
            .unwrap()
            .as_ref()
            .filter(|s| s.enabled())
            .and_then(|s| s.cached(id)))
    }
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
    async fn capabilities(&self, _agent: Option<&str>) -> Result<AgentCapabilitySnapshotDto> {
        let isolated = crate::execution::current().is_some();
        let command = self
            .command
            .clone()
            .or_else(|| std::env::var("CODEX_COMMAND").ok())
            .unwrap_or_else(|| "codex".into());
        let available = isolated || crate::acp::command_available(&command);
        let caps = self.negotiated_caps(Some("codex"));
        Ok(AgentCapabilitySnapshotDto {
            provider: self.provider,
            agent_id: "codex".into(),
            availability: if available { "ready" } else { "base_missing" }.into(),
            negotiated: Some(json!({"protocol":"codex-app-server","experimental":true})),
            effective_capabilities: available.then_some(caps.clone()),
            toolbox_items: native_toolbox(&caps),
        })
    }
    fn negotiated_caps(&self, _agent: Option<&str>) -> AgentProviderCapabilitiesDto {
        let mut caps = AgentProviderCapabilitiesDto::conversational();
        caps.sessions.import_local = true;
        caps.turns.steer = true;
        caps.turns.compact = true;
        caps.branching.fork = true;
        caps.branching.fork_at = true;
        caps.branching.hard_rollback = true;
        caps.controls.performance_mode = true;
        caps.controls.goals = true;
        caps.controls.live_model_switch = true;
        crate::acp::runtime::isolated_capabilities::restrict(
            &mut caps,
            crate::execution::current().is_some(),
        );
        caps
    }
    fn session_negotiated_caps(
        &self,
        agent: Option<&str>,
        session_id: &str,
    ) -> AgentProviderCapabilitiesDto {
        let mut caps = self.negotiated_caps(agent);
        crate::acp::runtime::isolated_capabilities::restrict(
            &mut caps,
            session_id.contains("::env/"),
        );
        caps
    }
    fn toolbox(&self, agent: Option<&str>) -> Vec<ToolboxItemDto> {
        native_toolbox(&self.negotiated_caps(agent))
    }
    async fn list_models(
        &self,
        _agent: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<Vec<ModelOptionDto>> {
        let (process, mut updates, _requests) = self.spawn(cwd.unwrap_or(".")).await?;
        let result=async {
            let mut models=vec![]; let mut cursor=Value::Null;
            let mut bridge=crate::acp::codex_models::ModelCatalogBridge::default();
            if let Some((_,catalog))=crate::acp::upstream_models::UpstreamModels::new(self.upstreams.clone()).catalog("codex").await? {bridge.set_upstream(catalog);}
            loop {let request=json!({"id":1,"method":"model/list","params":{"cursor":cursor}});bridge.observe_request(&request);let mut response=json!({"id":1,"result":updates.request(&process,"model/list",request["params"].clone()).await?});bridge.observe_response(&mut response);
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
        let context = crate::execution::current();
        let (_, raw, _canonical) = crate::acp::resume_session_identity(
            id,
            "codex",
            context
                .as_ref()
                .map(|c| (c.environment_id.as_str(), c.generation)),
        )?;
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
        self.load("thread/resume", Some(&raw), cwd.unwrap_or("."), settings)
            .await
    }
    async fn start_turn(
        &self,
        input: StartTurnInput,
        bus: EventBus,
        cancel: CancellationToken,
    ) -> Result<Vec<ThreadHistoryItemDto>> {
        let session = self.session(&input.provider_session_id)?;
        let _operation = session
            .operation
            .try_lock()
            .map_err(|_| anyhow!("conflict: session busy"))?;
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
        let command = (!input.hidden && input.images.is_empty())
            .then(|| commands::parse(&input.prompt))
            .flatten();
        if let Some(local) = command
            .as_ref()
            .filter(|c| !matches!(c, commands::Command::Review(_)))
        {
            return self
                .local_command(&session, &input, &bus, local, &settings, &policy)
                .await;
        }
        let mut updates = session.updates.lock().await;
        let mut requests = session.requests.lock().await;
        let include_context = !input.context_delivered
            && !input.hidden
            && !session.process.cli_env.is_empty()
            && !session
                .context_delivered
                .load(std::sync::atomic::Ordering::SeqCst);
        let prompt = if include_context {
            format!("{}\n\n{}", crate::acp::prompt::THREAD_CONTEXT, input.prompt)
        } else {
            input.prompt.clone()
        };
        let blocks = crate::acp::prompt::build_prompt_blocks(
            &prompt,
            std::path::Path::new(&session.cwd),
            true,
            &input.images,
        )?;
        let content:Vec<Value>=blocks.into_iter().map(|v|match v["type"].as_str(){Some("image")=>json!({"type":"image","url":format!("data:{};base64,{}",v["mimeType"].as_str().unwrap_or("image/png"),v["data"].as_str().unwrap_or(""))}),Some("text")=>json!({"type":"text","text":v["text"]}),_=>json!({"type":"text","text":format!("{}",v["uri"].as_str().unwrap_or(""))})}).collect();
        let mut params = json!({"threadId":native_raw(&input.provider_session_id),"input":content,"model":settings.model,"effort":settings.effort,"approvalPolicy":policy["approvalPolicy"],"sandboxPolicy":policy["sandboxPolicy"]});
        if let Some(mode) = &settings.collaboration_mode {
            params["collaborationMode"] = json!({"mode":mode,"settings":{"model":settings.model,"reasoning_effort":settings.effort,"developer_instructions":null}});
        }
        // The rollout reader also accounts for requests made by Codex subagents,
        // which are absent from the parent's app-server token counter.
        let mut usage_reader = session
            .process
            .execution
            .is_none()
            .then(|| crate::acp::CodexUsageReader::new(native_raw(&input.provider_session_id)));
        let mut usage_tick = tokio::time::interval(Duration::from_millis(250));
        let mut announcer = crate::subscription::Announcer::default();
        let mut settled = std::collections::HashSet::<i64>::new();
        let inspection = self
            .inspection
            .read()
            .unwrap()
            .clone()
            .filter(|_| !input.hidden);
        crate::native::turn_events::started(&bus, &input);
        // `/review…` runs Codex's review mode inline on this thread.
        let (method, params) = match &command {
            Some(commands::Command::Review(target)) => (
                "review/start",
                json!({"threadId":native_raw(&input.provider_session_id),"target":target,"delivery":"inline"}),
            ),
            _ => ("turn/start", params),
        };
        let started = updates
            .request(&session.process, method, params)
            .await
            .and_then(|response| {
                response["turn"]["id"]
                    .as_str()
                    .map(str::to_owned)
                    .context("Codex turn id missing")
            });
        let mut turn = match started {
            Ok(turn) => turn,
            Err(error) => {
                let failed: Result<()> = Err(error);
                crate::native::turn_events::finished(&bus, &input, &failed, false, None);
                return failed.map(|_| vec![]);
            }
        };
        let context_observer = session
            .context_reader
            .as_ref()
            .zip(inspection.as_ref())
            .map(|(reader, state)| {
                crate::acp::context::observer(
                    reader.clone(),
                    state.clone(),
                    input.provider_session_id.clone(),
                    input.thread_id.clone(),
                    bus.clone(),
                )
            });
        // A review does not carry the prompt, so the context still needs delivering.
        if include_context && !matches!(command, Some(commands::Command::Review(_))) {
            session
                .context_delivered
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        *session.active.write().unwrap() = Some(turn.clone());
        session.loaded.write().unwrap().prompted = true;
        let mut mapper = TurnMapper::new(&input.turn_id);
        let mut interrupted = false;
        let mut interrupt_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        // Plan Mode follow-through (codex-acp): the plan this turn produced.
        let in_plan = settings.collaboration_mode.as_deref() == Some("plan")
            && !matches!(command, Some(commands::Command::Review(_)));
        let mut plan: Option<(String, String)> = None;
        let mut reviewed = false;
        let result=async {loop {tokio::select! {
            _=usage_tick.tick(), if usage_reader.is_some() => {
                if let Some(reader)=usage_reader.as_mut() {for usage in reader.poll() {
                    crate::acp::context::native_usage(session.context_reader.as_ref(), &usage, inspection.as_ref(), &input.provider_session_id, &input.thread_id, &bus);
                    crate::acp::emit_mapped(&bus,&input.thread_id,&input.turn_id,crate::acp::mapper::MappedUpdate{usage:Some(usage),..Default::default()},input.hidden);
                }
                if let Some(limits)=reader.take_rate_limits() {announcer.announce(&bus,&input.thread_id,"codex",limits,input.hidden);}}
            },
            _=cancel.cancelled(), if !interrupted => {
                interrupted=true;
                interrupt_deadline=tokio::time::Instant::now()+Duration::from_secs(15);
                updates.request(&session.process, "turn/interrupt", serde_json::to_value(wire::TurnInterruptParams {
                    thread_id: native_raw(&input.provider_session_id).into(), turn_id: turn.clone(),
                })?).await?;
            },
            _=tokio::time::sleep_until(interrupt_deadline), if interrupted => {return Err(ExecutionUncertain("Codex did not confirm interruption".into()).into());},
            request=requests.recv()=>{let Some((rpc_id,method,params))=request else {return Err(ExecutionUncertain("app-server request stream closed".into()).into());};
                if method=="currentTime/read" {
                    session.process.respond(rpc_id,json!({"currentTimeAt":chrono::Utc::now().timestamp()})).await?;
                    continue;
                }
                if !matches!(method.as_str(),"item/commandExecution/requestApproval"|"item/fileChange/requestApproval"|"item/permissions/requestApproval"|"item/tool/requestUserInput"|"mcpServer/elicitation/request"|"execCommandApproval"|"applyPatchApproval") {
                    session.process.respond_error(rpc_id,"Unsupported native Codex callback").await?;
                    continue;
                }
                if settled.remove(&rpc_id) { continue; }
                let id=format!("native-codex:{}:{rpc_id}",session.process.id);
                let asks=method.contains("requestUserInput") || method.contains("elicitation");
                let decisions=approvals::decisions(&method,&params);
                let title=approval_title(&method,&params);
                let questions=if asks {crate::acp::elicitation::questions(&params,method.contains("requestUserInput"))?} else {approvals::questions(&title,&decisions)};
                let dto=ThreadActionRequestDto{id:id.clone(),kind:if asks {"requestUserInput"}else{"permissionRequest"}.into(),title,description:approval_description(&method,&params)?,turn_id:Some(input.turn_id.clone()),item_id:params["itemId"].as_str().map(str::to_owned),created_at:now_rfc3339(),questions};
                self.pending.lock().await.insert(id,Pending{process:session.process.clone(),rpc_id,method,thread:input.thread_id.clone(),dto:dto.clone(),params,bus:bus.clone(),decisions});event(&bus,&input,"thread.request.created",json!({"request":dto}));
            },
            update=updates.recv()=>{let Some(update)=update else {return Err(ExecutionUncertain("app-server notification stream closed".into()).into());};
                let update=json!({"method":update["_remoteMethod"],"params":update});
                let params=&update["params"];
                // Codex settled a callback itself (turn ended, request superseded):
                // the approval card must not wait for an answer nobody reads.
                if update["method"]=="serverRequest/resolved" {
                    let rpc=params["requestId"].as_i64().or_else(|| params["requestId"].as_str().and_then(|v| v.parse().ok()));
                    let mut pending=self.pending.lock().await;
                    let found:Vec<_>=pending.iter().filter(|(_,p)| Some(p.rpc_id)==rpc && p.process.id==session.process.id).map(|(id,_)|id.clone()).collect();
                    // The request travels on the other channel and may not be
                    // shown yet: remember it so it is never shown.
                    if found.is_empty() { if let Some(rpc)=rpc { settled.insert(rpc); } }
                    for id in found { if let Some(p)=pending.remove(&id) { p.bus.emit(ThreadEventEnvelope{event_type:ThreadEventType::ThreadRequestResolved,thread_id:p.thread,timestamp:now_rfc3339(),payload:json!({"requestId":id,"turnId":input.turn_id})}); } }
                    continue;
                }
                // Account-wide, not thread-scoped: the push the ACP adapter drops.
                if update["method"]=="account/rateLimits/updated" {
                    if let Some(limits)=crate::subscription::codex_rate_update(&params["rateLimits"]) {announcer.announce(&bus,&input.thread_id,"codex",limits,input.hidden);}
                    continue;
                }
                if params["threadId"].as_str().is_some_and(|id|id!=native_raw(&input.provider_session_id)){continue;}
                if update["method"]=="item/completed" && params["item"]["type"]=="plan" {
                    plan = Some((params["item"]["id"].as_str().unwrap_or("plan").to_owned(), params["item"]["text"].as_str().unwrap_or("").to_owned()));
                }
                if update["method"]=="turn/completed" && params["turn"]["id"]==turn {
                    if params["turn"]["status"]=="failed" {bail!("Codex turn failed: {}",params["turn"]["error"]);}
                    interrupted |= params["turn"]["status"]=="interrupted";
                    if in_plan && !interrupted && !reviewed && !input.hidden {
                        if let Some((item, text)) = plan.take().filter(|(_, t)| !t.trim().is_empty()) {
                            reviewed = true;
                            match self.review_plan(&input, &bus, &mut mapper, &item, &text, &cancel).await {
                                Some(true) => {
                                    turn = self.implement_plan(&session, &mut updates, &input, &bus).await?;
                                    continue;
                                }
                                Some(false) => {}
                                None => interrupted = true,
                            }
                        }
                    }
                    break;
                }
                for mut mapped in mapping::apply(&mut mapper,&update) {
                    if let Some(goal)=&mapped.goal {
                        session.goal_active.store(goal.as_ref().is_some_and(|g|g.status=="active"),std::sync::atomic::Ordering::SeqCst);
                    }
                    if let Some(usage)=mapped.usage.as_ref() {
                        crate::acp::context::native_usage(session.context_reader.as_ref(), usage, inspection.as_ref(), &input.provider_session_id, &input.thread_id, &bus);
                    }
                    if usage_reader.is_some() {if let Some(usage)=mapped.usage.as_mut() {
                        // Keep native context occupancy, but bill the complete
                        // per-request rollout report rather than both counters.
                        usage.as_object_mut().map(|v| v.remove("tokenUsage"));
                    }}
                    if let Some(agents)=&mapped.active_subagents {*session.subagents.write().unwrap()=agents.clone();}
                    crate::acp::emit_mapped(&bus,&input.thread_id,&input.turn_id,mapped,input.hidden);
                }
            }
        }} Ok(())}.await;
        if let Some(reader) = usage_reader.as_mut() {
            let final_usage = reader.poll_final();
            if let Some(limits) = reader.take_rate_limits() {
                announcer.announce(&bus, &input.thread_id, "codex", limits, input.hidden);
            }
            for usage in final_usage {
                crate::acp::context::native_usage(
                    session.context_reader.as_ref(),
                    &usage,
                    inspection.as_ref(),
                    &input.provider_session_id,
                    &input.thread_id,
                    &bus,
                );
                crate::acp::emit_mapped(
                    &bus,
                    &input.thread_id,
                    &input.turn_id,
                    crate::acp::mapper::MappedUpdate {
                        usage: Some(usage),
                        ..Default::default()
                    },
                    input.hidden,
                );
            }
        }
        if let Some(stop) = context_observer {
            stop.cancel();
        }
        *session.active.write().unwrap() = None;
        session.loaded.write().unwrap().last_activity_at = chrono::Utc::now();
        // Requests still open when the turn ends can no longer be answered:
        // tell viewers, as ACP does when its permission wait ends.
        let mut pending = self.pending.lock().await;
        let open: Vec<_> = pending
            .iter()
            .filter(|(_, p)| p.process.id == session.process.id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in open {
            if let Some(p) = pending.remove(&id) {
                p.bus.emit(ThreadEventEnvelope {
                    event_type: ThreadEventType::ThreadRequestResolved,
                    thread_id: p.thread,
                    timestamp: now_rfc3339(),
                    payload: json!({"requestId":id,"turnId":input.turn_id}),
                });
            }
        }
        drop(pending);
        let items = mapper.finish(interrupted || result.is_err());
        crate::native::turn_events::finished(&bus, &input, &result, interrupted, Some(&items));
        result?;
        Ok(items)
    }
    async fn interrupt(&self, id: &str, turn: &str) -> Result<()> {
        let session = self.session(id)?;
        let native = session
            .active
            .read()
            .unwrap()
            .clone()
            .unwrap_or_else(|| turn.into());
        session
            .request(
                "turn/interrupt",
                serde_json::to_value(wire::TurnInterruptParams {
                    thread_id: native_raw(id).into(),
                    turn_id: native,
                })?,
            )
            .await?;
        Ok(())
    }
    async fn send_input(&self, id: &str, _turn: &str, prompt: &str) -> Result<()> {
        let s = self.session(id)?;
        let turn = s.active.read().unwrap().clone().context("No active turn")?;
        s.request("turn/steer",json!({"threadId":native_raw(id),"expectedTurnId":turn,"input":[{"type":"text","text":prompt}]})).await?;
        Ok(())
    }
    async fn apply_session_settings(&self, id: &str, settings: SessionSettings) -> Result<()> {
        let s = self.session(id)?;
        let (model_changed, effort_changed, tier_changed) = (
            settings.model.is_some(),
            settings.effort.is_some(),
            settings.performance_mode.is_some(),
        );
        let merged = {
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
            current.clone()
        };
        let policy = policy(&merged);
        let tier = if merged.performance_mode == Some(true) {
            json!("fast")
        } else {
            Value::Null
        };
        s.request("thread/settings/update",json!({"threadId":native_raw(id),"model":merged.model,"effort":merged.effort,"sandboxPolicy":policy["sandboxPolicy"],"approvalPolicy":policy["approvalPolicy"],"serviceTier":tier})).await?;
        // A running turn takes a new model, effort or tier from its next model
        // request (`turn/settings/update`); ACP only applies it next turn.
        let live = s.active.read().unwrap().clone();
        if let Some(turn) = live {
            let mut change = json!({"threadId":native_raw(id),"turnId":turn});
            if model_changed {
                change["model"] = json!(merged.model);
            }
            if effort_changed {
                change["effort"] = json!(merged.effort);
            }
            if tier_changed {
                change["serviceTier"] = tier;
            }
            if change.as_object().is_some_and(|c| c.len() > 2) {
                // Codex validates the switch (feature enabled at launch, model
                // metadata, policy). If it refuses, the change still applies
                // from the next turn, as on ACP, instead of failing.
                if let Err(error) = s.request("turn/settings/update", change).await {
                    tracing::info!(%error, "Codex kept the running turn's settings; they apply from the next turn");
                }
            }
        }
        Ok(())
    }
    async fn respond_permission(&self, id: &str, allow: bool, answer: Option<&str>) -> Result<()> {
        if let Some(review) = self.plan_reviews.lock().await.remove(id) {
            let _ = review.reply.send(plan_review::approved(allow, answer));
            return Ok(());
        }
        let mut pending = self.pending.lock().await;
        let p = pending.get(id).context("Native Codex request not found")?;
        let response = if p.method.contains("requestUserInput") || p.method.contains("elicitation")
        {
            crate::acp::elicitation::response(
                &p.params,
                p.method.contains("requestUserInput"),
                allow,
                answer,
            )?
        } else {
            approvals::response(&p.method, &p.params, &p.decisions, allow, answer)
        };
        p.process.respond(p.rpc_id, response).await?;
        let p = pending
            .remove(id)
            .expect("request was held while responding");
        drop(pending);
        p.bus.emit(ThreadEventEnvelope {
            event_type: ThreadEventType::ThreadRequestResolved,
            thread_id: p.thread,
            timestamp: now_rfc3339(),
            payload: json!({"requestId":id}),
        });
        Ok(())
    }

    async fn pending_requests(&self, thread: &str) -> Vec<ThreadActionRequestDto> {
        let mut list: Vec<_> = self
            .pending
            .lock()
            .await
            .values()
            .filter(|p| p.thread == thread)
            .map(|p| p.dto.clone())
            .collect();
        list.extend(
            self.plan_reviews
                .lock()
                .await
                .values()
                .filter(|r| r.thread == thread)
                .map(|r| r.dto.clone()),
        );
        list
    }

    async fn compact_session(&self, id: &str, thread: &str, bus: EventBus) -> Result<()> {
        // Like ACP's hidden `/compact` turn: hold the session until Codex has
        // finished, so no prompt starts against a half-compacted thread and
        // the caller's "compacted" really means done.
        let session = self.session(id)?;
        let _operation = session
            .operation
            .try_lock()
            .map_err(|_| anyhow!("conflict: session busy"))?;
        let mut updates = session.updates.lock().await;
        updates
            .request(
                &session.process,
                "thread/compact/start",
                serde_json::to_value(wire::ThreadCompactStartParams {
                    thread_id: native_raw(id).into(),
                })?,
            )
            .await?;
        let raw = native_raw(id);
        let deadline = tokio::time::Instant::now() + COMPACTION_DEADLINE;
        let mut turn: Option<String> = None;
        loop {
            let update = tokio::time::timeout_at(deadline, updates.recv())
                .await
                .map_err(|_| ExecutionUncertain("Codex did not finish compacting".into()))?
                .ok_or_else(|| {
                    ExecutionUncertain("app-server notification stream closed".into())
                })?;
            let method = update["_remoteMethod"].as_str().unwrap_or("");
            if method == "account/rateLimits/updated" {
                if let Some(limits) = crate::subscription::codex_rate_update(&update["rateLimits"])
                {
                    crate::subscription::announce(&bus, thread, "codex", limits, false);
                }
                continue;
            }
            if update["threadId"].as_str().is_some_and(|t| t != raw) {
                continue;
            }
            match method {
                "turn/started" => turn = update["turn"]["id"].as_str().map(str::to_owned),
                "turn/completed"
                    if turn.is_none() || update["turn"]["id"].as_str() == turn.as_deref() =>
                {
                    if update["turn"]["status"] == "failed" {
                        bail!("Codex compaction failed: {}", update["turn"]["error"]);
                    }
                    return Ok(());
                }
                // Without a compaction turn, the completed item or the legacy
                // notification is the end; a later turn/completed is ignored.
                "thread/compacted" if turn.is_none() => return Ok(()),
                "item/completed"
                    if turn.is_none() && update["item"]["type"] == "contextCompaction" =>
                {
                    return Ok(())
                }
                _ => {}
            }
        }
    }
    async fn fork_session(&self, id: &str) -> Result<StartSessionResult> {
        // Like ACP: a fork would be a new, unbound thread and cannot inherit
        // the environment's thread-bound grants.
        anyhow::ensure!(
            !id.contains("::env/"),
            "unsupported_environment_fork: native forks are unavailable for isolated threads"
        );
        let s = self.session(id)?;
        let settings = s.settings.read().unwrap().clone();
        self.load("thread/fork", Some(native_raw(id)), &s.cwd, settings)
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
        let raw = native_raw(id);
        let mut remaining = count;
        let mut cursor = Value::Null;
        let mut visited = std::collections::HashSet::new();
        let mut before = None;
        while remaining > 0 {
            anyhow::ensure!(
                visited.insert(cursor.to_string()),
                "Codex history cursor repeated"
            );
            let page = s
                .request(
                    "thread/turns/list",
                    json!({"threadId":raw,"cursor":cursor,"limit":remaining.min(100),"sortDirection":"desc","itemsView":"summary"}),
                )
                .await?;
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
            s.request(
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
                if !s.subagents.read().unwrap().is_empty() {
                    loaded.busy.push(HarnessBusyReason::Subagent);
                }
                if s.goal_active.load(std::sync::atomic::Ordering::SeqCst) {
                    loaded.busy.push(HarnessBusyReason::Goal);
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
        let _operation = s.operation.try_lock().map_err(|_| TurnRunning)?;
        if s.active.read().unwrap().is_some() {
            return Err(TurnRunning.into());
        }
        s.process.terminate(Duration::from_secs(2)).await?;
        self.sessions.write().unwrap().remove(id);
        let loaded = s.loaded.read().unwrap();
        let _ = self.events.send(SessionLifecycle::Released {
            session_id: id.into(),
            thread_id: loaded.thread_id.clone(),
            reason: ReleaseReason::Explicit,
            load_reason: loaded.load_reason,
            prompted: loaded.prompted,
        });
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
            .request(
                "thread/goal/get",
                serde_json::to_value(wire::ThreadGoalGetParams {
                    thread_id: native_raw(id).into(),
                })?,
            )
            .await?;
        let goal = goal(&response["goal"]);
        self.session(id)?.goal_active.store(
            goal.as_ref().is_some_and(|g| g.status == "active"),
            std::sync::atomic::Ordering::SeqCst,
        );
        Ok(goal)
    }
    async fn set_goal(
        &self,
        id: &str,
        objective: Option<String>,
        status: Option<String>,
    ) -> Result<Option<GoalState>> {
        let response = self
            .session(id)?
            .request(
                "thread/goal/set",
                serde_json::to_value(wire::ThreadGoalSetParams {
                    thread_id: native_raw(id).into(),
                    objective,
                    status: status.map(Value::String),
                    origin: Some(json!("user")),
                    token_budget: None,
                })?,
            )
            .await?;
        let goal = goal(&response["goal"]);
        self.session(id)?.goal_active.store(
            goal.as_ref().is_some_and(|g| g.status == "active"),
            std::sync::atomic::Ordering::SeqCst,
        );
        Ok(goal)
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
    async fn active_subagents(&self, id: &str) -> Vec<ThreadSubagentDto> {
        self.session(id)
            .map(|s| s.subagents.read().unwrap().clone())
            .unwrap_or_default()
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

fn native_toolbox(caps: &AgentProviderCapabilitiesDto) -> Vec<ToolboxItemDto> {
    let mut items = toolbox_from_capabilities(caps);
    for item in commands::toolbox() {
        if !items.iter().any(|i| i.command == item.command) {
            items.push(item);
        }
    }
    items
}

impl CodexRuntime {
    /// Asks "Implement this plan?"; `None` when the turn was cancelled meanwhile.
    async fn review_plan(
        &self,
        input: &StartTurnInput,
        bus: &EventBus,
        mapper: &mut TurnMapper,
        item: &str,
        plan: &str,
        cancel: &CancellationToken,
    ) -> Option<bool> {
        let show = |mapper: &mut TurnMapper, decided| {
            let update = mapper.apply(&plan_review::row(item, plan, decided));
            crate::acp::emit_mapped(bus, &input.thread_id, &input.turn_id, update, input.hidden);
        };
        show(mapper, None);
        let id = format!("native-codex:plan:{}", uuid::Uuid::new_v4());
        let dto = plan_review::request(&id, &input.turn_id, item, plan);
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.plan_reviews.lock().await.insert(
            id.clone(),
            PlanReview {
                thread: input.thread_id.clone(),
                dto: dto.clone(),
                reply,
            },
        );
        event(bus, input, "thread.request.created", json!({"request":dto}));
        let decided = tokio::select! {
            answer = answer => Some(answer.unwrap_or(false)),
            _ = cancel.cancelled() => None,
            _ = tokio::time::sleep(Duration::from_secs(30 * 60)) => Some(false),
        };
        self.plan_reviews.lock().await.remove(&id);
        event(
            bus,
            input,
            "thread.request.resolved",
            json!({"requestId":id}),
        );
        show(mapper, Some(decided.unwrap_or(false)));
        decided
    }

    /// Leaves Plan Mode (persisted through `thread.collaboration.updated`) and
    /// starts the implementation turn; returns its native turn id.
    async fn implement_plan(
        &self,
        session: &Arc<Session>,
        updates: &mut Notifications,
        input: &StartTurnInput,
        bus: &EventBus,
    ) -> Result<String> {
        session.settings.write().unwrap().collaboration_mode = Some("default".into());
        event(
            bus,
            input,
            "thread.collaboration.updated",
            json!({"collaborationMode":"default"}),
        );
        let settings = session.settings.read().unwrap().clone();
        let policy = policy(&settings);
        let response = updates
            .request(
                &session.process,
                "turn/start",
                json!({"threadId":native_raw(&input.provider_session_id),"input":[{"type":"text","text":plan_review::PROMPT}],
                    "model":settings.model,"effort":settings.effort,"approvalPolicy":policy["approvalPolicy"],"sandboxPolicy":policy["sandboxPolicy"],
                    "collaborationMode":{"mode":"default","settings":{"model":settings.model,"reasoning_effort":settings.effort,"developer_instructions":null}}}),
            )
            .await?;
        let turn = response["turn"]["id"]
            .as_str()
            .context("Codex turn id missing")?
            .to_owned();
        *session.active.write().unwrap() = Some(turn.clone());
        Ok(turn)
    }

    /// `/mcp`, `/skills`, `/status`: answered from app-server state as one
    /// agent message in a normal (recorded) turn, without a model call.
    async fn local_command(
        &self,
        session: &Arc<Session>,
        input: &StartTurnInput,
        bus: &EventBus,
        command: &commands::Command,
        settings: &SessionSettings,
        policy: &Value,
    ) -> Result<Vec<ThreadHistoryItemDto>> {
        crate::native::turn_events::started(bus, input);
        let result: Result<String> = async {
            Ok(match command {
                commands::Command::Mcp => {
                    let mut pages = vec![];
                    let mut cursor = Value::Null;
                    for _ in 0..20 {
                        let page = session
                            .request(
                                "mcpServerStatus/list",
                                json!({"cursor":cursor,"detail":"toolsAndAuthOnly"}),
                            )
                            .await?;
                        cursor = page["nextCursor"].clone();
                        pages.push(page);
                        if cursor.is_null() {
                            break;
                        }
                    }
                    commands::render_mcp(&pages)
                }
                commands::Command::Skills => commands::render_skills(
                    &session
                        .request("skills/list", json!({"cwds":[session.cwd]}))
                        .await?,
                ),
                commands::Command::Status => {
                    let limits = session
                        .request("account/rateLimits/read", json!({}))
                        .await
                        .ok()
                        .and_then(|data| crate::subscription::parse_codex(&data));
                    commands::render_status(settings, policy, &session.cwd, limits.as_ref())
                }
                commands::Command::Review(_) => unreachable!("review runs as a Codex turn"),
            })
        }
        .await;
        let mut mapper = TurnMapper::new(&input.turn_id);
        if let Ok(text) = &result {
            let update = mapper.apply(&json!({"sessionUpdate":"agent_message_chunk","messageId":format!("{}:command",input.turn_id),"content":{"type":"text","text":text}}));
            crate::acp::emit_mapped(bus, &input.thread_id, &input.turn_id, update, input.hidden);
        }
        let items = mapper.finish(result.is_err());
        crate::native::turn_events::finished(bus, input, &result, false, Some(&items));
        result?;
        Ok(items)
    }
}

fn approval_title(method: &str, params: &Value) -> String {
    match method {
        // The row's own title: a single parsed action's command, else the
        // command without its `bash -lc` wrapper (codex-acp's title).
        "item/commandExecution/requestApproval" | "execCommandApproval" => params["commandActions"]
            .as_array()
            .filter(|a| a.len() == 1)
            .and_then(|a| a[0]["command"].as_str())
            .map(str::to_owned)
            .or_else(|| params["command"].as_str().map(mapping::strip_shell_prefix))
            .or_else(|| {
                params["command"].as_array().map(|parts| {
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
            })
            .unwrap_or_else(|| "Run command".into()),
        "item/fileChange/requestApproval" | "applyPatchApproval" => "Edit files".into(),
        "item/permissions/requestApproval" => "Additional permissions".into(),
        "item/tool/requestUserInput" => "Question".into(),
        "mcpServer/elicitation/request" => params["serverName"]
            .as_str()
            .map(|server| format!("{server} asks"))
            .unwrap_or_else(|| "MCP server request".into()),
        other => other.into(),
    }
}

/// What the approval card explains: Codex's reason, where a command runs, the
/// network host or extra permissions it needs, and the root a patch writes.
fn approval_description(method: &str, params: &Value) -> Result<Option<String>> {
    let mut lines: Vec<String> = Vec::new();
    if let Some(reason) = params["reason"].as_str().filter(|r| !r.is_empty()) {
        lines.push(reason.into());
    }
    if let Some(cwd) = params["cwd"].as_str() {
        lines.push(format!("in {cwd}"));
    }
    if let Some(host) = params
        .pointer("/networkApprovalContext/host")
        .and_then(Value::as_str)
    {
        lines.push(format!("network: {host}"));
    }
    if let Some(root) = params["grantRoot"].as_str() {
        lines.push(format!("write access under {root}"));
    }
    for key in ["permissions", "additionalPermissions"] {
        if !params[key].is_null() && method != "mcpServer/elicitation/request" {
            lines.push(serde_json::to_string_pretty(&params[key])?);
        }
    }
    Ok((!lines.is_empty()).then(|| lines.join("\n")))
}

const COMPACTION_DEADLINE: Duration = Duration::from_secs(15 * 60);

/// The environment image's launcher and the Codex release reviewed with it
/// (`containers/environment`). ACP gates the same login on codex-acp 2.5.0.
const ISOLATED_SUBSCRIPTION_LAUNCHER: &str = "/opt/pockymoe/bin/codex-subscription";
const ISOLATED_NATIVE_FILE_CODEX: &str = "0.167.0";

fn isolated_native_file(env: &[(&str, String)]) -> bool {
    env.iter()
        .any(|(k, v)| *k == "POCKYMOE_ENV_CODEX_NATIVE" && v == "1")
}

/// `userAgent` is `<client>/<codex version> (<os>) ...`.
fn codex_version(init: &Value) -> Option<&str> {
    init["userAgent"]
        .as_str()?
        .split_whitespace()
        .next()?
        .rsplit_once('/')
        .map(|(_, version)| version)
}

fn verify_isolated_native_file(init: &Value) -> Result<()> {
    anyhow::ensure!(
        codex_version(init) == Some(ISOLATED_NATIVE_FILE_CODEX),
        "unsupported_environment_codex_cli: native file authentication requires the environment image's Codex {ISOLATED_NATIVE_FILE_CODEX}"
    );
    Ok(())
}

fn native_raw(id: &str) -> &str {
    let raw = crate::import_id::raw_session_id(id);
    raw.strip_prefix("env/")
        .and_then(|s| s.splitn(3, '/').nth(2))
        .unwrap_or(raw)
}
