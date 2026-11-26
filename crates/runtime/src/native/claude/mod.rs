//! Native Claude Code stream-json runtime. ACP remains an independent backend.
mod completion;
mod mapping;
#[cfg(test)]
mod tests;
mod transport;

use crate::actor::*;
use anyhow::{bail, ensure, Context, Result};
use async_trait::async_trait;
use pockymoe_protocol::*;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use transport::Process;

struct Session {
    process: Arc<Process>,
    cwd: String,
    settings: Mutex<SessionSettings>,
    operation: Mutex<()>,
    active: Mutex<Option<(String, String)>>,
    background: AtomicBool,
    context_delivered: AtomicBool,
    load: Mutex<LoadedSession>,
    models: Vec<ModelOptionDto>,
    subagents: Mutex<Vec<ThreadSubagentDto>>,
}
struct Permission {
    session: String,
    thread: String,
    native_id: String,
    input: Value,
    process: Arc<Process>,
    dto: ThreadActionRequestDto,
    bus: EventBus,
}
pub struct ClaudeRuntime {
    acp: SharedRuntime,
    command: String,
    upstream_directory: Option<PathBuf>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    pending: Mutex<HashMap<String, Permission>>,
    lifecycle: broadcast::Sender<SessionLifecycle>,
}
impl ClaudeRuntime {
    pub fn new(acp: SharedRuntime, command: String, upstream_directory: Option<PathBuf>) -> Self {
        Self {
            acp,
            command,
            upstream_directory,
            sessions: Default::default(),
            pending: Default::default(),
            lifecycle: broadcast::channel(128).0,
        }
    }
    async fn session(&self, id: &str) -> Result<Arc<Session>> {
        self.sessions
            .lock()
            .await
            .get(id)
            .cloned()
            .context("Native Claude session is not loaded")
    }
    async fn load(
        &self,
        id: &str,
        cwd: &str,
        settings: SessionSettings,
        resume: Option<&str>,
        fork_at: Option<Option<&str>>,
    ) -> Result<StartSessionResult> {
        ensure!(
            !self.sessions.lock().await.contains_key(id),
            "conflict: Claude session already loaded"
        );
        let clean = self
            .upstream_directory
            .as_ref()
            .is_some_and(|dir| crate::upstreams::native::oauth_selected(dir, "claude"));
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
        ]
        .map(String::from)
        .into();
        // Permit switching to full access later; this flag alone grants nothing.
        // Root users are deliberately left on guarded mode (CLI policy).
        #[cfg(unix)]
        let can_bypass = unsafe { libc::geteuid() } != 0;
        #[cfg(not(unix))]
        let can_bypass = true;
        if can_bypass {
            args.push("--allow-dangerously-skip-permissions".into());
        }
        args.extend([
            "--permission-mode".into(),
            permission_mode(&settings).into(),
        ]);
        if let Some(model) = settings
            .model
            .as_ref()
            .filter(|m| !m.is_empty() && m.as_str() != "default")
        {
            args.extend(["--model".into(), model.clone()]);
        }
        if let Some(effort) = &settings.effort {
            args.extend(["--effort".into(), effort.clone()]);
        }
        if let Some(resume) = resume {
            args.extend(["--resume".into(), raw_id(resume)?.into()]);
        } else {
            args.extend(["--session-id".into(), raw_id(id)?.into()]);
        }
        if let Some(at) = fork_at {
            args.push("--fork-session".into());
            args.extend(["--session-id".into(), raw_id(id)?.into()]);
            if let Some(at) = at {
                args.extend(["--resume-session-at".into(), at.into()]);
            }
        }
        let caps = crate::acp::NegotiatedCaps {
            mcp_http: true,
            mcp_sse: true,
            ..Default::default()
        };
        let servers = crate::acp::mcp_servers::for_session("claude", &caps);
        let mut mcp = serde_json::Map::new();
        for s in servers.as_array().into_iter().flatten() {
            if let Some(name) = s["name"].as_str() {
                let config = match s["type"].as_str() {
                    Some("http" | "sse") => {
                        json!({"type":s["type"],"url":s["url"],"headers":s["headers"].as_array().into_iter().flatten().filter_map(|h|Some((h["name"].as_str()?.into(),h["value"].clone()))).collect::<serde_json::Map<String,Value>>()})
                    }
                    _ => {
                        json!({"command":s["command"],"args":s["args"],"env":s["env"].as_array().into_iter().flatten().filter_map(|e|Some((e["name"].as_str()?.into(),e["value"].clone()))).collect::<serde_json::Map<String,Value>>()})
                    }
                };
                mcp.insert(name.into(), config);
            }
        }
        if !mcp.is_empty() {
            args.extend(["--mcp-config".into(), json!({"mcpServers":mcp}).to_string()]);
        }
        let process = Process::spawn(&self.command, cwd, &args, clean).await?;
        let init = match process.request(json!({"subtype":"initialize","hooks":{},"sdkMcpServers":[],"agents":{},"forwardSubagentText":true})).await {
            Ok(init) => init,
            Err(e) => { process.stop().await; return Err(e); }
        };
        let intent = load_intent();
        let now = chrono::Utc::now();
        let load = LoadedSession {
            session_id: id.into(),
            thread_id: intent.as_ref().map(|i| i.thread_id.clone()),
            process_id: process.id.clone(),
            root_pid: process.pid,
            load_reason: intent
                .as_ref()
                .map(|i| i.reason)
                .unwrap_or(HarnessLoadReason::Turn),
            requested_at: intent.map(|i| i.requested_at).unwrap_or(now),
            loaded_at: now,
            last_activity_at: now,
            prompted: false,
            baseline: Vec::new(),
            busy: Vec::new(),
        };
        let _ = self.lifecycle.send(SessionLifecycle::Loaded {
            session_id: id.into(),
            thread_id: load.thread_id.clone(),
            reason: load.load_reason,
            requested_at: load.requested_at,
        });
        let result = StartSessionResult {
            provider_session_id: id.into(),
            model: settings.model.clone(),
            reasoning_effort: settings.effort.clone(),
        };
        self.sessions.lock().await.insert(
            id.into(),
            Arc::new(Session {
                process,
                cwd: cwd.into(),
                settings: Mutex::new(settings),
                operation: Mutex::new(()),
                active: Mutex::new(None),
                background: AtomicBool::new(false),
                context_delivered: AtomicBool::new(false),
                load: Mutex::new(load),
                models: parse_models(&init["models"]),
                subagents: Mutex::new(Vec::new()),
            }),
        );
        Ok(result)
    }
    async fn resolve(
        &self,
        permission: Permission,
        allow: bool,
        answer: Option<&str>,
    ) -> Result<()> {
        let response = if allow {
            let mut input = permission.input;
            if let Some(answer) = answer {
                if let Ok(value) = serde_json::from_str::<Value>(answer) {
                    if value.is_object() && permission.dto.kind == "requestUserInput" {
                        let mut answers = serde_json::Map::new();
                        for (index, q) in input["questions"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .enumerate()
                        {
                            if let Some(question) = q["question"].as_str() {
                                let answer = value[format!("question_{index}")]["answers"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                answers.insert(question.into(), json!(answer));
                            }
                        }
                        input["answers"] = json!(answers);
                        // Approval replies choose a decision; they never replace the
                        // original tool arguments with the UI's answer envelope.
                    }
                }
            }
            json!({"behavior":"allow","updatedInput":input})
        } else {
            json!({"behavior":"deny","message":answer.unwrap_or("The user denied this tool request")})
        };
        permission.process.write(json!({"type":"control_response","response":{"subtype":"success","request_id":permission.native_id,"response":response}})).await?;
        permission.bus.emit(ThreadEventEnvelope {
            event_type: "thread.request.resolved".into(),
            thread_id: permission.thread,
            timestamp: now_rfc3339(),
            payload: json!({"requestId":permission.dto.id}),
        });
        Ok(())
    }
    async fn control(
        &self,
        session_id: &str,
        session: &Arc<Session>,
        input: &StartTurnInput,
        bus: &EventBus,
        message: &Value,
    ) -> Result<()> {
        let native_id = message["request_id"]
            .as_str()
            .context("Claude control request missing id")?
            .to_owned();
        let request = &message["request"];
        if request["subtype"] != "can_use_tool" {
            // Unsupported callbacks must never hang or silently approve an action.
            return session.process.write(json!({"type":"control_response","response":{"subtype":"error","request_id":native_id,"error":"Unsupported native Claude control callback"}})).await;
        }
        let tool = request["tool_name"].as_str().unwrap_or("Tool");
        let settings = session.settings.lock().await.clone();
        let id = format!("claude-native:{}", uuid::Uuid::new_v4());
        let dto = ThreadActionRequestDto {
            id: id.clone(),
            kind: if tool == "AskUserQuestion" {
                "requestUserInput"
            } else {
                "permissionRequest"
            }
            .into(),
            title: tool.into(),
            description: Some(request["input"].to_string()),
            turn_id: Some(input.turn_id.clone()),
            item_id: request["tool_use_id"].as_str().map(str::to_owned),
            created_at: now_rfc3339(),
            questions: request_questions(tool, &request["input"]),
        };
        let permission = Permission {
            session: session_id.into(),
            thread: input.thread_id.clone(),
            native_id,
            input: request["input"].clone(),
            process: session.process.clone(),
            dto: dto.clone(),
            bus: bus.clone(),
        };
        // Full access wins over plan, exactly like the product ACP policy. Ask
        // tools and ExitPlanMode always require a user decision.
        let interactive = matches!(tool, "AskUserQuestion" | "ExitPlanMode");
        if permission_mode(&settings) == "bypassPermissions" && !interactive {
            return self.resolve(permission, true, None).await;
        }
        if settings.sandbox_mode.as_deref() == Some("read-only")
            && !matches!(
                tool,
                "Read" | "Glob" | "Grep" | "WebSearch" | "WebFetch" | "AskUserQuestion"
            )
        {
            return self
                .resolve(permission, false, Some("This thread is read-only"))
                .await;
        }
        self.pending.lock().await.insert(id, permission);
        bus.emit(ThreadEventEnvelope {
            event_type: "thread.request.created".into(),
            thread_id: input.thread_id.clone(),
            timestamp: now_rfc3339(),
            payload: json!({"turnId":input.turn_id,"request":dto}),
        });
        Ok(())
    }
}
fn raw_id(id: &str) -> Result<&str> {
    let id = id.strip_prefix("claude::").unwrap_or(id);
    ensure!(
        !id.contains("::") && !id.starts_with("env/") && !id.is_empty(),
        "Invalid host Claude session identity"
    );
    Ok(id)
}
fn permission_mode(settings: &SessionSettings) -> &'static str {
    let policy = crate::acp::ProductSessionPolicy {
        collaboration_mode: settings.collaboration_mode.clone(),
        sandbox_mode: settings.sandbox_mode.clone(),
        approval_mode: settings.approval_mode.clone(),
    };
    if policy.allows_writes_outside_workspace() {
        "bypassPermissions"
    } else if policy.rejects_writes() {
        "plan"
    } else {
        "default"
    }
}
fn parse_models(value: &Value) -> Vec<ModelOptionDto> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["value"].as_str()?;
            let efforts: Vec<_> = m["supportedEffortLevels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e.as_str())
                .map(|e| ReasoningEffortOptionDto {
                    reasoning_effort: e.into(),
                    description: e.into(),
                })
                .collect();
            Some(ModelOptionDto {
                id: id.into(),
                model: id.into(),
                display_name: m["displayName"].as_str().unwrap_or(id).into(),
                description: m["description"].as_str().unwrap_or("").into(),
                is_default: id == "default",
                hidden: false,
                supported_reasoning_efforts: efforts,
                default_reasoning_effort: None,
                selection_kind: None,
                acp_agent: None,
            })
        })
        .collect()
}

