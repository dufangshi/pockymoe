//! Per-device protocol selection with backend affinity for loaded sessions.
pub mod claude;
pub mod codex;
pub(crate) mod turn_events;
use crate::actor::*;
use anyhow::Result;
use async_trait::async_trait;
use pockymoe_protocol::*;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::sync::Mutex;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// Which runtime serves a Codex or Claude session.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum HarnessProtocol {
    Acp,
    #[default]
    Native,
}
/// The agent id inside an agent key (`acp:codex` → `codex`).
fn plain(key: Option<&str>) -> Option<&str> {
    key.map(|key| pockymoe_protocol::split_agent_key(key).0)
}
fn agent_of(session: &str) -> Option<&str> {
    session
        .split_once("::")
        .filter(|(agent, raw)| !agent.is_empty() && !raw.is_empty())
        .map(|(agent, _)| agent)
}
/// Routes each thread to the protocol it was created with. Codex and Claude
/// run natively; an agent key `acp:codex` (a thread created under the ACP
/// agent) selects the ACP adapter. Both read the same session store.
pub struct ProtocolRouter {
    acp: SharedRuntime,
    bound_agent: Option<String>,
    /// The Supervisor database, read for the protocol a session's thread
    /// pinned when the session is not loaded and not routed yet.
    database: Option<PathBuf>,
    backends: HashMap<String, SharedRuntime>,
    owners: RwLock<HashMap<String, SharedRuntime>>,
    routes: RwLock<HashMap<String, HarnessProtocol>>,
    events: broadcast::Sender<SessionLifecycle>,
}
impl ProtocolRouter {
    pub fn new(acp: SharedRuntime, bound_agent: Option<String>, database: Option<PathBuf>) -> Self {
        let (events, _) = broadcast::channel(256);
        let router = Self {
            acp,
            bound_agent,
            database,
            backends: HashMap::new(),
            owners: RwLock::new(HashMap::new()),
            routes: RwLock::new(HashMap::new()),
            events,
        };
        router.forward(&router.acp);
        router
    }
    pub fn with_backend(mut self, agent: &str, backend: SharedRuntime) -> Self {
        self.forward(&backend);
        self.backends.insert(agent.into(), backend);
        self
    }
    fn forward(&self, backend: &SharedRuntime) {
        if let (Some(mut rx), Ok(handle)) =
            (backend.lifecycle(), tokio::runtime::Handle::try_current())
        {
            let tx = self.events.clone();
            handle.spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(event) => {
                            let _ = tx.send(event);
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
            });
        }
    }
    /// The runtime for an agent key and the agent id it receives.
    fn route<'a>(&self, key: Option<&'a str>) -> (SharedRuntime, Option<&'a str>, HarnessProtocol) {
        let Some(key) = key else {
            // No agent means the provider's (or the catalog's default) Codex.
            let agent = self.bound_agent.as_deref().unwrap_or("codex");
            return match self.backends.get(agent) {
                Some(backend) => (backend.clone(), None, HarnessProtocol::Native),
                None => (self.acp.clone(), None, HarnessProtocol::Acp),
            };
        };
        match pockymoe_protocol::split_agent_key(key) {
            (agent, Some("acp")) => (self.acp.clone(), Some(agent), HarnessProtocol::Acp),
            (agent, _) => match self.backends.get(agent) {
                Some(backend) => (backend.clone(), Some(agent), HarnessProtocol::Native),
                None => (self.acp.clone(), Some(key), HarnessProtocol::Acp),
            },
        }
    }
    fn remember(&self, id: &str, backend: SharedRuntime, protocol: HarnessProtocol) {
        self.owners.write().unwrap().insert(id.into(), backend);
        self.routes.write().unwrap().insert(id.into(), protocol);
    }
    fn owner(&self, id: &str) -> Option<SharedRuntime> {
        self.owners
            .read()
            .unwrap()
            .get(id)
            .filter(|backend| backend.session_loaded(id))
            .cloned()
    }
    fn pinned(&self, id: &str) -> Option<HarnessProtocol> {
        if let Some(protocol) = self.routes.read().unwrap().get(id) {
            return Some(*protocol);
        }
        let protocol = self.stored_protocol(id)?;
        self.routes.write().unwrap().insert(id.into(), protocol);
        Some(protocol)
    }
    /// The protocol the session's thread recorded; NULL (a thread older than
    /// the choice) means native.
    fn stored_protocol(&self, id: &str) -> Option<HarnessProtocol> {
        let path = self.database.as_deref()?;
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;
        let _ = conn.busy_timeout(std::time::Duration::from_secs(2));
        let stored: Option<String> = conn
            .query_row(
                "SELECT harness_protocol FROM threads WHERE provider_session_id=?1 LIMIT 1",
                [id],
                |row| row.get(0),
            )
            .ok()?;
        Some(match stored.as_deref() {
            Some("acp") => HarnessProtocol::Acp,
            _ => HarnessProtocol::Native,
        })
    }
    fn session(&self, id: &str) -> SharedRuntime {
        // Session identity takes precedence over the provider's bound agent.
        // Missing and unknown prefixes must never become a native Codex session.
        if let Some(owner) = self.owner(id) {
            return owner;
        }
        let Some(agent) = agent_of(id) else {
            return self.acp.clone();
        };
        match (self.pinned(id), self.backends.get(agent)) {
            (Some(HarnessProtocol::Acp), _) | (_, None) => self.acp.clone(),
            (_, Some(backend)) => backend.clone(),
        }
    }
    fn protocol_of(&self, backend: &SharedRuntime) -> HarnessProtocol {
        if Arc::ptr_eq(backend, &self.acp) {
            HarnessProtocol::Acp
        } else {
            HarnessProtocol::Native
        }
    }
    /// Native Codex and Claude first, then their ACP adapters keyed
    /// `acp:codex` / `acp:claude`, then the other ACP agents.
    fn agent_list(&self, acp: Vec<ModelOptionDto>) -> Vec<ModelOptionDto> {
        let (mut native, mut adapters, mut others) = (Vec::new(), Vec::new(), Vec::new());
        for mut entry in acp {
            let agent = entry.model.clone();
            let adapter = self.backends.contains_key(&agent);
            if adapter {
                native.push(native_entry(&entry));
                let key = pockymoe_protocol::agent_key(&agent, Some("acp"));
                entry.id = key.clone();
                entry.model = key;
                entry.is_default = false;
            }
            if let Some(info) = entry
                .acp_agent
                .as_mut()
                .and_then(serde_json::Value::as_object_mut)
            {
                info.insert("harnessProtocol".into(), "acp".into());
                if adapter {
                    info.insert("baseAgentId".into(), agent.into());
                }
            }
            if adapter {
                adapters.push(entry);
            } else {
                others.push(entry);
            }
        }
        // ACP Codex and ACP Claude lead the ACP agents.
        native.extend(adapters);
        native.extend(others);
        native
    }
}

