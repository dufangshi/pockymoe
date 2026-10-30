use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiatedCommand {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct NegotiatedCaps {
    pub load_session: bool,
    pub resume: bool,
    pub list: bool,
    pub close: bool,
    pub delete: bool,
    pub fork: bool,
    pub fork_method: Option<String>,
    pub steer: bool,
    pub compact: bool,
    pub goals: bool,
    pub fast: bool,
    pub image: bool,
    pub goal_method: Option<String>,
    pub agent_name: Option<String>,
    /// `agentInfo.version` from initialize, for features adapters gained later.
    pub agent_version: Option<String>,
    pub available_commands: Vec<NegotiatedCommand>,
    /// `mcpCapabilities`: stdio MCP servers are always accepted; HTTP/SSE only when advertised.
    pub mcp_http: bool,
    pub mcp_sse: bool,
}

fn capability_advertised(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::Object(_)) => true,
        _ => false,
    }
}

pub(super) fn parse_commands(value: &Value) -> Vec<NegotiatedCommand> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|command| {
            let name = command
                .get("name")
                .and_then(Value::as_str)?
                .trim()
                .trim_start_matches('/');
            if name.is_empty() {
                return None;
            }
            Some(NegotiatedCommand {
                name: name.to_string(),
                description: command
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            })
        })
        .collect()
}

pub fn negotiate(initialize: &Value) -> NegotiatedCaps {
    let caps = initialize
        .get("agentCapabilities")
        .cloned()
        .unwrap_or(Value::Null);
    let session = caps
        .get("sessionCapabilities")
        .or_else(|| caps.get("session"))
        .cloned()
        .unwrap_or(Value::Null);
    let prompt = caps
        .get("promptCapabilities")
        .cloned()
        .unwrap_or(Value::Null);
    let meta = initialize.get("_meta").cloned().unwrap_or(Value::Null);
    let steering = meta.get("steering").cloned().unwrap_or(Value::Null);
    let goal = meta.get("goal").cloned().unwrap_or(Value::Null);
    let available_commands = parse_commands(&meta["availableCommands"]);
    let mcp = caps.get("mcpCapabilities").cloned().unwrap_or(Value::Null);
    NegotiatedCaps {
        mcp_http: mcp.get("http").and_then(Value::as_bool).unwrap_or(false),
        mcp_sse: mcp.get("sse").and_then(Value::as_bool).unwrap_or(false),
        load_session: capability_advertised(caps.get("loadSession"))
            || capability_advertised(session.get("load")),
        resume: capability_advertised(session.get("resume")),
        list: capability_advertised(session.get("list")),
        close: capability_advertised(session.get("close")),
        delete: capability_advertised(session.get("delete")),
        fork: capability_advertised(session.get("fork")),
        fork_method: None,
        steer: steering
            .get("supported")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        compact: available_commands
            .iter()
            .any(|command| command.name == "compact"),
        goals: goal.get("controlMethod").and_then(Value::as_str).is_some()
            || goal.get("version").is_some(),
        fast: false,
        image: prompt
            .get("image")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        goal_method: goal
            .get("controlMethod")
            .and_then(Value::as_str)
            .map(str::to_string),
        agent_name: initialize
            .pointer("/agentInfo/name")
            .and_then(Value::as_str)
            .map(str::to_string),
        agent_version: initialize
            .pointer("/agentInfo/version")
            .and_then(Value::as_str)
            .map(str::to_string),
        available_commands,
    }
}