#[async_trait]
impl AgentRuntime for ClaudeRuntime {
    fn session_loaded(&self, id: &str) -> bool {
        self.sessions.try_lock().ok().is_some_and(|sessions| {
            sessions
                .get(id)
                .is_some_and(|s| s.process.alive.load(Ordering::SeqCst))
        })
    }
    fn provider(&self) -> Provider {
        self.acp.provider()
    }
    fn descriptor(&self) -> AgentBackendDto {
        let mut d = self.acp.descriptor();
        d.capabilities = self.negotiated_caps(Some("claude"));
        d
    }
    async fn inspect_context(&self, id: &str, detail: &str) -> Result<Option<Value>> {
        let s = self.session(id).await?;
        let raw = s
            .process
            .request(json!({"subtype":"get_context_usage"}))
            .await?;
        Ok(crate::context_inspection::claude_snapshot(&raw, detail))
    }
    async fn start(&self) -> Result<()> {
        Ok(())
    }
    async fn restart(&self, _agent: &str) -> Result<usize> {
        let ids: Vec<_> = self.sessions.lock().await.keys().cloned().collect();
        let count = ids.len();
        for id in ids {
            self.release_session(&id).await?;
        }
        Ok(count)
    }
    async fn touch_session(&self, id: &str) {
        if let Ok(s) = self.session(id).await {
            s.load.lock().await.last_activity_at = chrono::Utc::now();
        }
    }
    async fn list_agents(&self) -> Result<Vec<ModelOptionDto>> {
        self.acp.list_agents().await
    }
    async fn list_models(
        &self,
        _agent_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<Vec<ModelOptionDto>> {
        for session in self.sessions.lock().await.values() {
            if session.process.alive.load(Ordering::SeqCst) && !session.models.is_empty() {
                return Ok(session.models.clone());
            }
        }
        let id = format!("claude::{}", uuid::Uuid::new_v4());
        self.load(
            &id,
            cwd.unwrap_or("."),
            SessionSettings::default(),
            None,
            None,
        )
        .await?;
        let models = self.session(&id).await?.models.clone();
        self.release_session(&id).await?;
        Ok(models)
    }
    fn negotiated_caps(&self, _agent: Option<&str>) -> AgentProviderCapabilitiesDto {
        let mut caps = AgentProviderCapabilitiesDto::conversational();
        caps.turns.stream_input = true;
        caps.turns.steer = true;
        caps.turns.compact = true;
        caps.branching.fork = true;
        caps.branching.fork_at = true;
        caps.branching.resume_at = true;
        caps.controls.live_model_switch = true;
        caps
    }
    async fn capabilities(&self, _agent: Option<&str>) -> Result<AgentCapabilitySnapshotDto> {
        Ok(AgentCapabilitySnapshotDto {
            provider: self.provider(),
            agent_id: "claude".into(),
            availability: "available".into(),
            negotiated: Some(json!({"protocol":"stream-json","isolatedExecution":false})),
            effective_capabilities: Some(self.negotiated_caps(None)),
            toolbox_items: self.toolbox(None),
        })
    }
    async fn start_session(&self, input: StartSessionInput) -> Result<StartSessionResult> {
        self.load(
            &format!("claude::{}", uuid::Uuid::new_v4()),
            &input.cwd,
            SessionSettings {
                model: Some(input.model),
                effort: input.reasoning_effort,
                sandbox_mode: input.sandbox_mode,
                approval_mode: Some(input.approval_mode),
                ..Default::default()
            },
            None,
            None,
        )
        .await
    }
    async fn resume_session(
        &self,
        id: &str,
        cwd: Option<&str>,
        settings: SessionSettings,
    ) -> Result<StartSessionResult> {
        raw_id(id)?;
        let canonical = format!("claude::{}", raw_id(id)?);
        let existing = self.sessions.lock().await.get(&canonical).cloned();
        if let Some(session) = existing {
            if session.process.alive.load(Ordering::SeqCst) {
                self.apply_session_settings(&canonical, settings.clone())
                    .await?;
                return Ok(StartSessionResult {
                    provider_session_id: canonical,
                    model: settings.model,
                    reasoning_effort: settings.effort,
                });
            }
        }
        self.sessions.lock().await.remove(&canonical);
        self.load(
            &canonical,
            cwd.context("Claude resume requires a working directory")?,
            settings,
            Some(id),
            None,
        )
        .await
    }
    async fn apply_session_settings(&self, id: &str, settings: SessionSettings) -> Result<()> {
        let s = self.session(id).await?;
        let mut current = s.settings.lock().await;
        if let Some(model) = &settings.model {
            if current.model.as_ref() != Some(model) {
                s.process
                    .request(json!({"subtype":"set_model","model":model}))
                    .await?;
                current.model = Some(model.clone());
            }
        }
        if let Some(effort) = &settings.effort {
            if current.effort.as_ref() != Some(effort) {
                s.process
                    .request(
                        json!({"subtype":"apply_flag_settings","settings":{"effortLevel":effort}}),
                    )
                    .await?;
                current.effort = Some(effort.clone());
            }
        }
        let mut next = current.clone();
        if settings.approval_mode.is_some() {
            next.approval_mode = settings.approval_mode;
        }
        if settings.sandbox_mode.is_some() {
            next.sandbox_mode = settings.sandbox_mode;
        }
        if settings.collaboration_mode.is_some() {
            next.collaboration_mode = settings.collaboration_mode;
        }
        if permission_mode(&next) != permission_mode(&current) {
            s.process
                .request(json!({"subtype":"set_permission_mode","mode":permission_mode(&next)}))
                .await?;
        }
        *current = next;
        Ok(())
    }
    async fn start_turn(
        &self,
        input: StartTurnInput,
        bus: EventBus,
        cancel: CancellationToken,
    ) -> Result<Vec<ThreadHistoryItemDto>> {
        let s = self.session(&input.provider_session_id).await?;
        let _operation = s
            .operation
            .try_lock()
            .context("conflict: Claude is already running a turn")?;
        ensure!(
            s.process.alive.load(Ordering::SeqCst),
            "Claude process exited; resume the session before sending again"
        );
        self.apply_session_settings(
            &input.provider_session_id,
            SessionSettings {
                model: input.model.clone(),
                effort: input.reasoning_effort.clone(),
                sandbox_mode: input.sandbox_mode.clone(),
                collaboration_mode: input.collaboration_mode.clone(),
                approval_mode: input.approval_mode.clone(),
                performance_mode: None,
            },
        )
        .await?;
        let mut actual_input = input.clone();
        if !input.hidden
            && !input.context_delivered
            && !s.context_delivered.load(Ordering::SeqCst)
            && !crate::interaction::launch_env().is_empty()
        {
            actual_input.prompt = crate::acp::prompt::with_thread_context(&input.prompt);
        }
        let blocks = prompt_blocks(&actual_input, &s.cwd)?;
        *s.active.lock().await = Some((input.thread_id.clone(), input.turn_id.clone()));
        if !input.hidden {
            bus.emit(ThreadEventEnvelope {
                event_type: "thread.harness.ready".into(),
                thread_id: input.thread_id.clone(),
                timestamp: now_rfc3339(),
                payload: json!({"turnId":input.turn_id,"instanceId":s.process.id}),
            });
        }
        {
            let mut load = s.load.lock().await;
            load.prompted = true;
            load.last_activity_at = chrono::Utc::now();
        }
        let mut mapper = mapping::Mapper::new(&input.turn_id);
        let mut lifecycle = completion::Completion::default();
        let mut events = s.process.events.lock().await;
        let outcome: Result<bool> = async {
            s.process.write(json!({"type":"user","uuid":uuid::Uuid::new_v4().to_string(),"session_id":raw_id(&input.provider_session_id)?,"parent_tool_use_id":null,"message":{"role":"user","content":blocks}})).await?;
            if !input.hidden { s.context_delivered.store(true,Ordering::SeqCst); }

            loop {
                let message=tokio::select! {
                    _=cancel.cancelled()=> { let _=tokio::time::timeout(Duration::from_secs(3),s.process.request(json!({"subtype":"interrupt"}))).await; s.process.stop().await; return Ok(true); }
                    message=events.recv()=> message.context(ExecutionUncertain("Claude stream closed mid-turn".into()))?
                };
                if message["type"]=="control_request" { self.control(&input.provider_session_id,&s,&input,&bus,&message).await?; continue; }
                if message["type"]=="control_cancel_request" {
                    let native=message["request_id"].as_str().unwrap_or("");
                    let mut pending=self.pending.lock().await;
                    let ids:Vec<_>=pending.iter().filter(|(_,p)|p.session==input.provider_session_id&&p.native_id==native).map(|(id,_)|id.clone()).collect();
                    for id in ids { if let Some(p)=pending.remove(&id) { p.bus.emit(ThreadEventEnvelope { event_type:"thread.request.resolved".into(), thread_id:p.thread,timestamp:now_rfc3339(),payload:json!({"requestId":id}) }); } }
                    continue;
                }
                lifecycle.record(&message);
                s.background.store(lifecycle.pending(),Ordering::SeqCst);
                for mapped in mapper.apply(&message) { if let Some(agents)=&mapped.active_subagents { *s.subagents.lock().await=agents.clone(); } crate::acp::runtime::emit_mapped(&bus,&input.thread_id,&input.turn_id,mapped,input.hidden); }
                if message["type"]=="result" && message["is_error"]==true { bail!("Claude turn failed ({})",message["subtype"].as_str().unwrap_or("unknown")); }
                if lifecycle.complete() { return Ok(false); }
            }
        }.await;
        let failed = outcome.is_err();
        let interrupted = outcome.as_ref().copied().unwrap_or(false);
        if failed {
            s.process.stop().await;
        }
        *s.active.lock().await = None;
        s.subagents.lock().await.clear();
        let mut pending = self.pending.lock().await;
        let ids: Vec<_> = pending
            .iter()
            .filter(|(_, p)| p.session == input.provider_session_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(p) = pending.remove(&id) {
                p.bus.emit(ThreadEventEnvelope {
                    event_type: "thread.request.resolved".into(),
                    thread_id: p.thread,
                    timestamp: now_rfc3339(),
                    payload: json!({"requestId":id}),
                });
            }
        }
        drop(pending);
        let mut items = mapper.finish(interrupted || failed);
        if failed {
            for item in &mut items {
                if item.status.as_deref() == Some("interrupted") {
                    item.status = Some("failed".into());
                }
            }
        }
        if !input.hidden {
            for item in &items {
                bus.emit(ThreadEventEnvelope {
                    event_type: "thread.item.completed".into(),
                    thread_id: input.thread_id.clone(),
                    timestamp: now_rfc3339(),
                    payload: json!({"turnId":input.turn_id,"item":item}),
                });
            }
        }
        outcome?;
        Ok(items)
    }
    async fn interrupt(&self, id: &str, turn: &str) -> Result<()> {
        let s = self.session(id).await?;
        ensure!(
            s.active
                .lock()
                .await
                .as_ref()
                .is_some_and(|(_, active)| active == turn),
            "Claude turn is no longer active"
        );
        s.process
            .request(json!({"subtype":"interrupt","cancel_queued_messages":true}))
            .await?;
        Ok(())
    }
    async fn send_input(&self, id: &str, turn: &str, prompt: &str) -> Result<()> {
        let s = self.session(id).await?;
        ensure!(
            s.active
                .lock()
                .await
                .as_ref()
                .is_some_and(|(_, active)| active == turn),
            "Claude turn is no longer active"
        );
        s.process.write(json!({"type":"user","uuid":uuid::Uuid::new_v4().to_string(),"session_id":raw_id(id)?,"parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"text","text":prompt}]}})).await
    }
    async fn active_subagents(&self, id: &str) -> Vec<ThreadSubagentDto> {
        match self.session(id).await {
            Ok(s) => s.subagents.lock().await.clone(),
            Err(_) => Vec::new(),
        }
    }
    async fn execution_state(&self, id: &str) -> ExecutionState {
        let Ok(s) = self.session(id).await else {
            return ExecutionState::Unknown;
        };
        if !s.process.alive.load(Ordering::SeqCst) {
            return ExecutionState::Unknown;
        }
        let active = s.active.lock().await;
        active
            .as_ref()
            .map(|(_, id)| ExecutionState::Running {
                turn_id: id.clone(),
            })
            .unwrap_or(ExecutionState::Idle)
    }
    async fn respond_permission(&self, id: &str, allow: bool, answer: Option<&str>) -> Result<()> {
        let p = self
            .pending
            .lock()
            .await
            .remove(id)
            .context("Claude permission no longer pending")?;
        self.resolve(p, allow, answer).await
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
    async fn loaded_sessions(&self) -> Vec<LoadedSession> {
        let sessions: Vec<_> = self.sessions.lock().await.values().cloned().collect();
        let mut loaded = Vec::new();
        for s in sessions {
            if !s.process.alive.load(Ordering::SeqCst) {
                continue;
            }
            let mut load = s.load.lock().await.clone();
            if s.active.lock().await.is_some() {
                load.busy.push(HarnessBusyReason::Turn);
            }
            if s.background.load(Ordering::SeqCst) {
                load.busy.push(HarnessBusyReason::BackgroundTask);
            }
            loaded.push(load);
        }
        loaded
    }
    async fn release_loaded(
        &self,
        id: &str,
        _scope: ReleaseScope,
        process_id: Option<&str>,
    ) -> Result<Vec<LoadedSession>> {
        let s = self.session(id).await?;
        let _op = s.operation.try_lock().map_err(|_| TurnRunning)?;
        let load = s.load.lock().await.clone();
        ensure!(
            process_id.is_none_or(|p| p == load.process_id),
            "conflict: Claude process changed"
        );
        ensure!(
            !s.background.load(Ordering::SeqCst),
            "conflict: Claude background work is still running"
        );
        s.process.stop().await;
        self.sessions.lock().await.remove(id);
        let _ = self.lifecycle.send(SessionLifecycle::Released {
            session_id: id.into(),
            thread_id: load.thread_id.clone(),
            reason: ReleaseReason::Explicit,
            load_reason: load.load_reason,
            prompted: load.prompted,
        });
        Ok(vec![load])
    }
    async fn release_session(&self, id: &str) -> Result<()> {
        self.release_loaded(id, ReleaseScope::Session, None).await?;
        Ok(())
    }
    fn lifecycle(&self) -> Option<broadcast::Receiver<SessionLifecycle>> {
        Some(self.lifecycle.subscribe())
    }
    async fn compact_session(&self, id: &str, thread: &str, bus: EventBus) -> Result<()> {
        self.start_turn(
            StartTurnInput {
                provider_session_id: id.into(),
                prompt: "/compact".into(),
                model: None,
                reasoning_effort: None,
                sandbox_mode: None,
                collaboration_mode: None,
                approval_mode: None,
                performance_mode: None,
                thread_id: thread.into(),
                turn_id: uuid::Uuid::new_v4().to_string(),
                hidden: true,
                images: vec![],
                title: None,
                context_delivered: true,
            },
            bus,
            CancellationToken::new(),
        )
        .await?;
        Ok(())
    }
    async fn fork_session(&self, id: &str) -> Result<StartSessionResult> {
        Ok(self
            .fork_session_at(id, &ForkPoint::default())
            .await?
            .session)
    }
    async fn fork_session_at(&self, id: &str, point: &ForkPoint) -> Result<ForkResult> {
        let s = self.session(id).await?;
        let _op = s.operation.try_lock().map_err(|_| TurnRunning)?;
        let original_at = point.reply.as_ref().and_then(|r| r.message_id.as_deref());
        let resolved_at =
            original_at.map(|at| resolve_fork_uuid(id, at).unwrap_or_else(|| at.into()));
        let at = resolved_at.as_deref();
        ensure!(
            point.rollback_count == 0 || at.is_some(),
            "Claude fork at a prior turn requires the native message UUID"
        );
        let new = format!("claude::{}", uuid::Uuid::new_v4());
        let settings = s.settings.lock().await.clone();
        let session = self
            .load(&new, &s.cwd, settings, Some(id), Some(at))
            .await?;
        Ok(ForkResult {
            session,
            release_source: false,
        })
    }
    async fn list_import_sessions(&self, agent: Option<&str>) -> Result<Vec<ImportSessionMeta>> {
        self.acp.list_import_sessions(agent).await
    }
    async fn session_instance_id(&self, id: &str) -> Option<String> {
        Some(self.session(id).await.ok()?.process.id.clone())
    }
    async fn session_instance_started_at(&self, id: &str) -> Option<String> {
        Some(
            self.session(id)
                .await
                .ok()?
                .load
                .lock()
                .await
                .loaded_at
                .to_rfc3339(),
        )
    }
}
fn prompt_blocks(input: &StartTurnInput, cwd: &str) -> Result<Vec<Value>> {
    let mut blocks = Vec::new();
    for block in crate::acp::prompt::build_prompt_blocks(
        &input.prompt,
        std::path::Path::new(cwd),
        true,
        &input.images,
    )? {
        blocks.push(match block["type"].as_str() {
            Some("image")=>json!({"type":"image","source":{"type":"base64","media_type":block["mimeType"],"data":block["data"]}}),
            Some("resource_link")=>json!({"type":"text","text":format!("[FILE {}] {}",block["name"].as_str().unwrap_or("file"),block["uri"].as_str().unwrap_or(""))}),
            _=>block,
        });
    }
    Ok(blocks)
}

// ACP groups replies by API message id, whereas --resume-session-at takes a
// persisted chain UUID. Resolve either spelling against Claude's shared store.
fn resolve_fork_uuid(session: &str, message: &str) -> Option<String> {
    use std::io::BufRead;
    let root = crate::local_sessions::LocalSessionHomes::from_env()
        .claude_home
        .join("projects");
    let filename = format!("{}.jsonl", raw_id(session).ok()?);
    for project in std::fs::read_dir(root).ok()?.flatten() {
        let Ok(file) = std::fs::File::open(project.path().join(&filename)) else {
            continue;
        };
        let mut found = None;
        for line in std::io::BufReader::new(file)
            .lines()
            .map_while(std::result::Result::ok)
        {
            let Ok(entry) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if entry["type"] == "assistant"
                && entry["isSidechain"] != true
                && (entry["message"]["id"] == message || entry["uuid"] == message)
            {
                found = entry["uuid"].as_str().map(str::to_owned);
            }
        }
        if found.is_some() {
            return found;
        }
    }
    None
}

fn request_questions(tool: &str, input: &Value) -> Vec<Value> {
    if tool == "AskUserQuestion" {
        input["questions"].as_array().into_iter().flatten().enumerate().map(|(index,q)|json!({
            "id":format!("question_{index}"),"header":q["header"],"question":q["question"],"options":q["options"],"multiSelect":q["multiSelect"].as_bool().unwrap_or(false),"isOther":true,"isSecret":false
        })).collect()
    } else {
        vec![
            json!({"id":"permission","header":"Permission","question":tool,"isOther":false,"isSecret":false,"options":[{"label":"Allow once","description":"allow once"},{"label":"Reject","description":"reject once"}]}),
        ]
    }
}
