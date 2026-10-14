use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Result};
use async_trait::async_trait;
use futures_util::{future::BoxFuture, stream::FuturesUnordered, StreamExt};
use pockymoe_protocol::{
    now_rfc3339, toolbox_from_capabilities, AgentBackendDto, AgentBackendInstallationDto,
    AgentBackendManagementSchemaDto, AgentCapabilitySnapshotDto, AgentProviderCapabilitiesDto,
    AgentRuntimeStatusDto, ModelOptionDto, Provider, ReasoningEffortOptionDto,
    ThreadActionRequestDto, ThreadEventEnvelope, ThreadHistoryItemDto, ThreadSubagentDto,
    ToolboxItemDto,
};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::actor::{
    AgentRuntime, EventBus, GoalState, ImportSessionMeta, SessionSettings, StartSessionInput,
    StartSessionResult, StartTurnInput,
};
use crate::files::{write_file_with_scope, WriteScope};
use crate::import_id::session_ids_match;

use super::adapter::{adapter_for, SessionSettingOp};
use super::capabilities::{negotiate, parse_commands, NegotiatedCaps, NegotiatedCommand};
use super::catalog::{
    builtin_agents, classify_availability, command_program, parse_command_models, AcpAgentDef,
};
use super::mapper::{MappedUpdate, TurnMapper};
use super::modes::{
    parse_available_modes, parse_permission_choices, permission_description, permission_questions,
    permission_title, resolve_mode, resolve_mode_config_value, select_permission_option,
    PermissionChoice, ProductSessionPolicy, SessionMode,
};
use super::prompt::build_prompt_blocks;
use super::rpc::{parse_spawn_command, AcpProcess};
use super::terminal::AgentTerminals;

#[cfg(all(test, unix))]
#[path = "fork_tests.rs"]
mod fork_tests;

#[path = "release.rs"]
mod release;
use release::LoadRecord;

#[cfg(all(test, unix))]
#[path = "release_tests.rs"]
mod release_tests;

#[cfg(all(test, unix))]
#[path = "completion_tests.rs"]
mod completion_tests;

// Startup and cancelled probes must not leave the notification mux holding a child alive.
struct StartupProcess(Option<Arc<AcpProcess>>);
impl Drop for StartupProcess {
    fn drop(&mut self) {
        if let Some(process) = self.0.take() {
            tokio::spawn(async move {
                let _ = process.shutdown().await;
            });
        }
    }
}

struct ActiveTurn {
    thread_id: String,
    turn_id: String,
    bus: EventBus,
    background_owned: bool,
    cancel: CancellationToken,
    pending_prompts: usize,
    continuations: mpsc::UnboundedSender<BoxFuture<'static, Result<Value>>>,
}

enum TurnOutcome {
    Completed,
    Interrupted,
    Failed(anyhow::Error),
}

struct LiveSession {
    process: Arc<AcpProcess>,
    codex_bridge: Option<Arc<super::codex_bridge::CodexBridge>>,
    dsh: Option<Arc<super::deepseek::Bridge>>,
    harness_info: Option<Value>,
    /// Latest DSH projection views (`plan`, `goal`, `todos`, `permissions`, `agentPreset`).
    dsh_projections: serde_json::Map<String, Value>,
    /// Commands and the run-mode lock from the last bridge session read.
    dsh_session: Value,
    /// Thread title last given to the DSH session.
    dsh_title: Option<String>,
    dsh_running: bool,
    /// `DSH_PERMISSION_MODE` the process booted with.
    dsh_env_preset: Option<String>,
    session_id: String,
    cwd: PathBuf,
    yolo: bool,
    sandbox_mode: Option<String>,
    collaboration_mode: Option<String>,
    negotiated: NegotiatedCaps,
    adapter_id: String,
    goal: Option<GoalState>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    active: Option<ActiveTurn>,
    operation: Arc<Mutex<()>>,
    config_options: Value,
    harness_state: Value,
    harness_models: Vec<ModelOptionDto>,
    available_modes: Vec<SessionMode>,
    current_mode_id: Option<String>,
    /// Prompt bootstrap is scoped to the loaded process and product identity.
    context_thread_id: Option<String>,
    active_subagents: HashMap<String, ThreadSubagentDto>,
    claude_lifecycle: super::claude_lifecycle::ClaudeLifecycle,
    upstream_revision: Option<Vec<u8>>,
    load: LoadRecord,
}

struct PendingPermission {
    tx: tokio::sync::oneshot::Sender<String>,
    options: Vec<PermissionChoice>,
}

struct PendingInput {
    tx: tokio::sync::oneshot::Sender<Value>,
    params: Value,
    native: bool,
}

struct Inner {
    sessions: Mutex<HashMap<String, LiveSession>>,
    health: std::sync::Mutex<HashMap<String, Option<String>>>,
    commands: Mutex<HashMap<(String, String), Vec<NegotiatedCommand>>>,
    lifecycle: Mutex<()>,
    pending_permissions: Mutex<HashMap<String, PendingPermission>>,
    pending_inputs: Mutex<HashMap<String, PendingInput>>,
    pending_dtos: Mutex<HashMap<String, Vec<ThreadActionRequestDto>>>,
    caps_by_agent: Mutex<HashMap<String, AgentProviderCapabilitiesDto>>,
    toolbox_by_agent: Mutex<HashMap<String, Vec<ToolboxItemDto>>>,
    updates: broadcast::Sender<Value>,
    terminals: AgentTerminals,
    /// Latest DSH run modes and features, for thread creation forms.
    dsh_catalog: Mutex<Option<Value>>,
    load_events: broadcast::Sender<crate::actor::SessionLifecycle>,
}

pub struct AcpRuntime {
    upstreams: Option<super::upstream_models::UpstreamModels>,
    provider: Provider,
    bound_agent: Option<String>,
    custom_command: Option<String>,
    startup_timeout: Duration,
    started_at: Mutex<Option<String>>,
    inner: Arc<Inner>,
}

impl AcpRuntime {
    async fn update_subagents(&self, input: &StartTurnInput, agents: &[ThreadSubagentDto]) -> bool {
        let next = agents
            .iter()
            .cloned()
            .map(|agent| (agent.id.clone(), agent))
            .collect();
        let mut sessions = self.inner.sessions.lock().await;
        let Some(live) = sessions.get_mut(&input.provider_session_id) else {
            return false;
        };
        if !live
            .active
            .as_ref()
            .is_some_and(|active| active.turn_id == input.turn_id)
            || live.active_subagents == next
        {
            return false;
        }
        live.active_subagents = next;
        true
    }

    pub fn catalog(custom: Option<String>, timeout_ms: u64) -> Self {
        Self::new(Provider::Acp, None, custom, timeout_ms)
    }

    pub fn bound(provider: Provider, agent_id: &str, timeout_ms: u64) -> Self {
        Self::new(provider, Some(agent_id.into()), None, timeout_ms)
    }

    fn new(
        provider: Provider,
        bound_agent: Option<String>,
        custom: Option<String>,
        timeout_ms: u64,
    ) -> Self {
        let (updates, _) = broadcast::channel(2048);
        Self {
            upstreams: None,
            provider,
            bound_agent,
            custom_command: custom,
            startup_timeout: Duration::from_millis(timeout_ms),
            started_at: Mutex::new(None),
            inner: Arc::new(Inner {
                sessions: Mutex::new(HashMap::new()),
                health: Default::default(),
                commands: Mutex::new(HashMap::new()),
                lifecycle: Mutex::new(()),
                pending_permissions: Mutex::new(HashMap::new()),
                pending_inputs: Mutex::new(HashMap::new()),
                pending_dtos: Mutex::new(HashMap::new()),
                caps_by_agent: Mutex::new(HashMap::new()),
                toolbox_by_agent: Mutex::new(HashMap::new()),
                updates,
                terminals: AgentTerminals::default(),
                dsh_catalog: Mutex::new(None),
                load_events: broadcast::channel(256).0,
            }),
        }
    }

