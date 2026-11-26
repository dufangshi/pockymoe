//! Unloaded session routing must be safe before any owner has been remembered.
use super::*;
use serde_json::{json, Value};

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<(&'static str, String)>>,
}
impl Recorder {
    fn record(&self, method: &'static str, id: &str) {
        self.calls.lock().unwrap().push((method, id.into()));
    }
}
#[async_trait]
impl AgentRuntime for Recorder {
    fn provider(&self) -> Provider {
        Provider::Acp
    }
    fn descriptor(&self) -> AgentBackendDto {
        crate::fake::FakeRuntime::new(Provider::Acp).descriptor()
    }
    async fn start(&self) -> Result<()> {
        Ok(())
    }
    async fn list_models(&self, _: Option<&str>, _: Option<&str>) -> Result<Vec<ModelOptionDto>> {
        Ok(vec![])
    }
    async fn list_agents(&self) -> Result<Vec<ModelOptionDto>> {
        Ok(vec![])
    }
    async fn capabilities(&self, _: Option<&str>) -> Result<AgentCapabilitySnapshotDto> {
        anyhow::bail!("not a session probe")
    }
    async fn start_session(&self, _: StartSessionInput) -> Result<StartSessionResult> {
        anyhow::bail!("not a session operation")
    }
    async fn respond_permission(&self, _: &str, _: bool, _: Option<&str>) -> Result<()> {
        anyhow::bail!("not a session operation")
    }

    async fn inspect_context(&self, id: &str, _: &str) -> Result<Option<Value>> {
        self.record("inspect_context", id);
        Ok(None)
    }
    async fn execution_state(&self, id: &str) -> ExecutionState {
        self.record("execution_state", id);
        ExecutionState::Unknown
    }
    async fn active_subagents(&self, id: &str) -> Vec<ThreadSubagentDto> {
        self.record("active_subagents", id);
        vec![]
    }
    async fn session_models(
        &self,
        _: Option<&str>,
        id: &str,
        _: Option<&str>,
    ) -> Result<Vec<ModelOptionDto>> {
        self.record("session_models", id);
        Ok(vec![])
    }
    async fn session_capabilities(
        &self,
        _: Option<&str>,
        id: &str,
    ) -> Result<AgentCapabilitySnapshotDto> {
        self.record("session_capabilities", id);
        anyhow::bail!("probe recorded")
    }
    fn session_negotiated_caps(&self, _: Option<&str>, id: &str) -> AgentProviderCapabilitiesDto {
        self.record("session_negotiated_caps", id);
        AgentProviderCapabilitiesDto::conversational()
    }
    async fn resume_session(
        &self,
        id: &str,
        _: Option<&str>,
        _: SessionSettings,
    ) -> Result<StartSessionResult> {
        self.record("resume_session", id);
        anyhow::bail!("keep unloaded")
    }
    async fn start_turn(
        &self,
        input: StartTurnInput,
        _: EventBus,
        _: CancellationToken,
    ) -> Result<Vec<ThreadHistoryItemDto>> {
        self.record("start_turn", &input.provider_session_id);
        Ok(vec![])
    }
    async fn interrupt(&self, id: &str, _: &str) -> Result<()> {
        self.record("interrupt", id);
        Ok(())
    }
    async fn release_session(&self, id: &str) -> Result<()> {
        self.record("release_session", id);
        Ok(())
    }
    async fn release_loaded(
        &self,
        id: &str,
        _: ReleaseScope,
        _: Option<&str>,
    ) -> Result<Vec<LoadedSession>> {
        self.record("release_loaded", id);
        Ok(vec![])
    }
    async fn touch_session(&self, id: &str) {
        self.record("touch_session", id);
    }
    async fn compact_session(&self, id: &str, _: &str, _: EventBus) -> Result<()> {
        self.record("compact_session", id);
        Ok(())
    }
    async fn rewind_session(&self, id: &str, _: u32) -> Result<()> {
        self.record("rewind_session", id);
        Ok(())
    }
    async fn fork_session(&self, id: &str) -> Result<StartSessionResult> {
        self.record("fork_session", id);
        anyhow::bail!("keep unloaded")
    }
    async fn fork_session_at(&self, id: &str, _: &ForkPoint) -> Result<ForkResult> {
        self.record("fork_session_at", id);
        anyhow::bail!("keep unloaded")
    }
    async fn send_input(&self, id: &str, _: &str, _: &str) -> Result<()> {
        self.record("send_input", id);
        Ok(())
    }
    async fn stage_goal(&self, id: &str, _: GoalState) -> Result<()> {
        self.record("stage_goal", id);
        Ok(())
    }
    async fn goal_prompt(&self, id: &str, _: &str) -> Result<Option<String>> {
        self.record("goal_prompt", id);
        Ok(None)
    }
    async fn get_goal(&self, id: &str) -> Result<Option<GoalState>> {
        self.record("get_goal", id);
        Ok(None)
    }
    async fn set_goal(
        &self,
        id: &str,
        _: Option<String>,
        _: Option<String>,
    ) -> Result<Option<GoalState>> {
        self.record("set_goal", id);
        Ok(None)
    }
    async fn harness_action(&self, id: &str, _: Value) -> Result<Value> {
        self.record("harness_action", id);
        Ok(Value::Null)
    }
    async fn apply_session_settings(&self, id: &str, _: SessionSettings) -> Result<()> {
        self.record("apply_session_settings", id);
        Ok(())
    }
    async fn resolve_import_session(
        &self,
        _: Option<&str>,
        id: &str,
    ) -> Result<Option<ImportSessionMeta>> {
        self.record("resolve_import_session", id);
        Ok(None)
    }
    fn session_loaded(&self, id: &str) -> bool {
        self.record("session_loaded", id);
        false
    }
    async fn session_instance_id(&self, id: &str) -> Option<String> {
        self.record("session_instance_id", id);
        None
    }
    async fn session_instance_started_at(&self, id: &str) -> Option<String> {
        self.record("session_instance_started_at", id);
        None
    }
}

