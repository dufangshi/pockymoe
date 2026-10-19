use pockymoe_protocol::{
    toolbox_from_capabilities, AgentProviderCapabilitiesDto, ModelOptionDto, ToolboxItemDto,
};
use serde_json::{json, Value};

use super::capabilities::NegotiatedCaps;
use super::grok;

#[derive(Debug, Clone)]
pub struct HarnessProjection {
    pub state: Value,
    pub models: Vec<ModelOptionDto>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SessionSettingOp {
    SetConfig { config_id: String, value: String },
    SetModel { model_id: String },
    SetMode { mode_id: String },
    LoadWithMeta { meta: Value },
}

/// Per-harness translator over a shared ACP client.
/// Generic session/prompt/cancel/permission stay in the runtime.
/// Native extensions must reuse the ACP-owned connection, never a second writer.
pub trait HarnessAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    /// ACP has prompt completion but no portable query for an outstanding turn.
    /// All harnesses share the connection/request lifecycle fallback. An adapter
    /// may refine this with a negotiated native read-only status extension.
    fn execution_state(
        &self,
        connected: bool,
        active_turn: Option<&str>,
    ) -> crate::actor::ExecutionState {
        use crate::actor::ExecutionState;
        if !connected {
            return ExecutionState::Unknown;
        }
        match active_turn {
            Some(id) => ExecutionState::Running { turn_id: id.into() },
            None => ExecutionState::Idle,
        }
    }
    fn compact_prompt(&self) -> Option<&'static str> {
        None
    }
    fn initialize_client_meta(&self) -> Value {
        json!({})
    }
    fn context_usage(&self, _update: &Value, _state: &Value) -> Option<Value> {
        None
    }
    fn billing_usage(&self, _update: &Value) -> Option<Value> {
        None
    }
    fn accepts_notification(&self, method: &str) -> bool {
        method == "session/update"
    }
    /// Method and required-field error used by a non-mutating empty-params probe.
    fn fork_probe(&self) -> Option<(&'static str, &'static str)> {
        None
    }
    fn extension_fork_params(&self, session_id: &str, cwd: &str) -> Value {
        json!({"sessionId":session_id,"cwd":cwd})
    }
    fn extension_fork_id(&self, response: &Value) -> Option<String> {
        response["sessionId"].as_str().map(str::to_string)
    }
    fn prompt_preamble(&self) -> Option<&'static str> {
        None
    }
    fn model_list_method(&self) -> Option<&'static str> {
        None
    }
    /// The session's effort option lists only its current model's levels, which
    /// differ by model, so a probe session visits the others to learn theirs.
    fn effort_options_follow_model(&self) -> bool {
        false
    }
    fn project_model_list(&self, _response: &Value) -> Option<Vec<ModelOptionDto>> {
        None
    }
    fn model_list_supports_performance(&self, _response: &Value) -> bool {
        false
    }
    fn fs_read_text_file(&self) -> bool {
        true
    }
    fn fs_write_text_file(&self) -> bool {
        true
    }
    fn session_new_meta(&self, _reasoning_effort: Option<&str>) -> Value {
        json!({})
    }
    fn session_load_meta(&self) -> Value {
        json!({})
    }
    fn project_session(&self, _response: &Value) -> Option<HarnessProjection> {
        None
    }
    fn apply_model(&self, _model: &str, _state: &Value) -> Option<SessionSettingOp> {
        None
    }
    fn apply_reasoning(&self, _effort: &str, _state: &Value) -> Option<SessionSettingOp> {
        None
    }
    fn patch_capabilities(
        &self,
        caps: &mut AgentProviderCapabilitiesDto,
        negotiated: &NegotiatedCaps,
    ) {
        let _ = (caps, negotiated);
    }
    fn toolbox_items(
        &self,
        caps: &AgentProviderCapabilitiesDto,
        negotiated: &NegotiatedCaps,
    ) -> Vec<ToolboxItemDto> {
        let mut items = toolbox_from_capabilities(caps);
        for command in &negotiated.available_commands {
            // Codex advertises individual skills as $name commands. Keep them
            // callable through ACP, without filling the product toolbox.
            if command.name.starts_with('$') {
                continue;
            }
            let slash_command = format!("/{}", command.name);
            if items.iter().any(|item| item.command == slash_command) {
                continue;
            }
            items.push(ToolboxItemDto {
                action: "prompt".into(),
                command: slash_command,
                label: command.name.clone(),
                description: command.description.clone(),
                panel: None,
            });
        }
        items
    }
}