    fn agent_def(&self, agent_id: Option<&str>) -> Result<AcpAgentDef> {
        let id = agent_id
            .map(str::to_string)
            .or_else(|| self.bound_agent.clone())
            .unwrap_or_else(|| "codex".into());
        builtin_agents(self.custom_command.as_deref())
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| anyhow!("unknown ACP agent {id}"))
    }

    pub fn with_upstreams(mut self, directory: PathBuf) -> Self {
        self.upstreams = Some(super::upstream_models::UpstreamModels::new(directory));
        self
    }

    fn scoped_id(agent_id: &str, session_id: &str) -> String {
        format!("{agent_id}::{session_id}")
    }

    async fn spawn_session(
        &self,
        def: &AcpAgentDef,
        cwd: &str,
        policy: ProductSessionPolicy,
        load_id: Option<&str>,
        reasoning_effort: Option<&str>,
    ) -> Result<(String, LiveSession)> {
        let result = self
            .spawn_session_inner(def, cwd, policy, load_id, reasoning_effort)
            .await;
        self.inner.health.lock().unwrap().insert(
            def.id.clone(),
            result.as_ref().err().map(|error| format!("{error:#}")),
        );
        result
    }

    async fn spawn_session_inner(
        &self,
        def: &AcpAgentDef,
        cwd: &str,
        policy: ProductSessionPolicy,
        load_id: Option<&str>,
        reasoning_effort: Option<&str>,
    ) -> Result<(String, LiveSession)> {
        let cwd = harness_cwd(cwd);
        let cwd = cwd.as_ref();
        let yolo = policy.auto_approve();
        super::dependencies::ensure(def, false).await?;
        let availability = classify_availability(def);
        if availability != "ready" {
            bail!("{} is not available ({availability})", def.display_name);
        }
        let adapter = adapter_for(&def.id);
        let mut extra_env = extra_env_for(def);
        let mut upstream_revision = None;
        if def.id == "grok" {
            if let Some(upstreams) = &self.upstreams {
                if let Some((profile, models)) = upstreams.catalog("grok").await? {
                    upstreams.prepare_grok(&profile, &models)?;
                    upstream_revision = Some(super::upstream_models::UpstreamModels::revision(
                        &profile, &models,
                    )?);
                }
            }
        }
        let codex_bridge = if def.id == "codex" {
            Some(Arc::new(
                super::codex_bridge::CodexBridge::new(&def.base_command, &mut extra_env, &policy)
                    .await?,
            ))
        } else {
            None
        };
        let server_command = agent_server_command(def, yolo);
        let dsh_env_preset =
            (def.id == "deepseek").then(|| super::deepseek::permission_preset(&policy).to_string());
        if let Some(preset) = &dsh_env_preset {
            // DSH sandboxes its own tools; ACP client fs/terminal policy never applies.
            extra_env.push((super::deepseek::PERMISSION_ENV, preset.clone()));
        }
        let (process, updates_rx, requests_rx, dsh, harness_info) = if dsh_env_preset.is_some() {
            let (process, updates, requests, bridge, hello) = self
                .spawn_dsh(def, &server_command, cwd, &extra_env)
                .await?;
            *self.inner.dsh_catalog.lock().await = Some(super::deepseek::catalog_view(&hello));
            (process, updates, requests, Some(bridge), Some(hello))
        } else {
            let (process, updates, requests) = tokio::time::timeout(
                self.startup_timeout,
                AcpProcess::spawn(&server_command, cwd, &extra_env),
            )
            .await
            .map_err(|_| anyhow!("ACP spawn timeout"))??;
            (Arc::new(process), updates, requests, None, None)
        };
        let mut startup_process = StartupProcess(Some(process.clone()));
        if let Some(bridge) = &dsh {
            spawn_dsh_mux(self.inner.clone(), process.id.clone(), bridge.clone());
        }
        let init = process
            .request(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientInfo": {
                        "name": "pockymoe",
                        "title": "Pockymoe",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "clientCapabilities": {
                        "fs": {
                            "readTextFile": adapter.fs_read_text_file(),
                            "writeTextFile": adapter.fs_write_text_file()
                        },
                        "terminal": true,
                        "elicitation": {"form": {}},
                        "session": { "compaction": {}, "configOptions": { "boolean": {} } },
                        "plan": {},
                        "_meta": adapter.initialize_client_meta()
                    }
                }),
            )
            .await?;
        let mut negotiated = negotiate(&init);
        if codex_bridge.is_some() {
            negotiated.fork = true;
        }
        if let Some((method, required_field)) = adapter.fork_probe().filter(|_| !negotiated.fork) {
            if process
                .request(method, json!({}))
                .await
                .err()
                .is_some_and(|e| e.to_string().contains(required_field))
            {
                negotiated.fork = true;
                negotiated.fork_method = Some(method.into());
            }
        }
        let mut caps = AgentProviderCapabilitiesDto::conversational();
        adapter.patch_capabilities(&mut caps, &negotiated);
        if let Some(info) = &harness_info {
            super::deepseek::patch_capabilities(&mut caps, info, &Value::Null);
        }
        let toolbox = adapter.toolbox_items(&caps, &negotiated);
        negotiated.compact = caps.turns.compact;
        negotiated.fork = caps.branching.fork;
        negotiated.goals = caps.controls.goals;
        negotiated.steer = caps.turns.steer;
        self.inner
            .caps_by_agent
            .lock()
            .await
            .insert(def.id.clone(), caps);
        self.inner
            .toolbox_by_agent
            .lock()
            .await
            .insert(def.id.clone(), toolbox);
        spawn_mux(
            self.inner.clone(),
            process.clone(),
            &def.id,
            updates_rx,
            requests_rx,
        );
        let raw_session = if let Some(existing) = load_id {
            if negotiated.load_session {
                process
                    .request(
                        "session/load",
                        json!({ "sessionId": existing, "cwd": cwd, "mcpServers": [], "_meta": adapter.session_load_meta() }),
                    )
                    .await?
            } else if negotiated.resume {
                process
                    .request(
                        "session/resume",
                        json!({ "sessionId": existing, "cwd": cwd, "mcpServers": [], "_meta": adapter.session_load_meta() }),
                    )
                    .await?
            } else {
                bail!("ACP agent does not support session/load or session/resume");
            }
        } else {
            let extra_meta = adapter.session_new_meta(reasoning_effort);
            let mut meta = json!({ "yoloMode": yolo });
            if let Some(map) = extra_meta.as_object() {
                for (key, value) in map {
                    meta[key] = value.clone();
                }
            }
            process
                .request(
                    "session/new",
                    json!({ "cwd": cwd, "mcpServers": [], "_meta": meta }),
                )
                .await?
        };
        let session_id = raw_session
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or(load_id.unwrap_or(""))
            .to_string();
        if session_id.is_empty() {
            bail!("ACP session id missing");
        }
        let config_options = raw_session
            .get("configOptions")
            .cloned()
            .unwrap_or_else(|| json!([]));
        if let Some(caps) = self.inner.caps_by_agent.lock().await.get_mut(&def.id) {
            apply_config_option_caps(caps, &config_options);
        }
        let projection = adapter.project_session(&raw_session);
        let (harness_state, harness_models, model, reasoning_effort) =
            if let Some(proj) = projection {
                if let Some(caps) = self.inner.caps_by_agent.lock().await.get_mut(&def.id) {
                    caps.management.models = true;
                }
                (proj.state, proj.models, proj.model, proj.reasoning_effort)
            } else {
                (json!({}), Vec::new(), None, None)
            };
        let (model, reasoning_effort) = if def.id == "deepseek" || def.id == "grok" {
            let current = |category: &str| {
                config_options
                    .as_array()
                    .and_then(|options| {
                        options
                            .iter()
                            .find(|option| option["category"].as_str() == Some(category))
                    })
                    .and_then(|option| option["currentValue"].as_str())
                    .map(str::to_string)
            };
            (
                if def.id == "grok" {
                    current("model").or(model)
                } else {
                    current("model")
                },
                current("thought_level"),
            )
        } else {
            (model, reasoning_effort)
        };
        let (current_mode_id, available_modes) = parse_available_modes(&raw_session);
        let scoped = Self::scoped_id(&def.id, &session_id);
        let harness_models = match &harness_info {
            Some(info) => super::deepseek::catalog(
                models_from_config_options(&config_options),
                &info["models"],
                reasoning_effort.as_deref(),
            ),
            None => harness_models,
        };
        let mut live = LiveSession {
            process,
            codex_bridge,
            dsh,
            harness_info,
            dsh_projections: Default::default(),
            dsh_session: Value::Null,
            dsh_title: None,
            dsh_running: false,
            dsh_env_preset,
            session_id,
            cwd: PathBuf::from(cwd),
            yolo,
            sandbox_mode: policy.sandbox_mode.clone(),
            collaboration_mode: policy.collaboration_mode.clone(),
            negotiated,
            adapter_id: def.id.clone(),
            goal: None,
            upstream_revision,
            model,
            reasoning_effort,
            active: None,
            claude_lifecycle: Default::default(),
            operation: Arc::new(Mutex::new(())),
            config_options: config_options.clone(),
            harness_state,
            harness_models,
            available_modes,
            current_mode_id,
            context_thread_id: None,
            active_subagents: HashMap::new(),
            load: LoadRecord::from_intent(),
        };
        Self::apply_product_mode(&live.process.clone(), &mut live).await?;
        startup_process.0 = None;
        Ok((scoped, live))
    }

    /// DSH boots with its own Web bundle (run modes, native console) when the
    /// installed release ships it, and falls back to the plain ACP profile.
    /// ACP goes live before sibling plugins settle; the bridge hello arrives
    /// after appReady, so initialize/new see the complete provider catalog.
    async fn spawn_dsh(
        &self,
        def: &AcpAgentDef,
        command: &str,
        cwd: &str,
        extra_env: &[(&str, String)],
    ) -> Result<(
        Arc<AcpProcess>,
        mpsc::UnboundedReceiver<Value>,
        mpsc::UnboundedReceiver<(i64, String, Value)>,
        Arc<super::deepseek::Bridge>,
        Value,
    )> {
        let native = (std::env::var("POCKYMOE_DSH_NATIVE").as_deref() != Ok("0"))
            .then(|| super::catalog::resolve_executable(&def.base_command))
            .flatten()
            .and_then(|executable| super::deepseek::native_composition(&executable));
        let mut fallback = None;
        // Without a native composition the first (plain) attempt returns or fails.
        for composition in [native, None] {
            let mut server_command = command.to_string();
            let launch =
                super::deepseek::Launch::prepare(&mut server_command, composition.as_ref()).await?;
            let (process, updates, requests) = tokio::time::timeout(
                self.startup_timeout,
                AcpProcess::spawn(&server_command, cwd, extra_env),
            )
            .await
            .map_err(|_| anyhow!("ACP spawn timeout"))??;
            let process = Arc::new(process);
            match tokio::time::timeout(self.startup_timeout, launch.connect()).await {
                Ok(Ok((bridge, mut hello))) => {
                    hello["composition"] = json!(if composition.is_some() {
                        "native"
                    } else {
                        "acp"
                    });
                    hello["compositionError"] = json!(fallback);
                    return Ok((process, updates, requests, Arc::new(bridge), hello));
                }
                result => {
                    let _ = process.shutdown().await;
                    let reason = match result {
                        Ok(Err(error)) => format!("{error:#}"),
                        _ => "timed out waiting for DSH startup".into(),
                    };
                    if composition.is_none() {
                        bail!("DSH bridge failed to start (requires DSH 0.1.5-rc.1 or newer): {reason}");
                    }
                    tracing::warn!(%reason, "DSH native composition failed; using the plain ACP profile");
                    fallback = Some(reason);
                }
            }
        }
        bail!("DSH did not start")
    }

    async fn list_agent_sessions(&self, def: &AcpAgentDef) -> Result<Vec<ImportSessionMeta>> {
        let cwd = std::env::temp_dir();
        let extra_env = extra_env_for(def);
        let adapter = adapter_for(&def.id);
        let server_command = agent_server_command(def, false);
        let (process, _updates_rx, _requests_rx) = tokio::time::timeout(
            self.startup_timeout,
            AcpProcess::spawn(&server_command, &cwd.to_string_lossy(), &extra_env),
        )
        .await
        .map_err(|_| anyhow!("ACP spawn timeout"))??;
        let init = process
            .request(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientInfo": {
                        "name": "pockymoe",
                        "title": "Pockymoe",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "clientCapabilities": {
                        "fs": {
                            "readTextFile": adapter.fs_read_text_file(),
                            "writeTextFile": adapter.fs_write_text_file()
                        },
                        "terminal": true,
                        "elicitation": {"form": {}},
                        "session": { "compaction": {}, "configOptions": { "boolean": {} } },
                        "plan": {},
                        "_meta": adapter.initialize_client_meta()
                    }
                }),
            )
            .await?;
        let negotiated = negotiate(&init);
        if !negotiated.list {
            return Ok(Vec::new());
        }
        let mut sessions = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut params = json!({});
            if let Some(cursor) = &cursor {
                params["cursor"] = json!(cursor);
            }
            let response = process.request("session/list", params).await?;
            if let Some(items) = response.get("sessions").and_then(Value::as_array) {
                for info in items {
                    let Some(session_id) = info.get("sessionId").and_then(Value::as_str) else {
                        continue;
                    };
                    let cwd = info
                        .get("cwd")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if cwd.is_empty() || !PathBuf::from(&cwd).is_absolute() {
                        continue;
                    }
                    sessions.push(ImportSessionMeta {
                        session_id: session_id.to_string(),
                        agent_id: def.id.clone(),
                        cwd,
                        title: info
                            .get("title")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or_else(|| "Untitled session".into()),
                        preview: None,
                        created_at: info
                            .get("createdAt")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        updated_at: info
                            .get("updatedAt")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        model: None,
                        turns: Vec::new(),
                    });
                }
            }
            cursor = response
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(sessions)
    }

    async fn models_from_live(&self, agent_id: &str) -> Option<Vec<ModelOptionDto>> {
        let (cached, configured, process) = {
            let sessions = self.inner.sessions.lock().await;
            let live = sessions
                .values()
                .find(|session| session.adapter_id == agent_id)?;
            (
                live.harness_models.clone(),
                models_from_config_options(&live.config_options),
                live.process.clone(),
            )
        };
        if !cached.is_empty() {
            return Some(cached);
        }
        if !configured.is_empty() {
            return Some(configured);
        }
        let models = self.request_adapter_models(agent_id, &process).await?;
        if let Some(live) = self
            .inner
            .sessions
            .lock()
            .await
            .values_mut()
            .find(|session| session.adapter_id == agent_id)
        {
            live.harness_models = models.clone();
        }
        Some(models)
    }

    async fn request_adapter_models(
        &self,
        agent_id: &str,
        process: &AcpProcess,
    ) -> Option<Vec<ModelOptionDto>> {
        let adapter = adapter_for(agent_id);
        let method = adapter.model_list_method()?;
        let response = process.request(method, json!({})).await.ok()?;
        let models = adapter.project_model_list(&response)?;
        if let Some(caps) = self.inner.caps_by_agent.lock().await.get_mut(agent_id) {
            caps.management.models = true;
            if adapter.model_list_supports_performance(&response) {
                caps.controls.performance_mode = true;
            }
        }
        Some(models)
    }

    async fn probe_models(&self, def: &AcpAgentDef, cwd: &str) -> Result<Vec<ModelOptionDto>> {
        let (_scoped, live) = self
            .spawn_session(
                def,
                cwd,
                ProductSessionPolicy {
                    approval_mode: Some("yolo".into()),
                    ..ProductSessionPolicy::default()
                },
                None,
                None,
            )
            .await?;
        let _probe_process = StartupProcess(Some(live.process.clone()));
        let models = if !live.harness_models.is_empty() {
            live.harness_models
        } else {
            let configured = models_from_config_options(&live.config_options);
            if !configured.is_empty() {
                configured
            } else {
                self.request_adapter_models(&def.id, &live.process)
                    .await
                    .unwrap_or_default()
            }
        };
        // The notification mux also owns an Arc. Dropping this probe alone
        // would leave its process running every time a creation form loads.
        live.process.shutdown().await?;
        Ok(models)
    }

    async fn apply_setting_op(
        process: &AcpProcess,
        live: &mut LiveSession,
        op: SessionSettingOp,
    ) -> Result<()> {
        match op {
            SessionSettingOp::SetConfig { config_id, value } => {
                let response = process
                    .request(
                        "session/set_config_option",
                        json!({
                            "sessionId": live.session_id,
                            "configId": config_id,
                            "value": value
                        }),
                    )
                    .await?;
                if live.adapter_id == "grok" && config_id == "reasoning_effort" {
                    let actual = response["configOptions"]
                        .as_array()
                        .and_then(|options| {
                            options
                                .iter()
                                .find(|option| option["category"] == "thought_level")
                        })
                        .and_then(|option| option["currentValue"].as_str());
                    if super::grok::normalize_acp_effort(actual)
                        != super::grok::normalize_acp_effort(Some(&value))
                    {
                        bail!("Grok did not confirm the requested reasoning effort");
                    }
                }
                if let Some(options) = response.get("configOptions") {
                    live.config_options = options.clone();
                    if live.adapter_id == "deepseek" {
                        live.reasoning_effort = None;
                    }
                    if let Some(options) = options.as_array() {
                        for option in options {
                            let value = option["currentValue"].as_str().map(str::to_string);
                            match option["category"].as_str() {
                                Some("model") => live.model = value,
                                Some("thought_level") => live.reasoning_effort = value,
                                _ => {}
                            }
                        }
                    }
                }
                if Some(config_id.as_str()) == reasoning_config_id(&live.config_options) {
                    live.reasoning_effort = Some(value.clone());
                }
                if let Some(info) = &live.harness_info {
                    // DSH drops an uncatalogued model once the session leaves it.
                    live.harness_models = super::deepseek::catalog(
                        models_from_config_options(&live.config_options),
                        &info["models"],
                        live.reasoning_effort.as_deref(),
                    );
                }
                if let Some(proj) = adapter_for(&live.adapter_id).project_session(&response) {
                    apply_projection(live, proj);
                }
            }
            SessionSettingOp::SetMode { mode_id } => {
                let params = json!({
                    "sessionId": live.session_id,
                    "modeId": mode_id
                });
                let response = match process.request("session/set_mode", params.clone()).await {
                    Ok(response) => response,
                    Err(_) => process.request("session/setMode", params).await?,
                };
                live.current_mode_id = Some(mode_id.clone());
                let (current, available) = parse_available_modes(&response);
                if !available.is_empty() {
                    live.available_modes = available;
                }
                if current.is_some() {
                    live.current_mode_id = current;
                }
            }
            SessionSettingOp::SetModel { model_id } => {
                if live.adapter_id == "grok" {
                    if let Some(models) = live.harness_state["availableModels"].as_array() {
                        let managed = models.iter().any(|m| {
                            m["modelId"]
                                .as_str()
                                .is_some_and(|id| id.starts_with("remote-codex/"))
                        });
                        if managed
                            && (!models.iter().any(|m| m["modelId"] == model_id)
                                || !models
                                    .iter()
                                    .any(|m| m["modelId"] == format!("remote-codex/{model_id}")))
                        {
                            bail!("Model is not available in this Grok session. Reconnect the session to refresh its upstream models.");
                        }
                    }
                }
                let response = process
                    .request(
                        "session/set_model",
                        json!({
                            "sessionId": live.session_id,
                            "modelId": model_id
                        }),
                    )
                    .await?;
                live.model = Some(if live.adapter_id == "grok" {
                    model_id
                        .strip_prefix("remote-codex/")
                        .unwrap_or(&model_id)
                        .to_owned()
                } else {
                    model_id.clone()
                });
                if let Some(obj) = live.harness_state.as_object_mut() {
                    obj.insert("currentModelId".into(), json!(model_id));
                }
                if live.adapter_id == "grok" {
                    // set_model may reset the harness setting without returning configOptions.
                    live.reasoning_effort = None;
                }
                if let Some(proj) = adapter_for(&live.adapter_id).project_session(&response) {
                    apply_projection(live, proj);
                }
            }
            SessionSettingOp::LoadWithMeta { meta } => {
                let response = process
                    .request(
                        "session/load",
                        json!({
                            "sessionId": live.session_id,
                            "cwd": live.cwd,
                            "mcpServers": [],
                            "_meta": meta
                        }),
                    )
                    .await?;
                if let Some(id) = response.get("sessionId").and_then(Value::as_str) {
                    live.session_id = id.to_string();
                }
                if let Some(proj) = adapter_for(&live.adapter_id).project_session(&response) {
                    apply_projection(live, proj);
                }
            }
        }
        Ok(())
    }

    fn policy_from_live(live: &LiveSession) -> ProductSessionPolicy {
        ProductSessionPolicy {
            collaboration_mode: live.collaboration_mode.clone(),
            sandbox_mode: live.sandbox_mode.clone(),
            approval_mode: Some(if live.yolo {
                "yolo".into()
            } else {
                "guarded".into()
            }),
        }
    }

    async fn apply_product_mode(process: &AcpProcess, live: &mut LiveSession) -> Result<()> {
        let policy = Self::policy_from_live(live);
        if let Some(bridge) = &live.codex_bridge {
            bridge.set_policy(&live.session_id, &policy).await?;
        }
        if let Some(bridge) = live.dsh.clone() {
            let state = Self::dsh_mode(
                &bridge,
                &live.session_id,
                &policy,
                live.dsh_env_preset.as_deref(),
            )
            .await?;
            Self::store_dsh_session(live, &state);
            return Ok(());
        }
        if let Some(mode_id) = resolve_mode(&live.available_modes, &policy) {
            if live.current_mode_id.as_deref() != Some(mode_id.as_str()) {
                Self::apply_setting_op(process, live, SessionSettingOp::SetMode { mode_id })
                    .await?;
            }
            return Ok(());
        }
        if let Some((config_id, value)) = resolve_mode_config_value(&live.config_options, &policy) {
            Self::apply_setting_op(
                process,
                live,
                SessionSettingOp::SetConfig { config_id, value },
            )
            .await?;
        }
        Ok(())
    }

    /// DSH has no ACP modes. Its own commands switch the session's permission
    /// preset (a user `permission.defaultPreset` overrides the launch env) and
    /// plan mode. Results are read back; without a way to apply the requested
    /// sandbox the call fails rather than running with a wider one.
    async fn dsh_mode(
        bridge: &super::deepseek::Bridge,
        session_id: &str,
        policy: &ProductSessionPolicy,
        env_preset: Option<&str>,
    ) -> Result<Value> {
        let read = || bridge.call("session", json!({"sessionId": session_id}));
        let has = |state: &Value, name: &str| {
            state["commands"]
                .as_array()
                .is_some_and(|commands| commands.iter().any(|command| command["name"] == name))
        };
        // A pending selection flips the logged mode at the next turn boundary.
        let plan_of = |state: &Value| {
            state["projections"]["plan"]["active"]
                .as_bool()
                .unwrap_or(false)
                != state["projections"]["plan"]["pending"]
                    .as_bool()
                    .unwrap_or(false)
        };
        let mut state = read().await?;
        let preset = super::deepseek::permission_preset(policy);
        let current = state["projections"]["permissions"]["currentValue"].as_str();
        match super::deepseek::permission_step(current, preset, has(&state, "permission"), env_preset)
        {
            super::deepseek::PermissionStep::Applied => {}
            super::deepseek::PermissionStep::Switch => {
                bridge
                    .command(session_id, &format!("/permission {preset}"))
                    .await?;
                state = read().await?;
                if state["projections"]["permissions"]["currentValue"].as_str() != Some(preset) {
                    bail!("DSH did not switch to the {preset} permission preset");
                }
            }
            super::deepseek::PermissionStep::Refuse => bail!(
                "This DSH profile cannot switch permissions live; restart the DSH session to apply {preset}"
            ),
        }
        let wants_plan = super::deepseek::wants_plan(policy);
        if wants_plan != plan_of(&state) {
            if has(&state, "plan") {
                bridge
                    .command(session_id, if wants_plan { "/plan" } else { "/plan off" })
                    .await?;
                state = read().await?;
                if wants_plan != plan_of(&state) {
                    bail!("DSH did not switch plan mode");
                }
            } else if wants_plan {
                bail!("This DSH session has no plan mode (its run mode or profile leaves it out)");
            } else {
                bail!("DSH plan mode is on but this session has no /plan command to leave it");
            }
        }
        Ok(state)
    }

    /// The idle session's operation lock and policy, for panel actions that
    /// must not interleave with a starting turn.
    async fn dsh_operation(
        &self,
        session_id: &str,
        process: &Arc<AcpProcess>,
    ) -> Result<(tokio::sync::OwnedMutexGuard<()>, ProductSessionPolicy)> {
        let sessions = self.inner.sessions.lock().await;
        let live = sessions
            .get(session_id)
            .filter(|live| Arc::ptr_eq(&live.process, process))
            .ok_or_else(|| anyhow!("ACP session changed during the harness action"))?;
        if live.active.is_some() {
            bail!("conflict: Wait for the turn to finish first");
        }
        let operation = live
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| anyhow!("conflict: The session is busy."))?;
        Ok((operation, Self::policy_from_live(live)))
    }

    /// Keep a bridge `session` reply: projection views plus panel metadata.
    fn store_dsh_session(live: &mut LiveSession, state: &Value) {
        if let Some(projections) = state["projections"].as_object() {
            live.dsh_projections.extend(projections.clone());
        }
        live.dsh_session = super::deepseek::session_meta(state);
    }

    async fn apply_live_settings(
        &self,
        session_key: &str,
        model: Option<&str>,
        effort: Option<&str>,
        sandbox: Option<&str>,
        collab: Option<&str>,
        approval: Option<&str>,
        performance: Option<bool>,
    ) -> Result<()> {
        let mut sessions = self.inner.sessions.lock().await;
        let Some(live) = sessions.get_mut(session_key) else {
            return Ok(());
        };
        if sandbox.is_some() {
            live.sandbox_mode = sandbox.map(str::to_string);
        }
        if collab.is_some() {
            live.collaboration_mode = collab.map(str::to_string);
        }
        if let Some(approval) = approval {
            live.yolo =
                approval == "yolo" || live.sandbox_mode.as_deref() == Some("danger-full-access");
        } else {
            live.yolo = live.yolo || live.sandbox_mode.as_deref() == Some("danger-full-access");
        }
        let adapter = adapter_for(&live.adapter_id);
        if let Some(model) = model.filter(|value| !value.is_empty() && *value != "default") {
            if live.model.as_deref() != Some(model)
                || (live.adapter_id == "grok"
                    && live.harness_state["currentModelId"].as_str() != Some(model))
            {
                if let Some(op) = adapter.apply_model(model, &live.harness_state) {
                    Self::apply_setting_op(&live.process.clone(), live, op).await?;
                } else {
                    Self::apply_setting_op(
                        &live.process.clone(),
                        live,
                        SessionSettingOp::SetConfig {
                            config_id: "model".into(),
                            value: model.to_string(),
                        },
                    )
                    .await?;
                }
            }
        }
        let grok_effort = if live.adapter_id == "grok" {
            effort.and_then(|effort| super::grok::resolve_effort(effort, &live.harness_state))
        } else {
            None
        };
        let effort = grok_effort.as_deref().or(effort);
        if let Some(effort) = effort.filter(|value| {
            *value != "auto" && (!value.is_empty() || live.adapter_id == "deepseek")
        }) {
            if live.reasoning_effort.as_deref() != Some(effort) {
                if let Some(op) = adapter.apply_reasoning(effort, &live.harness_state) {
                    Self::apply_setting_op(&live.process.clone(), live, op).await?;
                } else if let Some(config_id) = reasoning_config_id(&live.config_options) {
                    let config_id = config_id.to_string();
                    Self::apply_setting_op(
                        &live.process.clone(),
                        live,
                        SessionSettingOp::SetConfig {
                            config_id,
                            value: effort.to_string(),
                        },
                    )
                    .await?;
                } else if live.adapter_id == "grok" {
                    bail!("The selected Grok model does not advertise reasoning effort {effort}");
                }
            }
        }
        if let Some(enabled) = performance {
            Self::apply_fast_mode(&live.process.clone(), live, enabled).await?;
        }
        if !(sandbox.is_some() || collab.is_some() || approval.is_some()) {
            return Ok(());
        }
        let Some(bridge) = live.dsh.clone() else {
            return Self::apply_product_mode(&live.process.clone(), live).await;
        };
        // DSH commands may wait on its event loop; don't stall other sessions.
        let (raw_session, policy, env_preset, process) = (
            live.session_id.clone(),
            Self::policy_from_live(live),
            live.dsh_env_preset.clone(),
            live.process.clone(),
        );
        drop(sessions);
        let state = Self::dsh_mode(&bridge, &raw_session, &policy, env_preset.as_deref()).await?;
        if let Some(live) = self
            .inner
            .sessions
            .lock()
            .await
            .get_mut(session_key)
            .filter(|live| Arc::ptr_eq(&live.process, &process))
        {
            Self::store_dsh_session(live, &state);
        }
        Ok(())
    }

    /// Catalog changes take effect at the next idle operation, never mid-turn.
    async fn refresh_grok_session(&self, key: &str) -> Result<()> {
        let Some(upstreams) = &self.upstreams else {
            return Ok(());
        };
        if !key.starts_with("grok::") {
            return Ok(());
        }
        let Some((profile, models)) = upstreams.catalog("grok").await? else {
            return Ok(());
        };
        let revision = super::upstream_models::UpstreamModels::revision(&profile, &models)?;
        let _lifecycle = self.inner.lifecycle.lock().await;
        let mut sessions = self.inner.sessions.lock().await;
        let Some(live) = sessions.get_mut(key) else {
            return Ok(());
        };
        if live.upstream_revision.as_ref() == Some(&revision) {
            return Ok(());
        }
        if live.active.is_some() {
            bail!("conflict: Grok is busy; refresh after the turn finishes")
        }
        let _operation = live
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| anyhow!("conflict: Grok is busy"))?;
        let def = self.agent_def(Some("grok"))?;
        let (_, mut replacement) = self
            .spawn_session(
                &def,
                &live.cwd.to_string_lossy(),
                ProductSessionPolicy {
                    approval_mode: Some(if live.yolo { "yolo" } else { "guarded" }.into()),
                    sandbox_mode: live.sandbox_mode.clone(),
                    collaboration_mode: live.collaboration_mode.clone(),
                },
                Some(&live.session_id),
                None,
            )
            .await?;
        let adapter = adapter_for("grok");
        let restore = async {
            if let Some(model) = &live.model {
                if let Some(op) = adapter.apply_model(model, &replacement.harness_state) {
                    Self::apply_setting_op(&replacement.process.clone(), &mut replacement, op)
                        .await?;
                }
            }
            if let Some(effort) = &live.reasoning_effort {
                if let Some(op) = adapter.apply_reasoning(effort, &replacement.harness_state) {
                    Self::apply_setting_op(&replacement.process.clone(), &mut replacement, op)
                        .await?;
                }
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = restore {
            let _ = replacement.process.shutdown().await;
            return Err(error);
        }
        replacement.goal = live.goal.clone();
        replacement.load = live.load.clone();
        live.process.shutdown().await?;
        *live = replacement;
        Ok(())
    }

    async fn apply_fast_mode(
        process: &AcpProcess,
        live: &mut LiveSession,
        enabled: bool,
    ) -> Result<()> {
        let option = live.config_options.as_array().and_then(|options| {
            options.iter().find(|option| {
                matches!(
                    option.get("id").and_then(Value::as_str),
                    Some("fast" | "fast-mode")
                )
            })
        });
        let Some(option) = option else {
            if enabled {
                bail!("the selected ACP agent does not expose fast mode");
            }
            return Ok(());
        };
        let config_id = option
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("ACP fast mode config is missing its id"))?
            .to_string();
        let boolean = option.get("type").and_then(Value::as_str) == Some("boolean");
        let value = if boolean {
            json!(enabled)
        } else {
            let wanted = if enabled {
                ["on", "true", "fast"].as_slice()
            } else {
                ["off", "false", "standard"].as_slice()
            };
            let selected = collect_select_options(option)
                .into_iter()
                .find_map(|entry| {
                    let value = entry.get("value").and_then(Value::as_str)?;
                    wanted
                        .iter()
                        .any(|candidate| value.eq_ignore_ascii_case(candidate))
                        .then(|| value.to_string())
                })
                .ok_or_else(|| anyhow!("ACP fast mode config has no compatible value"))?;
            json!(selected)
        };
        if option.get("currentValue") == Some(&value) {
            return Ok(());
        }
        let mut params = json!({
            "sessionId": live.session_id,
            "configId": config_id,
            "value": value
        });
        if boolean {
            params["type"] = json!("boolean");
        }
        let response = process.request("session/set_config_option", params).await?;
        if let Some(options) = response.get("configOptions") {
            live.config_options = options.clone();
        }
        Ok(())
    }

    async fn caps_for(&self, agent_id: Option<&str>) -> AgentProviderCapabilitiesDto {
        let id = agent_id
            .map(str::to_string)
            .or_else(|| self.bound_agent.clone())
            .unwrap_or_else(|| "acp".into());
        self.inner
            .caps_by_agent
            .lock()
            .await
            .get(&id)
            .cloned()
            .unwrap_or_else(|| {
                let mut caps = AgentProviderCapabilitiesDto::conversational();
                adapter_for(&id).patch_capabilities(&mut caps, &NegotiatedCaps::default());
                caps
            })
    }
}

