use std::future::Future;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pockymoe_protocol::{
    AgentBackendDto, AgentCapabilitySnapshotDto, AgentProviderCapabilitiesDto, HarnessBusyReason,
    HarnessLoadReason, ModelOptionDto, Provider, ThreadActionRequestDto, ThreadEventEnvelope,
    ThreadHistoryItemDto, ThreadSubagentDto, ThreadTurnDto, ToolboxItemDto,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// Why a loaded harness was released (`thread.runtime.released`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReleaseReason {
    View,
    Idle,
    Limit,
    Explicit,
    Deleted,
    Closed,
}

/// The thread a caller loads a harness for, and why. A runtime records it on
/// any session it starts inside [`with_load_intent`].
#[derive(Debug, Clone)]
pub struct LoadIntent {
    pub thread_id: String,
    pub reason: HarnessLoadReason,
    pub requested_at: DateTime<Utc>,
}

tokio::task_local! { static LOAD_INTENT: LoadIntent; }

pub async fn with_load_intent<T>(
    thread_id: &str,
    reason: HarnessLoadReason,
    future: impl Future<Output = T>,
) -> T {
    let intent = LoadIntent {
        thread_id: thread_id.into(),
        reason,
        requested_at: Utc::now(),
    };
    LOAD_INTENT.scope(intent, future).await
}

pub fn load_intent() -> Option<LoadIntent> {
    LOAD_INTENT.try_with(Clone::clone).ok()
}

/// A session that currently holds a harness process.
#[derive(Debug, Clone)]
pub struct LoadedSession {
    pub session_id: String,
    /// Known when the load ran inside [`with_load_intent`]; a fork has none yet.
    pub thread_id: Option<String>,
    /// Sessions with the same value share one process.
    pub process_id: String,
    pub root_pid: Option<u32>,
    pub load_reason: HarnessLoadReason,
    /// When the caller asked for the load; `loaded_at` is when it finished.
    pub requested_at: DateTime<Utc>,
    pub loaded_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub prompted: bool,
    /// Processes (PID, start time) in the tree just before the first prompt:
    /// the harness itself and its MCP servers, never tool commands.
    pub baseline: Vec<(u32, Option<u64>)>,
    /// What the runtime itself can see: turns, steers, harness background
    /// tasks and terminals, subagents of the running turn, active goals.
    pub busy: Vec<HarnessBusyReason>,
}