pub struct StandardAdapter;

impl HarnessAdapter for StandardAdapter {
    fn id(&self) -> &'static str {
        "standard"
    }
    fn patch_capabilities(
        &self,
        caps: &mut AgentProviderCapabilitiesDto,
        negotiated: &NegotiatedCaps,
    ) {
        apply_negotiated(caps, negotiated);
    }
}

pub struct CodexAdapter;

impl HarnessAdapter for CodexAdapter {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn compact_prompt(&self) -> Option<&'static str> {
        Some("/compact")
    }
    fn project_session(&self, response: &Value) -> Option<HarnessProjection> {
        super::codex_models::project_session(response)
    }
    fn patch_capabilities(
        &self,
        caps: &mut AgentProviderCapabilitiesDto,
        negotiated: &NegotiatedCaps,
    ) {
        apply_negotiated(caps, negotiated);
        // Codex ACP compact is a hidden `/compact` turn, not a native method.
        caps.turns.compact = true;
        // Fork is enabled only after the owned app-server bridge is installed.
        caps.branching.fork = negotiated.fork;
        caps.branching.fork_at = negotiated.fork;
        caps.branching.hard_rollback = false;
        caps.management.mcp_status = false;
        caps.management.skills = false;
        caps.management.hooks = false;
        caps.management.hook_trust = false;
        caps.management.host_config_files = false;
        caps.management.provider_settings = false;
    }
}

pub struct ClaudeAdapter;

impl HarnessAdapter for ClaudeAdapter {
    fn id(&self) -> &'static str {
        "claude"
    }
    fn session_new_meta(&self, _reasoning_effort: Option<&str>) -> Value {
        // Request lifecycle bookends only; never duplicate streamed assistant text.
        json!({"claudeCode":{"emitRawSDKMessages":[
            {"type":"command_lifecycle"}, {"type":"result"}, {"type":"system"},
            {"type":"user","origin":"task-notification"}
        ]}})
    }
    fn accepts_notification(&self, method: &str) -> bool {
        matches!(method, "session/update" | "_claude/sdkMessage")
    }
    fn session_load_meta(&self) -> Value {
        self.session_new_meta(None)
    }
    fn effort_options_follow_model(&self) -> bool {
        true
    }
    fn patch_capabilities(
        &self,
        caps: &mut AgentProviderCapabilitiesDto,
        negotiated: &NegotiatedCaps,
    ) {
        apply_negotiated(caps, negotiated);
        // A model or effort chosen mid-turn goes to the running query and takes
        // effect at its next model call.
        caps.controls.live_model_switch = true;
    }
}

pub struct CursorAdapter;

