use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{bail, Result};
use async_trait::async_trait;
use remote_codex_protocol::{
    now_rfc3339, toolbox_from_capabilities, AgentBackendDto, AgentBackendInstallationDto,
    AgentBackendManagementSchemaDto, AgentCapabilitySnapshotDto, AgentProviderCapabilitiesDto,
    AgentRuntimeStatusDto, ModelOptionDto, Provider, ReasoningEffortOptionDto,
    ThreadActionRequestDto, ThreadEventEnvelope, ThreadHistoryItemDto,
};
use serde_json::json;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::acp::{adapter_for, NegotiatedCaps};
use crate::actor::{
    AgentRuntime, EventBus, GoalState, ImportSessionMeta, SessionSettings, StartSessionInput,
    StartSessionResult, StartTurnInput,
};
use crate::import_id::session_ids_match;

const HOLD_PROMPT: &str = "[remote-codex fixture: hold until controlled]";

#[derive(Clone)]
struct FixtureController {
    turn_id: String,
    tx: tokio::sync::mpsc::UnboundedSender<FixtureCommand>,
}

enum FixtureCommand {
    Complete(String),
    Fail(String),
    Request(String, String, tokio::sync::oneshot::Sender<()>),
}

fn fixture_controllers() -> &'static Mutex<HashMap<String, FixtureController>> {
    static CONTROLLERS: OnceLock<Mutex<HashMap<String, FixtureController>>> = OnceLock::new();
    CONTROLLERS.get_or_init(Default::default)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureEventInput {
    pub r#type: String,
    pub thread_id: String,
    pub text: Option<String>,
}