/// A native agent entry made from the catalog's ACP entry: it needs the base
/// CLI only, never the ACP adapter.
fn native_entry(acp: &ModelOptionDto) -> ModelOptionDto {
    let info = acp.acp_agent.clone().unwrap_or_default();
    let base_ready = matches!(
        info["availability"].as_str(),
        Some("ready" | "adapter_missing")
    );
    let mut entry = acp.clone();
    entry.is_default = acp.model == "codex";
    entry.acp_agent = Some(serde_json::json!({
        "id": acp.model,
        "displayName": acp.display_name,
        "transport": "native",
        "harnessProtocol": "native",
        "availability": if base_ready { "ready" } else { "base_missing" },
        "baseCommand": info["baseCommand"],
        "baseProbeCommand": info["baseProbeCommand"],
        "baseVersion": info["baseVersion"],
        "basePath": info["basePath"],
        "connectionStatus": if base_ready { "unverified" } else { "unavailable" },
        "connectionError": null,
        "busy": false,
        "statusMessage": if base_ready { "Ready".to_string() } else { format!("Install the base agent first: {}", info["baseCommand"].as_str().unwrap_or("")) },
    }));
    entry
}
#[async_trait]
impl AgentRuntime for ProtocolRouter {
    fn provider(&self) -> Provider {
        self.acp.provider()
    }
    fn configure_context_inspection(&self, state: Arc<crate::context_inspection::InspectionState>) {
        self.acp.configure_context_inspection(state.clone());
        for backend in self.backends.values() {
            backend.configure_context_inspection(state.clone());
        }
    }
    async fn inspect_context(
        &self,
        session_id: &str,
        detail: &str,
    ) -> Result<Option<serde_json::Value>> {
        self.session(session_id)
            .inspect_context(session_id, detail)
            .await
    }
    async fn execution_state(&self, session_id: &str) -> ExecutionState {
        self.session(session_id).execution_state(session_id).await
    }
    async fn active_subagents(&self, session_id: &str) -> Vec<ThreadSubagentDto> {
        self.session(session_id).active_subagents(session_id).await
    }
    fn descriptor(&self) -> AgentBackendDto {
        self.acp.descriptor()
    }
    async fn start(&self) -> Result<()> {
        self.acp.start().await
    }
    async fn invalidate_models(&self, agent_id: &str) {
        let (backend, agent, _) = self.route(Some(agent_id));
        backend.invalidate_models(agent.unwrap_or(agent_id)).await
    }
    async fn restart(&self, agent_id: &str) -> Result<usize> {
        let agent_id = pockymoe_protocol::split_agent_key(agent_id).0;
        let mut count = self.acp.restart(agent_id).await?;
        if let Some(backend) = self.backends.get(agent_id) {
            count += backend.restart(agent_id).await?;
        }
        Ok(count)
    }
    async fn list_models(
        &self,
        agent_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<Vec<ModelOptionDto>> {
        let (backend, agent, _) = self.route(agent_id);
        backend.list_models(agent, cwd).await
    }
    async fn session_models(
        &self,
        agent_id: Option<&str>,
        session_id: &str,
        cwd: Option<&str>,
    ) -> Result<Vec<ModelOptionDto>> {
        self.session(session_id)
            .session_models(plain(agent_id), session_id, cwd)
            .await
    }
    async fn list_agents(&self) -> Result<Vec<ModelOptionDto>> {
        Ok(self.agent_list(self.acp.list_agents().await?))
    }
    async fn capabilities(&self, agent_id: Option<&str>) -> Result<AgentCapabilitySnapshotDto> {
        let (backend, agent, _) = self.route(agent_id);
        backend.capabilities(agent).await
    }
    async fn session_capabilities(
        &self,
        agent_id: Option<&str>,
        session_id: &str,
    ) -> Result<AgentCapabilitySnapshotDto> {
        self.session(session_id)
            .session_capabilities(plain(agent_id), session_id)
            .await
    }
    fn negotiated_caps(&self, agent_id: Option<&str>) -> AgentProviderCapabilitiesDto {
        let (backend, agent, _) = self.route(agent_id);
        backend.negotiated_caps(agent)
    }
    fn session_negotiated_caps(
        &self,
        agent_id: Option<&str>,
        session_id: &str,
    ) -> AgentProviderCapabilitiesDto {
        self.session(session_id)
            .session_negotiated_caps(plain(agent_id), session_id)
    }
    async fn start_session(&self, mut input: StartSessionInput) -> Result<StartSessionResult> {
        let key = input.agent_id.take();
        let (backend, agent, protocol) = self.route(key.as_deref());
        input.agent_id = agent.map(str::to_string);
        let result = backend.start_session(input).await?;
        self.remember(&result.provider_session_id, backend, protocol);
        Ok(result)
    }
    async fn resume_session(
        &self,
        session_id: &str,
        cwd: Option<&str>,
        settings: SessionSettings,
    ) -> Result<StartSessionResult> {
        let backend = self.session(session_id);
        let result = backend.resume_session(session_id, cwd, settings).await?;
        let protocol = self.protocol_of(&backend);
        self.remember(session_id, backend.clone(), protocol);
        self.remember(&result.provider_session_id, backend, protocol);
        Ok(result)
    }
    async fn start_turn(
        &self,
        input: StartTurnInput,
        bus: EventBus,
        cancel: CancellationToken,
    ) -> Result<Vec<ThreadHistoryItemDto>> {
        self.session(&input.provider_session_id)
            .start_turn(input, bus, cancel)
            .await
    }
    async fn interrupt(&self, session_id: &str, turn_id: &str) -> Result<()> {
        self.session(session_id)
            .interrupt(session_id, turn_id)
            .await
    }
    async fn release_session(&self, session_id: &str) -> Result<()> {
        self.session(session_id).release_session(session_id).await
    }
    async fn loaded_sessions(&self) -> Vec<LoadedSession> {
        let mut out = self.acp.loaded_sessions().await;
        for backend in self.backends.values() {
            out.extend(backend.loaded_sessions().await);
        }
        out
    }
    async fn release_loaded(
        &self,
        session_id: &str,
        scope: ReleaseScope,
        process_id: Option<&str>,
    ) -> Result<Vec<LoadedSession>> {
        self.session(session_id)
            .release_loaded(session_id, scope, process_id)
            .await
    }
    async fn touch_session(&self, session_id: &str) {
        self.session(session_id).touch_session(session_id).await
    }
    fn lifecycle(&self) -> Option<broadcast::Receiver<SessionLifecycle>> {
        Some(self.events.subscribe())
    }
    async fn respond_permission(
        &self,
        request_id: &str,
        allow: bool,
        answer: Option<&str>,
    ) -> Result<()> {
        // Native requests carry a backend prefix. ACP tolerates unknown ids,
        // so broadcasting responses would silently swallow native approvals.
        let agent = if request_id.starts_with("native-codex:") {
            Some("codex")
        } else if request_id.starts_with("claude-native:") {
            Some("claude")
        } else {
            None
        };
        if let Some(agent) = agent {
            return self
                .backends
                .get(agent)
                .ok_or_else(|| anyhow::anyhow!("Native approval backend unavailable"))?
                .respond_permission(request_id, allow, answer)
                .await;
        }
        self.acp.respond_permission(request_id, allow, answer).await
    }

    async fn pending_requests(&self, thread_id: &str) -> Vec<ThreadActionRequestDto> {
        let mut out = self.acp.pending_requests(thread_id).await;
        for backend in self.backends.values() {
            out.extend(backend.pending_requests(thread_id).await);
        }
        out
    }
    async fn compact_session(
        &self,
        session_id: &str,
        thread_id: &str,
        bus: EventBus,
    ) -> Result<()> {
        self.session(session_id)
            .compact_session(session_id, thread_id, bus)
            .await
    }
    async fn rewind_session(&self, session_id: &str, num_turns: u32) -> Result<()> {
        self.session(session_id)
            .rewind_session(session_id, num_turns)
            .await
    }
    async fn fork_session(&self, session_id: &str) -> Result<StartSessionResult> {
        let backend = self.session(session_id);
        let result = backend.fork_session(session_id).await?;
        let protocol = self.protocol_of(&backend);
        self.remember(&result.provider_session_id, backend, protocol);
        Ok(result)
    }
    async fn fork_session_at(&self, session_id: &str, point: &ForkPoint) -> Result<ForkResult> {
        let backend = self.session(session_id);
        let result = backend.fork_session_at(session_id, point).await?;
        let protocol = self.protocol_of(&backend);
        self.remember(&result.session.provider_session_id, backend, protocol);
        Ok(result)
    }
    async fn send_input(&self, session_id: &str, turn_id: &str, prompt: &str) -> Result<()> {
        self.session(session_id)
            .send_input(session_id, turn_id, prompt)
            .await
    }
    async fn stage_goal(&self, session_id: &str, goal: GoalState) -> Result<()> {
        self.session(session_id).stage_goal(session_id, goal).await
    }
    async fn goal_prompt(&self, session_id: &str, argument: &str) -> Result<Option<String>> {
        self.session(session_id)
            .goal_prompt(session_id, argument)
            .await
    }
    async fn get_goal(&self, session_id: &str) -> Result<Option<GoalState>> {
        self.session(session_id).get_goal(session_id).await
    }
    async fn set_goal(
        &self,
        session_id: &str,
        objective: Option<String>,
        status: Option<String>,
    ) -> Result<Option<GoalState>> {
        self.session(session_id)
            .set_goal(session_id, objective, status)
            .await
    }
    fn toolbox(&self, agent_id: Option<&str>) -> Vec<ToolboxItemDto> {
        let (backend, agent, _) = self.route(agent_id);
        backend.toolbox(agent)
    }
    async fn harness_catalog(
        &self,
        agent_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<serde_json::Value> {
        let (backend, agent, _) = self.route(agent_id);
        backend.harness_catalog(agent, cwd).await
    }
    async fn mcp_servers(&self, session_id: &str) -> Result<Option<serde_json::Value>> {
        self.session(session_id).mcp_servers(session_id).await
    }
    async fn harness_action(
        &self,
        session_id: &str,
        action: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.session(session_id)
            .harness_action(session_id, action)
            .await
    }
    async fn apply_session_settings(
        &self,
        session_id: &str,
        settings: SessionSettings,
    ) -> Result<()> {
        self.session(session_id)
            .apply_session_settings(session_id, settings)
            .await
    }
    async fn install(&self, agent_id: Option<&str>) -> Result<AgentBackendDto> {
        // Installing is for the ACP adapter, whichever key named it.
        self.acp.install(plain(agent_id)).await
    }
    async fn list_import_sessions(&self, agent_id: Option<&str>) -> Result<Vec<ImportSessionMeta>> {
        let (backend, agent, _) = self.route(agent_id);
        backend.list_import_sessions(agent).await
    }
    async fn resolve_import_session(
        &self,
        agent_id: Option<&str>,
        session_id: &str,
    ) -> Result<Option<ImportSessionMeta>> {
        self.session(session_id)
            .resolve_import_session(plain(agent_id), session_id)
            .await
    }
    fn session_loaded(&self, session_id: &str) -> bool {
        self.session(session_id).session_loaded(session_id)
    }
    fn set_session_route(&self, session_id: &str, agent_key: &str) {
        let (_, _, protocol) = self.route(Some(agent_key));
        self.owners.write().unwrap().remove(session_id);
        self.routes
            .write()
            .unwrap()
            .insert(session_id.into(), protocol);
    }
    async fn session_instance_id(&self, session_id: &str) -> Option<String> {
        self.session(session_id)
            .session_instance_id(session_id)
            .await
    }
    async fn session_instance_started_at(&self, session_id: &str) -> Option<String> {
        self.session(session_id)
            .session_instance_started_at(session_id)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Backend {
        name: &'static str,
        loaded: RwLock<Vec<String>>,
    }
    #[async_trait]
    impl AgentRuntime for Backend {
        fn provider(&self) -> Provider {
            Provider::Acp
        }
        fn descriptor(&self) -> AgentBackendDto {
            crate::fake::FakeRuntime::new(Provider::Acp).descriptor()
        }
        async fn start(&self) -> Result<()> {
            Ok(())
        }
        async fn list_models(
            &self,
            agent: Option<&str>,
            _: Option<&str>,
        ) -> Result<Vec<ModelOptionDto>> {
            anyhow::bail!("{} {}", self.name, agent.unwrap_or("-"))
        }
        async fn list_agents(&self) -> Result<Vec<ModelOptionDto>> {
            Ok(["opencode", "codex", "claude"]
                .into_iter()
                .map(|id| ModelOptionDto {
                    id: id.into(),
                    model: id.into(),
                    display_name: id.into(),
                    description: String::new(),
                    is_default: id == "codex",
                    hidden: false,
                    supported_reasoning_efforts: vec![],
                    default_reasoning_effort: None,
                    selection_kind: Some("agent".into()),
                    acp_agent: Some(serde_json::json!({"availability": if id == "claude" { "adapter_missing" } else { "ready" }, "baseCommand": id})),
                })
                .collect())
        }
        async fn capabilities(&self, _: Option<&str>) -> Result<AgentCapabilitySnapshotDto> {
            anyhow::bail!(self.name)
        }
        fn negotiated_caps(&self, _: Option<&str>) -> AgentProviderCapabilitiesDto {
            let mut caps = AgentProviderCapabilitiesDto::conversational();
            caps.turns.steer = self.name == "native";
            caps
        }
        async fn start_session(&self, input: StartSessionInput) -> Result<StartSessionResult> {
            // The router hands every backend the plain agent id.
            assert!(input.agent_id.as_deref().is_none_or(|a| !a.contains(':')));
            self.loaded.write().unwrap().push("codex::shared".into());
            Ok(StartSessionResult {
                provider_session_id: "codex::shared".into(),
                model: Some(self.name.into()),
                reasoning_effort: None,
            })
        }
        async fn resume_session(
            &self,
            _: &str,
            _: Option<&str>,
            _: SessionSettings,
        ) -> Result<StartSessionResult> {
            self.start_session(input()).await
        }
        async fn start_turn(
            &self,
            _: StartTurnInput,
            _: EventBus,
            _: CancellationToken,
        ) -> Result<Vec<ThreadHistoryItemDto>> {
            Ok(vec![])
        }
        async fn interrupt(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        async fn respond_permission(&self, id: &str, _: bool, _: Option<&str>) -> Result<()> {
            if self.name == "acp" || id == "native-codex:valid" {
                return Ok(());
            }
            anyhow::bail!("invalid native answer")
        }
        async fn release_session(&self, id: &str) -> Result<()> {
            self.loaded.write().unwrap().retain(|v| v != id);
            Ok(())
        }
        fn session_loaded(&self, id: &str) -> bool {
            self.loaded.read().unwrap().iter().any(|v| v == id)
        }
    }
    fn input() -> StartSessionInput {
        StartSessionInput {
            cwd: ".".into(),
            agent_id: Some("codex".into()),
            model: "model".into(),
            reasoning_effort: None,
            approval_mode: "guarded".into(),
            sandbox_mode: None,
        }
    }
    fn pair() -> (Arc<Backend>, Arc<Backend>) {
        let make = |name| {
            Arc::new(Backend {
                name,
                loaded: Default::default(),
            })
        };
        (make("acp"), make("native"))
    }
    async fn model_of(router: &ProtocolRouter, id: &str) -> String {
        router
            .resume_session(id, None, SessionSettings::default())
            .await
            .unwrap()
            .model
            .unwrap()
    }
    #[tokio::test]
    async fn agent_keys_pick_the_protocol_and_a_session_keeps_its_route() {
        for bound in [None, Some("codex".into())] {
            let (acp, native) = pair();
            let router =
                ProtocolRouter::new(acp.clone(), bound, None).with_backend("codex", native.clone());
            let err = |e: anyhow::Error| e.to_string();
            // Codex (and no agent, which means Codex) is native; `acp:codex` is the adapter.
            assert_eq!(
                err(router.capabilities(Some("codex")).await.unwrap_err()),
                "native"
            );
            assert_eq!(err(router.capabilities(None).await.unwrap_err()), "native");
            assert_eq!(
                err(router.capabilities(Some("acp:codex")).await.unwrap_err()),
                "acp"
            );
            assert_eq!(
                err(router
                    .list_models(Some("acp:codex"), None)
                    .await
                    .unwrap_err()),
                "acp codex"
            );
            assert_eq!(
                err(router
                    .list_models(Some("opencode"), None)
                    .await
                    .unwrap_err()),
                "acp opencode"
            );
            let mut input = input();
            input.agent_id = Some("acp:codex".into());
            assert_eq!(
                router.start_session(input).await.unwrap().model.as_deref(),
                Some("acp")
            );
            // Loaded, then released: the session stays on the protocol it started with.
            assert_eq!(model_of(&router, "codex::shared").await, "acp");
            router.release_session("codex::shared").await.unwrap();
            assert_eq!(model_of(&router, "codex::shared").await, "acp");
            assert!(
                !router
                    .session_negotiated_caps(Some("codex"), "codex::shared")
                    .turns
                    .steer
            );
            // A protocol switch re-routes it once the loaded session is released.
            router.release_session("codex::shared").await.unwrap();
            router.set_session_route("codex::shared", "codex");
            assert_eq!(model_of(&router, "codex::shared").await, "native");
            router.release_session("codex::shared").await.unwrap();
            router.set_session_route("codex::shared", "acp:codex");
            assert_eq!(model_of(&router, "codex::shared").await, "acp");
        }
    }
    #[tokio::test]
    async fn unrouted_sessions_follow_the_protocol_their_thread_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE threads(provider_session_id TEXT, harness_protocol TEXT);
            INSERT INTO threads VALUES('codex::old',NULL),('codex::pinned','acp'),('codex::new','native');").unwrap();
        let (acp, native) = pair();
        let router = ProtocolRouter::new(acp, None, Some(db)).with_backend("codex", native);
        // A thread from before the choice (NULL) moves to native at its next load.
        for (id, backend) in [
            ("codex::old", "native"),
            ("codex::pinned", "acp"),
            ("codex::new", "native"),
            ("codex::unknown", "native"),
        ] {
            assert_eq!(
                router
                    .session_capabilities(Some("codex"), id)
                    .await
                    .unwrap_err()
                    .to_string(),
                backend,
                "{id}"
            );
        }
        // Agents without a native runtime are never routed natively.
        assert_eq!(
            router
                .session_capabilities(None, "opencode::x")
                .await
                .unwrap_err()
                .to_string(),
            "acp"
        );
    }
    #[tokio::test]
    async fn agent_list_offers_native_agents_and_their_acp_adapters() {
        let (acp, native) = pair();
        let router = ProtocolRouter::new(acp, None, None)
            .with_backend("codex", native.clone())
            .with_backend("claude", native);
        let agents = router.list_agents().await.unwrap();
        let keys: Vec<_> = agents.iter().map(|a| a.model.as_str()).collect();
        assert_eq!(
            keys,
            ["codex", "claude", "acp:codex", "acp:claude", "opencode"]
        );
        let info = |key: &str| {
            agents
                .iter()
                .find(|a| a.model == key)
                .unwrap()
                .acp_agent
                .clone()
                .unwrap()
        };
        assert_eq!(info("codex")["harnessProtocol"], "native");
        assert_eq!(info("codex")["transport"], "native");
        // Native Claude needs the CLI only, not the missing ACP adapter.
        assert_eq!(info("claude")["availability"], "ready");
        assert_eq!(info("acp:claude")["availability"], "adapter_missing");
        assert_eq!(info("acp:codex")["baseAgentId"], "codex");
        assert_eq!(info("opencode")["harnessProtocol"], "acp");
        assert!(agents[0].is_default && !agents[2].is_default);
    }
    #[tokio::test]
    async fn native_approval_errors_are_never_swallowed_by_acp() {
        let acp = Arc::new(Backend {
            name: "acp",
            loaded: Default::default(),
        });
        let native = Arc::new(Backend {
            name: "native",
            loaded: Default::default(),
        });
        let router = ProtocolRouter::new(acp, None, None).with_backend("codex", native);
        assert!(router
            .respond_permission("native-codex:valid", true, None)
            .await
            .is_ok());
        assert!(router
            .respond_permission("native-codex:invalid", true, None)
            .await
            .is_err());
        assert!(router
            .respond_permission("acp-request", true, None)
            .await
            .is_ok());
    }
}

#[cfg(test)]
mod router_tests;