impl HarnessAdapter for CursorAdapter {
    fn id(&self) -> &'static str {
        "cursor"
    }
    fn initialize_client_meta(&self) -> Value {
        json!({ "parameterizedModelPicker": true })
    }
    fn prompt_preamble(&self) -> Option<&'static str> {
        Some(
            "Cursor ACP client constraint: do not launch background subagents. If you delegate \
             work, wait for every subagent result in the current turn and deliver the complete \
             requested answer before ending the turn.",
        )
    }
    fn model_list_method(&self) -> Option<&'static str> {
        Some("cursor/list_available_models")
    }
    fn project_model_list(&self, response: &Value) -> Option<Vec<ModelOptionDto>> {
        let models = response.get("models")?.as_array()?;
        let projected: Vec<_> = models
            .iter()
            .enumerate()
            .filter_map(|(index, model)| cursor_model(model, index))
            .collect();
        (!projected.is_empty()).then_some(projected)
    }
    fn model_list_supports_performance(&self, response: &Value) -> bool {
        response
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|model| {
                model
                    .get("configOptions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .any(is_fast_option)
    }
    fn patch_capabilities(
        &self,
        caps: &mut AgentProviderCapabilitiesDto,
        negotiated: &NegotiatedCaps,
    ) {
        apply_negotiated(caps, negotiated);
        caps.management.models = true;
    }
}

pub struct GrokAdapter;

impl HarnessAdapter for GrokAdapter {
    fn fork_probe(&self) -> Option<(&'static str, &'static str)> {
        Some(("_x.ai/session/fork", "sourceSessionId"))
    }
    fn extension_fork_params(&self, session_id: &str, cwd: &str) -> Value {
        json!({"sourceSessionId":session_id,"sourceCwd":cwd,"newCwd":cwd})
    }
    fn extension_fork_id(&self, response: &Value) -> Option<String> {
        response["newSessionId"].as_str().map(str::to_string)
    }
    fn accepts_notification(&self, method: &str) -> bool {
        matches!(
            method,
            "session/update" | "_x.ai/session/update" | "_x.ai/session_notification"
        )
    }
    fn billing_usage(&self, update: &Value) -> Option<Value> {
        grok::billing_usage(update)
    }
    fn context_usage(&self, update: &Value, state: &Value) -> Option<Value> {
        grok::context_usage(update, state)
    }
    fn id(&self) -> &'static str {
        "grok"
    }
    fn fs_read_text_file(&self) -> bool {
        false
    }
    fn session_new_meta(&self, reasoning_effort: Option<&str>) -> Value {
        match grok::normalize_acp_effort(reasoning_effort) {
            Some(effort) => json!({ "reasoningEffort": effort }),
            None => json!({}),
        }
    }
    fn project_session(&self, response: &Value) -> Option<HarnessProjection> {
        grok::project_session(response)
    }
    fn apply_model(&self, model: &str, _state: &Value) -> Option<SessionSettingOp> {
        Some(grok::apply_model(model, _state))
    }
    fn apply_reasoning(&self, effort: &str, state: &Value) -> Option<SessionSettingOp> {
        grok::apply_reasoning(effort, state)
    }
    fn patch_capabilities(
        &self,
        caps: &mut AgentProviderCapabilitiesDto,
        negotiated: &NegotiatedCaps,
    ) {
        apply_negotiated(caps, negotiated);
        caps.management.models = true;
    }
}

pub struct DeepSeekAdapter;

impl HarnessAdapter for DeepSeekAdapter {
    fn id(&self) -> &'static str {
        "deepseek"
    }
    fn apply_model(&self, model: &str, _state: &Value) -> Option<SessionSettingOp> {
        Some(SessionSettingOp::SetConfig {
            config_id: "model".into(),
            value: model.into(),
        })
    }

    fn patch_capabilities(
        &self,
        caps: &mut AgentProviderCapabilitiesDto,
        negotiated: &NegotiatedCaps,
    ) {
        apply_negotiated(caps, negotiated);
        // Web presets and plan mode are not exposed by the ACP profile.
        caps.controls.plan_mode = false;
    }
}

pub struct GeminiAdapter;

impl HarnessAdapter for GeminiAdapter {
    fn id(&self) -> &'static str {
        "gemini"
    }
    fn project_session(&self, response: &Value) -> Option<HarnessProjection> {
        // Gemini exposes the legacy ACP model selector, with no reasoning selector.
        let state = response.get("models")?;
        let current = state["currentModelId"].as_str();
        let mut available = state["availableModels"].as_array()?.clone();
        // The built-in catalog can omit a model configured through a custom gateway.
        if let Some(id) =
            current.filter(|id| !available.iter().any(|entry| entry["modelId"] == *id))
        {
            available.insert(0, json!({"modelId": id}));
        }
        let models = available
            .iter()
            .filter_map(|entry| {
                let id = entry["modelId"].as_str()?;
                Some(ModelOptionDto {
                    id: id.into(),
                    model: id.into(),
                    display_name: entry["name"].as_str().unwrap_or(id).into(),
                    description: entry["description"].as_str().unwrap_or_default().into(),
                    is_default: current == Some(id),
                    hidden: false,
                    supported_reasoning_efforts: vec![],
                    default_reasoning_effort: None,
                    selection_kind: Some("model".into()),
                    acp_agent: None,
                })
            })
            .collect();
        Some(HarnessProjection {
            state: state.clone(),
            models,
            model: current.map(str::to_string),
            reasoning_effort: None,
        })
    }
    fn apply_model(&self, model: &str, _state: &Value) -> Option<SessionSettingOp> {
        Some(SessionSettingOp::SetModel {
            model_id: model.into(),
        })
    }
    fn patch_capabilities(
        &self,
        caps: &mut AgentProviderCapabilitiesDto,
        negotiated: &NegotiatedCaps,
    ) {
        apply_negotiated(caps, negotiated);
    }
}