impl crate::Supervisor {
    /// Only the fake-mode HTTP router exposes this control. Turn admission and
    /// settlement still use the ordinary durable prompt/runtime paths.
    pub async fn fixture_event(
        self: &Arc<Self>,
        input: FixtureEventInput,
    ) -> Result<serde_json::Value> {
        anyhow::ensure!(
            self.config.fake_runtime,
            "fixture controls require fake runtime"
        );
        anyhow::ensure!(
            matches!(
                input.r#type.as_str(),
                "start" | "complete" | "fail" | "approval" | "input"
            ),
            "invalid fixture event type"
        );
        self.get_thread(&input.thread_id)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        if !fixture_controllers()
            .lock()
            .unwrap()
            .contains_key(&input.thread_id)
        {
            anyhow::ensure!(
                self.get_thread(&input.thread_id)?.active_turn_id.is_none(),
                "conflict: thread is running a non-controlled turn"
            );
            let prompt = serde_json::from_value(serde_json::json!({"prompt": HOLD_PROMPT}))?;
            self.accept_prompt(&input.thread_id, &prompt).await?;
            self.dispatch_inbox(input.thread_id.clone());
        }
        let controller = loop {
            let controller = fixture_controllers()
                .lock()
                .unwrap()
                .get(&input.thread_id)
                .cloned();
            if let Some(controller) = controller {
                break controller;
            }
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "fixture turn did not start"
            );
            sleep(Duration::from_millis(2)).await;
        };
        let text = input.text.unwrap_or_else(|| match input.r#type.as_str() {
            "fail" => "Fixture: dependency registry unavailable; retry after reconnect.".into(),
            "approval" => "Run the focused regression checks?".into(),
            "input" => "Which rollout environment should be used?".into(),
            _ => "Implemented the change and verified the focused regression checks.".into(),
        });
        match input.r#type.as_str() {
            "start" => {}
            "approval" | "input" => {
                let (ack, ready) = tokio::sync::oneshot::channel();
                controller
                    .tx
                    .send(FixtureCommand::Request(input.r#type.clone(), text, ack))?;
                tokio::time::timeout_at(deadline, ready).await??;
            }
            _ => {
                let command = if input.r#type == "fail" {
                    FixtureCommand::Fail(text.clone())
                } else {
                    FixtureCommand::Complete(text.clone())
                };
                controller.tx.send(command)?;
                loop {
                    let thread = self.get_thread(&input.thread_id)?;
                    if thread.active_turn_id.is_none()
                        && thread.status != "running"
                        && !fixture_controllers()
                            .lock()
                            .unwrap()
                            .contains_key(&input.thread_id)
                    {
                        break;
                    }
                    anyhow::ensure!(
                        tokio::time::Instant::now() < deadline,
                        "fixture turn did not settle"
                    );
                    sleep(Duration::from_millis(2)).await;
                }
                if input.r#type == "fail" {
                    emit(
                        &self.bus,
                        &input.thread_id,
                        "thread.turn.failed",
                        json!({
                            "turnId": controller.turn_id, "status": "failed", "error": text
                        }),
                    );
                }
            }
        }
        Ok(json!({"threadId":input.thread_id,"turnId":controller.turn_id,"type":input.r#type}))
    }
}

pub struct FakeRuntime {
    provider: Provider,
    active: Mutex<HashMap<String, String>>,
    sessions: Mutex<HashMap<String, String>>,
    started_at: Mutex<Option<String>>,
    goal: Mutex<Option<GoalState>>,
    import_sessions: Mutex<Vec<ImportSessionMeta>>,
    pending: Mutex<HashMap<String, Vec<ThreadActionRequestDto>>>,
}

impl FakeRuntime {
    pub fn new(provider: Provider) -> Self {
        Self {
            provider,
            active: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            started_at: Mutex::new(None),
            goal: Mutex::new(None),
            import_sessions: Mutex::new(Vec::new()),
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn seed_import_session(&self, session: ImportSessionMeta) {
        self.import_sessions.lock().unwrap().push(session);
    }

    async fn controlled_turn(
        &self,
        input: &StartTurnInput,
        bus: &EventBus,
        cancel: CancellationToken,
    ) -> Result<Vec<ThreadHistoryItemDto>> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        fixture_controllers().lock().unwrap().insert(
            input.thread_id.clone(),
            FixtureController {
                turn_id: input.turn_id.clone(),
                tx,
            },
        );
        struct Cleanup<'a>(&'a FakeRuntime, String);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                fixture_controllers().lock().unwrap().remove(&self.1);
                self.0.pending.lock().unwrap().remove(&self.1);
            }
        }
        let _cleanup = Cleanup(self, input.thread_id.clone());
        emit(
            bus,
            &input.thread_id,
            "thread.turn.started",
            json!({"turnId":input.turn_id}),
        );
        let outcome = loop {
            tokio::select! {
                _ = cancel.cancelled() => break ("interrupted", String::new()),
                command = rx.recv() => match command {
                    Some(FixtureCommand::Complete(text)) => break ("completed", text),
                    Some(FixtureCommand::Fail(text)) => break ("failed", text),
                    Some(FixtureCommand::Request(kind, text, ack)) => {
                        let request = ThreadActionRequestDto {
                            id: format!("fixture-request-{}", Uuid::new_v4()),
                            kind: if kind == "input" { "requestUserInput" } else { "permissionRequest" }.into(),
                            title: text.clone(), description: Some(text),
                            turn_id: Some(input.turn_id.clone()), item_id: None,
                            created_at: now_rfc3339(),
                            questions: if kind == "input" { vec![json!({
                                "id":"environment", "header":"Environment", "question":"Which rollout environment?",
                                "options":[{"label":"Staging","description":"Validate before production."},
                                    {"label":"Preview","description":"Use an isolated preview."}]
                            })] } else { vec![] },
                        };
                        self.pending.lock().unwrap().entry(input.thread_id.clone()).or_default().push(request.clone());
                        emit(bus, &input.thread_id, "thread.request.created", json!({"request":request}));
                        let _ = ack.send(());
                    }
                    None => break ("interrupted", String::new()),
                }
            }
        };
        let requests = self
            .pending
            .lock()
            .unwrap()
            .remove(&input.thread_id)
            .unwrap_or_default();
        for request in requests {
            emit(
                bus,
                &input.thread_id,
                "thread.request.resolved",
                json!({"requestId":request.id}),
            );
        }
        let (status, text) = outcome;
        let items = if status == "completed" {
            let result = item(&format!("{}:assistant", input.turn_id), &text, Some(status));
            emit(
                bus,
                &input.thread_id,
                "thread.item.completed",
                json!({"turnId":input.turn_id,"item":result}),
            );
            vec![result]
        } else {
            vec![]
        };
        emit(
            bus,
            &input.thread_id,
            "thread.turn.completed",
            json!({
                "turnId":input.turn_id,"status":status,"error":if status == "failed" {Some(text.clone())} else {None}
            }),
        );
        if status == "failed" {
            bail!("{text}");
        }
        Ok(items)
    }

    fn agent_id(&self) -> &'static str {
        self.provider.as_str()
    }

    fn caps(&self) -> AgentProviderCapabilitiesDto {
        let mut caps = AgentProviderCapabilitiesDto::conversational();
        let negotiated = match self.provider {
            Provider::Claude => NegotiatedCaps {
                load_session: true,
                resume: true,
                close: true,
                fork: true,
                steer: true,
                image: true,
                ..NegotiatedCaps::default()
            },
            _ => NegotiatedCaps {
                load_session: true,
                resume: true,
                close: true,
                steer: true,
                image: true,
                goals: true,
                ..NegotiatedCaps::default()
            },
        };
        adapter_for(self.agent_id()).patch_capabilities(&mut caps, &negotiated);
        caps.sessions.import_local = true;
        caps
    }

    fn models() -> Vec<ModelOptionDto> {
        vec![
            ModelOptionDto {
                id: "ios-e2e-stream".into(),
                model: "ios-e2e-stream".into(),
                display_name: "E2E Stream".into(),
                description: "Deterministic streaming test model.".into(),
                is_default: true,
                hidden: false,
                supported_reasoning_efforts: efforts(),
                default_reasoning_effort: Some("medium".into()),
                selection_kind: Some("model".into()),
                acp_agent: None,
            },
            ModelOptionDto {
                id: "gpt-5.4".into(),
                model: "gpt-5.4".into(),
                display_name: "GPT-5.4".into(),
                description: "Default coding model.".into(),
                is_default: false,
                hidden: false,
                supported_reasoning_efforts: efforts(),
                default_reasoning_effort: Some("medium".into()),
                selection_kind: Some("model".into()),
                acp_agent: None,
            },
        ]
    }
}