#[tokio::test]
async fn every_unloaded_unknown_session_operation_stays_on_acp() {
    for bound in [None, Some("codex".into()), Some("claude".into())] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        ProtocolSettings {
            codex: HarnessProtocol::Native,
            claude: HarnessProtocol::Native,
        }
        .save(&path)
        .unwrap();
        let acp = Arc::new(Recorder::default());
        let native = Arc::new(Recorder::default());
        let router = ProtocolRouter::new(acp.clone(), bound, path)
            .with_backend("codex", native.clone())
            .with_backend("claude", native.clone());
        for id in [
            "gemini::x",
            "gemini::env/owned/1/x",
            "cursor::x",
            "copilot::x",
            "custom::x",
            "unlisted::x",
            "raw-id",
            "codex:malformed",
            "codex::",
        ] {
            acp.calls.lock().unwrap().clear();
            let _ = router.inspect_context(id, "summary").await;
            let _ = router.execution_state(id).await;
            let _ = router.active_subagents(id).await;
            // Deliberately inconsistent agent arguments must not override identity.
            let _ = router.session_models(Some("codex"), id, None).await;
            let _ = router.session_capabilities(Some("codex"), id).await;
            let _ = router.session_negotiated_caps(Some("codex"), id);
            let _ = router
                .resume_session(id, None, SessionSettings::default())
                .await;
            let _ = router
                .start_turn(
                    StartTurnInput {
                        provider_session_id: id.into(),
                        thread_id: "thread".into(),
                        turn_id: "turn".into(),
                        prompt: "test".into(),
                        model: None,
                        reasoning_effort: None,
                        approval_mode: None,
                        sandbox_mode: None,
                        collaboration_mode: None,
                        performance_mode: None,
                        hidden: false,
                        images: vec![],
                        title: None,
                        context_delivered: true,
                    },
                    EventBus::new(),
                    CancellationToken::new(),
                )
                .await;
            let _ = router.interrupt(id, "turn").await;
            let _ = router.release_session(id).await;
            let _ = router.release_loaded(id, ReleaseScope::Session, None).await;
            router.touch_session(id).await;
            let _ = router.compact_session(id, "thread", EventBus::new()).await;
            let _ = router.rewind_session(id, 1).await;
            let _ = router.fork_session(id).await;
            let _ = router.fork_session_at(id, &ForkPoint::default()).await;
            let _ = router.send_input(id, "turn", "steer").await;
            let _ = router
                .stage_goal(
                    id,
                    GoalState {
                        objective: "test".into(),
                        status: "paused".into(),
                        tokens_used: 0,
                        time_used_seconds: 0,
                    },
                )
                .await;
            let _ = router.goal_prompt(id, "test").await;
            let _ = router.get_goal(id).await;
            let _ = router.set_goal(id, None, None).await;
            let _ = router.harness_action(id, json!({})).await;
            let _ = router
                .apply_session_settings(id, SessionSettings::default())
                .await;
            let _ = router.resolve_import_session(Some("codex"), id).await;
            assert!(!router.session_loaded(id));
            let _ = router.session_instance_id(id).await;
            let _ = router.session_instance_started_at(id).await;
            let expected = [
                "inspect_context",
                "execution_state",
                "active_subagents",
                "session_models",
                "session_capabilities",
                "session_negotiated_caps",
                "resume_session",
                "start_turn",
                "interrupt",
                "release_session",
                "release_loaded",
                "touch_session",
                "compact_session",
                "rewind_session",
                "fork_session",
                "fork_session_at",
                "send_input",
                "stage_goal",
                "goal_prompt",
                "get_goal",
                "set_goal",
                "harness_action",
                "apply_session_settings",
                "resolve_import_session",
                "session_loaded",
                "session_instance_id",
                "session_instance_started_at",
            ];
            assert_eq!(
                *acp.calls.lock().unwrap(),
                expected
                    .into_iter()
                    .map(|method| (method, id.to_owned()))
                    .collect::<Vec<_>>(),
                "{id}"
            );
            assert!(
                native.calls.lock().unwrap().is_empty(),
                "native backend saw {id}"
            );
        }
    }
}