pub fn adapter_for(agent_id: &str) -> Box<dyn HarnessAdapter> {
    match agent_id {
        "codex" => Box::new(CodexAdapter),
        "claude" => Box::new(ClaudeAdapter),
        "cursor" => Box::new(CursorAdapter),
        "grok" => Box::new(GrokAdapter),
        "deepseek" => Box::new(DeepSeekAdapter),
        "gemini" => Box::new(GeminiAdapter),
        _ => Box::new(StandardAdapter),
    }
}

fn cursor_model(model: &Value, index: usize) -> Option<ModelOptionDto> {
    let value = model.get("value").and_then(Value::as_str)?;
    let config_options = model
        .get("configOptions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let reasoning = config_options.iter().find(|option| {
        option.get("category").and_then(Value::as_str) == Some("thought_level")
            && !select_options(option).is_empty()
    });
    let efforts: Vec<_> = reasoning
        .into_iter()
        .flat_map(select_options)
        .filter_map(|entry| {
            let raw = entry.get("value").and_then(Value::as_str)?;
            Some(pockymoe_protocol::ReasoningEffortOptionDto {
                reasoning_effort: grok::normalize_acp_effort(Some(raw))?,
                description: entry
                    .get("description")
                    .or_else(|| entry.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect();
    let default_reasoning_effort = reasoning
        .and_then(|option| option.get("currentValue"))
        .and_then(Value::as_str)
        .and_then(|value| grok::normalize_acp_effort(Some(value)));
    Some(ModelOptionDto {
        id: value.to_string(),
        model: value.to_string(),
        display_name: model
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(value)
            .to_string(),
        description: model
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        is_default: index == 0,
        hidden: false,
        supported_reasoning_efforts: efforts,
        default_reasoning_effort,
        selection_kind: Some("model".into()),
        acp_agent: None,
    })
}

fn select_options(option: &Value) -> Vec<&Value> {
    option
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|entry| {
            if entry.get("options").is_some() {
                select_options(entry)
            } else {
                vec![entry]
            }
        })
        .collect()
}

fn is_fast_option(option: &Value) -> bool {
    matches!(
        option.get("id").and_then(Value::as_str),
        Some("fast" | "fast-mode")
    )
}

fn apply_negotiated(caps: &mut AgentProviderCapabilitiesDto, negotiated: &NegotiatedCaps) {
    // Only upgrade advertised product defaults. Empty/default negotiated caps must not
    // mark an installed harness as unable to start or resume threads.
    if negotiated.load_session {
        caps.sessions.load = Some(true);
    }
    if negotiated.resume {
        caps.sessions.resume = true;
    }
    if negotiated.close {
        caps.sessions.close = Some(true);
    }
    if negotiated.delete {
        caps.sessions.delete = Some(true);
    }
    if negotiated.list {
        caps.sessions.list = true;
    }
    if negotiated.steer {
        caps.turns.steer = true;
    }
    if negotiated.compact {
        caps.turns.compact = true;
    }
    if negotiated.fork {
        caps.branching.fork = true;
    }
    if negotiated.goals {
        caps.controls.goals = true;
    }
    if negotiated.fast {
        caps.controls.performance_mode = true;
    }
    caps.controls.permission_requests = true;
    caps.usage.token_usage = true;
    caps.usage.cost_usd = true;
    caps.usage.context_window = true;
}