fn efforts() -> Vec<ReasoningEffortOptionDto> {
    ["low", "medium", "high"]
        .into_iter()
        .map(|level| ReasoningEffortOptionDto {
            reasoning_effort: level.into(),
            description: level.to_string(),
        })
        .collect()
}

fn emit(bus: &EventBus, thread_id: &str, event_type: &str, payload: serde_json::Value) {
    bus.emit(ThreadEventEnvelope {
        event_type: event_type.into(),
        thread_id: thread_id.into(),
        timestamp: now_rfc3339(),
        payload,
    });
}

#[async_trait]
impl AgentRuntime for FakeRuntime {
    fn provider(&self) -> Provider {
        self.provider
    }

    fn descriptor(&self) -> AgentBackendDto {
        let started = self.started_at.lock().unwrap().clone();
        let caps = self.caps();
        let mut schema = AgentBackendManagementSchemaDto::default();
        schema.toolbox_items = toolbox_from_capabilities(&caps);
        AgentBackendDto {
            provider: self.provider,
            display_name: format!("Fake {}", self.provider.as_str()),
            description: "Deterministic runtime for local tests.".into(),
            enabled: true,
            is_default: self.provider == Provider::Codex,
            status: AgentRuntimeStatusDto {
                state: "ready".into(),
                transport: "none".into(),
                last_started_at: started,
                last_error: None,
                restart_count: 0,
            },
            capabilities: caps,
            management_schema: schema,
            installation: AgentBackendInstallationDto {
                package_name: Some("remote-codex-e2e-fake-runtime".into()),
                installed: true,
                installed_version: Some("test".into()),
                latest_version: None,
                install_command: None,
                update_command: None,
                busy: false,
                last_error: None,
            },
        }
    }

    fn negotiated_caps(&self, _agent_id: Option<&str>) -> AgentProviderCapabilitiesDto {
        self.caps()
    }

    async fn start(&self) -> Result<()> {
        *self.started_at.lock().unwrap() = Some(now_rfc3339());
        Ok(())
    }

    async fn list_models(
        &self,
        agent_id: Option<&str>,
        _cwd: Option<&str>,
    ) -> Result<Vec<ModelOptionDto>> {
        let mut models = Self::models();
        if self.provider == Provider::Claude || agent_id == Some("claude") {
            let mut opus = models[0].clone();
            opus.id = "claude-opus-5-5".into();
            opus.model = opus.id.clone();
            opus.display_name = "Opus 5.5".into();
            let mut sonnet = opus.clone();
            sonnet.id = "claude-sonnet-5-5".into();
            sonnet.model = sonnet.id.clone();
            sonnet.display_name = "Sonnet 5.5".into();
            models.retain(|model| !model.id.starts_with("gpt"));
            models.extend([opus, sonnet]);
        }
        Ok(models)
    }

