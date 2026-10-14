use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{bail, Result};
use async_trait::async_trait;
use pockymoe_protocol::{
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
    AgentRuntime, EventBus, GoalState, ImportSessionMeta, LoadedSession, ReleaseScope,
    SessionLifecycle, SessionSettings, StartSessionInput, StartSessionResult, StartTurnInput,
    TurnRunning,
};
use crate::import_id::session_ids_match;
use pockymoe_protocol::{HarnessBusyReason, HarnessLoadReason};

pub struct FakeRuntime {
    provider: Provider,
    active: Mutex<HashMap<String, String>>,
    sessions: Mutex<HashMap<String, String>>,
    /// What each loaded session would report as a process-backed harness.
    loads: Mutex<HashMap<String, LoadedSession>>,
    lifecycle: tokio::sync::broadcast::Sender<SessionLifecycle>,
    /// Every `session/new`, for tests of the settings a session starts with.
    starts: Mutex<Vec<StartSessionInput>>,
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
            loads: Mutex::new(HashMap::new()),
            lifecycle: tokio::sync::broadcast::channel(64).0,
            starts: Mutex::new(Vec::new()),
            started_at: Mutex::new(None),
            goal: Mutex::new(None),
            import_sessions: Mutex::new(Vec::new()),
            pending: Default::default(),
        }
    }

    fn record_load(&self, session_id: &str, reason: Option<HarnessLoadReason>) {
        let now = chrono::Utc::now();
        let intent = crate::actor::load_intent();
        let session = LoadedSession {
            session_id: session_id.into(),
            thread_id: intent.as_ref().map(|intent| intent.thread_id.clone()),
            process_id: Uuid::new_v4().to_string(),
            root_pid: None,
            load_reason: reason.unwrap_or_else(|| {
                intent
                    .as_ref()
                    .map_or(HarnessLoadReason::Other, |intent| intent.reason)
            }),
            requested_at: intent.map_or(now, |intent| intent.requested_at),
            loaded_at: now,
            last_activity_at: now,
            prompted: false,
            baseline: Vec::new(),
            busy: Vec::new(),
        };
        let _ = self.lifecycle.send(SessionLifecycle::Loaded {
            session_id: session.session_id.clone(),
            thread_id: session.thread_id.clone(),
            reason: session.load_reason,
            requested_at: session.requested_at,
        });
        self.loads
            .lock()
            .unwrap()
            .insert(session_id.into(), session);
    }

    pub fn seed_pending_requests(&self, thread: &str, requests: Vec<ThreadActionRequestDto>) {
        self.pending.lock().unwrap().insert(thread.into(), requests);
    }

    /// Simulate a dead process while its in-flight request still awaits cancellation.
    pub fn simulate_process_exit(&self) {
        self.sessions.lock().unwrap().clear();
        self.loads.lock().unwrap().clear();
    }

    pub fn started_sessions(&self) -> Vec<StartSessionInput> {
        self.starts.lock().unwrap().clone()
    }

    /// Serve `fork` from `parent`'s process, as Claude and Grok forks are.
    pub fn share_process(&self, parent: &str, fork: &str) {
        let mut loads = self.loads.lock().unwrap();
        if let Some(process) = loads.get(parent).map(|load| load.process_id.clone()) {
            if let Some(load) = loads.get_mut(fork) {
                load.process_id = process;
            }
        }
    }

    fn touch(&self, session_id: &str, prompted: bool) {
        if let Some(load) = self.loads.lock().unwrap().get_mut(session_id) {
            load.last_activity_at = chrono::Utc::now();
            load.prompted |= prompted;
        }
    }

    pub fn seed_import_session(&self, session: ImportSessionMeta) {
        self.import_sessions.lock().unwrap().push(session);
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

fn emit(
    bus: &EventBus,
    thread_id: &str,
    event_type: pockymoe_protocol::ThreadEventType,
    payload: serde_json::Value,
) {
    bus.emit(ThreadEventEnvelope {
        event_type,
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
                package_name: Some("pockymoe-e2e-fake-runtime".into()),
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
        self.starts.lock().unwrap().push(input.clone());
        self.sessions.lock().unwrap().insert(id.clone(), input.cwd);
        self.record_load(&id, None);
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
        let inserted = {
            let mut sessions = self.sessions.lock().unwrap();
            sessions
                .insert(session_id.into(), cwd.unwrap_or(".").into())
                .is_none()
        };
        if inserted {
            self.record_load(session_id, None);
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
        self.touch(&input.provider_session_id, true);
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
            pockymoe_protocol::ThreadEventType::ThreadTurnStarted,
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
                    emit(&bus, &input.thread_id, pockymoe_protocol::ThreadEventType::ThreadTurnCompleted, json!({
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
                        pockymoe_protocol::ThreadEventType::ThreadTurnCompleted,
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
                    pockymoe_protocol::ThreadEventType::ThreadOutputDelta,
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
            pockymoe_protocol::ThreadEventType::ThreadItemCompleted,
            json!({ "turnId": input.turn_id, "item": final_item }),
        );
        emit(
            &bus,
            &input.thread_id,
            pockymoe_protocol::ThreadEventType::ThreadTurnCompleted,
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

    async fn pending_requests(&self, thread: &str) -> Vec<ThreadActionRequestDto> {
        self.pending
            .lock()
            .unwrap()
            .get(thread)
            .cloned()
            .unwrap_or_default()
    }

    async fn respond_permission(
        &self,
        _request_id: &str,
        _allow: bool,
        _answer: Option<&str>,
    ) -> Result<()> {
        Ok(())
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
        self.record_load(&id, Some(HarnessLoadReason::Fork));
        if let Some(load) = self.loads.lock().unwrap().get_mut(&id) {
            load.thread_id = None;
        }
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

    async fn loaded_sessions(&self) -> Vec<LoadedSession> {
        let active = self.active.lock().unwrap().clone();
        let goal_active = self
            .goal
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|goal| goal.status == "active");
        self.loads
            .lock()
            .unwrap()
            .values()
            .cloned()
            .map(|mut load| {
                if active.contains_key(&load.session_id) {
                    load.busy.push(HarnessBusyReason::Turn);
                    if goal_active {
                        load.busy.push(HarnessBusyReason::Goal);
                    }
                }
                load
            })
            .collect()
    }

    async fn release_loaded(
        &self,
        session_id: &str,
        scope: ReleaseScope,
        process_id: Option<&str>,
    ) -> Result<Vec<LoadedSession>> {
        let key = self
            .sessions
            .lock()
            .unwrap()
            .keys()
            .find(|stored| session_ids_match(stored, session_id))
            .cloned();
        let Some(key) = key else {
            return Ok(Vec::new());
        };
        if self.active.lock().unwrap().contains_key(&key) {
            return Err(TurnRunning.into());
        }
        if process_id.is_some_and(|expected| {
            self.loads
                .lock()
                .unwrap()
                .get(&key)
                .is_some_and(|load| load.process_id != expected)
        }) {
            bail!("conflict: The harness was reloaded; check it again");
        }
        let mut loads = self.loads.lock().unwrap();
        let process = loads.get(&key).map(|load| load.process_id.clone());
        let keys: Vec<String> = match scope {
            ReleaseScope::Session => vec![key],
            ReleaseScope::Process => loads
                .values()
                .filter(|load| Some(&load.process_id) == process.as_ref())
                .map(|load| load.session_id.clone())
                .collect(),
        };
        if keys
            .iter()
            .any(|key| self.active.lock().unwrap().contains_key(key))
        {
            return Err(TurnRunning.into());
        }
        let mut sessions = self.sessions.lock().unwrap();
        Ok(keys
            .iter()
            .filter_map(|key| {
                sessions.remove(key);
                loads.remove(key)
            })
            .collect())
    }

    fn lifecycle(&self) -> Option<tokio::sync::broadcast::Receiver<SessionLifecycle>> {
        Some(self.lifecycle.subscribe())
    }

    async fn touch_session(&self, session_id: &str) {
        self.touch(session_id, false);
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
        kind: pockymoe_protocol::ThreadItemKind::AgentMessage,
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
