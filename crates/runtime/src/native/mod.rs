//! Per-device protocol selection with backend affinity for loaded sessions.
pub mod claude;
pub mod codex;
use crate::actor::*;
use anyhow::Result;
use async_trait::async_trait;
use pockymoe_protocol::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum HarnessProtocol {
    #[default]
    Acp,
    Native,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolSettings {
    #[serde(default)]
    pub codex: HarnessProtocol,
    #[serde(default)]
    pub claude: HarnessProtocol,
}
impl ProtocolSettings {
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        crate::upstreams::private_write(path, &serde_json::to_vec(self)?)
    }
}
pub fn settings_path(database: &Path) -> PathBuf {
    database.with_extension("harness-protocols.json")
}
fn agent_of(session: &str) -> &str {
    session
        .split(':')
        .next()
        .filter(|s| matches!(*s, "codex" | "claude" | "grok" | "opencode" | "deepseek"))
        .unwrap_or("codex")
}
pub struct ProtocolRouter {
    acp: SharedRuntime,
    bound_agent: Option<String>,
    path: PathBuf,
    backends: HashMap<String, SharedRuntime>,
    owners: RwLock<HashMap<String, SharedRuntime>>,
    events: broadcast::Sender<SessionLifecycle>,
}
impl ProtocolRouter {
    pub fn new(acp: SharedRuntime, bound_agent: Option<String>, path: PathBuf) -> Self {
        let (events, _) = broadcast::channel(256);
        let router = Self {
            acp,
            bound_agent,
            path,
            backends: HashMap::new(),
            owners: RwLock::new(HashMap::new()),
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
    fn selected(&self, agent: Option<&str>) -> SharedRuntime {
        let agent = self.bound_agent.as_deref().or(agent).unwrap_or("codex");
        let settings = ProtocolSettings::load(&self.path).unwrap_or_default();
        let protocol = match agent {
            "codex" => settings.codex,
            "claude" => settings.claude,
            _ => HarnessProtocol::Acp,
        };
        if protocol == HarnessProtocol::Native {
            if let Some(backend) = self.backends.get(agent) {
                return backend.clone();
            }
        }
        self.acp.clone()
    }
    fn remember(&self, id: &str, backend: SharedRuntime) {
        self.owners.write().unwrap().insert(id.into(), backend);
    }
    fn owner(&self, id: &str) -> Option<SharedRuntime> {
        self.owners
            .read()
            .unwrap()
            .get(id)
            .filter(|backend| backend.session_loaded(id))
            .cloned()
    }
    fn session(&self, id: &str) -> SharedRuntime {
        self.owner(id)
            .unwrap_or_else(|| self.selected(Some(agent_of(id))))
    }
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
        self.selected(Some(agent_id))
            .invalidate_models(agent_id)
            .await
    }
    async fn restart(&self, agent_id: &str) -> Result<usize> {
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
        self.selected(agent_id).list_models(agent_id, cwd).await
    }
    async fn list_agents(&self) -> Result<Vec<ModelOptionDto>> {
        self.acp.list_agents().await
    }
    async fn capabilities(&self, agent_id: Option<&str>) -> Result<AgentCapabilitySnapshotDto> {
        self.selected(agent_id).capabilities(agent_id).await
    }
    async fn session_capabilities(
        &self,
        agent_id: Option<&str>,
        session_id: &str,
    ) -> Result<AgentCapabilitySnapshotDto> {
        self.session(session_id)
            .session_capabilities(agent_id, session_id)
            .await
    }
    fn negotiated_caps(&self, agent_id: Option<&str>) -> AgentProviderCapabilitiesDto {
        self.selected(agent_id).negotiated_caps(agent_id)
    }
    async fn start_session(&self, input: StartSessionInput) -> Result<StartSessionResult> {
        let backend = self.selected(input.agent_id.as_deref());
        let result = backend.start_session(input).await?;
        self.remember(&result.provider_session_id, backend);
        Ok(result)
    }
    async fn resume_session(
        &self,
        session_id: &str,
        cwd: Option<&str>,
        settings: SessionSettings,
    ) -> Result<StartSessionResult> {
        let backend = self
            .owner(session_id)
            .unwrap_or_else(|| self.selected(Some(agent_of(session_id))));
        let result = backend.resume_session(session_id, cwd, settings).await?;
        self.remember(session_id, backend.clone());
        self.remember(&result.provider_session_id, backend);
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
        self.remember(&result.provider_session_id, backend);
        Ok(result)
    }
    async fn fork_session_at(&self, session_id: &str, point: &ForkPoint) -> Result<ForkResult> {
        let backend = self.session(session_id);
        let result = backend.fork_session_at(session_id, point).await?;
        self.remember(&result.session.provider_session_id, backend);
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
        self.selected(agent_id).toolbox(agent_id)
    }
    async fn harness_catalog(
        &self,
        agent_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<serde_json::Value> {
        self.selected(agent_id).harness_catalog(agent_id, cwd).await
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
        self.acp.install(agent_id).await
    }
    async fn list_import_sessions(&self, agent_id: Option<&str>) -> Result<Vec<ImportSessionMeta>> {
        self.selected(agent_id).list_import_sessions(agent_id).await
    }
    async fn resolve_import_session(
        &self,
        agent_id: Option<&str>,
        session_id: &str,
    ) -> Result<Option<ImportSessionMeta>> {
        self.session(session_id)
            .resolve_import_session(agent_id, session_id)
            .await
    }
    fn session_loaded(&self, session_id: &str) -> bool {
        self.session(session_id).session_loaded(session_id)
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
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<Vec<ModelOptionDto>> {
            Ok(vec![])
        }
        async fn list_agents(&self) -> Result<Vec<ModelOptionDto>> {
            Ok(vec![])
        }
        async fn capabilities(&self, _: Option<&str>) -> Result<AgentCapabilitySnapshotDto> {
            anyhow::bail!(self.name)
        }
        async fn start_session(&self, _: StartSessionInput) -> Result<StartSessionResult> {
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
    #[tokio::test]
    async fn loaded_session_keeps_backend_until_release_for_catalog_and_bound_providers() {
        for bound in [None, Some("codex".into())] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("protocol.json");
            let acp = Arc::new(Backend {
                name: "acp",
                loaded: Default::default(),
            });
            let native = Arc::new(Backend {
                name: "native",
                loaded: Default::default(),
            });
            let router = ProtocolRouter::new(acp.clone(), bound, path.clone())
                .with_backend("codex", native.clone());
            assert_eq!(
                router
                    .start_session(input())
                    .await
                    .unwrap()
                    .model
                    .as_deref(),
                Some("acp")
            );
            ProtocolSettings {
                codex: HarnessProtocol::Native,
                ..Default::default()
            }
            .save(&path)
            .unwrap();
            assert_eq!(
                router
                    .resume_session("codex::shared", None, SessionSettings::default())
                    .await
                    .unwrap()
                    .model
                    .as_deref(),
                Some("acp")
            );
            router.release_session("codex::shared").await.unwrap();
            assert_eq!(
                router
                    .resume_session("codex::shared", None, SessionSettings::default())
                    .await
                    .unwrap()
                    .model
                    .as_deref(),
                Some("native")
            );
            ProtocolSettings::default().save(&path).unwrap();
            assert_eq!(
                router
                    .resume_session("codex::shared", None, SessionSettings::default())
                    .await
                    .unwrap()
                    .model
                    .as_deref(),
                Some("native")
            );
            router.release_session("codex::shared").await.unwrap();
            assert_eq!(
                router
                    .resume_session("codex::shared", None, SessionSettings::default())
                    .await
                    .unwrap()
                    .model
                    .as_deref(),
                Some("acp")
            );
        }
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
        let router =
            ProtocolRouter::new(acp, None, PathBuf::from("missing")).with_backend("codex", native);
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
    #[test]
    fn settings_reject_typo_and_default_to_acp() {
        assert!(serde_json::from_str::<ProtocolSettings>(r#"{"codex":"nativ"}"#).is_err());
        assert_eq!(
            serde_json::from_str::<ProtocolSettings>("{}")
                .unwrap()
                .codex,
            HarnessProtocol::Acp
        );
    }
}