    async fn list_agents(&self) -> Result<Vec<ModelOptionDto>> {
        Ok(vec![])
    }

    async fn capabilities(&self, agent_id: Option<&str>) -> Result<AgentCapabilitySnapshotDto> {
        Ok(AgentCapabilitySnapshotDto {
            provider: self.provider,
            agent_id: agent_id.unwrap_or(self.agent_id()).into(),
            availability: "ready".into(),
            negotiated: None,
            effective_capabilities: Some(self.caps()),
            toolbox_items: self.toolbox(agent_id),
        })
    }

    async fn start_session(&self, input: StartSessionInput) -> Result<StartSessionResult> {
        let id = format!("e2e-session-{}", Uuid::new_v4());
        self.sessions.lock().unwrap().insert(id.clone(), input.cwd);
        Ok(StartSessionResult {
            provider_session_id: id,
            model: Some(input.model),
            reasoning_effort: input.reasoning_effort,
        })
    }

    async fn resume_session(
        &self,
        session_id: &str,
        cwd: Option<&str>,
        _settings: SessionSettings,
    ) -> Result<StartSessionResult> {
        let mut sessions = self.sessions.lock().unwrap();
        if !sessions.contains_key(session_id) {
            sessions.insert(session_id.into(), cwd.unwrap_or(".").into());
        }
        Ok(StartSessionResult {
            provider_session_id: session_id.into(),
            model: None,
            reasoning_effort: None,
        })
    }

    async fn execution_state(&self, session: &str) -> crate::actor::ExecutionState {
        if let Some(turn) = self.active.lock().unwrap().get(session) {
            return crate::actor::ExecutionState::Running {
                turn_id: turn.clone(),
            };
        }
        if self.sessions.lock().unwrap().contains_key(session) {
            crate::actor::ExecutionState::Idle
        } else {
            crate::actor::ExecutionState::Unknown
        }
    }