#[async_trait]
impl AgentRuntime for AcpRuntime {
    fn provider(&self) -> Provider {
        self.provider
    }

    fn descriptor(&self) -> AgentBackendDto {
        let started = self.started_at.try_lock().ok().and_then(|g| g.clone());
        let bound = self.bound_agent.clone().unwrap_or_else(|| "acp".into());
        let def = self.agent_def(Some(&bound)).ok();
        let availability = def
            .as_ref()
            .map(classify_availability)
            .unwrap_or("base_missing");
        let installed = availability == "ready" || self.provider == Provider::Acp;
        let verified = {
            let health = self.inner.health.lock().unwrap();
            if self.provider == Provider::Acp {
                health.values().any(Option::is_none)
            } else {
                health.get(&bound) == Some(&None)
            }
        };
        let last_error = self.inner.health.lock().unwrap().get(&bound).cloned().flatten().or_else(|| {
            def.as_ref().and_then(|def| match availability {
                "adapter_missing" => Some(format!("Missing executable {} (package {}). Install in the new-thread dialog or Settings > Harnesses > ACP adapter.", def.server_command, super::dependencies::package(&def.id).unwrap_or("unknown"))),
                "base_missing" => Some(format!("Missing base executable: {}", def.base_command)),
                "server_unavailable" => Some(format!("Missing ACP executable: {}", def.server_command)),
                _ => None,
            })
        });
        let caps = self
            .inner
            .caps_by_agent
            .try_lock()
            .ok()
            .and_then(|g| g.get(&bound).cloned())
            .unwrap_or_else(|| {
                let mut caps = AgentProviderCapabilitiesDto::conversational();
                adapter_for(&bound).patch_capabilities(&mut caps, &NegotiatedCaps::default());
                caps
            });
        let mut caps = caps;
        caps.sessions.import_local = true;
        let mut schema = AgentBackendManagementSchemaDto::default();
        schema.toolbox_items = toolbox_from_capabilities(&caps);
        AgentBackendDto {
            provider: self.provider,
            display_name: def
                .as_ref()
                .map(|d| d.display_name.clone())
                .unwrap_or_else(|| "ACP Agent".into()),
            description: "Generic ACP runtime with per-harness adapters.".into(),
            enabled: installed || availability == "adapter_missing",
            is_default: self.provider == Provider::Codex || self.provider == Provider::Acp,
            status: AgentRuntimeStatusDto {
                state: if last_error.is_some() {
                    "degraded"
                } else if verified {
                    "ready"
                } else {
                    "stopped"
                }
                .into(),
                transport: "stdio".into(),
                last_started_at: started,
                last_error: last_error.clone(),
                restart_count: 0,
            },
            capabilities: caps,
            management_schema: schema,
            installation: AgentBackendInstallationDto {
                package_name: def.as_ref().map(|d| d.base_command.clone()),
                installed,
                installed_version: None,
                latest_version: None,
                install_command: def.and_then(|d| d.install_command),
                update_command: None,
                busy: false,
                last_error: last_error.clone(),
            },
        }
    }

    fn negotiated_caps(&self, agent_id: Option<&str>) -> AgentProviderCapabilitiesDto {
        let id = agent_id
            .map(str::to_string)
            .or_else(|| self.bound_agent.clone())
            .unwrap_or_else(|| "acp".into());
        self.inner
            .caps_by_agent
            .try_lock()
            .ok()
            .and_then(|g| g.get(&id).cloned())
            .unwrap_or_else(|| {
                let mut caps = AgentProviderCapabilitiesDto::conversational();
                adapter_for(&id).patch_capabilities(&mut caps, &NegotiatedCaps::default());
                caps
            })
    }

    fn toolbox(&self, agent_id: Option<&str>) -> Vec<ToolboxItemDto> {
        let id = agent_id
            .map(str::to_string)
            .or_else(|| self.bound_agent.clone())
            .unwrap_or_else(|| "acp".into());
        self.inner
            .toolbox_by_agent
            .try_lock()
            .ok()
            .and_then(|toolboxes| toolboxes.get(&id).cloned())
            .unwrap_or_else(|| toolbox_from_capabilities(&self.negotiated_caps(Some(&id))))
    }

    async fn restart(&self, agent_id: &str) -> Result<usize> {
        if self.bound_agent.as_deref().is_some_and(|id| id != agent_id) {
            return Ok(0);
        }
        self.agent_def(Some(agent_id))?;
        let _lifecycle = self.inner.lifecycle.lock().await;
        let mut sessions = self.inner.sessions.lock().await;
        let keys: Vec<_> = sessions
            .iter()
            .filter(|(_, live)| live.adapter_id == agent_id)
            .map(|(key, _)| key.clone())
            .collect();
        let mut guards = Vec::new();
        for key in &keys {
            let live = &sessions[key];
            if live.active.is_some() {
                bail!("conflict: Harness is running a turn. Stop it or wait before restarting.");
            }
            guards.push(
                live.operation
                    .clone()
                    .try_lock_owned()
                    .map_err(|_| anyhow!("conflict: Harness is busy"))?,
            );
        }
        for key in &keys {
            if let Some(live) = sessions.remove(key) {
                live.process.shutdown().await?;
                self.announce_release(key, &live, crate::actor::ReleaseReason::Explicit);
            }
        }
        self.inner
            .commands
            .lock()
            .await
            .retain(|(agent, _), _| agent != agent_id);
        self.inner.caps_by_agent.lock().await.remove(agent_id);
        self.inner.toolbox_by_agent.lock().await.remove(agent_id);
        if let Some(upstreams) = &self.upstreams {
            upstreams.invalidate(agent_id).await;
        }
        *self.started_at.lock().await = Some(now_rfc3339());
        Ok(keys.len())
    }

    async fn invalidate_models(&self, agent_id: &str) {
        if let Some(upstreams) = &self.upstreams {
            upstreams.invalidate(agent_id).await;
        }
    }

    async fn start(&self) -> Result<()> {
        *self.started_at.lock().await = Some(now_rfc3339());
        Ok(())
    }

    async fn list_models(
        &self,
        agent_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<Vec<ModelOptionDto>> {
        let def = match self.agent_def(agent_id) {
            Ok(def) => def,
            Err(_) => return Ok(default_model_stub(agent_id.or(self.bound_agent.as_deref()))),
        };
        // DSH's own ACP options name its models (`["provider","model"]`) and
        // already reflect an applied upstream; plain catalog ids are not values.
        let upstreams = self.upstreams.as_ref().filter(|_| def.id != "deepseek");
        if let Some(upstreams) = upstreams {
            if let Some((profile, models)) = upstreams.catalog(&def.id).await? {
                let known = if def.id == "grok" {
                    if models.iter().any(|m| m.reasoning.is_none()) {
                        let _lifecycle = self.inner.lifecycle.lock().await;
                        let revision =
                            super::upstream_models::UpstreamModels::revision(&profile, &models)?;
                        if let Some(known) = upstreams.native_models(&revision).await {
                            Some(known)
                        } else {
                            let known = self.probe_models(&def, cwd.unwrap_or(".")).await?;
                            upstreams.cache_native(revision, known.clone()).await;
                            Some(known)
                        }
                    } else {
                        None
                    }
                } else {
                    self.models_from_live(&def.id).await
                };
                return Ok(models
                    .iter()
                    .map(|model| {
                        let fallback = known
                            .as_ref()
                            .and_then(|known| known.iter().find(|m| m.model == model.id));
                        model.option(&profile, fallback)
                    })
                    .collect());
            }
        }
        if classify_availability(&def) != "ready" {
            return Ok(default_model_stub(Some(&def.id)));
        }
        if let Some(models) = self.models_from_live(&def.id).await {
            if !models.is_empty() {
                return Ok(models);
            }
        }
        if let Some(cwd) = cwd.filter(|value| !value.trim().is_empty()) {
            let models = self
                .probe_models(&def, cwd)
                .await
                .map_err(|error| anyhow!("Probe {} models: {error:#}", def.display_name))?;
            if !models.is_empty() {
                return Ok(models);
            }
        }
        let command_models = list_command_models(&def);
        if !command_models.is_empty() {
            return Ok(command_models);
        }
        Ok(default_model_stub(Some(&def.id)))
    }

    async fn list_agents(&self) -> Result<Vec<ModelOptionDto>> {
        if self.bound_agent.is_some() {
            return Ok(vec![]);
        }
        Ok(futures_util::future::join_all(builtin_agents(self.custom_command.as_deref())
            .into_iter()
            .map(|entry| async move {
                let availability = classify_availability(&entry);
                let (base, server) = tokio::join!(
                    crate::management::inspect(&entry.base_command, &entry.id),
                    crate::management::inspect(&entry.server_command, &entry.id));
                let health = self.inner.health.lock().unwrap().get(&entry.id).cloned();
                let verified = availability == "ready" && matches!(health, Some(None));
                ModelOptionDto {
                    id: entry.id.clone(),
                    model: entry.id.clone(),
                    display_name: entry.display_name.clone(),
                    description: entry.description.clone(),
                    is_default: entry.id == "codex" || entry.id == "grok",
                    hidden: false,
                    supported_reasoning_efforts: vec![],
                    default_reasoning_effort: None,
                    selection_kind: Some("agent".into()),
                    acp_agent: Some(json!({
                        "id": entry.id,
                        "displayName": entry.display_name,
                        "transport": entry.transport,
                        "availability": availability,
                        "baseCommand": entry.base_command,
                        "baseProbeCommand": format!("{} --version", entry.base_command),
                        "serverCommand": entry.server_command,
                        "serverProbeCommand": format!("{} --help", entry.server_command),
                        "baseVersion": base.as_ref().ok().and_then(|found| found.version.clone()),
                        "basePath": base.as_ref().ok().map(|found| &found.path),
                        "serverVersion": server.as_ref().ok().and_then(|found| found.version.clone()),
                        "serverPath": server.as_ref().ok().map(|found| &found.path),
                        "connectionStatus": if availability != "ready" { "unavailable" } else if verified { "verified" } else if health.is_some() { "failed" } else { "unverified" },
                        "connectionError": health.clone().flatten(),
                        "installCommand": entry.install_command,
                        "busy": false,
                        "statusMessage": match availability {
                            "ready" => match health { Some(Some(error)) => format!("ACP verification failed: {error}"), Some(None) => "Ready".into(), None => "Installed · ACP connection is checked when selecting a model".into() },
                            "adapter_missing" => format!(
                                "Base agent detected. Install its ACP adapter: {}",
                                entry.install_command.as_deref().unwrap_or(entry.server_command.as_str())
                            ),
                            "base_missing" => format!("Install the base agent first: {}", entry.base_command),
                            _ => format!("{} is unavailable.", entry.display_name),
                        },
                    })),
                }
            })
            ).await)
    }

    async fn capabilities(&self, agent_id: Option<&str>) -> Result<AgentCapabilitySnapshotDto> {
        let id = agent_id
            .map(str::to_string)
            .or_else(|| self.bound_agent.clone())
            .unwrap_or_else(|| "acp".into());
        let availability = self
            .agent_def(Some(&id))
            .ok()
            .map(|def| classify_availability(&def))
            .unwrap_or("base_missing");
        let effective = if availability == "ready" {
            Some(self.caps_for(Some(&id)).await)
        } else {
            None
        };
        let toolbox_items = self.toolbox(Some(&id));
        Ok(AgentCapabilitySnapshotDto {
            provider: self.provider,
            agent_id: id,
            availability: availability.into(),
            negotiated: None,
            effective_capabilities: effective,
            toolbox_items,
        })
    }

    async fn session_capabilities(
        &self,
        agent_id: Option<&str>,
        session_id: &str,
    ) -> Result<AgentCapabilitySnapshotDto> {
        let mut snapshot = self.capabilities(agent_id).await?;
        let sessions = self.inner.sessions.lock().await;
        if let Some(live) = sessions.values().find(|live| {
            session_ids_match(
                &Self::scoped_id(&live.adapter_id, &live.session_id),
                session_id,
            )
        }) {
            let mut negotiated = live.negotiated.clone();
            if let Some(commands) = self
                .inner
                .commands
                .lock()
                .await
                .get(&(live.process.id.clone(), live.session_id.clone()))
            {
                negotiated.available_commands = commands.clone();
            }
            let adapter = adapter_for(&live.adapter_id);
            let mut caps = AgentProviderCapabilitiesDto::conversational();
            adapter.patch_capabilities(&mut caps, &negotiated);
            if let Some(info) = &live.harness_info {
                super::deepseek::patch_capabilities(&mut caps, info, &live.dsh_session);
            }
            apply_config_option_caps(&mut caps, &live.config_options);
            snapshot.toolbox_items = adapter.toolbox_items(&caps, &negotiated);
            snapshot.effective_capabilities = Some(caps);
            if let Some(info) = &live.harness_info {
                let harness = super::deepseek::harness_view(
                    info,
                    &live.dsh_projections,
                    &live.dsh_session,
                    live.dsh_running && live.active.is_none(),
                );
                snapshot.negotiated =
                    Some(json!({"harness":harness,"configOptions":live.config_options}));
                snapshot.toolbox_items.push(ToolboxItemDto {
                    action: "harness".into(),
                    command: "/harness".into(),
                    label: "Harness settings".into(),
                    description: Some("Session configuration and loaded plugins".into()),
                    panel: None,
                });
            }
        }
        Ok(snapshot)
    }

    async fn start_session(&self, input: StartSessionInput) -> Result<StartSessionResult> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let def = self.agent_def(input.agent_id.as_deref())?;
        let (scoped, mut live) = self
            .spawn_session(
                &def,
                &input.cwd,
                ProductSessionPolicy {
                    collaboration_mode: None,
                    sandbox_mode: input.sandbox_mode.clone(),
                    approval_mode: Some(input.approval_mode.clone()),
                },
                None,
                input.reasoning_effort.as_deref(),
            )
            .await?;
        let adapter = adapter_for(&live.adapter_id);
        if !input.model.is_empty() && input.model != "default" {
            if let Some(op) = adapter.apply_model(&input.model, &live.harness_state) {
                if let Err(error) =
                    Self::apply_setting_op(&live.process.clone(), &mut live, op).await
                {
                    let _ = live.process.shutdown().await;
                    return Err(error);
                }
            } else {
                let _ = Self::apply_setting_op(
                    &live.process.clone(),
                    &mut live,
                    SessionSettingOp::SetConfig {
                        config_id: "model".into(),
                        value: input.model.clone(),
                    },
                )
                .await;
                live.model = Some(input.model.clone());
            }
        }
        let grok_effort = if live.adapter_id == "grok" {
            input
                .reasoning_effort
                .as_deref()
                .and_then(|effort| super::grok::resolve_effort(effort, &live.harness_state))
        } else {
            None
        };
        if let Some(effort) = grok_effort
            .as_deref()
            .or(input.reasoning_effort.as_deref())
            .filter(|value| {
                *value != "auto" && (!value.is_empty() || live.adapter_id == "deepseek")
            })
        {
            if live.reasoning_effort.as_deref() != Some(effort) {
                if let Some(op) = adapter.apply_reasoning(effort, &live.harness_state) {
                    if let Err(error) =
                        Self::apply_setting_op(&live.process.clone(), &mut live, op).await
                    {
                        let _ = live.process.shutdown().await;
                        return Err(error);
                    }
                } else if let Some(config_id) = reasoning_config_id(&live.config_options) {
                    let config_id = config_id.to_string();
                    Self::apply_setting_op(
                        &live.process.clone(),
                        &mut live,
                        SessionSettingOp::SetConfig {
                            config_id,
                            value: effort.to_string(),
                        },
                    )
                    .await?;
                }
            }
        }
        let model = live.model.clone().or(Some(input.model.clone()));
        let effort = live.reasoning_effort.clone();
        self.announce_load(&scoped, &live);
        self.inner
            .sessions
            .lock()
            .await
            .insert(scoped.clone(), live);
        Ok(StartSessionResult {
            provider_session_id: scoped,
            model,
            reasoning_effort: effort,
        })
    }

