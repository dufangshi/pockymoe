use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Result};
use async_trait::async_trait;
use remote_codex_protocol::{
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
}

enum TurnOutcome {
    Completed,
    Interrupted,
    Failed(anyhow::Error),
}

struct LiveSession {
    process: Arc<AcpProcess>,
    codex_bridge: Option<Arc<super::codex_bridge::CodexBridge>>,
    _discovery_directory: Option<tempfile::TempDir>,
    harness_info: Option<Value>,
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
    upstream_revision: Option<Vec<u8>>,
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
        let mut server_command = agent_server_command(def, yolo);
        let discovery = if def.id == "deepseek" {
            Some(super::deepseek::Discovery::prepare(&mut server_command).await?)
        } else {
            None
        };
        let (process, updates_rx, requests_rx) = tokio::time::timeout(
            self.startup_timeout,
            AcpProcess::spawn(&server_command, cwd, &extra_env),
        )
        .await
        .map_err(|_| anyhow!("ACP spawn timeout"))??;
        let process = Arc::new(process);
        let mut startup_process = StartupProcess(Some(process.clone()));
        let harness_info = if let Some(discovery) = &discovery {
            match tokio::time::timeout(self.startup_timeout, discovery.receive()).await {
                Ok(Ok(info)) => Some(info),
                result => {
                    let _ = process.shutdown().await;
                    bail!("DSH startup discovery failed (requires DSH 0.1.5-rc.1 or newer): {result:?}");
                }
            }
        } else {
            None
        };
        let init = process
            .request(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientInfo": {
                        "name": "remote-codex",
                        "title": "Remote Codex",
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
                        json!({ "sessionId": existing, "cwd": cwd, "mcpServers": [] }),
                    )
                    .await?
            } else if negotiated.resume {
                process
                    .request(
                        "session/resume",
                        json!({ "sessionId": existing, "cwd": cwd, "mcpServers": [] }),
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
        let mut live = LiveSession {
            process,
            codex_bridge,
            _discovery_directory: discovery.map(|discovery| discovery.directory),
            harness_info: harness_info.clone(),
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
            operation: Arc::new(Mutex::new(())),
            config_options: config_options.clone(),
            harness_state,
            harness_models: if let Some(info) = &harness_info {
                let mut models: Vec<ModelOptionDto> =
                    serde_json::from_value(info["models"].clone())?;
                let current = models_from_config_options(&config_options)
                    .into_iter()
                    .find(|model| model.is_default);
                for model in &mut models {
                    model.is_default = current
                        .as_ref()
                        .is_some_and(|current| current.model == model.model);
                }
                models
            } else {
                harness_models
            },
            available_modes,
            current_mode_id,
            context_thread_id: None,
            active_subagents: HashMap::new(),
        };
        Self::apply_product_mode(&live.process.clone(), &mut live).await?;
        startup_process.0 = None;
        Ok((scoped, live))
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
                        "name": "remote-codex",
                        "title": "Remote Codex",
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
        if sandbox.is_some() || collab.is_some() || approval.is_some() {
            Self::apply_product_mode(&live.process.clone(), live).await?;
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
                "adapter_missing" => Some(format!("Missing executable {} (package {}). Install in Settings > Harnesses > ACP adapter; it is also prepared automatically before execution.", def.server_command, super::dependencies::package(&def.id).unwrap_or("unknown"))),
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
        if let Some(upstreams) = &self.upstreams {
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
        super::dependencies::ensure(&def, false).await?;
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
            apply_config_option_caps(&mut caps, &live.config_options);
            snapshot.toolbox_items = adapter.toolbox_items(&caps, &negotiated);
            snapshot.effective_capabilities = Some(caps);
            if let Some(info) = &live.harness_info {
                snapshot.negotiated =
                    Some(json!({"harness":info,"configOptions":live.config_options}));
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
        let (process, session_id, cwd, image_capable, adapter_id, harness_state, needs_context) = {
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
                live.context_thread_id.as_deref() != Some(input.thread_id.as_str()),
            )
        };
        let adapter = adapter_for(&adapter_id);
        let prompt = adapter
            .prompt_preamble()
            .map(|preamble| format!("{preamble}\n\n{}", input.prompt))
            .unwrap_or_else(|| input.prompt.clone());
        let include_context = needs_context && !input.hidden && !process.cli_env.is_empty();
        let prompt = if include_context {
            format!("[remoteCodex: use `remote-codex thread self` or REMOTE_CODEX_THREAD_ID for this device's thread identity. `remote-codex skill` documents peer collaboration; read it before delegating. Messages default to a passive inbox, so check and acknowledge `remote-codex inbox` at checkpoints. To collect delegates' results, block with `remote-codex thread wait NAME...` or `remote-codex inbox wait` instead of polling or ending your turn. Threads you create group under you and are bounded: 3 levels deep, 20 open per root; close finished ones with `remote-codex thread close NAME`. Connection credentials are in the environment.]\n\n{prompt}")
        } else {
            prompt
        };
        let prompt_blocks = build_prompt_blocks(&prompt, &cwd, image_capable, &input.images)?;
        if !input.hidden {
            // Settings may have reloaded the process; identify the actual writer
            // selected for this turn, rather than the one before preflight.
            bus.emit(ThreadEventEnvelope {
                event_type: "thread.harness.ready".into(),
                thread_id: input.thread_id.clone(),
                timestamp: now_rfc3339(),
                payload: json!({"turnId":input.turn_id,"instanceId":process.id}),
            });
        }
        let mut updates = self.inner.updates.subscribe();
        {
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
            });
            live.active_subagents.clear();
        }
        let mut adapter_usage = super::usage::AdapterUsageAccumulator::default();
        let mut usage_reader =
            (adapter_id == "codex").then(|| super::usage::CodexUsageReader::new(&session_id));
        let mut claude_usage_reader = (adapter_id == "claude")
            .then(|| super::claude_usage::ClaudeUsageReader::new(&session_id));
        let mut usage_tick = tokio::time::interval(Duration::from_secs(1));
        let prompt_rpc = process.request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": prompt_blocks
            }),
        );
        let mut mapper = TurnMapper::new(&input.turn_id);
        if !input.hidden {
            bus.emit(ThreadEventEnvelope {
                event_type: "thread.turn.started".into(),
                thread_id: input.thread_id.clone(),
                timestamp: now_rfc3339(),
                payload: json!({ "turnId": input.turn_id }),
            });
        }
        tokio::pin!(prompt_rpc);
        let mut prompt_done = false;
        let mut cancel_sent = false;
        let mut cancel_deadline = tokio::time::Instant::now();
        let mut discard_session = false;
        let outcome = loop {
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
                result = &mut prompt_rpc, if !prompt_done => {
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
                            }
                            if let Some(usage) = response.get("usage").filter(|v| v.is_object() && !matches!(adapter_id.as_str(), "codex" | "copilot")) {
                                emit_usage(&bus, &input.thread_id, &input.turn_id, usage.clone(), input.hidden);
                            }
                            if response.get("stopReason").and_then(Value::as_str) == Some("cancelled") {
                                cancel.cancel();
                                cancel_sent = true;
                            }
                            if cancel_sent {
                                prompt_done = true;
                                continue;
                            }
                            prompt_done = true;
                        },
                        Err(err) => {
                            if cancel_sent || cancel.is_cancelled() {
                                discard_session = process.exited().await.unwrap_or(true);
                                break TurnOutcome::Interrupted;
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
                            if let Some(usage) = adapter.billing_usage(&update).and_then(|usage| adapter_usage.apply(usage)) {
                                emit_usage(&bus, &input.thread_id, &input.turn_id, usage, input.hidden);
                            }
                            if let Some(context) = adapter.context_usage(&update, &harness_state) {
                                emit_usage(&bus, &input.thread_id, &input.turn_id, context, input.hidden);
                            }
                            let mapped = mapper.apply(&update);
                            if let Some(subagents) = mapped.active_subagents.clone() {
                                if let Some(live) = self
                                    .inner
                                    .sessions
                                    .lock()
                                    .await
                                    .get_mut(&input.provider_session_id)
                                {
                                    live.active_subagents = subagents
                                        .iter()
                                        .cloned()
                                        .map(|agent| (agent.id.clone(), agent))
                                        .collect();
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
                _ = tokio::time::sleep(Duration::from_millis(250)), if prompt_done => {
                    break if cancel.is_cancelled() { TurnOutcome::Interrupted } else { TurnOutcome::Completed };
                }
                _ = tokio::time::sleep(Duration::from_millis(500)), if !prompt_done => {
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
            live.active_subagents.clear();
            if include_context && prompt_done {
                live.context_thread_id = Some(input.thread_id.clone());
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
        if let Some(reader) = claude_usage_reader.as_mut() {
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
        let mut items = mapper.finish(!matches!(outcome, TurnOutcome::Completed));
        for item in &mut items {
            if item.status.as_deref() != Some("failed") {
                item.status = Some(status.into());
            }
        }
        if !input.hidden {
            bus.emit(ThreadEventEnvelope {
                event_type: "thread.subagents.updated".into(),
                thread_id: input.thread_id.clone(),
                timestamp: now_rfc3339(),
                payload: json!({ "turnId": input.turn_id, "activeSubagents": [] }),
            });
            for item in &items {
                bus.emit(ThreadEventEnvelope {
                    event_type: "thread.item.completed".into(),
                    thread_id: input.thread_id.clone(),
                    timestamp: now_rfc3339(),
                    payload: json!({ "turnId": input.turn_id, "item": item }),
                });
            }
            bus.emit(ThreadEventEnvelope {
                event_type: "thread.turn.completed".into(),
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
        if let Some(live) = self.inner.sessions.lock().await.get(session_id) {
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
            // The fork's Remote Codex id does not exist yet. Leave it unloaded so
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
                _discovery_directory: None,
                harness_info: None,
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
                operation: Arc::new(Mutex::new(())),
                config_options,
                harness_state,
                harness_models,
                available_modes,
                current_mode_id,
                context_thread_id: None,
                active_subagents: HashMap::new(),
                upstream_revision: None,
            },
        );
        Ok(StartSessionResult {
            provider_session_id: scoped,
            model,
            reasoning_effort: effort,
        })
    }

    async fn send_input(&self, session_id: &str, turn_id: &str, prompt: &str) -> Result<()> {
        let (process, provider_session_id) = {
            let sessions = self.inner.sessions.lock().await;
            let live = sessions
                .get(session_id)
                .ok_or_else(|| anyhow!("ACP session is not running"))?;
            if !live.negotiated.steer {
                bail!("this harness does not support steering");
            }
            if !live
                .active
                .as_ref()
                .is_some_and(|active| active.turn_id == turn_id)
            {
                bail!("conflict: The active turn finished before this prompt could be steered.");
            }
            (live.process.clone(), live.session_id.clone())
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
        let sessions = self.inner.sessions.lock().await;
        let live = sessions
            .get(session_id)
            .ok_or_else(|| anyhow!("ACP session is not running"))?;
        Ok((live.adapter_id == "codex" && live.negotiated.goals)
            .then(|| format!("/goal {argument}")))
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
            let (thread_id, turn_id, bus) = active
                .as_ref()
                .ok_or_else(|| anyhow!("No active turn for user input request"))?;
            let request_id = format!("input-{}", uuid::Uuid::new_v4());
            let (tx, mut rx) = tokio::sync::oneshot::channel();
            inner.pending_inputs.lock().await.insert(
                request_id.clone(),
                PendingInput {
                    tx,
                    params: params.clone(),
                    native,
                },
            );
            let dto = ThreadActionRequestDto {
                id: request_id.clone(),
                kind: "requestUserInput".into(),
                title: "Question".into(),
                description: None,
                turn_id: Some(turn_id.clone()),
                item_id: params
                    .get("toolCallId")
                    .or_else(|| params.get("itemId"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
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
                event_type: "thread.request.created".into(),
                thread_id: thread_id.clone(),
                timestamp: now_rfc3339(),
                payload: json!({"request":dto}),
            });
            let deadline = params
                .get("autoResolutionMs")
                .or_else(|| params.pointer("/_meta/codex/autoResolutionMs"))
                .and_then(Value::as_u64)
                .and_then(|ms| std::time::Instant::now().checked_add(Duration::from_millis(ms)));
            let response = loop {
                tokio::select! {
                    reply = &mut rx => break reply.unwrap_or_else(|_| if native {json!({"answers":{}})} else {json!({"action":"cancel"})}),
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        let still_active = inner.sessions.lock().await.values().any(|session|session.active.as_ref().is_some_and(|a|a.thread_id == *thread_id && a.turn_id == *turn_id));
                        if !still_active || deadline.is_some_and(|end| std::time::Instant::now() >= end) {break if native {json!({"answers":{}})} else {json!({"action":"cancel"})};}
                    }
                }
            };
            inner.pending_inputs.lock().await.remove(&request_id);
            if let Some(list) = inner.pending_dtos.lock().await.get_mut(thread_id) {
                list.retain(|q| q.id != request_id);
            }
            process.respond(req_id, response).await?;
            bus.emit(ThreadEventEnvelope {
                event_type: "thread.request.resolved".into(),
                thread_id: thread_id.clone(),
                timestamp: now_rfc3339(),
                payload: json!({"requestId":request_id}),
            });
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
            let request_id = format!("perm-{}", uuid::Uuid::new_v4());
            let (tx, rx) = tokio::sync::oneshot::channel();
            inner.pending_permissions.lock().await.insert(
                request_id.clone(),
                PendingPermission {
                    tx,
                    options: choices.clone(),
                },
            );
            let title = permission_title(&params);
            if let Some((thread_id, turn_id, bus)) = &active {
                let dto = ThreadActionRequestDto {
                    id: request_id.clone(),
                    kind: "permissionRequest".into(),
                    title: title.clone(),
                    description: permission_description(&params),
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
                    event_type: "thread.request.created".into(),
                    thread_id: thread_id.clone(),
                    timestamp: now_rfc3339(),
                    payload: json!({ "request": dto }),
                });
            }
            let selected = tokio::time::timeout(Duration::from_secs(300), rx)
                .await
                .ok()
                .and_then(Result::ok);
            if let Some(option) = selected.filter(|option| option != "cancelled") {
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
            // Include timeout/cancellation, not just replies through HTTP. The
            // LLM clock and question UI must not remain paused after resolution.
            inner.pending_permissions.lock().await.remove(&request_id);
            if let Some((thread_id, turn_id, bus)) = &active {
                if let Some(list) = inner.pending_dtos.lock().await.get_mut(thread_id) {
                    list.retain(|request| request.id != request_id);
                }
                bus.emit(ThreadEventEnvelope {
                    event_type: "thread.request.resolved".into(),
                    thread_id: thread_id.clone(),
                    timestamp: now_rfc3339(),
                    payload: json!({"requestId":request_id,"turnId":turn_id}),
                });
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
                .create(command, &args, term_cwd, &env, output_byte_limit)
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

fn emit_mapped(bus: &EventBus, thread_id: &str, turn_id: &str, mapped: MappedUpdate, hidden: bool) {
    if hidden {
        return;
    }
    if let Some(usage) = mapped.usage {
        emit_usage(bus, thread_id, turn_id, usage, hidden);
    }
    if let Some(active_subagents) = mapped.active_subagents {
        bus.emit(ThreadEventEnvelope {
            event_type: "thread.subagents.updated".into(),
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
            event_type: "thread.output.delta".into(),
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
                "thread.item.completed".into()
            } else {
                "thread.item.started".into()
            },
            thread_id: thread_id.into(),
            timestamp: now_rfc3339(),
            payload: json!({ "turnId": turn_id, "item": item }),
        });
    }
    if let Some(plan) = mapped.plan {
        bus.emit(ThreadEventEnvelope {
            event_type: "thread.plan.updated".into(),
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
            event_type: "thread.context.updated".into(),
            thread_id: thread_id.into(),
            timestamp: now_rfc3339(),
            payload: json!({"contextUsage":context}),
        });
    }
    if crate::usage::normalize_usage(&usage).is_some() {
        bus.emit(ThreadEventEnvelope {
            event_type: "runtime.usage.updated".into(),
            thread_id: thread_id.into(),
            timestamp: now_rfc3339(),
            payload: json!({"turnId":turn_id,"usage":usage}),
        });
    }
}

fn cli_context_matches(existing: &[(String, String)]) -> bool {
    crate::interaction::launch_env()
        .iter()
        .filter(|(key, _)| key.starts_with("REMOTE_CODEX_"))
        .all(|pair| existing.contains(pair))
}