    async fn start_turn(
        &self,
        input: StartTurnInput,
        bus: EventBus,
        cancel: CancellationToken,
    ) -> Result<Vec<ThreadHistoryItemDto>> {
        if input.hidden {
            return Ok(vec![]);
        }
        self.active
            .lock()
            .unwrap()
            .insert(input.provider_session_id.clone(), input.turn_id.clone());
        struct ActiveGuard<'a>(&'a Mutex<HashMap<String, String>>, String);
        impl Drop for ActiveGuard<'_> {
            fn drop(&mut self) {
                self.0.lock().unwrap().remove(&self.1);
            }
        }
        let _active = ActiveGuard(&self.active, input.provider_session_id.clone());
        if input.prompt.ends_with(HOLD_PROMPT) {
            return self.controlled_turn(&input, &bus, cancel).await;
        }
        if let Some(argument) = input.prompt.strip_prefix("/goal ") {
            self.set_goal(
                &input.provider_session_id,
                (argument != "resume").then(|| argument.to_string()),
                Some("active".into()),
            )
            .await?;
        }
        let reply = reply_for(&input.prompt);
        let item_id = format!("{}:assistant", input.turn_id);
        emit(
            &bus,
            &input.thread_id,
            "thread.turn.started",
            json!({ "turnId": input.turn_id }),
        );
        let slow = input.prompt.len() > 180
            || input
                .prompt
                .to_lowercase()
                .contains("inspect this repository");
        if slow {
            tokio::select! {
                _ = cancel.cancelled() => {
                    emit(&bus, &input.thread_id, "thread.turn.completed", json!({
                        "turnId": input.turn_id,
                        "status": "interrupted",
                        "error": null
                    }));
                    return Ok(vec![item(&item_id, "", Some("interrupted"))]);
                }
                _ = sleep(Duration::from_secs(25)) => {}
            }
        } else {
            for (i, ch) in reply.chars().enumerate() {
                if cancel.is_cancelled() {
                    emit(
                        &bus,
                        &input.thread_id,
                        "thread.turn.completed",
                        json!({
                            "turnId": input.turn_id,
                            "status": "interrupted",
                            "error": null
                        }),
                    );
                    return Ok(vec![item(&item_id, "", Some("interrupted"))]);
                }
                emit(
                    &bus,
                    &input.thread_id,
                    "thread.output.delta",
                    json!({
                        "turnId": input.turn_id,
                        "itemId": item_id,
                        "sequence": i,
                        "delta": ch.to_string(),
                    }),
                );
                sleep(Duration::from_millis(4)).await;
            }
        }
        let final_item = item(&item_id, &reply, Some("completed"));
        emit(
            &bus,
            &input.thread_id,
            "thread.item.completed",
            json!({ "turnId": input.turn_id, "item": final_item }),
        );
        emit(
            &bus,
            &input.thread_id,
            "thread.turn.completed",
            json!({
                "turnId": input.turn_id,
                "status": "completed",
                "error": null
            }),
        );
        Ok(vec![final_item])
    }

    async fn interrupt(&self, _session_id: &str, _turn_id: &str) -> Result<()> {
        Ok(())
    }

    async fn send_input(&self, _session_id: &str, _turn_id: &str, _prompt: &str) -> Result<()> {
        Ok(())
    }

    async fn respond_permission(
        &self,
        request_id: &str,
        _allow: bool,
        _answer: Option<&str>,
    ) -> Result<()> {
        for requests in self.pending.lock().unwrap().values_mut() {
            requests.retain(|request| request.id != request_id);
        }
        Ok(())
    }

    async fn pending_requests(&self, thread_id: &str) -> Vec<ThreadActionRequestDto> {
        self.pending
            .lock()
            .unwrap()
            .get(thread_id)
            .cloned()
            .unwrap_or_default()
    }

    async fn compact_session(
        &self,
        _session_id: &str,
        _thread_id: &str,
        _bus: EventBus,
    ) -> Result<()> {
        if adapter_for(self.agent_id()).compact_prompt().is_none() {
            bail!("this harness does not implement compact");
        }
        Ok(())
    }

    async fn fork_session(&self, session_id: &str) -> Result<StartSessionResult> {
        if !self.caps().branching.fork {
            bail!("this harness does not support session/fork");
        }
        let id = format!("{session_id}-fork-{}", Uuid::new_v4());
        self.sessions
            .lock()
            .unwrap()
            .insert(id.clone(), "fork".into());
        Ok(StartSessionResult {
            provider_session_id: id,
            model: None,
            reasoning_effort: None,
        })
    }

    async fn stage_goal(&self, _session_id: &str, goal: GoalState) -> Result<()> {
        *self.goal.lock().unwrap() = Some(goal);
        Ok(())
    }

    async fn goal_prompt(&self, _session_id: &str, argument: &str) -> Result<Option<String>> {
        Ok(self
            .caps()
            .controls
            .goals
            .then(|| format!("/goal {argument}")))
    }

    async fn get_goal(&self, _session_id: &str) -> Result<Option<GoalState>> {
        Ok(self.goal.lock().unwrap().clone())
    }

    async fn set_goal(
        &self,
        _session_id: &str,
        objective: Option<String>,
        status: Option<String>,
    ) -> Result<Option<GoalState>> {
        if !self.caps().controls.goals {
            bail!("this harness does not support goals");
        }
        let mut current = self.goal.lock().unwrap();
        let next = if objective.as_ref().map(|s| s.is_empty()).unwrap_or(true)
            && status.as_deref() == Some("clear")
        {
            None
        } else if let Some(objective) = objective {
            Some(GoalState {
                objective,
                status: status.unwrap_or_else(|| "active".into()),
                tokens_used: 0,
                time_used_seconds: 0,
            })
        } else if let Some(mut goal) = current.clone() {
            if let Some(status) = status {
                goal.status = status;
            }
            Some(goal)
        } else {
            Some(GoalState {
                objective: "goal".into(),
                status: status.unwrap_or_else(|| "active".into()),
                tokens_used: 0,
                time_used_seconds: 0,
            })
        };
        *current = next.clone();
        Ok(next)
    }

    async fn list_import_sessions(&self, agent_id: Option<&str>) -> Result<Vec<ImportSessionMeta>> {
        let wanted = agent_id.unwrap_or(self.agent_id());
        Ok(self
            .import_sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|session| session.agent_id == wanted || session.agent_id == self.agent_id())
            .cloned()
            .collect())
    }

    fn session_loaded(&self, session_id: &str) -> bool {
        let sessions = self.sessions.lock().unwrap();
        sessions.contains_key(session_id)
            || sessions
                .keys()
                .any(|stored| session_ids_match(stored, session_id))
    }
}