    async fn resume_session(
        &self,
        session_id: &str,
        cwd: Option<&str>,
        settings: SessionSettings,
    ) -> Result<StartSessionResult> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let old_process = {
            let mut sessions = self.inner.sessions.lock().await;
            let existing = sessions
                .keys()
                .find(|key| session_ids_match(key, session_id))
                .cloned();
            if let Some(existing) = existing {
                let live = &sessions[&existing];
                if cli_context_matches(&live.process.cli_env)
                    && live.process.connection_open().await
                {
                    return Ok(StartSessionResult {
                        provider_session_id: existing,
                        model: None,
                        reasoning_effort: None,
                    });
                }
                // A fork may share its parent's process. Load it independently before exposing
                // process-level CLI identity; never replace the parent's environment or stop it.
                if live.active.is_some() {
                    bail!("conflict: Cannot rebind an active session");
                }
                let removed = sessions.remove(&existing).unwrap();
                if sessions
                    .values()
                    .any(|other| Arc::ptr_eq(&other.process, &removed.process))
                {
                    None
                } else {
                    Some(removed.process)
                }
            } else {
                None
            }
        };
        if let Some(process) = old_process {
            process.shutdown().await?;
        }
        let (agent_id, raw) = match session_id.split_once("::") {
            Some((agent, rest)) if !agent.is_empty() && !rest.is_empty() => {
                (agent.to_string(), rest.to_string())
            }
            _ => (
                self.bound_agent.clone().unwrap_or_else(|| "codex".into()),
                session_id.to_string(),
            ),
        };
        let def = self.agent_def(Some(&agent_id))?;
        let cwd = cwd
            .map(str::to_string)
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| ".".into());
        let (scoped, live) = self
            .spawn_session(
                &def,
                &cwd,
                ProductSessionPolicy {
                    approval_mode: settings.approval_mode,
                    sandbox_mode: settings.sandbox_mode,
                    collaboration_mode: settings.collaboration_mode,
                },
                Some(raw.as_str()),
                None,
            )
            .await?;
        self.announce_load(&scoped, &live);
        self.inner
            .sessions
            .lock()
            .await
            .insert(scoped.clone(), live);
        Ok(StartSessionResult {
            provider_session_id: scoped,
            model: None,
            reasoning_effort: None,
        })
    }

    async fn execution_state(&self, session_id: &str) -> crate::actor::ExecutionState {
        let sessions = self.inner.sessions.lock().await;
        let Some(live) = sessions.get(session_id) else {
            return crate::actor::ExecutionState::Unknown;
        };
        adapter_for(&live.adapter_id).execution_state(
            live.process.connection_open().await,
            live.active.as_ref().map(|turn| turn.turn_id.as_str()),
        )
    }

    async fn active_subagents(&self, session_id: &str) -> Vec<ThreadSubagentDto> {
        self.inner
            .sessions
            .lock()
            .await
            .get(session_id)
            .map(|live| live.active_subagents.values().cloned().collect())
            .unwrap_or_default()
    }

    async fn start_turn(
        &self,
        input: StartTurnInput,
        bus: EventBus,
        cancel: CancellationToken,
    ) -> Result<Vec<ThreadHistoryItemDto>> {
        self.refresh_grok_session(&input.provider_session_id)
            .await?;
        let operation = self
            .inner
            .sessions
            .lock()
            .await
            .get(&input.provider_session_id)
            .ok_or_else(|| anyhow!("ACP session is not running"))?
            .operation
            .clone();
        let _operation = operation
            .try_lock()
            .map_err(|_| anyhow!("conflict: The session is busy."))?;
        {
            let sessions = self.inner.sessions.lock().await;
            let live = sessions
                .get(&input.provider_session_id)
                .ok_or_else(|| anyhow!("ACP session is not running"))?;
            if live.active.is_some() {
                bail!("ACP session already has an active turn");
            }
        }
        self.apply_live_settings(
            &input.provider_session_id,
            input.model.as_deref(),
            input.reasoning_effort.as_deref(),
            input.sandbox_mode.as_deref(),
            input.collaboration_mode.as_deref(),
            input.approval_mode.as_deref(),
            input.performance_mode,
        )
        .await?;
        let (
            process,
            session_id,
            cwd,
            image_capable,
            adapter_id,
            harness_state,
            needs_context,
            dsh,
            mut dsh_goal_active,
            dsh_title,
        ) = {
            let sessions = self.inner.sessions.lock().await;
            let live = sessions
                .get(&input.provider_session_id)
                .ok_or_else(|| anyhow!("ACP session is not running"))?;
            (
                live.process.clone(),
                live.session_id.clone(),
                live.cwd.clone(),
                live.negotiated.image,
                live.adapter_id.clone(),
                live.harness_state.clone(),
                live.context_thread_id.as_deref() != Some(input.thread_id.as_str())
                    && !input.context_delivered,
                live.dsh.clone(),
                live.dsh_projections
                    .get("goal")
                    .and_then(super::deepseek::goal_state)
                    .is_some_and(|goal| goal.status == "active"),
                live.dsh_title.clone(),
            )
        };
        // DSH titles a session from its first prompt, which carries the Remote
        // Codex context; give it the thread's title instead. Best effort.
        let title = input
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty());
        if let (Some(bridge), Some(title)) = (&dsh, title) {
            if dsh_title.as_deref() != Some(title) {
                let renamed = tokio::time::timeout(
                    Duration::from_secs(2),
                    bridge.call("rename", json!({"sessionId": session_id, "title": title})),
                )
                .await;
                if matches!(renamed, Ok(Ok(_))) {
                    if let Some(live) = self
                        .inner
                        .sessions
                        .lock()
                        .await
                        .get_mut(&input.provider_session_id)
                    {
                        live.dsh_title = Some(title.to_string());
                    }
                } else {
                    tracing::debug!(session_id, "DSH session title was not updated");
                }
            }
        }
        // The goal wire prompt is a DSH command: it starts autonomous rounds
        // that this turn follows until DSH is idle. ACP does not parse commands.
        let dsh_goal_command = dsh
            .as_ref()
            .filter(|_| input.prompt.starts_with("/goal "))
            .cloned();
        let adapter = adapter_for(&adapter_id);
        let prompt = adapter
            .prompt_preamble()
            .map(|preamble| format!("{preamble}\n\n{}", input.prompt))
            .unwrap_or_else(|| input.prompt.clone());
        // A DSH goal command sends no prompt, so it cannot deliver the context.
        let include_context = needs_context
            && !input.hidden
            && !process.cli_env.is_empty()
            && dsh_goal_command.is_none();
        let prompt = if include_context {
            format!("[remoteCodex: this conversation is a Pockymoe thread; `pockymoe thread self` shows its identity. Before you create or message threads, or show the user a local web app, read `pockymoe skill` (short: rules and an index); every command's `--help` has its flags and rules. Never put credentials in prompts or output.]\n\n{prompt}")
        } else {
            prompt
        };
        let prompt_blocks = build_prompt_blocks(&prompt, &cwd, image_capable, &input.images)?;
        if !input.hidden {
            // Settings may have reloaded the process; identify the actual writer
            // selected for this turn, rather than the one before preflight.
            bus.emit(ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadHarnessReady,
                thread_id: input.thread_id.clone(),
                timestamp: now_rfc3339(),
                payload: json!({"turnId":input.turn_id,"instanceId":process.id}),
            });
        }
        let mut updates = self.inner.updates.subscribe();
        let (continuation_tx, mut continuation_rx) = mpsc::unbounded_channel();
        // Before the first prompt after loading, the tree holds only the harness
        // and its MCP servers; later processes are the agent's tool commands.
        let first_prompt = !input.hidden
            && self
                .inner
                .sessions
                .lock()
                .await
                .get(&input.provider_session_id)
                .is_some_and(|live| !live.load.prompted);
        let baseline = match process.pid.filter(|_| first_prompt) {
            Some(pid) => tokio::task::spawn_blocking(move || {
                crate::harness_processes::tree_now(&[(pid, None)])
            })
            .await
            .unwrap_or_default(),
            None => Vec::new(),
        };
        let claude_sequence = {
            let mut sessions = self.inner.sessions.lock().await;
            let live = sessions
                .get_mut(&input.provider_session_id)
                .ok_or_else(|| anyhow!("ACP session is not running"))?;
            if live.active.is_some() {
                bail!("ACP session already has an active turn");
            }
            live.active = Some(ActiveTurn {
                thread_id: input.thread_id.clone(),
                turn_id: input.turn_id.clone(),
                bus: bus.clone(),
                background_owned: false,
                cancel: cancel.clone(),
                pending_prompts: 0,
                continuations: continuation_tx,
            });
            if input.hidden {
                live.load.touch();
            } else {
                if !live.load.prompted {
                    live.load.baseline.extend(baseline);
                }
                live.load.prompt();
            }
            live.active_subagents.clear();
            if let Some(meta) = live.dsh_session.as_object_mut() {
                // DSH fixes the run mode at its first turn; the next read confirms.
                meta.insert("presetLocked".into(), Value::Bool(true));
            }
            live.claude_lifecycle.begin_turn()
        };
        let mut adapter_usage = super::usage::AdapterUsageAccumulator::default();
        let mut usage_reader =
            (adapter_id == "codex").then(|| super::usage::CodexUsageReader::new(&session_id));
        let mut claude_usage_reader = (adapter_id == "claude")
            .then(|| super::claude_usage::ClaudeUsageReader::new(&session_id));
        if let Some(reader) = claude_usage_reader
            .as_mut()
            .filter(|_| input.images.is_empty())
        {
            reader.expect_prompt(&prompt);
        }
        let mut usage_tick = tokio::time::interval(Duration::from_secs(1));
        let mut dsh_events = dsh.as_ref().map(|bridge| bridge.subscribe());
        let mut dsh_running = false;
        let prompt_rpc: std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>,
        > = if let Some(bridge) = dsh_goal_command {
            let (session_id, line) = (session_id.clone(), input.prompt.clone());
            dsh_goal_active = true;
            Box::pin(async move {
                bridge.command(&session_id, &line).await?;
                Ok(json!({"stopReason":"end_turn"}))
            })
        } else {
            Box::pin(process.request(
                "session/prompt",
                json!({
                    "sessionId": session_id,
                    "prompt": prompt_blocks
                }),
            ))
        };
        let mut mapper = TurnMapper::new(&input.turn_id);
        if !input.hidden {
            bus.emit(ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadTurnStarted,
                thread_id: input.thread_id.clone(),
                timestamp: now_rfc3339(),
                payload: json!({ "turnId": input.turn_id }),
            });
        }
        let mut prompt_requests = FuturesUnordered::new();
        prompt_requests.push(Box::pin(async move { (false, prompt_rpc.await) })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = (bool, Result<Value>)> + Send + '_>,
            >);
        let mut prompt_done = false;
        let mut cancel_sent = false;
        let mut cancel_deadline = tokio::time::Instant::now();
        let mut discard_session = false;
        let mut pending_claude_error = None;
        let mut coalesced_ready_since = None;
        let mut draining_coalesced = false;
        let mut outcome = loop {
            let (sdk_pending, sdk_waiting, pending_prompts) = if adapter_id == "claude" {
                let mut sessions = self.inner.sessions.lock().await;
                sessions
                    .get_mut(&input.provider_session_id)
                    .map(|live| {
                        let sdk_pending = live.claude_lifecycle.background_pending();
                        if let Some(active) = live.active.as_mut() {
                            active.background_owned = sdk_pending
                                || claude_usage_reader
                                    .as_ref()
                                    .is_some_and(|reader| reader.background_pending());
                        }
                        (
                            sdk_pending,
                            live.claude_lifecycle.waiting_count(),
                            live.active
                                .as_ref()
                                .map_or(0, |active| active.pending_prompts),
                        )
                    })
                    .unwrap_or_default()
            } else {
                (false, 0, 0)
            };
            let native_pending = claude_usage_reader
                .as_ref()
                .is_some_and(|reader| reader.background_pending());
            let claude_background_pending = sdk_pending || native_pending;
            let waiting = sdk_waiting.max(
                claude_usage_reader
                    .as_ref()
                    .map_or(0, |reader| reader.waiting_count()),
            );
            if waiting > 0 && !cancel_sent {
                emit_mapped(
                    &bus,
                    &input.thread_id,
                    &input.turn_id,
                    mapper.native_background_wait(waiting),
                    input.hidden,
                );
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled(), if !cancel_sent => {
                    cancel_sent = true;
                    cancel_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                    let _ = process.notify("session/cancel", json!({ "sessionId": session_id })).await;
                }
                _ = tokio::time::sleep_until(cancel_deadline), if cancel_sent && !prompt_done => {
                    // Never reuse a session while its previous prompt may still be
                    // executing. A stalled cancellation must resume a fresh process.
                    let _ = process.shutdown().await;
                    discard_session = true;
                    break TurnOutcome::Interrupted;
                }
                continuation = continuation_rx.recv(), if adapter_id == "claude" && !cancel_sent => {
                    if let Some(response) = continuation {
                        prompt_requests.push(Box::pin(async move { (true, response.await) }));
                        prompt_done = false;
                    }
                }
                result = prompt_requests.next(), if !prompt_requests.is_empty() => {
                    let (continuation, result) = result.expect("nonempty prompt response queue");
                    if continuation {
                        let mut sessions = self.inner.sessions.lock().await;
                        if let Some(active) = sessions.get_mut(&input.provider_session_id).and_then(|live| live.active.as_mut()) {
                            active.pending_prompts = active.pending_prompts.saturating_sub(1);
                        }
                    }
                    match result {
                        Ok(response) => {
                            if let Some(usage) = adapter.billing_usage(&response).and_then(|usage| adapter_usage.apply(usage)) {
                                emit_usage(&bus, &input.thread_id, &input.turn_id, usage, input.hidden);
                            }
                            if let Some(context) = adapter.context_usage(&response, &harness_state) {
                                emit_usage(&bus, &input.thread_id, &input.turn_id, context, input.hidden);
                            }
                            if let Some(reader) = usage_reader.as_mut() {
                                for usage in reader.poll_final() { emit_usage(&bus, &input.thread_id, &input.turn_id, usage, input.hidden); }
                            }
                            if let Some(reader) = claude_usage_reader.as_mut() {
                                for usage in reader.poll_final() { emit_usage(&bus, &input.thread_id, &input.turn_id, usage, input.hidden); }
                                for notice in reader.take_task_notices() {
                                    emit_mapped(&bus, &input.thread_id, &input.turn_id, mapper.native_task_notification(&notice), input.hidden);
                                }
                            }
                            if let Some(usage) = response.get("usage").filter(|v| v.is_object() && !matches!(adapter_id.as_str(), "codex" | "copilot")) {
                                emit_usage(&bus, &input.thread_id, &input.turn_id, usage.clone(), input.hidden);
                            }
                            if response.get("stopReason").and_then(Value::as_str) == Some("cancelled") && !draining_coalesced {
                                cancel.cancel();
                                cancel_sent = true;
                            }
                            if cancel_sent {
                                prompt_done = prompt_requests.is_empty();
                                continue;
                            }
                            prompt_done = prompt_requests.is_empty();
                        },
                        Err(err) => {
                            if cancel_sent || cancel.is_cancelled() {
                                discard_session = process.exited().await.unwrap_or(true);
                                break TurnOutcome::Interrupted;
                            }
                            if draining_coalesced && serde_json::from_str::<Value>(&err.to_string()).ok()
                                .is_some_and(|error| error["code"] == -32800) {
                                prompt_done = prompt_requests.is_empty();
                                continue;
                            }
                            if adapter_id == "claude"
                                && super::claude_completion::incomplete_tool_ids(&err.to_string()).is_some()
                                && process.connection_open().await
                            {
                                // Drain queued final text/tool updates before comparing
                                // against the native transcript. This is not a retry.
                                pending_claude_error = Some(err);
                                prompt_done = prompt_requests.is_empty();
                                continue;
                            }
                            tracing::warn!(
                                error = %err,
                                session_id = %session_id,
                                "ACP session/prompt failed"
                            );
                            let failure = if process.connection_open().await {
                                anyhow!("ACP session/prompt failed: {err}")
                            } else {
                                crate::actor::ExecutionUncertain(err.to_string()).into()
                            };
                            break TurnOutcome::Failed(failure);
                        }
                    }
                }
                _ = usage_tick.tick(), if usage_reader.is_some() || claude_usage_reader.is_some() => {
                    if let Some(reader) = usage_reader.as_mut() {
                        for usage in reader.poll() { emit_usage(&bus, &input.thread_id, &input.turn_id, usage, input.hidden); }
                    }
                    if let Some(reader) = claude_usage_reader.as_mut() {
                        for usage in reader.poll() { emit_usage(&bus, &input.thread_id, &input.turn_id, usage, input.hidden); }
                        for notice in reader.take_task_notices() {
                            emit_mapped(&bus, &input.thread_id, &input.turn_id, mapper.native_task_notification(&notice), input.hidden);
                        }
                        let subagents = merge_active_subagents(mapper.active_subagents(), Some(reader));
                        if self.update_subagents(&input, &subagents).await {
                            emit_mapped(&bus, &input.thread_id, &input.turn_id,
                                MappedUpdate { active_subagents: Some(subagents), ..Default::default() }, input.hidden);
                        }
                        if !prompt_done && !cancel_sent && !draining_coalesced {
                            let ready = match reader.completed_queued_command(mapper.final_agent_text()) {
                                Some(id) if mapper.active_subagents().is_empty() => self.inner.sessions.lock().await
                                    .get(&input.provider_session_id).is_some_and(|live|
                                        live.process.id == process.id && live.claude_lifecycle.completed_coalesced_command(id, claude_sequence)),
                                _ => false,
                            };
                            if ready {
                                let since = coalesced_ready_since.get_or_insert_with(tokio::time::Instant::now);
                                if since.elapsed() >= Duration::from_secs(3) {
                                    // This command already ran in an autonomous SDK cycle.
                                    // Drain the stranded ACP request and await its response;
                                    // never resubmit work or cancel a live background task.
                                    // Serialize the last check/write against steering so
                                    // new user input cannot be swept into housekeeping.
                                    let mut sessions = self.inner.sessions.lock().await;
                                    if !cancel.is_cancelled() && sessions.get_mut(&input.provider_session_id).is_some_and(|live|
                                        live.process.id == process.id && reader.completed_queued_command(mapper.final_agent_text())
                                            .is_some_and(|id| live.claude_lifecycle.begin_drain(id, claude_sequence))) {
                                        tracing::warn!(session_id = %session_id, "Draining Claude ACP prompt completed in an autonomous cycle");
                                        if let Err(error) = process.notify("session/cancel", json!({"sessionId":session_id})).await {
                                            break TurnOutcome::Failed(crate::actor::ExecutionUncertain(error.to_string()).into());
                                        }
                                        draining_coalesced = true;
                                    }
                                }
                            } else { coalesced_ready_since = None; }
                        }
                    }
                }
                // Bridge frames precede the ACP update for the same step.
                event = next_dsh_event(&mut dsh_events) => {
                    use super::deepseek::BridgeEvent;
                    match event {
                        Some(BridgeEvent::Stream { session_id: sid, attempt_id, kind, text, outcome }) if sid == session_id => {
                            let mapped = match kind.as_str() {
                                "start" => { mapper.stream_start(&attempt_id); MappedUpdate::default() }
                                "end" => mapper.stream_end(&attempt_id, outcome.as_deref() == Some("committed")),
                                _ => mapper.stream_text(&attempt_id, &kind, text.as_deref().unwrap_or("")),
                            };
                            emit_mapped(&bus, &input.thread_id, &input.turn_id, mapped, input.hidden);
                        }
                        Some(BridgeEvent::Status { session_id: sid, status }) if sid == session_id => {
                            dsh_running = status == "running";
                        }
                        Some(BridgeEvent::Usage { session_id: sid, report }) if sid == session_id => {
                            if let Some(usage) = adapter_usage.apply(report) {
                                emit_usage(&bus, &input.thread_id, &input.turn_id, usage, input.hidden);
                            }
                        }
                        Some(BridgeEvent::Projection { session_id: sid, key, value }) if sid == session_id => {
                            if key == "goal" {
                                dsh_goal_active = super::deepseek::goal_state(&value)
                                    .is_some_and(|goal| goal.status == "active");
                            } else if key == "plan" && value["active"] == false && value["pending"] == false {
                                // An approved plan review left DSH plan mode.
                                let left_plan = self.inner.sessions.lock().await
                                    .get_mut(&input.provider_session_id)
                                    .filter(|live| live.collaboration_mode.as_deref() == Some("plan"))
                                    .map(|live| live.collaboration_mode = Some("default".into()))
                                    .is_some();
                                if left_plan {
                                    bus.emit(ThreadEventEnvelope {
                                        event_type: pockymoe_protocol::ThreadEventType::ThreadCollaborationUpdated,
                                        thread_id: input.thread_id.clone(),
                                        timestamp: now_rfc3339(),
                                        payload: json!({"turnId": input.turn_id, "collaborationMode": "default"}),
                                    });
                                }
                            } else if key == "todos" {
                                if let Some(plan) = super::deepseek::todo_steps(&value) {
                                    emit_mapped(&bus, &input.thread_id, &input.turn_id,
                                        MappedUpdate { plan: Some(plan), ..Default::default() }, input.hidden);
                                }
                            }
                        }
                        Some(BridgeEvent::Lagged) => {
                            // A missed idle status must not hold the turn open.
                            if let Some(bridge) = &dsh {
                                if let Ok(state) = bridge.call("session", json!({"sessionId": session_id})).await {
                                    dsh_running = state["running"].as_bool().unwrap_or(false);
                                    dsh_goal_active = super::deepseek::goal_state(&state["projections"]["goal"])
                                        .is_some_and(|goal| goal.status == "active");
                                }
                            }
                        }
                        Some(BridgeEvent::Closed) | None => {
                            dsh_events = None;
                            dsh_running = false;
                        }
                        Some(_) => {}
                    }
                }
                recv = updates.recv() => {
                    match recv {
                        Ok(update) => {
                            if update["_remoteProcessId"].as_str() != Some(process.id.as_str()) { continue; }
                            if let Some(sid) = update.get("sessionId").and_then(Value::as_str) {
                                if sid != session_id {
                                    continue;
                                }
                            }
                            coalesced_ready_since = None;
                            if update["_remoteMethod"] == "_claude/sdkMessage" {
                                if let Some(reader) = claude_usage_reader.as_mut() { reader.record_sdk(&update["message"]); }
                                let mapped = mapper.native_task_notification(&update["message"]);
                                emit_mapped(&bus, &input.thread_id, &input.turn_id, mapped, input.hidden);
                                continue;
                            }
                            if let Some(reader) = claude_usage_reader.as_mut() {
                                for usage in reader.poll() { emit_usage(&bus, &input.thread_id, &input.turn_id, usage, input.hidden); }
                                for notice in reader.take_task_notices() {
                                    emit_mapped(&bus, &input.thread_id, &input.turn_id, mapper.native_task_notification(&notice), input.hidden);
                                }
                            }
                            if let Some(usage) = adapter.billing_usage(&update).and_then(|usage| adapter_usage.apply(usage)) {
                                emit_usage(&bus, &input.thread_id, &input.turn_id, usage, input.hidden);
                            }
                            if let Some(context) = adapter.context_usage(&update, &harness_state) {
                                emit_usage(&bus, &input.thread_id, &input.turn_id, context, input.hidden);
                            }
                            let mut mapped = mapper.apply(&update);
                            if let Some(subagents) = mapped.active_subagents.take() {
                                let subagents = merge_active_subagents(subagents, claude_usage_reader.as_ref());
                                if self.update_subagents(&input, &subagents).await {
                                    mapped.active_subagents = Some(subagents);
                                }
                            }
                            if let Some(goal) = mapped.goal.clone() {
                                if let Some(live) = self
                                    .inner
                                    .sessions
                                    .lock()
                                    .await
                                    .get_mut(&input.provider_session_id)
                                {
                                    live.goal = goal;
                                }
                            }
                            emit_mapped(
                                &bus,
                                &input.thread_id,
                                &input.turn_id,
                                mapped,
                                input.hidden,
                            );
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "ACP session/update receiver lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            break if prompt_done {
                                TurnOutcome::Completed
                            } else {
                                TurnOutcome::Failed(crate::actor::ExecutionUncertain("ACP update channel closed".into()).into())
                            };
                        }
                    }
                }
                // DSH keeps working after its ACP prompt settles: goal rounds, and
                // background subagents (the bridge reports a session busy while any
                // descendant runs). Claude's native tasks likewise keep this
                // subscription alive through their task-notification follow-up
                // and SDK idle bookend. ACP foreground completion alone must
                // not discard that output. An active DSH goal gets time for
                // its driver to admit the next round.
                _ = tokio::time::sleep(Duration::from_millis(if dsh_goal_active { 3000 } else { 250 })),
                    if prompt_done && (pending_prompts == 0 || cancel.is_cancelled()) && ((!dsh_running && !claude_background_pending) || cancel.is_cancelled()) => {
                    // A waiting steer may have been admitted while this timer
                    // was armed. Check under the delivery lock before retiring
                    // the output owner, then prevent further handoffs.
                    let sessions = self.inner.sessions.lock().await;
                    if !cancel.is_cancelled() && sessions.get(&input.provider_session_id)
                        .and_then(|live| live.active.as_ref())
                        .is_some_and(|active| active.pending_prompts > 0) {
                        continue;
                    }
                    continuation_rx.close();
                    break if cancel.is_cancelled() { TurnOutcome::Interrupted } else { TurnOutcome::Completed };
                }
                _ = tokio::time::sleep(Duration::from_millis(500)), if !prompt_done || claude_background_pending => {
                    match process.exited().await {
                        Ok(true) => {
                            if cancel_sent { discard_session = true; break TurnOutcome::Interrupted; }
                            break TurnOutcome::Failed(crate::actor::ExecutionUncertain("ACP process exited".into()).into());
                        }
                        Ok(false) => {}
                        Err(err) => {
                            break TurnOutcome::Failed(anyhow!("failed to inspect ACP process: {err}"));
                        }
                    }
                }
            }
        };
        continuation_rx.close();
        if let Some(reader) = claude_usage_reader.as_mut() {
            for usage in reader.poll_final() {
                emit_usage(&bus, &input.thread_id, &input.turn_id, usage, input.hidden);
            }
        }
        let mut abandoned_tools = Vec::new();
        if matches!(outcome, TurnOutcome::Completed) {
            if let Some(error) = pending_claude_error {
                if let Some(ids) = claude_usage_reader.as_ref().and_then(|reader| {
                    reader.abandoned_tools(&error.to_string(), mapper.final_agent_text())
                }) {
                    tracing::warn!(session_id = %session_id, tool_ids = ?ids,
                        "Reconciled Claude ACP tool placeholders discarded during steering");
                    mapper.reconcile_abandoned_tools(&ids);
                    abandoned_tools = ids;
                } else {
                    outcome = TurnOutcome::Failed(anyhow!("ACP session/prompt failed: {error}"));
                }
            }
        }
        if let Some(live) = self
            .inner
            .sessions
            .lock()
            .await
            .get_mut(&input.provider_session_id)
        {
            if live
                .active
                .as_ref()
                .is_some_and(|active| active.turn_id == input.turn_id)
            {
                live.active = None;
            }
            live.load.touch();
            live.active_subagents.clear();
            if include_context && prompt_done {
                live.context_thread_id = Some(input.thread_id.clone());
                // Persisted by the service, so a resumed session keeps knowing.
                bus.emit(ThreadEventEnvelope {
                    event_type: pockymoe_protocol::ThreadEventType::ThreadContextDelivered,
                    thread_id: input.thread_id.clone(),
                    timestamp: now_rfc3339(),
                    payload: json!({"providerSessionId": input.provider_session_id}),
                });
            }
        }
        if discard_session {
            self.inner
                .sessions
                .lock()
                .await
                .remove(&input.provider_session_id);
        }
        if let Some(reader) = usage_reader.as_mut() {
            for usage in reader.poll_final() {
                emit_usage(&bus, &input.thread_id, &input.turn_id, usage, input.hidden);
            }
        }
        let (status, error) = match &outcome {
            TurnOutcome::Completed => ("completed", None),
            TurnOutcome::Interrupted => ("interrupted", None),
            TurnOutcome::Failed(error) => (
                if error.is::<crate::actor::ExecutionUncertain>() {
                    "recovering"
                } else {
                    "failed"
                },
                Some(error.to_string()),
            ),
        };
        if let Some(reader) = claude_usage_reader.as_mut() {
            for usage in reader.poll_final() {
                emit_usage(&bus, &input.thread_id, &input.turn_id, usage, input.hidden);
            }
            for notice in reader.take_task_notices() {
                emit_mapped(
                    &bus,
                    &input.thread_id,
                    &input.turn_id,
                    mapper.native_task_notification(&notice),
                    input.hidden,
                );
            }
        }
        let completed_tools = mapper.completed_tool_ids();
        let mut items = mapper.finish(!matches!(outcome, TurnOutcome::Completed));
        if let Some(reader) = claude_usage_reader.as_ref() {
            reader.annotate_background_reply_phases(
                &mut items,
                matches!(outcome, TurnOutcome::Completed),
            );
        }
        for item in &mut items {
            if item.status.as_deref() != Some("failed")
                && !abandoned_tools.contains(&item.id)
                && !completed_tools.contains(&item.id)
                && item.extra.get("origin").and_then(Value::as_str)
                    != Some("nativeTaskNotification")
            {
                item.status = Some(status.into());
            }
        }
        if !input.hidden {
            bus.emit(ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadSubagentsUpdated,
                thread_id: input.thread_id.clone(),
                timestamp: now_rfc3339(),
                payload: json!({ "turnId": input.turn_id, "activeSubagents": [] }),
            });
            for item in &items {
                bus.emit(ThreadEventEnvelope {
                    event_type: pockymoe_protocol::ThreadEventType::ThreadItemCompleted,
                    thread_id: input.thread_id.clone(),
                    timestamp: now_rfc3339(),
                    payload: json!({ "turnId": input.turn_id, "item": item }),
                });
            }
            bus.emit(ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadTurnCompleted,
                thread_id: input.thread_id,
                timestamp: now_rfc3339(),
                payload: json!({
                    "turnId": input.turn_id,
                    "status": status,
                    "error": error
                }),
            });
        }
        match outcome {
            TurnOutcome::Failed(error) => Err(error),
            TurnOutcome::Completed | TurnOutcome::Interrupted => Ok(items),
        }
    }

    async fn interrupt(&self, session_id: &str, turn_id: &str) -> Result<()> {
        let goal_bridge = {
            let sessions = self.inner.sessions.lock().await;
            let Some(live) = sessions.get(session_id) else {
                return Ok(());
            };
            if !live
                .active
                .as_ref()
                .is_some_and(|active| active.turn_id == turn_id)
            {
                return Ok(());
            }
            live.process
                .notify("session/cancel", json!({ "sessionId": live.session_id }))
                .await?;
            live.dsh
                .clone()
                .filter(|_| {
                    live.goal
                        .as_ref()
                        .is_some_and(|goal| goal.status == "active")
                })
                .map(|bridge| (bridge, live.session_id.clone()))
        };
        // Cancel stops the running round; pausing keeps the goal from re-arming.
        if let Some((bridge, raw_session)) = goal_bridge {
            pause_dsh_goal(&bridge, &raw_session).await;
        }
        Ok(())
    }

    async fn respond_permission(
        &self,
        request_id: &str,
        allow: bool,
        answer: Option<&str>,
    ) -> Result<()> {
        {
            let mut inputs = self.inner.pending_inputs.lock().await;
            if let Some(pending) = inputs.get(request_id) {
                let response =
                    super::elicitation::response(&pending.params, pending.native, allow, answer)?;
                if let Some(pending) = inputs.remove(request_id) {
                    let _ = pending.tx.send(response);
                }
                for list in self.inner.pending_dtos.lock().await.values_mut() {
                    list.retain(|item| item.id != request_id);
                }
                return Ok(());
            }
        }
        let structured: Option<Value> = answer.and_then(|text| serde_json::from_str(text).ok());
        let selected = structured
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|obj| {
                obj.values()
                    .find_map(|v| v["answers"].as_array()?.first()?.as_str())
            });
        let answer = selected.or(answer);
        if let Some(pending) = self
            .inner
            .pending_permissions
            .lock()
            .await
            .remove(request_id)
        {
            let option = select_permission_option(&pending.options, allow, answer)
                .unwrap_or_else(|| "cancelled".into());
            let _ = pending.tx.send(option);
        }
        for list in self.inner.pending_dtos.lock().await.values_mut() {
            list.retain(|item| item.id != request_id);
        }
        self.touch_active_sessions().await;
        Ok(())
    }

    async fn pending_requests(&self, thread_id: &str) -> Vec<ThreadActionRequestDto> {
        self.inner
            .pending_dtos
            .lock()
            .await
            .get(thread_id)
            .cloned()
            .unwrap_or_default()
    }

    async fn compact_session(
        &self,
        session_id: &str,
        thread_id: &str,
        bus: EventBus,
    ) -> Result<()> {
        let dsh = self
            .inner
            .sessions
            .lock()
            .await
            .get(session_id)
            .and_then(|live| {
                live.dsh
                    .clone()
                    .map(|bridge| (bridge, live.session_id.clone(), live.operation.clone()))
            });
        if let Some((bridge, raw_session, operation)) = dsh {
            // A DSH command, not a model turn: ACP would send `/compact` as text.
            let _operation = operation
                .try_lock()
                .map_err(|_| anyhow!("conflict: The session is busy."))?;
            bridge.command(&raw_session, "/compact").await?;
            if let Some(live) = self.inner.sessions.lock().await.get_mut(session_id) {
                live.context_thread_id = None;
            }
            return Ok(());
        }
        let agent_id = session_id.split("::").next().unwrap_or("codex");
        let adapter = adapter_for(agent_id);
        let prompt = adapter
            .compact_prompt()
            .or_else(|| {
                self.negotiated_caps(Some(agent_id))
                    .turns
                    .compact
                    .then_some("/compact")
            })
            .ok_or_else(|| anyhow!("this harness does not implement compact"))?;
        let cancel = CancellationToken::new();
        let _items = self
            .start_turn(
                StartTurnInput {
                    provider_session_id: session_id.into(),
                    prompt: prompt.into(),
                    model: None,
                    reasoning_effort: None,
                    sandbox_mode: None,
                    collaboration_mode: None,
                    approval_mode: None,
                    performance_mode: None,
                    thread_id: thread_id.into(),
                    turn_id: format!("compact-{}", Uuid::new_v4()),
                    hidden: true,
                    images: Vec::new(),
                    title: None,
                    context_delivered: true,
                },
                bus,
                cancel,
            )
            .await?;
        if let Some(live) = self.inner.sessions.lock().await.get_mut(session_id) {
            live.context_thread_id = None;
        }
        Ok(())
    }

    async fn fork_session(&self, session_id: &str) -> Result<StartSessionResult> {
        self.fork_session_at(session_id, 0).await
    }

    async fn fork_session_at(
        &self,
        session_id: &str,
        rollback_count: u32,
    ) -> Result<StartSessionResult> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let operation = self
            .inner
            .sessions
            .lock()
            .await
            .get(session_id)
            .ok_or_else(|| anyhow!("ACP session is not running"))?
            .operation
            .clone();
        let _operation = operation
            .try_lock()
            .map_err(|_| anyhow!("conflict: The session is busy."))?;
        let (
            process,
            codex_bridge,
            raw_session,
            cwd,
            yolo,
            sandbox_mode,
            collaboration_mode,
            negotiated,
            adapter_id,
            goal,
            model,
            effort,
            config_options,
            harness_state,
            harness_models,
            available_modes,
            current_mode_id,
        ) = {
            let sessions = self.inner.sessions.lock().await;
            let live = sessions
                .get(session_id)
                .ok_or_else(|| anyhow!("ACP session is not running"))?;
            if live.active.is_some() {
                bail!("conflict: Cannot fork an active turn.");
            }
            if !live.negotiated.fork {
                bail!("this harness does not support session/fork");
            }
            (
                live.process.clone(),
                live.codex_bridge.clone(),
                live.session_id.clone(),
                live.cwd.clone(),
                live.yolo,
                live.sandbox_mode.clone(),
                live.collaboration_mode.clone(),
                live.negotiated.clone(),
                live.adapter_id.clone(),
                live.goal.clone(),
                live.model.clone(),
                live.reasoning_effort.clone(),
                live.config_options.clone(),
                live.harness_state.clone(),
                live.harness_models.clone(),
                live.available_modes.clone(),
                live.current_mode_id.clone(),
            )
        };
        if let Some(bridge) = &codex_bridge {
            let id = bridge
                .fork(
                    &raw_session,
                    rollback_count,
                    &cwd.to_string_lossy(),
                    &ProductSessionPolicy {
                        collaboration_mode,
                        sandbox_mode,
                        approval_mode: Some(if yolo { "yolo" } else { "guarded" }.into()),
                    },
                )
                .await?;
            // The fork's Pockymoe id does not exist yet. Leave it unloaded so
            // resume_session starts ACP with the new thread's own CLI environment.
            return Ok(StartSessionResult {
                provider_session_id: Self::scoped_id(&adapter_id, &id),
                model,
                reasoning_effort: effort,
            });
        }
        let response = {
            if rollback_count != 0 {
                bail!("conflict: This backend supports latest-session fork only.");
            }
            if let Some(method) = negotiated.fork_method.as_deref() {
                let adapter = adapter_for(&adapter_id);
                let forked = process
                    .request(
                        method,
                        adapter.extension_fork_params(&raw_session, &cwd.to_string_lossy()),
                    )
                    .await?;
                let id = adapter
                    .extension_fork_id(&forked)
                    .ok_or_else(|| anyhow!("harness fork returned no session id"))?;
                let mut loaded = process
                    .request(
                        "session/load",
                        json!({"sessionId":id, "cwd":cwd, "mcpServers":[]}),
                    )
                    .await?;
                loaded["sessionId"] = json!(id);
                loaded
            } else {
                process
                    .request(
                        "session/fork",
                        json!({"sessionId":raw_session, "cwd":cwd, "mcpServers":[]}),
                    )
                    .await?
            }
        };
        let new_id = response
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("fork did not return sessionId"))?
            .to_string();
        let scoped = Self::scoped_id(&adapter_id, &new_id);
        self.inner.sessions.lock().await.insert(
            scoped.clone(),
            LiveSession {
                process,
                codex_bridge,
                dsh: None,
                harness_info: None,
                dsh_projections: Default::default(),
                dsh_session: Value::Null,
                dsh_title: None,
                dsh_running: false,
                dsh_env_preset: None,
                session_id: new_id,
                cwd,
                yolo,
                sandbox_mode,
                collaboration_mode,
                negotiated,
                adapter_id,
                goal,
                model: model.clone(),
                reasoning_effort: effort.clone(),
                active: None,
                claude_lifecycle: Default::default(),
                operation: Arc::new(Mutex::new(())),
                config_options,
                harness_state,
                harness_models,
                available_modes,
                current_mode_id,
                context_thread_id: None,
                active_subagents: HashMap::new(),
                upstream_revision: None,
                load: LoadRecord::fork(),
            },
        );
        let _ = self
            .inner
            .load_events
            .send(crate::actor::SessionLifecycle::Loaded {
                session_id: scoped.clone(),
                thread_id: None,
                reason: pockymoe_protocol::HarnessLoadReason::Fork,
                requested_at: chrono::Utc::now(),
            });
        Ok(StartSessionResult {
            provider_session_id: scoped,
            model,
            reasoning_effort: effort,
        })
    }

    async fn send_input(&self, session_id: &str, turn_id: &str, prompt: &str) -> Result<()> {
        let (process, provider_session_id, background_owned) = {
            let mut sessions = self.inner.sessions.lock().await;
            let live = sessions
                .get_mut(session_id)
                .ok_or_else(|| anyhow!("ACP session is not running"))?;
            if !live.negotiated.steer {
                bail!("this harness does not support steering");
            }
            if !live.active.as_ref().is_some_and(|active| {
                active.turn_id == turn_id
                    && !active.cancel.is_cancelled()
                    && !active.continuations.is_closed()
            }) {
                bail!("conflict: The active turn finished before this prompt could be steered.");
            }
            let background_owned = live.adapter_id == "claude"
                && live
                    .active
                    .as_ref()
                    .is_some_and(|active| active.background_owned);
            if live.adapter_id == "claude" && !live.claude_lifecycle.steering_started() {
                bail!("conflict: The active turn is finishing; send this prompt as a new turn.");
            }
            live.load.prompt();
            (
                live.process.clone(),
                live.session_id.clone(),
                background_owned,
            )
        };
        // Steering is an extension request, not a notification. Only acknowledge
        // delivery after the harness confirms it; never silently queue a prompt.
        // Release the sessions lock so updates and cancellation can still progress.
        // If the turn ends during delivery, let the supervisor own any new turn;
        // the harness must not silently start an untracked detached prompt.
        let result = process
            .request(
                "_session/steering",
                json!({
                    "sessionId": provider_session_id,
                    "prompt": [{ "type": "text", "text": prompt }],
                    "_meta": {"steering": {"idleBehavior": "promptRequired"}}
                }),
            )
            .await?;
        match result.get("outcome").and_then(Value::as_str) {
            Some("injected") => Ok(()),
            Some("promptRequired") if background_owned && result["reason"] == "noRunningTurn" => {
                // Claude's foreground is idle while native tasks still belong to
                // this product turn. Submit a normal prompt and hand its response
                // to that turn's existing loop; never start a detached SDK cycle.
                let mut sessions = self.inner.sessions.lock().await;
                let live = sessions
                    .get_mut(session_id)
                    .ok_or_else(|| anyhow!("ACP session is not running"))?;
                let active = live
                    .active
                    .as_mut()
                    .filter(|active| active.turn_id == turn_id && !active.cancel.is_cancelled())
                    .ok_or_else(|| {
                        anyhow!("conflict: The active turn finished before delivery.")
                    })?;
                if !process.connection_open().await || active.continuations.is_closed() {
                    bail!("ACP turn output owner is unavailable");
                }
                let response = process
                    .begin_request(
                        "session/prompt",
                        json!({
                            "sessionId": provider_session_id,
                            "prompt": [{"type":"text", "text":prompt}]
                        }),
                    )
                    .await?;
                active.pending_prompts += 1;
                active
                    .continuations
                    .send(response)
                    .map_err(|_| anyhow!("ACP turn output owner closed during delivery"))?;
                Ok(())
            }
            _ => bail!("ACP harness did not confirm steering delivery: {result}"),
        }
    }

    async fn stage_goal(&self, session_id: &str, goal: GoalState) -> Result<()> {
        let mut sessions = self.inner.sessions.lock().await;
        let live = sessions
            .get_mut(session_id)
            .ok_or_else(|| anyhow!("ACP session is not running"))?;
        live.goal = Some(goal);
        Ok(())
    }

    async fn goal_prompt(&self, session_id: &str, argument: &str) -> Result<Option<String>> {
        let (codex_goals, dsh) = {
            let sessions = self.inner.sessions.lock().await;
            let live = sessions
                .get(session_id)
                .ok_or_else(|| anyhow!("ACP session is not running"))?;
            (
                live.adapter_id == "codex" && live.negotiated.goals,
                live.dsh
                    .clone()
                    .map(|bridge| (bridge, live.session_id.clone())),
            )
        };
        let Some((bridge, raw_session)) = dsh else {
            return Ok(codex_goals.then(|| format!("/goal {argument}")));
        };
        // The wire prompt runs DSH's `/goal`; a profile may leave it out.
        let state = bridge
            .call("session", json!({"sessionId": raw_session}))
            .await?;
        let available = state["commands"]
            .as_array()
            .is_some_and(|commands| commands.iter().any(|command| command["name"] == "goal"));
        Ok(available.then(|| format!("/goal {argument}")))
    }

    async fn get_goal(&self, session_id: &str) -> Result<Option<GoalState>> {
        Ok(self
            .inner
            .sessions
            .lock()
            .await
            .get(session_id)
            .and_then(|s| s.goal.clone()))
    }

    async fn set_goal(
        &self,
        session_id: &str,
        objective: Option<String>,
        status: Option<String>,
    ) -> Result<Option<GoalState>> {
        let dsh = self
            .inner
            .sessions
            .lock()
            .await
            .get(session_id)
            .and_then(|live| {
                live.dsh
                    .clone()
                    .map(|bridge| (bridge, live.session_id.clone(), live.goal.is_some()))
            });
        if let Some((bridge, raw_session, has_goal)) = dsh {
            // New and resumed goals run as turns (`goal_prompt`); this edits
            // or stops the existing goal without starting autonomous rounds.
            let line = match (objective.as_deref().map(str::trim), status.as_deref()) {
                (Some(objective), _) if !objective.is_empty() && has_goal => {
                    format!("/goal edit {objective}")
                }
                (Some(objective), _) if !objective.is_empty() => {
                    bail!("This DSH profile cannot start goals")
                }
                (_, Some("paused")) => "/goal pause".into(),
                (_, Some("clear" | "terminated")) | (None, None) => "/goal clear".into(),
                (_, Some(other)) => bail!("DSH goals cannot be set to {other}"),
                _ => bail!("objective must not be empty"),
            };
            bridge.command(&raw_session, &line).await?;
            let state = bridge
                .call("session", json!({"sessionId": raw_session}))
                .await?;
            let goal = super::deepseek::goal_state(&state["projections"]["goal"]);
            if let Some(live) = self.inner.sessions.lock().await.get_mut(session_id) {
                live.goal = goal.clone();
            }
            return Ok(goal);
        }
        let (process, provider_session_id, method) = {
            let sessions = self.inner.sessions.lock().await;
            let live = sessions
                .get(session_id)
                .ok_or_else(|| anyhow!("ACP session is not running"))?;
            if !live.negotiated.goals {
                bail!("this harness does not support goals");
            }
            (
                live.process.clone(),
                live.session_id.clone(),
                live.negotiated
                    .goal_method
                    .clone()
                    .unwrap_or_else(|| "session/set_goal".into()),
            )
        };
        let action = if objective
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        {
            "set"
        } else if status.as_deref() == Some("paused") {
            "pause"
        } else if status.as_deref() == Some("active") {
            "resume"
        } else {
            "clear"
        };
        // A slow extension must not hold the global sessions mutex. Bound the
        // entire exchange, including a blocked stdin write.
        tokio::time::timeout(
            Duration::from_secs(15),
            process.request(
                &method,
                json!({"sessionId": provider_session_id, "action": action, "objective": objective}),
            ),
        )
        .await
        .map_err(|_| anyhow!("Goal control timed out; inspect the goal before retrying"))??;
        let mut sessions = self.inner.sessions.lock().await;
        let live = sessions
            .get_mut(session_id)
            .filter(|live| Arc::ptr_eq(&live.process, &process))
            .ok_or_else(|| anyhow!("ACP session changed during goal control"))?;
        live.load.touch();
        if action == "set" {
            live.goal = Some(GoalState {
                objective: objective.unwrap_or_default(),
                status: "active".into(),
                tokens_used: 0,
                time_used_seconds: 0,
            });
        } else if action == "clear" {
            live.goal = None;
        } else if let Some(goal) = live.goal.as_mut() {
            goal.status = if action == "pause" {
                "paused"
            } else {
                "active"
            }
            .into();
        }
        Ok(live.goal.clone())
    }

    async fn harness_catalog(&self, agent_id: Option<&str>, cwd: Option<&str>) -> Result<Value> {
        let def = self.agent_def(agent_id)?;
        if def.id != "deepseek" {
            return Ok(Value::Null);
        }
        if let Some(catalog) = self.inner.dsh_catalog.lock().await.clone() {
            return Ok(catalog);
        }
        // A model probe boots DSH once and caches its run modes.
        if let Some(cwd) = cwd.filter(|cwd| !cwd.trim().is_empty()) {
            self.probe_models(&def, cwd).await?;
        }
        Ok(self
            .inner
            .dsh_catalog
            .lock()
            .await
            .clone()
            .unwrap_or(Value::Null))
    }

    async fn harness_action(&self, session_id: &str, action: Value) -> Result<Value> {
        use super::deepseek::PanelAction;
        let action = PanelAction::parse(&action)?;
        let (bridge, raw_session, process, turn_active, goal_active) = {
            let sessions = self.inner.sessions.lock().await;
            let live = sessions
                .get(session_id)
                .ok_or_else(|| anyhow!("ACP session is not running"))?;
            (
                live.dsh
                    .clone()
                    .ok_or_else(|| anyhow!("this harness has no settings panel actions"))?,
                live.session_id.clone(),
                live.process.clone(),
                live.active.is_some(),
                live.goal
                    .as_ref()
                    .is_some_and(|goal| goal.status == "active"),
            )
        };
        let invoke = |method: &str, args: Value| json!({"namespace":"pluginManager","method":method,"args":args});
        let result = match action {
            PanelAction::Refresh => Value::Null,
            PanelAction::Settings => {
                return Ok(json!({"settings": bridge.call("settings", json!({})).await?}));
            }
            PanelAction::UpdateSetting {
                ns,
                key,
                value,
                revision,
            } => {
                bridge
                    .call(
                        "updateSetting",
                        json!({"ns":ns,"key":key,"value":value,"revision":revision}),
                    )
                    .await?
            }
            PanelAction::SetPluginEnabled { id, enabled } => {
                bridge
                    .call(
                        "invoke",
                        invoke("setPluginEnabled", json!({"id":id,"enabled":enabled})),
                    )
                    .await?
            }
            PanelAction::SetBundleEnabled { name, enabled } => {
                bridge
                    .call(
                        "invoke",
                        invoke("setBundleEnabled", json!({"name":name,"enabled":enabled})),
                    )
                    .await?
            }
            PanelAction::Stop => {
                if turn_active {
                    bail!("conflict: Use the turn's Stop button while a turn is running");
                }
                // Without an ACP prompt in flight DSH cancels its autonomous work.
                process
                    .notify("session/cancel", json!({"sessionId": raw_session}))
                    .await?;
                if goal_active {
                    pause_dsh_goal(&bridge, &raw_session).await;
                }
                Value::Null
            }
            PanelAction::SelectRunMode { id } => {
                if turn_active {
                    bail!("conflict: Wait for the turn to finish before changing the run mode");
                }
                // Hold the session so no prompt starts with half-switched tools.
                let (_operation, policy) = self.dsh_operation(session_id, &process).await?;
                let read = || bridge.call("session", json!({"sessionId": raw_session}));
                let before = read().await?;
                let selected = bridge
                    .call(
                        "selectPreset",
                        json!({"sessionId": raw_session, "preset": id}),
                    )
                    .await?;
                // A mode without plan mode or goals would silently drop the
                // thread's active plan restrictions or goal: switch back.
                let after = read().await?;
                let has = |name: &str| {
                    after["commands"].as_array().map_or(true, |commands| {
                        commands.iter().any(|command| command["name"] == name)
                    })
                };
                let plan = super::deepseek::wants_plan(&policy)
                    || before["projections"]["plan"]["active"].as_bool() == Some(true);
                let missing = if plan && !has("plan") {
                    Some("plan mode")
                } else if goal_active && !has("goal") {
                    Some("goals")
                } else {
                    None
                };
                if let Some(feature) = missing {
                    if let Some(previous) = before["projections"]["agentPreset"].as_str() {
                        bridge
                            .call(
                                "selectPreset",
                                json!({"sessionId": raw_session, "preset": previous}),
                            )
                            .await?;
                    }
                    bail!("The {id} run mode has no {feature}; turn {feature} off in this thread first");
                }
                selected
            }
            PanelAction::Console => {
                return Ok(json!({"console": bridge.call("console", json!({})).await?}));
            }
            PanelAction::Command { line } => {
                if turn_active {
                    bail!("conflict: Wait for the turn to finish before running a DSH command");
                }
                let _operation = self.dsh_operation(session_id, &process).await?;
                bridge.command(&raw_session, &line).await?
            }
            PanelAction::Restart => {
                let removed = {
                    let mut sessions = self.inner.sessions.lock().await;
                    let live = sessions
                        .get(session_id)
                        .filter(|live| Arc::ptr_eq(&live.process, &process))
                        .ok_or_else(|| anyhow!("ACP session changed during the harness action"))?;
                    if live.active.is_some() {
                        bail!("conflict: Wait for the turn to finish before restarting DSH");
                    }
                    let _operation = live
                        .operation
                        .clone()
                        .try_lock_owned()
                        .map_err(|_| anyhow!("conflict: The session is busy."))?;
                    sessions.remove(session_id)
                };
                // The next request resumes the session with the saved profile.
                if let Some(live) = removed {
                    live.process.shutdown().await?;
                    self.announce_release(session_id, &live, crate::actor::ReleaseReason::Explicit);
                }
                return Ok(json!({"result": {"restarted": true}}));
            }
        };
        // Profile edits change the plugin and bundle inventory; keep discovery models.
        let mut snapshot = bridge.call("snapshot", json!({})).await?;
        let state = bridge
            .call("session", json!({"sessionId": raw_session}))
            .await?;
        let mut sessions = self.inner.sessions.lock().await;
        let live = sessions
            .get_mut(session_id)
            .filter(|live| Arc::ptr_eq(&live.process, &process))
            .ok_or_else(|| anyhow!("ACP session changed during the harness action"))?;
        live.load.touch();
        Self::store_dsh_session(live, &state);
        if let Some(info) = live.harness_info.as_mut() {
            // Launch facts the bridge snapshot does not carry.
            for key in ["models", "composition", "compositionError"] {
                snapshot[key] = info[key].take();
            }
            *info = snapshot;
        }
        let harness = super::deepseek::harness_view(
            live.harness_info.as_ref().unwrap_or(&Value::Null),
            &live.dsh_projections,
            &live.dsh_session,
            live.dsh_running && live.active.is_none(),
        );
        Ok(json!({"result": result, "harness": harness}))
    }

    async fn apply_session_settings(
        &self,
        session_id: &str,
        settings: SessionSettings,
    ) -> Result<()> {
        self.refresh_grok_session(session_id).await?;
        self.apply_live_settings(
            session_id,
            settings.model.as_deref(),
            settings.effort.as_deref(),
            settings.sandbox_mode.as_deref(),
            settings.collaboration_mode.as_deref(),
            settings.approval_mode.as_deref(),
            settings.performance_mode,
        )
        .await
    }

    async fn install(&self, agent_id: Option<&str>) -> Result<AgentBackendDto> {
        let def = self.agent_def(agent_id)?;
        super::dependencies::ensure(&def, true).await?;
        let cwd = std::env::current_dir()?;
        self.probe_models(&def, &cwd.to_string_lossy())
            .await
            .map_err(|error| {
                anyhow!("Adapter installed, but ACP verification failed: {error:#}")
            })?;
        Ok(self.descriptor())
    }

    async fn list_import_sessions(&self, agent_id: Option<&str>) -> Result<Vec<ImportSessionMeta>> {
        let def = match self.agent_def(agent_id) {
            Ok(def) => def,
            Err(_) => return Ok(Vec::new()),
        };
        if classify_availability(&def) != "ready" {
            return Ok(Vec::new());
        }
        match self.list_agent_sessions(&def).await {
            Ok(sessions) => Ok(sessions),
            Err(err) => {
                tracing::warn!(error = %err, agent = %def.id, "ACP session list failed");
                Ok(Vec::new())
            }
        }
    }

    fn session_loaded(&self, session_id: &str) -> bool {
        self.inner
            .sessions
            .try_lock()
            .ok()
            .map(|sessions| {
                sessions.iter().any(|(key, live)| {
                    (session_ids_match(key, session_id)
                        || session_ids_match(&live.session_id, session_id))
                        && cli_context_matches(&live.process.cli_env)
                })
            })
            .unwrap_or(false)
    }

    async fn release_session(&self, session_id: &str) -> Result<()> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let mut sessions = self.inner.sessions.lock().await;
        let key = sessions
            .iter()
            .find(|(key, live)| {
                session_ids_match(key, session_id)
                    || session_ids_match(&live.session_id, session_id)
            })
            .map(|(key, _)| key.clone());
        let Some(key) = key else {
            return Ok(());
        };
        let live = &sessions[&key];
        ensure!(
            live.active.is_none(),
            "conflict: child harness is still running"
        );
        let _operation = live
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| anyhow!("conflict: child harness is busy"))?;
        ensure!(
            !sessions
                .iter()
                .any(|(other, sibling)| other != &key
                    && Arc::ptr_eq(&sibling.process, &live.process)),
            "conflict: child shares a harness process; reconnect it independently before deletion"
        );
        live.process.shutdown().await?;
        sessions.remove(&key);
        Ok(())
    }

    async fn loaded_sessions(&self) -> Vec<crate::actor::LoadedSession> {
        self.loaded().await
    }

    async fn release_loaded(
        &self,
        session_id: &str,
        scope: crate::actor::ReleaseScope,
        process_id: Option<&str>,
    ) -> Result<Vec<crate::actor::LoadedSession>> {
        self.release(session_id, scope, process_id).await
    }

    async fn touch_session(&self, session_id: &str) {
        self.touch(session_id).await;
    }

    fn lifecycle(&self) -> Option<broadcast::Receiver<crate::actor::SessionLifecycle>> {
        Some(self.inner.load_events.subscribe())
    }

    async fn session_instance_id(&self, session_id: &str) -> Option<String> {
        let sessions = self.inner.sessions.lock().await;
        let live = sessions
            .iter()
            .find(|(key, live)| {
                session_ids_match(key, session_id)
                    || session_ids_match(&live.session_id, session_id)
            })?
            .1;
        if live.process.connection_open().await {
            Some(live.process.id.clone())
        } else {
            None
        }
    }

    async fn session_instance_started_at(&self, session_id: &str) -> Option<String> {
        let sessions = self.inner.sessions.lock().await;
        let live = sessions
            .iter()
            .find(|(key, live)| {
                session_ids_match(key, session_id)
                    || session_ids_match(&live.session_id, session_id)
            })?
            .1;
        if live.process.connection_open().await {
            Some(live.process.started_at.clone())
        } else {
            None
        }
    }
}