#[derive(Debug, Clone)]
pub enum SessionLifecycle {
    Loaded {
        session_id: String,
        thread_id: Option<String>,
        reason: HarnessLoadReason,
        requested_at: DateTime<Utc>,
    },
    /// A session ended outside [`AgentRuntime::release_loaded`], such as a
    /// harness restart from Settings.
    Released {
        session_id: String,
        thread_id: Option<String>,
        reason: ReleaseReason,
        load_reason: HarnessLoadReason,
        prompted: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseScope {
    /// The session and every session that shares its process.
    Process,
    /// Only this session. A process it shares with others keeps running.
    Session,
}

/// Release refused because an affected session is running a turn.
#[derive(Debug, thiserror::Error)]
#[error("conflict: A turn is running; interrupt it before releasing the harness")]
pub struct TurnRunning;

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<ThreadEventEnvelope>,
    persister: Arc<RwLock<Option<Arc<EventPersister>>>>,
    scope: Option<Arc<EventScope>>,
}

struct EventScope {
    thread_id: String,
    turn_id: String,
    open: std::sync::atomic::AtomicBool,
}

type EventPersister = dyn Fn(&mut ThreadEventEnvelope) -> Result<()> + Send + Sync;

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(2048);
        Self {
            tx,
            persister: Arc::new(RwLock::new(None)),
            scope: None,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ThreadEventEnvelope> {
        self.tx.subscribe()
    }

    pub fn for_turn(&self, thread_id: &str, turn_id: &str) -> Self {
        let mut bus = self.clone();
        bus.scope = Some(Arc::new(EventScope {
            thread_id: thread_id.into(),
            turn_id: turn_id.into(),
            open: std::sync::atomic::AtomicBool::new(true),
        }));
        bus
    }

    pub fn close_turn(&self) {
        if let Some(scope) = &self.scope {
            scope.open.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    pub fn emit(&self, mut event: ThreadEventEnvelope) {
        if let Some(scope) = &self.scope {
            if !scope.open.load(std::sync::atomic::Ordering::SeqCst)
                || event.thread_id != scope.thread_id
                || event
                    .payload
                    .get("turnId")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|id| id != scope.turn_id)
            {
                return;
            }
        }
        // Persistence must finish before a websocket can expose the update. The
        // callback may emit a derived event (token usage), so release the lock first.
        let persister = self.persister.read().unwrap().clone();
        if let Some(persist) = persister {
            if let Err(error) = persist(&mut event) {
                tracing::error!(
                    %error,
                    event_type = %event.event_type,
                    thread_id = %event.thread_id,
                    "failed to persist runtime event before broadcast"
                );
                // Never expose output that a subsequent snapshot cannot recover.
                let _ = self.tx.send(ThreadEventEnvelope {
                    event_type: "thread.persistence.failed".into(),
                    thread_id: event.thread_id,
                    timestamp: event.timestamp,
                    payload: serde_json::json!({"message":"Output could not be saved. Live updates are paused until storage recovers."}),
                });
                return;
            }
        }
        let completed = event.event_type == "thread.turn.completed";
        let _ = self.tx.send(event);
        if completed {
            self.close_turn();
        }
    }

    pub(crate) fn set_persister(&self, persister: Arc<EventPersister>) {
        *self.persister.write().unwrap() = Some(persister);
    }
}

#[derive(Debug, Clone)]
pub struct StartSessionInput {
    pub cwd: String,
    pub agent_id: Option<String>,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub approval_mode: String,
    pub sandbox_mode: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StartSessionResult {
    pub provider_session_id: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StartTurnInput {
    pub provider_session_id: String,
    pub prompt: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub sandbox_mode: Option<String>,
    pub collaboration_mode: Option<String>,
    pub approval_mode: Option<String>,
    pub performance_mode: Option<bool>,
    pub thread_id: String,
    pub turn_id: String,
    pub hidden: bool,
    pub images: Vec<PromptImage>,
    /// The thread's title, for harnesses that list their own sessions (DSH).
    pub title: Option<String>,
    /// This harness session already received the Pockymoe context (it is
    /// in its history), so a resumed or restarted process must not add it again.
    pub context_delivered: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SessionSettings {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub sandbox_mode: Option<String>,
    pub collaboration_mode: Option<String>,
    pub approval_mode: Option<String>,
    pub performance_mode: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct PromptImage {
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Default)]
pub struct GoalState {
    pub objective: String,
    pub status: String,
    pub tokens_used: u32,
    pub time_used_seconds: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ImportSessionMeta {
    pub session_id: String,
    pub agent_id: String,
    pub cwd: String,
    pub title: String,
    pub preview: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub model: Option<String>,
    pub turns: Vec<ThreadTurnDto>,
}

/// Observed on the backend-owned connection, never inferred from transcript age.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionState {
    Running { turn_id: String },
    Idle,
    Unknown,
}

/// The transport ended without a protocol completion. Do not call this a model failure.
#[derive(Debug, thiserror::Error)]
#[error("Backend connection lost before completion was confirmed: {0}")]
pub struct ExecutionUncertain(pub String);

#[async_trait]
pub trait AgentRuntime: Send + Sync {
    fn provider(&self) -> Provider;
    async fn execution_state(&self, _session_id: &str) -> ExecutionState {
        ExecutionState::Unknown
    }
    async fn active_subagents(&self, _session_id: &str) -> Vec<ThreadSubagentDto> {
        Vec::new()
    }
    fn descriptor(&self) -> AgentBackendDto;
    async fn start(&self) -> Result<()>;
    async fn invalidate_models(&self, _agent_id: &str) {}
    async fn restart(&self, _agent_id: &str) -> Result<usize> {
        self.start().await?;
        Ok(0)
    }
    async fn list_models(
        &self,
        agent_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<Vec<ModelOptionDto>>;
    async fn list_agents(&self) -> Result<Vec<ModelOptionDto>>;
    async fn capabilities(&self, agent_id: Option<&str>) -> Result<AgentCapabilitySnapshotDto>;
    async fn session_capabilities(
        &self,
        agent_id: Option<&str>,
        _session_id: &str,
    ) -> Result<AgentCapabilitySnapshotDto> {
        self.capabilities(agent_id).await
    }
    fn negotiated_caps(&self, _agent_id: Option<&str>) -> AgentProviderCapabilitiesDto {
        AgentProviderCapabilitiesDto::conversational()
    }
    async fn start_session(&self, input: StartSessionInput) -> Result<StartSessionResult>;
    async fn resume_session(
        &self,
        session_id: &str,
        cwd: Option<&str>,
        settings: SessionSettings,
    ) -> Result<StartSessionResult>;
    async fn start_turn(
        &self,
        input: StartTurnInput,
        bus: EventBus,
        cancel: CancellationToken,
    ) -> Result<Vec<ThreadHistoryItemDto>>;
    async fn interrupt(&self, session_id: &str, turn_id: &str) -> Result<()>;
    /// Release an idle session's owned process during explicit thread cleanup.
    async fn release_session(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }
    async fn loaded_sessions(&self) -> Vec<LoadedSession> {
        Vec::new()
    }
    /// End a loaded session: send `session/close` when the adapter supports
    /// it, then stop its process tree. Fails with [`TurnRunning`] while an
    /// affected session runs a turn, and with a conflict when `process_id` no
    /// longer serves the session. Returns the sessions that were removed.
    async fn release_loaded(
        &self,
        _session_id: &str,
        _scope: ReleaseScope,
        _process_id: Option<&str>,
    ) -> Result<Vec<LoadedSession>> {
        Ok(Vec::new())
    }
    /// Record activity the runtime did not see, such as a scheduled turn.
    async fn touch_session(&self, _session_id: &str) {}
    fn lifecycle(&self) -> Option<broadcast::Receiver<SessionLifecycle>> {
        None
    }
    async fn respond_permission(
        &self,
        request_id: &str,
        allow: bool,
        answer: Option<&str>,
    ) -> Result<()>;
    async fn pending_requests(&self, _thread_id: &str) -> Vec<ThreadActionRequestDto> {
        Vec::new()
    }
    async fn compact_session(
        &self,
        _session_id: &str,
        _thread_id: &str,
        _bus: EventBus,
    ) -> Result<()> {
        anyhow::bail!("compact is not supported by this harness");
    }
    async fn fork_session(&self, _session_id: &str) -> Result<StartSessionResult> {
        anyhow::bail!("fork is not supported by this harness");
    }
    async fn fork_session_at(
        &self,
        session_id: &str,
        rollback_count: u32,
    ) -> Result<StartSessionResult> {
        if rollback_count != 0 {
            anyhow::bail!("conflict: This backend supports latest-session fork only.");
        }
        self.fork_session(session_id).await
    }
    async fn send_input(&self, _session_id: &str, _turn_id: &str, _prompt: &str) -> Result<()> {
        anyhow::bail!("steering is not supported by this harness");
    }
    /// Harness-owned goal commands that must run inside a tracked prompt turn.
    async fn stage_goal(&self, _session_id: &str, _goal: GoalState) -> Result<()> {
        Ok(())
    }
    async fn goal_prompt(&self, _session_id: &str, _argument: &str) -> Result<Option<String>> {
        Ok(None)
    }
    async fn get_goal(&self, _session_id: &str) -> Result<Option<GoalState>> {
        Ok(None)
    }
    async fn set_goal(
        &self,
        _session_id: &str,
        _objective: Option<String>,
        _status: Option<String>,
    ) -> Result<Option<GoalState>> {
        anyhow::bail!("goals are not supported by this harness");
    }
    fn toolbox(&self, agent_id: Option<&str>) -> Vec<ToolboxItemDto> {
        pockymoe_protocol::toolbox_from_capabilities(&self.negotiated_caps(agent_id))
    }
    /// Harness-specific options for thread creation (DSH run modes), probing
    /// in `cwd` when nothing is cached. Null when the harness has none.
    async fn harness_catalog(
        &self,
        _agent_id: Option<&str>,
        _cwd: Option<&str>,
    ) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    /// A typed harness-panel action. Adapters accept only their own action
    /// kinds; this is never a raw passthrough to the harness.
    async fn harness_action(
        &self,
        _session_id: &str,
        _action: serde_json::Value,
    ) -> Result<serde_json::Value> {
        anyhow::bail!("this harness has no settings panel actions");
    }
    async fn apply_session_settings(
        &self,
        _session_id: &str,
        _settings: SessionSettings,
    ) -> Result<()> {
        Ok(())
    }
    async fn install(&self, _agent_id: Option<&str>) -> Result<AgentBackendDto> {
        Ok(self.descriptor())
    }
    async fn list_import_sessions(
        &self,
        _agent_id: Option<&str>,
    ) -> Result<Vec<ImportSessionMeta>> {
        Ok(Vec::new())
    }
    async fn resolve_import_session(
        &self,
        agent_id: Option<&str>,
        session_id: &str,
    ) -> Result<Option<ImportSessionMeta>> {
        Ok(self
            .list_import_sessions(agent_id)
            .await?
            .into_iter()
            .find(|session| crate::import_id::session_ids_match(&session.session_id, session_id)))
    }
    fn session_loaded(&self, _session_id: &str) -> bool {
        false
    }
    async fn session_instance_id(&self, _session_id: &str) -> Option<String> {
        None
    }
    async fn session_instance_started_at(&self, _session_id: &str) -> Option<String> {
        None
    }
}

pub type SharedRuntime = Arc<dyn AgentRuntime>;

#[cfg(test)]
mod reliability_tests {
    use super::*;
    use serde_json::json;
    fn event(kind: &str) -> ThreadEventEnvelope {
        ThreadEventEnvelope {
            event_type: kind.into(),
            thread_id: "thread".into(),
            timestamp: "now".into(),
            payload: json!({"turnId":"turn"}),
        }
    }
    #[test]
    fn failed_persistence_is_visible_without_broadcasting_unsaved_output() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.set_persister(Arc::new(|_| anyhow::bail!("disk full")));
        bus.emit(event("thread.output.delta"));
        assert_eq!(
            rx.try_recv().unwrap().event_type,
            "thread.persistence.failed"
        );
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn completion_fences_late_events_from_the_previous_turn() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let scoped = bus.for_turn("thread", "turn");
        scoped.emit(event("thread.turn.completed"));
        assert_eq!(rx.try_recv().unwrap().event_type, "thread.turn.completed");
        scoped.emit(event("thread.output.delta"));
        assert!(rx.try_recv().is_err());
        let mut wrong = event("thread.output.delta");
        wrong.payload["turnId"] = json!("other");
        bus.for_turn("thread", "turn").emit(wrong);
        assert!(rx.try_recv().is_err());
    }
}