fn reply_for(prompt: &str) -> String {
    let lower = prompt.to_lowercase();
    if lower.contains("hello") {
        "hello".into()
    } else if let Some(rest) = prompt.strip_prefix("Reply with exactly ") {
        rest.trim_end_matches('.').trim().to_string()
    } else {
        format!("ok: {}", prompt.chars().take(80).collect::<String>())
    }
}

fn item(id: &str, text: &str, status: Option<&str>) -> ThreadHistoryItemDto {
    ThreadHistoryItemDto {
        id: id.into(),
        created_at: Some(now_rfc3339()),
        kind: "agentMessage".into(),
        text: text.into(),
        preview_text: None,
        detail_text: None,
        status: status.map(str::to_string),
        sequence: None,
        source_turn_id: None,
        artifact: None,
        extra: Default::default(),
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;
    use crate::{Database, RuntimeConfig, Supervisor};

    #[tokio::test]
    async fn e2e_fixture_controls_hold_settle_and_resolve_real_requests() {
        let directory = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            mode: remote_codex_protocol::Mode::Local,
            host: "127.0.0.1".into(),
            port: 0,
            workspace_root: directory.path().into(),
            database_url: directory.path().join("fixture.sqlite"),
            app_name: "test".into(),
            app_version: "test".into(),
            environment: "test".into(),
            auth_required: false,
            admin_username: None,
            admin_password: None,
            session_secret: None,
            relay_server_url: None,
            relay_agent_token: None,
            enabled_providers: vec![Provider::Codex],
            acp_command: None,
            acp_startup_timeout_ms: 1000,
            fake_runtime: true,
        };
        let db = Database::open(&config.database_url).unwrap();
        let state = Arc::new(Supervisor::new(
            config,
            db,
            vec![Arc::new(FakeRuntime::new(Provider::Codex))],
        ));
        state.spawn_live_item_persister();
        let workspace = state
            .create_workspace(
                serde_json::from_value(json!({
                    "absPath":directory.path(),"label":"fixture"
                }))
                .unwrap(),
            )
            .unwrap();
        let thread = state
            .create_thread(
                serde_json::from_value(json!({
                    "workspaceId":workspace.id,"model":"ios-e2e-stream"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let mut events = state.bus.subscribe();
        for kind in ["start", "approval", "input"] {
            state
                .fixture_event(FixtureEventInput {
                    r#type: kind.into(),
                    thread_id: thread.id.clone(),
                    text: None,
                })
                .await
                .unwrap();
        }
        assert_eq!(state.get_thread(&thread.id).unwrap().status, "running");
        let requests = state
            .runtime(Provider::Codex)
            .unwrap()
            .pending_requests(&thread.id)
            .await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].kind, "permissionRequest");
        assert_eq!(requests[1].kind, "requestUserInput");
        for request in requests {
            state
                .respond_request(&thread.id, &request.id, true, Some("staging"))
                .await
                .unwrap();
        }
        assert!(state
            .runtime(Provider::Codex)
            .unwrap()
            .pending_requests(&thread.id)
            .await
            .is_empty());
        state
            .fixture_event(FixtureEventInput {
                r#type: "complete".into(),
                thread_id: thread.id.clone(),
                text: None,
            })
            .await
            .unwrap();
        assert_eq!(state.get_thread(&thread.id).unwrap().status, "idle");
        state
            .fixture_event(FixtureEventInput {
                r#type: "fail".into(),
                thread_id: thread.id.clone(),
                text: Some("test failure".into()),
            })
            .await
            .unwrap();
        let failed = state.get_thread(&thread.id).unwrap();
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.last_error.as_deref(), Some("test failure"));
        let mut types = Vec::new();
        while let Ok(event) = events.try_recv() {
            types.push(event.event_type);
        }
        for kind in [
            "thread.turn.started",
            "thread.turn.completed",
            "thread.turn.failed",
            "thread.request.created",
            "thread.request.resolved",
        ] {
            assert!(types.iter().any(|event| event == kind), "missing {kind}");
        }
    }
}