fn spawn_mux(
    inner: Arc<Inner>,
    process: Arc<AcpProcess>,
    agent_id: &str,
    mut updates: mpsc::UnboundedReceiver<Value>,
    mut requests: mpsc::UnboundedReceiver<(i64, String, Value)>,
) {
    let adapter = adapter_for(agent_id);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(mut update) = updates.recv() => {
                    if !adapter.accepts_notification(update["_remoteMethod"].as_str().unwrap_or("session/update")) { continue; }
                    update["_remoteProcessId"] = json!(process.id);
                    if update["_remoteMethod"] == "_claude/sdkMessage" {
                        if let Some(sid) = update["sessionId"].as_str() {
                            if let Some(live) = inner.sessions.lock().await.values_mut()
                                .find(|live| live.process.id == process.id && live.session_id == sid) {
                                live.claude_lifecycle.record(&update["message"]);
                            }
                        }
                    }
                    if update["update"]["sessionUpdate"] == "available_commands_update" {
                        if let Some(sid) = update["sessionId"].as_str() {
                            let mut commands = inner.commands.lock().await;
                            commands.insert((process.id.clone(), sid.into()), parse_commands(&update["update"]["availableCommands"]));
                        }
                    }
                    apply_mode_update(&inner, &update).await;
                    let _ = inner.updates.send(update);
                }
                Some((req_id, method, params)) = requests.recv() => {
                    let inner = inner.clone();
                    let process = process.clone();
                    tokio::spawn(async move {
                        if let Err(err) = handle_agent_request(&inner, &process, req_id, &method, params).await {
                            tracing::warn!(error = %err, method, req_id, "ACP client request failed");
                            let _ = process.respond_error(req_id, &err.to_string()).await;
                        }
                    });
                }
                else => break,
            }
        }
    });
}

