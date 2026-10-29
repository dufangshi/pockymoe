//! Device-owned hooks. Conditions are data, never executable code.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationDefinition {
    pub name: String,
    pub trigger: AutomationTrigger,
    #[serde(default)]
    pub condition: AutomationCondition,
    pub action: AutomationAction,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "lateness")]
    pub max_lateness_seconds: u64,
    #[serde(default)]
    pub missed_run_policy: MissedRunPolicy,
    /// Explicit opt-in when registering an already terminal immutable source.
    #[serde(default)]
    pub replay_existing: bool,
}
fn yes() -> bool {
    true
}
fn lateness() -> u64 {
    86400
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MissedRunPolicy {
    #[default]
    CoalesceLatest,
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AutomationTrigger {
    Interval {
        every_seconds: u64,
        anchor_at: Option<String>,
    },
    At {
        at: String,
    },
    /// Every subsequent complete turn of this source; historical replay is rejected.
    ThreadEnded {
        source_thread_id: String,
    },
    TurnEnded {
        source_thread_id: String,
        turn_id: String,
    },
    TaskEnded {
        root_thread_id: String,
        task_number: i64,
    },
    CommandEnded {
        source_thread_id: String,
        command_id: Option<String>,
        command_key: Option<String>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AutomationCondition {
    ReplyLengthAtLeast {
        chars: u64,
    },
    All {
        conditions: Vec<AutomationCondition>,
    },
    Any {
        conditions: Vec<AutomationCondition>,
    },
    Not {
        condition: Box<AutomationCondition>,
    },
    StatusIn {
        values: Vec<String>,
    },
    ExitCodeEquals {
        value: i32,
    },
    WorkspaceId {
        value: String,
    },
    CommandId {
        value: String,
    },
}
impl Default for AutomationCondition {
    fn default() -> Self {
        Self::All { conditions: vec![] }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AutomationAction {
    PluginAction {
        plugin_id: String,
        action: String,
        #[serde(default)]
        input: serde_json::Value,
        #[serde(default = "timeout")]
        timeout_seconds: u64,
    },
    Prompt {
        text: String,
    },
    NotifyInbox {
        subject: String,
        text: String,
        #[serde(default = "result_kind")]
        message_kind: String,
        #[serde(default)]
        include_closing_message: bool,
    },
    RunScript {
        #[serde(flatten)]
        command: CommandSpec,
    },
}
fn result_kind() -> String {
    "result".into()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSpec {
    /// Fixed argv. Mutually exclusive with shell.
    #[serde(default)]
    pub argv: Vec<String>,
    /// Explicit shell text: /bin/sh -c (Windows: cmd /C).
    pub shell: Option<String>,
    /// Absolute or relative to the target thread workspace.
    pub cwd: String,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
    /// Explicit extra environment. The command never inherits the Supervisor's
    /// own environment beyond the non-secret basics (PATH, HOME, locale, ...).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
}
fn timeout() -> u64 {
    60
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandRunInput {
    #[serde(flatten)]
    pub command: CommandSpec,
    pub command_key: Option<String>,
    /// A retry returns the original execution; it never spawns again.
    pub client_request_id: Option<String>,
}
