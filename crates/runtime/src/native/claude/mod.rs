//! Native Claude Code stream-json runtime. ACP remains an independent backend.
mod completion;
mod isolated;
mod mapping;
mod permissions;
mod pump;
#[cfg(test)]
mod replay_tests;
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
    /// The native store's session UUID, resolved once in the load's context.
    raw: String,
    cwd: String,
    settings: Mutex<SessionSettings>,
    operation: Mutex<()>,
    active: Mutex<Option<(String, String)>>,
    /// Session-level CLI state (tasks, idle bookends), fed every frame.
    completion: std::sync::Mutex<completion::Completion>,
    /// The running turn's inbox. Frames outside a turn never reach a later
    /// turn: the pump records them and answers their callbacks.
    sink: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<Value>>>,
    /// A stop was requested for the running turn: its error result is the
    /// interruption, not a failure.
    interrupting: AtomicBool,
    /// The model's context window once Claude reports it (0 = unknown).
    window: std::sync::atomic::AtomicU64,
    /// Claude's current permission mode as the CLI last reported or accepted
    /// it; a plan approval moves it without us, so each turn re-asserts ours.
    mode: std::sync::Mutex<String>,
    /// The account offers Claude's `auto` mode.
    auto_mode: bool,
    /// Slash commands and skills, from initialize and `commands_changed`.
    commands: std::sync::Mutex<Vec<crate::acp::NegotiatedCommand>>,
    /// Device-registry MCP servers passed at launch, and those whose startup
    /// failure was already reported (Claude repeats `init` on every turn).
    configured_mcp: Vec<String>,
    reported_mcp: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Fast mode as last applied.
    fast: std::sync::Mutex<Option<bool>>,
    context_delivered: AtomicBool,
    load: Mutex<LoadedSession>,
    models: Vec<ModelOptionDto>,
    subagents: Mutex<Vec<ThreadSubagentDto>>,
}
impl Session {
    /// Track the mode Claude reports (init, a plan exit, `/permissions`).
    fn observe(&self, frame: &Value) {
        if frame["type"] == "system" && frame["subtype"] == "commands_changed" {
            *self.commands.lock().unwrap() = crate::acp::parse_commands(&frame["commands"]);
        }
        if frame["type"] == "system" {
            if let Some(mode) = frame["permissionMode"].as_str() {
                *self.mode.lock().unwrap() = mode.to_owned();
            }
        }
    }
    /// Put Claude in the product policy's mode if it drifted.
    async fn assert_mode(&self, settings: &SessionSettings) -> Result<()> {
        let desired = desired_mode(settings, self.auto_mode);
        if *self.mode.lock().unwrap() == desired {
            return Ok(());
        }
        self.process
            .request(json!({"subtype":"set_permission_mode","mode":desired}))
            .await?;
        *self.mode.lock().unwrap() = desired.to_owned();
        Ok(())
    }
}
struct Permission {
    /// An MCP server's form (`elicitation`), answered as an MCP ElicitResult.
    elicitation: Option<Value>,
    session: String,
    choices: Vec<permissions::Choice>,
    tool_use: Option<String>,
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
    /// The latest slash commands and account features Claude reported, for
    /// capabilities and the toolbox before a session is loaded.
    features: std::sync::Mutex<Features>,
    inspection: std::sync::RwLock<Option<Arc<crate::context_inspection::InspectionState>>>,
}
#[derive(Clone, Default)]
struct Features {
    commands: Vec<crate::acp::NegotiatedCommand>,
    fast: bool,
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
            features: Default::default(),
            inspection: Default::default(),
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
        let isolated = isolated::Launch::current()?;
        if let Some(launch) = &isolated {
            args.extend(launch.args());
        }
        let caps = crate::acp::NegotiatedCaps {
            mcp_http: true,
            mcp_sse: true,
            ..Default::default()
        };
        // The isolated projection refused a nonempty host registry already; the
        // host registry is never converted for a container session.
        let servers = if isolated.is_some() {
            json!([])
        } else {
            crate::acp::mcp_servers::for_session("claude", &caps)
        };
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
        let configured_mcp: Vec<String> = mcp.keys().cloned().collect();
        if !mcp.is_empty() {
            args.extend(["--mcp-config".into(), json!({"mcpServers":mcp}).to_string()]);
        }
        let process = match &isolated {
            Some(launch) => {
                Process::spawn_isolated(&launch.context, &args, cwd, launch.env.clone()).await?
            }
            None => Process::spawn(&self.command, cwd, &args, clean).await?,
        };
        let init = match process.request(json!({"subtype":"initialize","hooks":{},"sdkMcpServers":[],"agents":{},"forwardSubagentText":false})).await {
            Ok(init) => init,
            Err(e) => { process.stop().await; return Err(e); }
        };
        if isolated.is_some() {
            if let Err(error) = isolated::verify(&init) {
                process.stop().await;
                return Err(error);
            }
            if let Some(model) = settings.model.as_deref().filter(|m| !m.is_empty()) {
                // The broker admits exactly the thread model; a model the CLI does
                // not advertise fails here rather than at the first prompt.
                let advertised = parse_models(&init["models"]);
                if !advertised.iter().any(|m| m.id == model) {
                    process.stop().await;
                    bail!("unsupported_environment_claude_model: select an advertised model");
                }
            }
        }
        let spawned_mode = init["current_permission_mode"]
            .as_str()
            .unwrap_or(permission_mode(&settings))
            .to_owned();
        let auto_mode = init["models"]
            .as_array()
            .is_some_and(|models| models.iter().any(|m| m["supportsAutoMode"] == true));
        let commands = crate::acp::parse_commands(&init["commands"]);
        *self.features.lock().unwrap() = Features {
            commands: commands.clone(),
            fast: init["models"]
                .as_array()
                .is_some_and(|models| models.iter().any(|m| m["supportsFastMode"] == true)),
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
                raw: raw_id(id)?.into(),
                cwd: cwd.into(),
                settings: Mutex::new(settings),
                operation: Mutex::new(()),
                active: Mutex::new(None),
                completion: Default::default(),
                sink: Default::default(),
                interrupting: AtomicBool::new(false),
                window: Default::default(),
                mode: std::sync::Mutex::new(spawned_mode),
                auto_mode,
                commands: std::sync::Mutex::new(commands.clone()),
                fast: Default::default(),
                configured_mcp,
                reported_mcp: Default::default(),
                context_delivered: AtomicBool::new(false),
                load: Mutex::new(load),
                models: parse_models(&init["models"]),
                subagents: Mutex::new(Vec::new()),
            }),
        );
        let session = self.sessions.lock().await.get(id).cloned();
        if let Some(session) = session {
            pump::spawn(&session);
            // The window before the first result: Claude's own context count.
            let probe = session.clone();
            tokio::spawn(async move {
                if let Ok(usage) = probe
                    .process
                    .request(json!({"subtype":"get_context_usage","detail":"summary"}))
                    .await
                {
                    if let Some(window) = usage["rawMaxTokens"]
                        .as_u64()
                        .or(usage["maxTokens"].as_u64())
                    {
                        let _ = probe.window.compare_exchange(
                            0,
                            window,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        );
                    }
                }
            });
            let settings = session.settings.lock().await.clone();
            session.assert_mode(&settings).await?;
        }
        Ok(result)
    }
    /// An MCP server asks the user for input (form mode), as ACP elicitation.
    async fn elicit(
        &self,
        session_id: &str,
        session: &Arc<Session>,
        input: &StartTurnInput,
        bus: &EventBus,
        native_id: String,
        request: &Value,
    ) -> Result<()> {
        let params = json!({
            "mode": request["mode"].as_str().unwrap_or("form"),
            "message": request["message"],
            "requestedSchema": request["requested_schema"],
        });
        let questions = match crate::acp::elicitation_questions(&params, false) {
            Ok(questions) => questions,
            Err(_) => {
                // URL mode and empty forms are declined, as on ACP.
                return session.process.write(json!({"type":"control_response","response":{"subtype":"success","request_id":native_id,"response":{"action":"decline"}}})).await;
            }
        };
        let id = format!("claude-native:{}", uuid::Uuid::new_v4());
        let server = request["mcp_server_name"].as_str().unwrap_or("MCP server");
        let dto = ThreadActionRequestDto {
            id: id.clone(),
            kind: "requestUserInput".into(),
            title: request["title"].as_str().unwrap_or(server).into(),
            description: request["message"].as_str().map(str::to_owned),
            turn_id: Some(input.turn_id.clone()),
            item_id: None,
            created_at: now_rfc3339(),
            questions,
        };
        self.pending.lock().await.insert(
            id,
            Permission {
                elicitation: Some(params),
                session: session_id.into(),
                choices: Vec::new(),
                tool_use: None,
                thread: input.thread_id.clone(),
                native_id,
                input: Value::Null,
                process: session.process.clone(),
                dto: dto.clone(),
                bus: bus.clone(),
            },
        );
        bus.emit(ThreadEventEnvelope {
            event_type: "thread.request.created".into(),
            thread_id: input.thread_id.clone(),
            timestamp: now_rfc3339(),
            payload: json!({"turnId":input.turn_id,"request":dto}),
        });
        Ok(())
    }
    /// Ask Claude what its context holds and publish it (inspection opt-in).
    fn spawn_context_summary(
        &self,
        session: &Arc<Session>,
        id: &str,
        thread: &str,
        bus: &EventBus,
        busy: Option<Arc<AtomicBool>>,
    ) {
        let Some(state) = self
            .inspection
            .read()
            .unwrap()
            .clone()
            .filter(|s| s.enabled())
        else {
            return;
        };
        if busy
            .as_ref()
            .is_some_and(|b| b.swap(true, Ordering::AcqRel))
        {
            return;
        }
        let process = session.process.clone();
        let (id, thread, bus) = (id.to_owned(), thread.to_owned(), bus.session_observation());
        tokio::spawn(async move {
            let raw = tokio::time::timeout(
                Duration::from_secs(20),
                process.request(json!({"subtype":"get_context_usage","detail":"summary"})),
            )
            .await;
            if let Ok(Ok(raw)) = raw {
                if let Some(snapshot) = crate::context_inspection::claude_snapshot(&raw, "summary")
                {
                    crate::context_inspection::emit(&state, &id, &thread, &bus, snapshot);
                }
            }
            if let Some(busy) = busy {
                busy.store(false, Ordering::Release);
            }
        });
    }
    /// Claude's own model list: a loaded session's, or a short probe session's.
    async fn native_models(&self, cwd: Option<&str>) -> Result<Vec<ModelOptionDto>> {
        for session in self.sessions.lock().await.values() {
            if session.process.alive.load(Ordering::SeqCst) && !session.models.is_empty() {
                return Ok(session.models.clone());
            }
        }
        let id = isolated::session_id(
            crate::execution::current().as_ref(),
            &uuid::Uuid::new_v4().to_string(),
        );
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
    async fn resolve(
        &self,
        permission: Permission,
        allow: bool,
        answer: Option<&str>,
    ) -> Result<()> {
        let response = if let Some(params) = &permission.elicitation {
            // A malformed answer declines rather than failing the request.
            crate::acp::elicitation_response(params, false, allow, answer)
                .unwrap_or_else(|_| json!({"action":"decline"}))
        } else if permission.dto.kind != "requestUserInput" {
            permissions::response(
                &permission.choices,
                allow,
                answer,
                &permission.input,
                permission.tool_use.as_deref(),
            )
        } else if allow {
            let mut input = permission.input.clone();
            if let Some(answer) = answer {
                if let Ok(value) = serde_json::from_str::<Value>(answer) {
                    if value.is_object() {
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
                    }
                }
            }
            json!({"behavior":"allow","updatedInput":input})
        } else {
            json!({"behavior":"deny","message":"The user declined to answer"})
        };
        self.answer(permission, response).await
    }
    async fn answer(&self, permission: Permission, response: Value) -> Result<()> {
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
        if request["subtype"] == "elicitation" {
            return self
                .elicit(session_id, session, input, bus, native_id, request)
                .await;
        }
        if request["subtype"] != "can_use_tool" {
            // Unsupported callbacks must never hang or silently approve an action.
            return session.process.write(json!({"type":"control_response","response":{"subtype":"error","request_id":native_id,"error":"Unsupported native Claude control callback"}})).await;
        }
        let tool = request["tool_name"].as_str().unwrap_or("Tool");
        let settings = session.settings.lock().await.clone();
        let id = format!("claude-native:{}", uuid::Uuid::new_v4());
        let choices = permissions::choices(tool, request, session.auto_mode);
        let ask = tool == "AskUserQuestion";
        let (title, description, questions) = if ask {
            (
                "Question".to_owned(),
                None,
                request_questions(tool, &request["input"]),
            )
        } else {
            let (title, description, questions) =
                permissions::dto_parts(tool, request, &choices, &session.cwd);
            let title = if tool == "ExitPlanMode" {
                "Approve Plan".into()
            } else {
                title
            };
            (title, description, questions)
        };
        let dto = ThreadActionRequestDto {
            id: id.clone(),
            kind: if ask {
                "requestUserInput"
            } else {
                "permissionRequest"
            }
            .into(),
            title,
            description,
            turn_id: Some(input.turn_id.clone()),
            item_id: request["tool_use_id"].as_str().map(str::to_owned),
            created_at: now_rfc3339(),
            questions,
        };
        let permission = Permission {
            session: session_id.into(),
            elicitation: None,
            choices,
            tool_use: request["tool_use_id"].as_str().map(str::to_owned),
            thread: input.thread_id.clone(),
            native_id,
            input: request["input"].clone(),
            process: session.process.clone(),
            dto: dto.clone(),
            bus: bus.clone(),
        };
        // The product policy answers first, as on ACP: full access or yolo
        // approves (ACP's `auto_approve`), read-only refuses writes. Questions
        // and plan exits always need a person.
        let interactive = matches!(tool, "AskUserQuestion" | "ExitPlanMode");
        if settings.sandbox_mode.as_deref() == Some("read-only")
            && !matches!(
                tool,
                "Read" | "Glob" | "Grep" | "WebSearch" | "WebFetch" | "AskUserQuestion"
            )
        {
            return self
                .answer(
                    permission,
                    json!({"behavior":"deny","message":"This thread is read-only"}),
                )
                .await;
        }
        if policy(&settings).auto_approve() && !interactive {
            return self.resolve(permission, true, None).await;
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
/// The native store's session UUID. An environment-scoped identity
/// (`claude::env/{environment}/{generation}/{raw}`) is accepted only inside the
/// matching environment context; a host identity only on the host.
fn raw_id(id: &str) -> Result<&str> {
    let context = crate::execution::current();
    raw_in(
        id,
        context
            .as_ref()
            .map(|c| (c.environment_id.as_str(), c.generation)),
    )
}
fn raw_in<'a>(id: &'a str, context: Option<(&str, u64)>) -> Result<&'a str> {
    let scoped = id.strip_prefix("claude::").unwrap_or(id);
    let (raw, environment) = match scoped.strip_prefix("env/") {
        Some(reference) => {
            let mut parts = reference.splitn(3, '/');
            let environment = parts.next().unwrap_or("");
            let generation = parts.next().and_then(|g| g.parse::<u64>().ok());
            let raw = parts.next().unwrap_or("");
            ensure!(
                !environment.is_empty() && generation.is_some(),
                "Invalid environment Claude session identity"
            );
            (raw, Some((environment, generation.unwrap_or(0))))
        }
        None => (scoped, None),
    };
    ensure!(
        !raw.contains("::") && !raw.contains('/') && !raw.is_empty(),
        "Invalid Claude session identity"
    );
    match (environment, context) {
        (None, None) => {}
        (Some((environment, generation)), Some((current, current_generation))) => ensure!(
            current == environment && generation <= current_generation,
            "environment_session_context_mismatch"
        ),
        _ => bail!("environment_session_context_mismatch"),
    }
    Ok(raw)
}
/// The identity a session has in the current context: environment sessions are
/// re-scoped to the current generation, like ACP's `resume_session_identity`.
fn canonical_id(id: &str) -> Result<String> {
    let raw = raw_id(id)?;
    Ok(isolated::session_id(
        crate::execution::current().as_ref(),
        raw,
    ))
}
fn policy(settings: &SessionSettings) -> crate::acp::ProductSessionPolicy {
    crate::acp::ProductSessionPolicy {
        collaboration_mode: settings.collaboration_mode.clone(),
        sandbox_mode: settings.sandbox_mode.clone(),
        approval_mode: settings.approval_mode.clone(),
    }
}
/// Claude's permission mode for the product policy. Full access wins over
/// plan; plan and read-only refuse writes. Guarded work uses Claude's `auto`
/// mode where the account offers it, like the ACP adapter's default mode
/// (`acp/modes.rs`), and `default` otherwise.
fn desired_mode(settings: &SessionSettings, auto: bool) -> &'static str {
    match permission_mode(settings) {
        "default" if auto => "auto",
        mode => mode,
    }
}
fn permission_mode(settings: &SessionSettings) -> &'static str {
    let policy = policy(settings);
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
                supported_reasoning_efforts: efforts.clone(),
                // As ACP learns per model: medium where offered, else the first.
                default_reasoning_effort: efforts
                    .iter()
                    .find(|e| e.reasoning_effort == "medium")
                    .or(efforts.first())
                    .map(|e| e.reasoning_effort.clone()),
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
        // Opt-in, like ACP's context bridge; native needs no bridge and works
        // inside environments too.
        let Some(state) = self
            .inspection
            .read()
            .unwrap()
            .clone()
            .filter(|s| s.enabled())
        else {
            return Ok(None);
        };
        let s = self.session(id).await?;
        let raw = tokio::time::timeout(
            Duration::from_secs(20),
            s.process
                .request(json!({"subtype":"get_context_usage","detail":detail})),
        )
        .await??;
        let snapshot = crate::context_inspection::claude_snapshot(&raw, detail);
        if let Some(snapshot) = &snapshot {
            state.store(id, snapshot.clone());
        }
        Ok(snapshot)
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
        let native = self.native_models(cwd).await?;
        // A saved upstream connection names the models, as on ACP; Claude's own
        // list supplies efforts for the ones it knows.
        if let Some(directory) = self.upstream_directory.clone() {
            if let Some((profile, models)) =
                crate::acp::upstream_models::UpstreamModels::new(directory)
                    .catalog("claude")
                    .await?
            {
                return Ok(models
                    .iter()
                    .map(|m| m.option(&profile, native.iter().find(|n| n.model == m.id)))
                    .collect());
            }
        }
        Ok(native)
    }
    fn negotiated_caps(&self, _agent: Option<&str>) -> AgentProviderCapabilitiesDto {
        let features = self.features.lock().unwrap().clone();
        caps(
            &features.commands,
            features.fast,
            crate::execution::current().is_some(),
        )
    }
    fn session_negotiated_caps(
        &self,
        agent: Option<&str>,
        session_id: &str,
    ) -> AgentProviderCapabilitiesDto {
        let session = self
            .sessions
            .try_lock()
            .ok()
            .and_then(|sessions| sessions.get(session_id).cloned());
        let mut caps = match session {
            Some(s) => caps(
                &s.commands.lock().unwrap(),
                self.features.lock().unwrap().fast,
                session_id.contains("::env/"),
            ),
            None => self.negotiated_caps(agent),
        };
        crate::acp::runtime::isolated_capabilities::restrict(
            &mut caps,
            session_id.contains("::env/"),
        );
        caps
    }
    fn toolbox(&self, _agent: Option<&str>) -> Vec<ToolboxItemDto> {
        let features = self.features.lock().unwrap().clone();
        toolbox(&self.negotiated_caps(None), &features.commands)
    }
    async fn session_capabilities(
        &self,
        agent: Option<&str>,
        session_id: &str,
    ) -> Result<AgentCapabilitySnapshotDto> {
        let mut snapshot = self.capabilities(agent).await?;
        if let Ok(s) = self.session(session_id).await {
            let caps = self.session_negotiated_caps(agent, session_id);
            snapshot.toolbox_items = toolbox(&caps, &s.commands.lock().unwrap());
            snapshot.effective_capabilities = Some(caps);
        }
        Ok(snapshot)
    }
    async fn mcp_servers(&self, id: &str) -> Result<Option<Value>> {
        let s = self.session(id).await?;
        let status = tokio::time::timeout(
            Duration::from_secs(10),
            s.process.request(json!({"subtype":"mcp_status"})),
        )
        .await??;
        Ok(Some(mcp_servers(&status)))
    }
    fn configure_context_inspection(&self, state: Arc<crate::context_inspection::InspectionState>) {
        *self.inspection.write().unwrap() = Some(state);
    }
    async fn capabilities(&self, _agent: Option<&str>) -> Result<AgentCapabilitySnapshotDto> {
        Ok(AgentCapabilitySnapshotDto {
            provider: self.provider(),
            agent_id: "claude".into(),
            availability: "available".into(),
            negotiated: Some(
                json!({"protocol":"stream-json","isolatedExecution":crate::execution::current().is_some()}),
            ),
            effective_capabilities: Some(self.negotiated_caps(None)),
            toolbox_items: self.toolbox(None),
        })
    }
    async fn start_session(&self, input: StartSessionInput) -> Result<StartSessionResult> {
        self.load(
            &isolated::session_id(
                crate::execution::current().as_ref(),
                &uuid::Uuid::new_v4().to_string(),
            ),
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
        let canonical = canonical_id(id)?;
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
        s.assert_mode(&next).await?;
        if let Some(fast) = settings.performance_mode {
            let applied = *s.fast.lock().unwrap();
            if applied.unwrap_or(false) != fast {
                // Fast mode is Claude's own flag setting, as claude-agent-acp sets it.
                let result = s
                    .process
                    .request(json!({"subtype":"apply_flag_settings","settings":{"fastMode":fast}}))
                    .await;
                if fast {
                    result.context("Claude did not accept fast mode for this model")?;
                }
                *s.fast.lock().unwrap() = Some(fast);
            }
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
                performance_mode: input.performance_mode,
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
        crate::native::turn_events::started(&bus, &input);
        // On the host the JSONL reader bills every request (subagents included)
        // exactly as the ACP path does; in an environment the stream bills.
        let host = !input.provider_session_id.contains("::env/");
        let mut reader =
            host.then(|| crate::acp::ClaudeUsageReader::new(&input.provider_session_id));
        let known_window = s.window.load(Ordering::SeqCst);
        let mut mapper = mapping::Mapper::with(
            &input.turn_id,
            !host,
            (known_window > 0).then_some(known_window),
        );
        let include_context = actual_input.prompt != input.prompt;
        let mut usage_tick = tokio::time::interval(Duration::from_secs(1));
        let context_busy = Arc::new(AtomicBool::new(false));
        let mut allowance = crate::subscription::Announcer::default();
        let prompt_id = uuid::Uuid::new_v4().to_string();
        s.interrupting.store(false, Ordering::SeqCst);
        s.completion.lock().unwrap().begin_turn(&prompt_id);
        let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        *s.sink.lock().unwrap() = Some(tx);
        let mut waiting_shown = false;
        let outcome: Result<bool> = async {
            s.process.write(json!({"type":"user","uuid":prompt_id,"session_id":s.raw,"parent_tool_use_id":null,"message":{"role":"user","content":blocks}})).await?;
            if !input.hidden { s.context_delivered.store(true,Ordering::SeqCst); }
            let mut stopping: Option<tokio::time::Instant> = None;
            loop {
                let message = tokio::select! {
                    _ = cancel.cancelled(), if stopping.is_none() => {
                        // Stop the turn, keep the process: Claude settles the
                        // interrupted prompt with a result and returns to idle.
                        s.interrupting.store(true, Ordering::SeqCst);
                        stopping = Some(tokio::time::Instant::now() + STOP_GRACE);
                        if tokio::time::timeout(Duration::from_secs(3), s.process.request(json!({"subtype":"interrupt"}))).await.is_err() {
                            s.process.stop().await;
                            return Ok(true);
                        }
                        continue;
                    }
                    _ = tokio::time::sleep_until(stopping.unwrap_or_else(tokio::time::Instant::now)), if stopping.is_some() => {
                        // Claude did not settle the stop in time: end the process.
                        s.process.stop().await;
                        return Ok(true);
                    }
                    _ = usage_tick.tick() => {
                        let reports = reader.as_mut().map(|r| r.poll()).unwrap_or_default();
                        if !reports.is_empty() && !input.hidden {
                            // One composition per model request, as ACP's bridge does.
                            self.spawn_context_summary(&s, &input.provider_session_id, &input.thread_id, &bus, Some(context_busy.clone()));
                        }
                        for usage in reports {
                            crate::acp::runtime::emit_mapped(&bus,&input.thread_id,&input.turn_id,mapping::usage(usage),input.hidden);
                        }
                        continue;
                    }
                    message = events.recv() => message.context(ExecutionUncertain("Claude stream closed mid-turn".into()))?
                };
                s.completion.lock().unwrap().record(&message);
                s.observe(&message);
                if message["type"]=="system" && message["subtype"]=="init" {
                    // A device MCP server that failed to start, once per session, as ACP
                    // reports client-supplied servers; the account's own connectors are not ours.
                    for server in message["mcp_servers"].as_array().into_iter().flatten() {
                        let Some(name) = server["name"].as_str().filter(|n| s.configured_mcp.iter().any(|c| c == n)) else { continue };
                        let notice = match server["status"].as_str() {
                            Some("failed") => Some(("warning", format!("MCP server \"{name}\" failed to start"), "Its tools are unavailable in this session.")),
                            Some("needs-auth") => Some(("info", format!("MCP server \"{name}\" needs authentication"), "Sign in to it to use its tools.")),
                            _ => None,
                        };
                        if let Some((severity, title, detail)) = notice {
                            if s.reported_mcp.lock().unwrap().insert(name.to_owned()) {
                                crate::acp::runtime::emit_mapped(&bus,&input.thread_id,&input.turn_id,mapper.notice(severity,&title,detail),input.hidden);
                            }
                        }
                    }
                }
                if message["type"]=="rate_limit_event" {
                    // Live plan allowance from the API response itself; ACP drops it.
                    if let Some(update)=crate::subscription::claude_rate_update(&message["rate_limit_info"]) { allowance.announce(&bus,&input.thread_id,"claude",update,input.hidden); }
                    continue;
                }
                if message["type"]=="control_request" { self.control(&input.provider_session_id,&s,&input,&bus,&message).await?; continue; }
                if message["type"]=="control_cancel_request" {
                    let native=message["request_id"].as_str().unwrap_or("");
                    let mut pending=self.pending.lock().await;
                    let ids:Vec<_>=pending.iter().filter(|(_,p)|p.session==input.provider_session_id&&p.native_id==native).map(|(id,_)|id.clone()).collect();
                    for id in ids { if let Some(p)=pending.remove(&id) { p.bus.emit(ThreadEventEnvelope { event_type:"thread.request.resolved".into(), thread_id:p.thread,timestamp:now_rfc3339(),payload:json!({"requestId":id}) }); } }
                    continue;
                }
                mapper.set_bills(reader.as_ref().is_none_or(|r| !r.found()));
                if s.interrupting.load(Ordering::SeqCst) { mapper.set_stopping(); }
                for mapped in mapper.apply(&message) { if let Some(agents)=&mapped.active_subagents { *s.subagents.lock().await=agents.clone(); } crate::acp::runtime::emit_mapped(&bus,&input.thread_id,&input.turn_id,mapped,input.hidden); }
                let (complete, waiting, error) = {
                    let c = s.completion.lock().unwrap();
                    (c.complete(), c.waiting(), c.error().cloned())
                };
                if let Some(count) = waiting.filter(|_| !waiting_shown) {
                    waiting_shown = true;
                    crate::acp::runtime::emit_mapped(&bus,&input.thread_id,&input.turn_id,mapper.inner.native_background_wait(count),input.hidden);
                } else if waiting.is_none() {
                    waiting_shown = false;
                }
                if complete {
                    if s.interrupting.load(Ordering::SeqCst) { return Ok(true); }
                    if let Some(error) = error { return Err(TurnResultError(result_error(&error)).into()); }
                    return Ok(false);
                }
            }
        }.await;
        // Frames already forwarded but not read still update the session.
        s.sink.lock().unwrap().take();
        while let Ok(frame) = events.try_recv() {
            s.completion.lock().unwrap().record(&frame);
        }
        let failed = outcome.is_err();
        let interrupted = outcome.as_ref().copied().unwrap_or(false);
        // A result error (API error, refusal, limit) leaves a healthy process;
        // only a broken stream or protocol fault ends it.
        if failed
            && !outcome
                .as_ref()
                .err()
                .is_some_and(|e| e.is::<TurnResultError>())
        {
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
        if let Some(reader) = reader.as_mut() {
            for usage in reader.poll_final() {
                crate::acp::runtime::emit_mapped(
                    &bus,
                    &input.thread_id,
                    &input.turn_id,
                    mapping::usage(usage),
                    input.hidden,
                );
            }
        }
        if let Some(window) = mapper.window() {
            s.window.store(window, Ordering::SeqCst);
        }
        if !input.hidden && !failed {
            self.spawn_context_summary(
                &s,
                &input.provider_session_id,
                &input.thread_id,
                &bus,
                None,
            );
        }
        if include_context && !failed && !input.hidden {
            // Persisted by the service, so a resumed session keeps knowing.
            bus.emit(ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadContextDelivered,
                thread_id: input.thread_id.clone(),
                timestamp: now_rfc3339(),
                payload: json!({"providerSessionId": input.provider_session_id}),
            });
        }
        let completed_tools = mapper.inner.completed_tool_ids();
        let mut items = mapper.finish(interrupted || failed);
        if let Some(reader) = reader.as_ref() {
            reader.annotate_background_reply_phases(&mut items, !interrupted && !failed);
        }
        // Native completion already waited for every task and the final idle,
        // so the last reply of a completed turn is its answer, not commentary.
        if !interrupted && !failed {
            if let Some(last) = items.iter_mut().rev().find(|i| i.kind == "agentMessage") {
                if last.extra.contains_key("responsePhase") {
                    last.extra.insert("responsePhase".into(), json!("final"));
                }
            }
        }
        // Like ACP: whatever did not finish takes the turn's status, so no
        // tool stays "running" in history.
        let status = match &outcome {
            Ok(_) if interrupted => "interrupted",
            Ok(_) => "completed",
            Err(e) if e.is::<ExecutionUncertain>() => "recovering",
            Err(_) => "failed",
        };
        for item in &mut items {
            if item.status.as_deref() != Some("failed")
                && !completed_tools.contains(&item.id)
                && item.extra.get("origin").and_then(Value::as_str)
                    != Some("nativeTaskNotification")
            {
                item.status = Some(status.into());
            }
        }
        crate::native::turn_events::finished(&bus, &input, &outcome, interrupted, Some(&items));
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
        s.interrupting.store(true, Ordering::SeqCst);
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
        let uuid = uuid::Uuid::new_v4().to_string();
        s.completion.lock().unwrap().add_prompt(&uuid);
        s.process.write(json!({"type":"user","uuid":uuid,"session_id":s.raw,"parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"text","text":prompt}]}})).await
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
            let idle_turn = s.active.lock().await.is_none();
            let background = {
                let c = s.completion.lock().unwrap();
                c.holds_background_work() || (idle_turn && c.autonomous_running())
            };
            if background {
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
            !{
                let c = s.completion.lock().unwrap();
                c.holds_background_work() || c.autonomous_running()
            },
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
        // Like ACP: a fork would be a new, unbound thread and cannot inherit
        // the environment's thread-bound grants. Never scan host stores for it.
        ensure!(
            !id.contains("::env/"),
            "unsupported_environment_fork: native forks are unavailable for isolated threads"
        );
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
        let new = isolated::session_id(
            crate::execution::current().as_ref(),
            &uuid::Uuid::new_v4().to_string(),
        );
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
/// Capabilities as the ACP Claude adapter computes them (`ClaudeAdapter`),
/// plus what the native protocol adds: input during a turn, fork at a reply.
fn caps(
    commands: &[crate::acp::NegotiatedCommand],
    fast: bool,
    isolated: bool,
) -> AgentProviderCapabilitiesDto {
    use crate::acp::HarnessAdapter;
    let negotiated = crate::acp::NegotiatedCaps {
        load_session: true,
        resume: true,
        list: true,
        close: true,
        fork: true,
        steer: true,
        compact: true,
        fast,
        image: true,
        available_commands: commands.to_vec(),
        ..Default::default()
    };
    let mut caps = AgentProviderCapabilitiesDto::conversational();
    crate::acp::ClaudeAdapter.patch_capabilities(&mut caps, &negotiated);
    caps.turns.stream_input = true;
    caps.turns.compact = true;
    caps.branching.fork_at = true;
    caps.branching.resume_at = true;
    caps.sessions.import_local = true;
    // Live MCP server state from Claude itself (`mcp_status`); ACP has none.
    caps.management.mcp_status = true;
    crate::acp::runtime::isolated_capabilities::restrict(&mut caps, isolated);
    caps
}

/// Claude's `mcp_status` as `ThreadMcpServersDto`, plus each server's live
/// state, which ACP never reports. Configs (URLs, headers, commands) stay out.
fn mcp_servers(status: &Value) -> Value {
    let servers: Vec<Value> = status["mcpServers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|server| {
            let state = server["status"].as_str().unwrap_or("pending");
            let auth = match (state, server["config"]["type"].as_str()) {
                ("needs-auth", _) => "notLoggedIn",
                (_, Some("claudeai-proxy")) => "oAuth",
                (_, Some("http" | "sse")) if server["config"]["headers"].is_object() => {
                    "bearerToken"
                }
                _ => "unsupported",
            };
            json!({
                "name": server["name"],
                "authStatus": auth,
                "status": match state { "needs-auth" => "needsAuth", other => other },
                "error": server["error"],
                "scope": server["scope"],
                "tools": server["tools"].as_array().into_iter().flatten().map(|t| json!({
                    "name": t["name"],
                    "title": t["annotations"]["title"],
                    "description": t["description"],
                })).collect::<Vec<_>>(),
                // Claude does not count resources.
                "resourceCount": null,
                "resourceTemplateCount": null,
            })
        })
        .collect();
    json!({ "servers": servers })
}

/// The toolbox: product actions plus Claude's slash commands and skills.
fn toolbox(
    caps: &AgentProviderCapabilitiesDto,
    commands: &[crate::acp::NegotiatedCommand],
) -> Vec<ToolboxItemDto> {
    use crate::acp::HarnessAdapter;
    let negotiated = crate::acp::NegotiatedCaps {
        available_commands: commands.to_vec(),
        ..Default::default()
    };
    crate::acp::ClaudeAdapter.toolbox_items(caps, &negotiated)
}

/// How long a stopped turn may take to settle before its process is ended.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// Claude answered the prompt with an error result. The process stays usable.
#[derive(Debug)]
struct TurnResultError(String);
impl std::fmt::Display for TurnResultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for TurnResultError {}

/// The message a failed result carries, as claude-agent-acp reports it: the
/// result text for an API error, otherwise the error list or the subtype.
fn result_error(result: &Value) -> String {
    let text = result["result"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let errors = result["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .filter_map(Value::as_str)
                .filter(|e| !e.starts_with("[ede_diagnostic]"))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|e| !e.is_empty());
    let detail = text
        .map(str::to_owned)
        .or(errors)
        .unwrap_or_else(|| result["subtype"].as_str().unwrap_or("error").to_owned());
    format!("Claude turn failed: {detail}")
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