/// DSH may already have paused the goal after a cancel; stopping still worked.
async fn pause_dsh_goal(bridge: &super::deepseek::Bridge, session_id: &str) {
    if let Err(error) = bridge.command(session_id, "/goal pause").await {
        tracing::debug!(%error, "DSH goal was not paused after cancel");
    }
}

/// DSH bridge signals outside the turn loop: projection state for the panel
/// and goal display, activity status, and human questions such as plan reviews.
fn spawn_dsh_mux(inner: Arc<Inner>, process_id: String, bridge: Arc<super::deepseek::Bridge>) {
    use super::deepseek::BridgeEvent;
    let mut events = bridge.subscribe();
    tokio::spawn(async move {
        let questions: Arc<std::sync::Mutex<HashMap<u64, String>>> = Default::default();
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            };
            match event {
                BridgeEvent::Projection {
                    session_id,
                    key,
                    value,
                } => {
                    let mut sessions = inner.sessions.lock().await;
                    if let Some(live) = sessions
                        .values_mut()
                        .find(|live| live.process.id == process_id && live.session_id == session_id)
                    {
                        if key == "goal" {
                            live.goal = super::deepseek::goal_state(&value);
                        }
                        live.dsh_projections.insert(key, value);
                    }
                }
                BridgeEvent::Status { session_id, status } => {
                    let mut sessions = inner.sessions.lock().await;
                    if let Some(live) = sessions
                        .values_mut()
                        .find(|live| live.process.id == process_id && live.session_id == session_id)
                    {
                        live.dsh_running = status == "running";
                    }
                }
                BridgeEvent::Question {
                    id,
                    session_id,
                    questions: asked,
                } => {
                    let (inner, bridge, process_id, registry) = (
                        inner.clone(),
                        bridge.clone(),
                        process_id.clone(),
                        questions.clone(),
                    );
                    tokio::spawn(async move {
                        let result = answer_dsh_question(
                            &inner,
                            &process_id,
                            &session_id,
                            id,
                            &asked,
                            &registry,
                        )
                        .await;
                        let _ = bridge
                            .answer(id, result.map_err(|error| error.to_string()))
                            .await;
                    });
                }
                BridgeEvent::Approval {
                    id,
                    session_id,
                    request,
                } => {
                    let (inner, bridge, process_id, registry) = (
                        inner.clone(),
                        bridge.clone(),
                        process_id.clone(),
                        questions.clone(),
                    );
                    tokio::spawn(async move {
                        let outcome = answer_dsh_approval(
                            &inner,
                            &process_id,
                            &session_id,
                            id,
                            &request,
                            &registry,
                        )
                        .await;
                        let _ = bridge.answer(id, Ok(json!(outcome))).await;
                    });
                }
                BridgeEvent::QuestionCancelled { id } => {
                    let request_id = questions.lock().unwrap().remove(&id);
                    if let Some(request_id) = request_id {
                        // Dropping the reply sender resolves the request as unanswered.
                        inner.pending_inputs.lock().await.remove(&request_id);
                        inner.pending_permissions.lock().await.remove(&request_id);
                    }
                }
                BridgeEvent::Closed => break,
                BridgeEvent::Stream { .. } | BridgeEvent::Usage { .. } | BridgeEvent::Lagged => {}
            }
        }
    });
}

async fn answer_dsh_question(
    inner: &Inner,
    process_id: &str,
    session_id: &str,
    question_id: u64,
    asked: &Value,
    registry: &std::sync::Mutex<HashMap<u64, String>>,
) -> Result<Value> {
    let active = {
        let sessions = inner.sessions.lock().await;
        sessions
            .values()
            .find(|live| live.process.id == process_id && live.session_id == session_id)
            .and_then(|live| live.active.as_ref())
            .map(|active| {
                (
                    active.thread_id.clone(),
                    active.turn_id.clone(),
                    active.bus.clone(),
                )
            })
    }
    .ok_or_else(|| anyhow!("No Pockymoe turn is active to answer this DSH question"))?;
    let request_id = format!("dsh-question-{}", Uuid::new_v4());
    registry
        .lock()
        .unwrap()
        .insert(question_id, request_id.clone());
    let questions = super::deepseek::questions(asked);
    let params = json!({ "questions": questions });
    let response = ask_user(
        inner,
        &active,
        &request_id,
        (params, true),
        questions,
        None,
        None,
    )
    .await;
    registry.lock().unwrap().remove(&question_id);
    emit_request_resolved(&active, &request_id);
    let answers = &response["answers"];
    if answers.as_object().is_none_or(|answers| answers.is_empty()) {
        bail!("The user dismissed the question in Pockymoe; stop and wait for their next message.");
    }
    super::deepseek::answer(asked, answers)
}

async fn next_dsh_event(
    events: &mut Option<broadcast::Receiver<super::deepseek::BridgeEvent>>,
) -> Option<super::deepseek::BridgeEvent> {
    let Some(receiver) = events else {
        return std::future::pending().await;
    };
    loop {
        match receiver.recv().await {
            Ok(event) => return Some(event),
            // Committed ACP chunks replace any stream text a lag skipped.
            Err(broadcast::error::RecvError::Lagged(_)) => {
                return Some(super::deepseek::BridgeEvent::Lagged)
            }
            Err(broadcast::error::RecvError::Closed) => return None,
        }
    }
}

/// Show an ACP-shaped permission request in the active turn and wait for the
/// user's option (None when cancelled, timed out or no turn can show it).
async fn ask_permission(
    inner: &Inner,
    active: &Option<(String, String, EventBus)>,
    request_id: &str,
    params: &Value,
) -> Option<String> {
    let choices = parse_permission_choices(params);
    let (tx, rx) = tokio::sync::oneshot::channel();
    inner.pending_permissions.lock().await.insert(
        request_id.to_string(),
        PendingPermission {
            tx,
            options: choices.clone(),
        },
    );
    let title = permission_title(params);
    if let Some((thread_id, turn_id, bus)) = active {
        let dto = ThreadActionRequestDto {
            id: request_id.to_string(),
            kind: "permissionRequest".into(),
            title: title.clone(),
            description: permission_description(params),
            turn_id: Some(turn_id.clone()),
            item_id: params
                .pointer("/toolCall/toolCallId")
                .and_then(Value::as_str)
                .map(str::to_string),
            created_at: now_rfc3339(),
            questions: permission_questions(&title, &choices),
        };
        inner
            .pending_dtos
            .lock()
            .await
            .entry(thread_id.clone())
            .or_default()
            .push(dto.clone());
        bus.emit(ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadRequestCreated,
            thread_id: thread_id.clone(),
            timestamp: now_rfc3339(),
            payload: json!({ "request": dto }),
        });
    }
    let selected = tokio::time::timeout(Duration::from_secs(300), rx)
        .await
        .ok()
        .and_then(Result::ok)
        .filter(|option| option != "cancelled");
    // Include timeout/cancellation, not just replies through HTTP. The
    // LLM clock and question UI must not remain paused after resolution.
    inner.pending_permissions.lock().await.remove(request_id);
    if let Some((thread_id, turn_id, bus)) = active {
        if let Some(list) = inner.pending_dtos.lock().await.get_mut(thread_id) {
            list.retain(|request| request.id != request_id);
        }
        bus.emit(ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadRequestResolved,
            thread_id: thread_id.clone(),
            timestamp: now_rfc3339(),
            payload: json!({"requestId":request_id,"turnId":turn_id}),
        });
    }
    selected
}

/// DSH tool approval (sandbox escalation) for a root session or one of its
/// subagents, as the thread's permission request. Unanswerable requests are
/// rejected: DSH must never wait on an approval nobody can see.
async fn answer_dsh_approval(
    inner: &Inner,
    process_id: &str,
    session_id: &str,
    approval_id: u64,
    asked: &Value,
    registry: &std::sync::Mutex<HashMap<u64, String>>,
) -> &'static str {
    let (policy, active) = {
        let sessions = inner.sessions.lock().await;
        match sessions
            .values()
            .find(|live| live.process.id == process_id && live.session_id == session_id)
        {
            Some(live) => (
                AcpRuntime::policy_from_live(live),
                live.active
                    .as_ref()
                    .map(|a| (a.thread_id.clone(), a.turn_id.clone(), a.bus.clone())),
            ),
            None => return "rejected",
        }
    };
    if policy.auto_approve() {
        return "allowed-once";
    }
    if active.is_none() {
        return "rejected";
    }
    let params = super::deepseek::approval_permission(asked);
    let request_id = format!("dsh-approval-{}", Uuid::new_v4());
    registry
        .lock()
        .unwrap()
        .insert(approval_id, request_id.clone());
    let selected = ask_permission(inner, &active, &request_id, &params).await;
    registry.lock().unwrap().remove(&approval_id);
    match selected.as_deref() {
        Some("allow-once") => "allowed-once",
        _ => "rejected",
    }
}

async fn handle_agent_request(
    inner: &Inner,
    process: &AcpProcess,
    req_id: i64,
    method: &str,
    params: Value,
) -> Result<()> {
    let raw_session = params
        .get("sessionId")
        .or_else(|| params.get("threadId"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let (cwd, policy, active) = {
        let sessions = inner.sessions.lock().await;
        let live = sessions.values().find(|s| s.session_id == raw_session);
        match live {
            Some(live) => (
                live.cwd.clone(),
                AcpRuntime::policy_from_live(live),
                live.active
                    .as_ref()
                    .map(|a| (a.thread_id.clone(), a.turn_id.clone(), a.bus.clone())),
            ),
            None => (PathBuf::from("."), ProductSessionPolicy::default(), None),
        }
    };
    match method {
        "elicitation/create" | "item/tool/requestUserInput" => {
            let native = method == "item/tool/requestUserInput";
            let questions = super::elicitation::questions(&params, native)?;
            let active = active
                .as_ref()
                .ok_or_else(|| anyhow!("No active turn for user input request"))?;
            let request_id = format!("input-{}", uuid::Uuid::new_v4());
            let item_id = params
                .get("toolCallId")
                .or_else(|| params.get("itemId"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let deadline = params
                .get("autoResolutionMs")
                .or_else(|| params.pointer("/_meta/codex/autoResolutionMs"))
                .and_then(Value::as_u64)
                .and_then(|ms| std::time::Instant::now().checked_add(Duration::from_millis(ms)));
            let response = ask_user(
                inner,
                active,
                &request_id,
                (params, native),
                questions,
                item_id,
                deadline,
            )
            .await;
            process.respond(req_id, response).await?;
            emit_request_resolved(active, &request_id);
        }
        "session/request_permission" => {
            let choices = parse_permission_choices(&params);
            if policy.auto_approve() {
                if let Some(option) = select_permission_option(&choices, true, None) {
                    process
                        .respond(
                            req_id,
                            json!({ "outcome": { "outcome": "selected", "optionId": option } }),
                        )
                        .await?;
                } else {
                    process
                        .respond(req_id, json!({ "outcome": { "outcome": "cancelled" } }))
                        .await?;
                }
                return Ok(());
            }
            let selected = ask_permission(inner, &active, &format!("perm-{req_id}"), &params).await;
            if let Some(option) = selected {
                process
                    .respond(
                        req_id,
                        json!({ "outcome": { "outcome": "selected", "optionId": option } }),
                    )
                    .await?;
            } else {
                process
                    .respond(req_id, json!({ "outcome": { "outcome": "cancelled" } }))
                    .await?;
            }
        }
        "fs/read_text_file" => {
            let path = params.get("path").and_then(Value::as_str).unwrap_or("");
            let content = tokio::fs::read_to_string(
                PathBuf::from(path)
                    .is_absolute()
                    .then(|| PathBuf::from(path))
                    .unwrap_or_else(|| cwd.join(path)),
            )
            .await
            .unwrap_or_default();
            process
                .respond(req_id, json!({ "content": content }))
                .await?;
        }
        "fs/write_text_file" => {
            let path = params.get("path").and_then(Value::as_str).unwrap_or("");
            let content = params.get("content").and_then(Value::as_str).unwrap_or("");
            if policy.rejects_writes() {
                process
                    .respond_error(req_id, "writes are disabled in the current session mode")
                    .await?;
            } else {
                let scope = if policy.allows_writes_outside_workspace() {
                    WriteScope::Unrestricted
                } else {
                    WriteScope::Workspace
                };
                match write_file_with_scope(&cwd, path, content, scope) {
                    Ok(()) => process.respond(req_id, json!({})).await?,
                    Err(err) => process.respond_error(req_id, &err.to_string()).await?,
                }
            }
        }
        "terminal/create" => {
            let command = params
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("sh");
            let args = params
                .get("args")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut env = params
                .get("env")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|entry| {
                            Some((
                                entry.get("name")?.as_str()?.to_string(),
                                entry.get("value")?.as_str()?.to_string(),
                            ))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            env.extend(process.cli_env.clone());
            let output_byte_limit = params
                .get("outputByteLimit")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok());
            let term_cwd = params
                .get("cwd")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .unwrap_or(cwd);
            let id = inner
                .terminals
                .create(
                    &process.id,
                    command,
                    &args,
                    term_cwd,
                    &env,
                    output_byte_limit,
                )
                .await?;
            process.respond(req_id, json!({ "terminalId": id })).await?;
        }
        "terminal/output" => {
            let id = params
                .get("terminalId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let output = inner
                .terminals
                .output(id)
                .unwrap_or(json!({ "output": "" }));
            process.respond(req_id, output).await?;
        }
        "terminal/wait_for_exit" => {
            let id = params
                .get("terminalId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let status = inner.terminals.wait_for_exit(id).await?;
            process.respond(req_id, status).await?;
        }
        "terminal/kill" => {
            let id = params
                .get("terminalId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let _ = inner.terminals.kill(id);
            process.respond(req_id, json!({})).await?;
        }
        "terminal/release" => {
            let id = params
                .get("terminalId")
                .and_then(Value::as_str)
                .unwrap_or("");
            inner.terminals.release(id);
            process.respond(req_id, json!({})).await?;
        }
        _ => {
            process
                .respond(req_id, json!({ "error": "unsupported" }))
                .await
                .ok();
        }
    }
    Ok(())
}

/// Present product questions for the active turn and wait for the reply, the
/// turn's end, or an optional deadline. Unanswered requests resolve empty.
async fn ask_user(
    inner: &Inner,
    (thread_id, turn_id, bus): &(String, String, EventBus),
    request_id: &str,
    (params, native): (Value, bool),
    questions: Vec<Value>,
    item_id: Option<String>,
    deadline: Option<std::time::Instant>,
) -> Value {
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    inner
        .pending_inputs
        .lock()
        .await
        .insert(request_id.to_string(), PendingInput { tx, params, native });
    let dto = ThreadActionRequestDto {
        id: request_id.to_string(),
        kind: "requestUserInput".into(),
        title: "Question".into(),
        description: None,
        turn_id: Some(turn_id.clone()),
        item_id,
        created_at: now_rfc3339(),
        questions,
    };
    inner
        .pending_dtos
        .lock()
        .await
        .entry(thread_id.clone())
        .or_default()
        .push(dto.clone());
    bus.emit(ThreadEventEnvelope {
        event_type: pockymoe_protocol::ThreadEventType::ThreadRequestCreated,
        thread_id: thread_id.clone(),
        timestamp: now_rfc3339(),
        payload: json!({"request":dto}),
    });
    let empty = || {
        if native {
            json!({"answers":{}})
        } else {
            json!({"action":"cancel"})
        }
    };
    let response = loop {
        tokio::select! {
            reply = &mut rx => break reply.unwrap_or_else(|_| empty()),
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                let still_active = inner.sessions.lock().await.values().any(|session|session.active.as_ref().is_some_and(|a|a.thread_id == *thread_id && a.turn_id == *turn_id));
                if !still_active || deadline.is_some_and(|end| std::time::Instant::now() >= end) {break empty();}
            }
        }
    };
    inner.pending_inputs.lock().await.remove(request_id);
    if let Some(list) = inner.pending_dtos.lock().await.get_mut(thread_id) {
        list.retain(|q| q.id != request_id);
    }
    response
}

fn emit_request_resolved((thread_id, _, bus): &(String, String, EventBus), request_id: &str) {
    bus.emit(ThreadEventEnvelope {
        event_type: pockymoe_protocol::ThreadEventType::ThreadRequestResolved,
        thread_id: thread_id.clone(),
        timestamp: now_rfc3339(),
        payload: json!({"requestId":request_id}),
    });
}

async fn apply_mode_update(inner: &Inner, update: &Value) {
    let body = update.get("update").unwrap_or(update);
    if body.get("sessionUpdate").and_then(Value::as_str) != Some("current_mode_update") {
        return;
    }
    let mode_id = body
        .get("modeId")
        .or_else(|| body.get("currentModeId"))
        .and_then(Value::as_str);
    let Some(mode_id) = mode_id else {
        return;
    };
    let session_id = update
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut sessions = inner.sessions.lock().await;
    if let Some(live) = sessions
        .values_mut()
        .find(|session| session.session_id == session_id)
    {
        live.current_mode_id = Some(mode_id.to_string());
    }
}

fn extra_env_for(def: &AcpAgentDef) -> Vec<(&'static str, String)> {
    let mut env = crate::upstreams::launch_environment(&def.id);
    let home_key = match def.id.as_str() {
        "codex" => Some("CODEX_HOME"),
        "grok" => Some("GROK_HOME"),
        "claude" => Some("CLAUDE_CONFIG_DIR"),
        "opencode" => Some("OPENCODE_HOME"),
        "gemini" => Some("GEMINI_CLI_HOME"),
        _ => None,
    };
    if let Some(key) = home_key {
        if let Some(value) = std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            env.push((key, value));
        }
    }
    if def.id == "codex" {
        let codex_path = codex_path_for(def);
        env.push(("CODEX_PATH", codex_path));
    }
    env
}

fn codex_path_for(def: &AcpAgentDef) -> String {
    command_program(&def.base_command).unwrap_or_else(|| def.base_command.clone())
}

fn harness_cwd(cwd: &str) -> Cow<'_, str> {
    #[cfg(windows)]
    {
        if let Some(path) = cwd.strip_prefix(r"\\?\UNC\") {
            return Cow::Owned(format!(r"\\{path}"));
        }
        if let Some(path) = cwd.strip_prefix(r"\\?\") {
            return Cow::Borrowed(path);
        }
    }
    Cow::Borrowed(cwd)
}

fn agent_server_command(def: &AcpAgentDef, auto_approve: bool) -> String {
    if def.id != "grok" {
        return def.server_command.clone();
    }
    if auto_approve {
        "grok agent --always-approve --no-leader stdio".into()
    } else {
        "grok agent --no-leader stdio".into()
    }
}

fn apply_projection(live: &mut LiveSession, proj: super::adapter::HarnessProjection) {
    live.harness_state = proj.state;
    live.harness_models = proj.models;
    live.model = proj.model.or(live.model.clone());
    live.reasoning_effort = proj.reasoning_effort.or(live.reasoning_effort.clone());
}

fn default_model_stub(agent_id: Option<&str>) -> Vec<ModelOptionDto> {
    vec![ModelOptionDto {
        id: "default".into(),
        model: agent_id.unwrap_or("default").into(),
        display_name: "Default".into(),
        description: "Harness default model.".into(),
        is_default: true,
        hidden: false,
        supported_reasoning_efforts: vec![
            ReasoningEffortOptionDto {
                reasoning_effort: "low".into(),
                description: "Low".into(),
            },
            ReasoningEffortOptionDto {
                reasoning_effort: "medium".into(),
                description: "Medium".into(),
            },
            ReasoningEffortOptionDto {
                reasoning_effort: "high".into(),
                description: "High".into(),
            },
        ],
        default_reasoning_effort: Some("medium".into()),
        selection_kind: Some("model".into()),
        acp_agent: None,
    }]
}

fn list_command_models(def: &AcpAgentDef) -> Vec<ModelOptionDto> {
    let Some(command) = def.model_list_command.as_deref() else {
        return Vec::new();
    };
    let Ok(parsed) = parse_spawn_command(command) else {
        return Vec::new();
    };
    let mut command = std::process::Command::new(&parsed.program);
    command
        .env("PATH", super::catalog::child_path())
        .args(&parsed.args);
    crate::child_process::hide_std(&mut command);
    let output = command.output().ok();
    let Some(output) = output.filter(|output| output.status.success()) else {
        return Vec::new();
    };
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_command_models(&text)
        .into_iter()
        .map(|(model, is_default)| ModelOptionDto {
            id: model.clone(),
            model: model.clone(),
            display_name: model,
            description: String::new(),
            is_default,
            hidden: false,
            supported_reasoning_efforts: vec![],
            default_reasoning_effort: None,
            selection_kind: Some("model".into()),
            acp_agent: None,
        })
        .collect()
}

// ACP config IDs are agent-defined; category is the portable semantic key.
// Missing options mean reasoning is agent-managed, not an implicit thought-level option.
fn reasoning_config_id(options: &Value) -> Option<&str> {
    options.as_array().and_then(|options| {
        options
            .iter()
            .find(|option| option.get("category").and_then(Value::as_str) == Some("thought_level"))
            .or_else(|| {
                options.iter().find(|option| {
                    matches!(
                        option.get("id").and_then(Value::as_str),
                        Some("reasoning_effort" | "thought_level" | "thought-level")
                    )
                })
            })
            .and_then(|option| option.get("id").and_then(Value::as_str))
    })
}

pub(super) fn models_from_config_options(options: &Value) -> Vec<ModelOptionDto> {
    let Some(arr) = options.as_array() else {
        return Vec::new();
    };
    let model_opt = arr.iter().find(|opt| {
        opt.get("category").and_then(Value::as_str) == Some("model")
            || opt
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.to_ascii_lowercase().contains("model"))
    });
    let Some(model_opt) = model_opt else {
        return Vec::new();
    };
    let current = model_opt
        .get("currentValue")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let efforts = reasoning_efforts_from_options(arr);
    collect_select_options(model_opt)
        .into_iter()
        .filter_map(|entry| {
            let value = entry.get("value").and_then(Value::as_str)?;
            Some(ModelOptionDto {
                id: value.to_string(),
                model: value.to_string(),
                display_name: entry
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(value)
                    .to_string(),
                description: entry
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                is_default: value == current,
                hidden: false,
                supported_reasoning_efforts: efforts.clone(),
                default_reasoning_effort: efforts
                    .iter()
                    .find(|effort| effort.reasoning_effort == "medium")
                    .or(efforts.first())
                    .map(|effort| effort.reasoning_effort.clone()),
                selection_kind: Some("model".into()),
                acp_agent: None,
            })
        })
        .collect()
}

fn collect_select_options(opt: &Value) -> Vec<&Value> {
    let Some(options) = opt.get("options").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in options {
        if entry.get("options").is_some() {
            out.extend(collect_select_options(entry));
        } else {
            out.push(entry);
        }
    }
    out
}

fn reasoning_efforts_from_options(options: &[Value]) -> Vec<ReasoningEffortOptionDto> {
    let thought = options.iter().find(|opt| {
        opt.get("category").and_then(Value::as_str) == Some("thought_level")
            || opt.get("id").and_then(Value::as_str).is_some_and(|id| {
                let id = id.to_ascii_lowercase();
                id.contains("thought") || id.contains("reasoning") || id.contains("effort")
            })
    });
    let Some(thought) = thought else {
        return Vec::new();
    };
    collect_select_options(thought)
        .into_iter()
        .filter_map(|entry| {
            let value = entry.get("value").and_then(Value::as_str)?;
            Some(ReasoningEffortOptionDto {
                reasoning_effort: value.to_string(),
                description: entry
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or(value)
                    .to_string(),
            })
        })
        .collect()
}

fn apply_config_option_caps(caps: &mut AgentProviderCapabilitiesDto, options: &Value) {
    let Some(arr) = options.as_array() else {
        return;
    };
    if arr.iter().any(|opt| {
        matches!(
            opt.get("id").and_then(Value::as_str),
            Some("fast-mode" | "fast")
        )
    }) {
        caps.controls.performance_mode = true;
    }
}

fn merge_active_subagents(
    agents: Vec<ThreadSubagentDto>,
    claude: Option<&super::claude_usage::ClaudeUsageReader>,
) -> Vec<ThreadSubagentDto> {
    let mut agents: HashMap<_, _> = agents
        .into_iter()
        .map(|agent| (agent.id.clone(), agent))
        .collect();
    if let Some(reader) = claude {
        for agent in reader.background_subagents() {
            agents.insert(agent.id.clone(), agent);
        }
    }
    let mut agents: Vec<_> = agents.into_values().collect();
    agents.sort_by(|a, b| a.id.cmp(&b.id));
    agents
}

fn emit_mapped(bus: &EventBus, thread_id: &str, turn_id: &str, mapped: MappedUpdate, hidden: bool) {
    if hidden {
        return;
    }
    if let Some(usage) = mapped.usage {
        emit_usage(bus, thread_id, turn_id, usage, hidden);
    }
    if let Some(active_subagents) = mapped.active_subagents {
        bus.emit(ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadSubagentsUpdated,
            thread_id: thread_id.into(),
            timestamp: now_rfc3339(),
            payload: json!({
                "turnId": turn_id,
                "activeSubagents": active_subagents,
            }),
        });
    }
    for (item_id, delta, sequence) in mapped.deltas {
        bus.emit(ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadOutputDelta,
            thread_id: thread_id.into(),
            timestamp: now_rfc3339(),
            payload: json!({
                "turnId": turn_id,
                "itemId": item_id,
                "sequence": sequence,
                "delta": delta
            }),
        });
    }
    for item in mapped.items {
        let completed = matches!(
            item.status.as_deref(),
            Some("completed" | "failed" | "interrupted")
        );
        bus.emit(ThreadEventEnvelope {
            event_type: if completed {
                pockymoe_protocol::ThreadEventType::ThreadItemCompleted
            } else {
                pockymoe_protocol::ThreadEventType::ThreadItemStarted
            },
            thread_id: thread_id.into(),
            timestamp: now_rfc3339(),
            payload: json!({ "turnId": turn_id, "item": item }),
        });
    }
    if let Some(plan) = mapped.plan {
        bus.emit(ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadPlanUpdated,
            thread_id: thread_id.into(),
            timestamp: now_rfc3339(),
            payload: json!({
                "turnId": turn_id,
                "explanation": Value::Null,
                "plan": plan
                    .into_iter()
                    .map(|(step, status)| json!({ "step": step, "status": status }))
                    .collect::<Vec<_>>()
            }),
        });
    }
}

fn emit_usage(bus: &EventBus, thread_id: &str, turn_id: &str, usage: Value, hidden: bool) {
    if hidden {
        return;
    }
    if let Some(context) = crate::usage::context_usage(&usage, &now_rfc3339()) {
        bus.emit(ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadContextUpdated,
            thread_id: thread_id.into(),
            timestamp: now_rfc3339(),
            payload: json!({"contextUsage":context}),
        });
    }
    if crate::usage::normalize_usage(&usage).is_some() {
        bus.emit(ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::RuntimeUsageUpdated,
            thread_id: thread_id.into(),
            timestamp: now_rfc3339(),
            payload: json!({"turnId":turn_id,"usage":usage}),
        });
    }
}

fn cli_context_matches(existing: &[(String, String)]) -> bool {
    crate::interaction::launch_env()
        .iter()
        .filter(|(key, _)| key.starts_with("POCKYMOE_"))
        .all(|pair| existing.contains(pair))
}
