pub mod access;
mod thread_schema;
pub use thread_schema::*;
mod automation;
pub use automation::*;
mod harness_resources;
pub use harness_resources::*;
mod interaction;
mod metrics;
pub use interaction::{
    ThreadSendInput, ThreadTranscriptQuery, MESSAGE_KINDS, THREAD_INTERACTION_GUIDES,
    THREAD_INTERACTION_SKILL,
};
pub use metrics::*;
pub mod treer;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Reported by `/api/version`. Native apps released before the rename may
/// identify a Supervisor by this value, so it keeps the pre-rename name.
pub const APP_NAME: &str = "remote-codex";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Local,
    Server,
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Codex,
    Claude,
    Opencode,
    Acp,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Opencode => "opencode",
            Self::Acp => "acp",
        }
    }

    pub fn from_name(raw: &str) -> Option<Self> {
        match raw {
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Claude),
            "opencode" => Some(Self::Opencode),
            "acp" => Some(Self::Acp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ApiError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            details: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthDto {
    pub status: String,
    pub timestamp: String,
    pub active_turn_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDto {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeConfigDto {
    pub app_name: String,
    pub app_version: String,
    pub mode: Mode,
    pub host: String,
    pub port: u16,
    pub workspace_root: String,
    pub environment: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<PlatformCapabilitiesDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCapabilitiesDto {
    pub terminal: bool,
    pub tmux: bool,
    pub managed_signals: bool,
    pub windows_task_scheduler: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSessionDto {
    pub authenticated: bool,
    pub username: Option<String>,
    pub expires_at: Option<String>,
    pub mode: Mode,
    pub auth_required: bool,
    /// Local mode with a passcode: browsers sign in with the passcode alone.
    #[serde(default)]
    pub passcode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDto {
    pub id: String,
    pub host_id: String,
    pub label: String,
    pub abs_path: String,
    pub is_favorite: bool,
    pub created_at: String,
    pub last_opened_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkspaceInput {
    #[serde(default)]
    pub abs_path: Option<String>,
    #[serde(default)]
    pub git_url: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSettingsDto {
    pub workspace_root: String,
    pub dev_home: String,
    pub default_backend: Provider,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWorkspaceSettingsInput {
    #[serde(default)]
    pub dev_home: Option<String>,
    #[serde(default)]
    pub default_backend: Option<Provider>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadWorkspaceTreeNodeDto {
    pub name: String,
    pub path: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_children: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children_loaded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<ThreadWorkspaceTreeNodeDto>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadWorkspaceFilePreviewDto {
    pub path: String,
    pub name: String,
    pub content: String,
    pub language: String,
    pub size: u64,
    pub truncated: bool,
    pub next_offset: u64,
}

/// One thread's activity for the thread map: how many turns it has had and how
/// long its turns ran inside the requested window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadActivityDto {
    pub thread_id: String,
    pub turns: u32,
    pub active_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDto {
    pub id: String,
    pub workspace_id: String,
    pub provider: Provider,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub provider_session_id: Option<String>,
    pub source: String,
    pub title: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub fast_mode: bool,
    pub collaboration_mode: String,
    pub approval_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<String>,
    pub status: ThreadStatus,
    pub summary_text: Option<String>,
    pub last_error: Option<String>,
    pub active_turn_id: Option<String>,
    pub is_loaded: bool,
    pub is_pinned: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_turn_started_at: Option<String>,
    pub last_turn_completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<Value>,
    /// Immediate creator. None means a person started this thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_thread_id: Option<String>,
    /// Top-level ancestor, denormalised so grouping is one indexed lookup.
    /// None on a root; every descendant carries the root's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_thread_id: Option<String>,
    /// 0 for a root, parent + 1 otherwise. Bounded by MAX_LINEAGE_DEPTH.
    #[serde(default)]
    pub lineage_depth: i64,
    /// Number of threads anywhere beneath this one. Only populated for roots in
    /// list responses, so a client can render "N agent threads" without a second call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descendant_count: Option<i64>,
    /// Address unique among the open threads of one lineage (`reviewer`), so
    /// agents need not juggle UUIDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Role template the thread was started from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_role: Option<String>,
    /// Isolated git worktree this thread works in instead of the workspace root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    /// Set when the delegate was closed; it then holds no slot and no name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadHistoryItemDto {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    pub kind: ThreadItemKind,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<Value>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTurnDto {
    pub id: String,
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    pub status: TurnStatus,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_estimate: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_deferred_items: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred_item_count: Option<u32>,
    /// Per-kind operation counts of a summarized turn, so a folded turn can say
    /// what it did without loading its deferred items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<Value>,
    pub items: Vec<ThreadHistoryItemDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkTurnOptionDto {
    pub turn_id: String,
    pub turn_index: u32,
    pub started_at: Option<String>,
    pub status: TurnStatus,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkThreadInput {
    pub mode: String,
    #[serde(default)]
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadActionRequestDto {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub description: Option<String>,
    pub turn_id: Option<String>,
    pub item_id: Option<String>,
    pub created_at: String,
    pub questions: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPendingSteerDto {
    pub id: String,
    pub client_request_id: Option<String>,
    pub turn_id: String,
    pub prompt: String,
    pub delivery: String,
    pub created_at: String,
}

/// A native subagent currently managed by the provider harness.
/// Native Claude/Codex agents run inside the provider session and do not have
/// their own Pockymoe thread id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSubagentDto {
    pub id: String,
    pub name: Option<String>,
    pub status: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub parent_tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_background: Option<bool>,
}

/// Read-only native agent observability. Missing usage is unknown, never zero.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSubagentDto {
    #[serde(flatten)]
    pub agent: ThreadSubagentDto,
    pub provider: String,
    pub native_session_id: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub updated_at: Option<String>,
    pub latest_activity: Option<String>,
    pub token_usage: Option<Value>,
    pub price_estimate: Option<Value>,
    pub activity_count: usize,
    pub details_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSubagentDetailDto {
    pub agent: NativeSubagentDto,
    pub items: Vec<ThreadHistoryItemDto>,
    pub has_earlier_items: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalDto {
    pub thread_id: String,
    pub local_goal_id: Option<String>,
    pub objective: String,
    pub status: String,
    pub token_budget: Option<u64>,
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDetailDto {
    pub thread: ThreadDto,
    pub workspace: WorkspaceDto,
    pub workspace_path_status: String,
    pub turns: Vec<ThreadTurnDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_turn_count: Option<u32>,
    pub pending_requests: Vec<ThreadActionRequestDto>,
    pub pending_steers: Vec<ThreadPendingSteerDto>,
    #[serde(default)]
    pub active_subagents: Vec<ThreadSubagentDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_notes: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateThreadInput {
    pub workspace_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub provider: Option<Provider>,
    #[serde(default)]
    pub agent_id: Option<String>,
    pub model: String,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default = "default_approval")]
    pub approval_mode: String,
    /// Thread that asked for this one. Set by the CLI bridge from the caller's
    /// identity; a user-created thread leaves it None and becomes a lineage root.
    #[serde(default)]
    pub parent_thread_id: Option<String>,
}

fn default_approval() -> String {
    "yolo".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportThreadInput {
    pub session_id: String,
    #[serde(default)]
    pub provider: Option<Provider>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportThreadCandidateDto {
    pub provider: Provider,
    pub agent_id: Option<String>,
    pub session_id: String,
    pub cwd: String,
    pub title: String,
    pub preview: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub history_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendThreadPromptInput {
    pub prompt: String,
    #[serde(default)]
    pub client_request_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub collaboration_mode: Option<String>,
    #[serde(default)]
    pub images: Vec<PromptImageDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptImageDto {
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptTurnInput {
    #[serde(default)]
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCaps {
    pub list: bool,
    pub read: bool,
    pub resume: bool,
    pub import_local: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCaps {
    pub start: bool,
    pub stream_input: bool,
    pub steer: bool,
    pub interrupt: bool,
    pub compact: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchCaps {
    pub fork: bool,
    #[serde(default)]
    pub fork_at: bool,
    pub hard_rollback: bool,
    pub resume_at: bool,
    pub rewind_files: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlCaps {
    pub plan_mode: bool,
    pub permission_requests: bool,
    pub sandbox_mode: bool,
    pub performance_mode: bool,
    pub goals: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagementCaps {
    pub models: bool,
    pub mcp_status: bool,
    pub skills: bool,
    pub hooks: bool,
    pub hook_trust: bool,
    pub host_config_files: bool,
    pub provider_settings: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCaps {
    pub context_window: bool,
    pub token_usage: bool,
    pub cost_usd: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProviderCapabilitiesDto {
    pub sessions: SessionCaps,
    pub turns: TurnCaps,
    pub branching: BranchCaps,
    pub controls: ControlCaps,
    pub management: ManagementCaps,
    pub usage: UsageCaps,
}

impl AgentProviderCapabilitiesDto {
    pub fn conversational() -> Self {
        Self {
            sessions: SessionCaps {
                list: true,
                read: true,
                resume: true,
                import_local: false,
                load: Some(true),
                close: Some(true),
                delete: Some(false),
            },
            turns: TurnCaps {
                start: true,
                stream_input: false,
                steer: false,
                interrupt: true,
                compact: false,
            },
            branching: BranchCaps {
                fork: false,
                fork_at: false,
                hard_rollback: false,
                resume_at: false,
                rewind_files: false,
            },
            controls: ControlCaps {
                plan_mode: true,
                permission_requests: true,
                sandbox_mode: true,
                performance_mode: false,
                goals: false,
            },
            management: ManagementCaps {
                models: true,
                mcp_status: false,
                skills: false,
                hooks: false,
                hook_trust: false,
                host_config_files: false,
                provider_settings: false,
            },
            usage: UsageCaps {
                context_window: true,
                token_usage: true,
                cost_usd: false,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeStatusDto {
    pub state: String,
    pub transport: String,
    pub last_started_at: Option<String>,
    pub last_error: Option<String>,
    pub restart_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendInstallationDto {
    pub package_name: Option<String>,
    pub installed: bool,
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
    pub install_command: Option<String>,
    pub update_command: Option<String>,
    pub busy: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolboxItemDto {
    pub action: String,
    pub command: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendManagementSchemaDto {
    pub host_config_files: Vec<Value>,
    pub toolbox_items: Vec<ToolboxItemDto>,
    pub hook_command_templates: Vec<Value>,
    pub provider_config_format: String,
    pub mcp_config_format: String,
    pub config_archives: bool,
    pub build_restart: bool,
}

pub fn toolbox_from_capabilities(caps: &AgentProviderCapabilitiesDto) -> Vec<ToolboxItemDto> {
    let mut items = Vec::new();
    if caps.controls.performance_mode {
        items.push(ToolboxItemDto {
            action: "fast".into(),
            command: "/fast".into(),
            label: "Fast mode".into(),
            description: Some("Toggle fast / performance mode.".into()),
            panel: None,
        });
    }
    if caps.turns.compact {
        items.push(ToolboxItemDto {
            action: "compact".into(),
            command: "/compact".into(),
            label: "Compact context".into(),
            description: Some("Compact the current session.".into()),
            panel: None,
        });
    }
    if caps.controls.goals {
        items.push(ToolboxItemDto {
            action: "goal".into(),
            command: "/goal".into(),
            label: "Goal".into(),
            description: Some("Set or pause the session goal.".into()),
            panel: None,
        });
    }
    if caps.branching.fork {
        items.push(ToolboxItemDto {
            action: "fork".into(),
            command: "/fork".into(),
            label: "Fork".into(),
            description: Some("Fork this session.".into()),
            panel: Some("fork".into()),
        });
    }
    if caps.management.skills {
        items.push(ToolboxItemDto {
            action: "skills".into(),
            command: "/skills".into(),
            label: "Skills".into(),
            description: None,
            panel: Some("skills".into()),
        });
    }
    if caps.management.mcp_status {
        items.push(ToolboxItemDto {
            action: "mcp".into(),
            command: "/mcp".into(),
            label: "MCP".into(),
            description: None,
            panel: Some("mcp".into()),
        });
    }
    if caps.management.hooks {
        items.push(ToolboxItemDto {
            action: "hooks".into(),
            command: "/hooks".into(),
            label: "Hooks".into(),
            description: None,
            panel: Some("hooks".into()),
        });
    }
    items
}

impl Default for AgentBackendManagementSchemaDto {
    fn default() -> Self {
        Self {
            host_config_files: vec![],
            toolbox_items: Vec::new(),
            hook_command_templates: vec![],
            provider_config_format: "none".into(),
            mcp_config_format: "none".into(),
            config_archives: false,
            build_restart: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendDto {
    pub provider: Provider,
    pub display_name: String,
    pub description: String,
    pub enabled: bool,
    pub is_default: bool,
    pub status: AgentRuntimeStatusDto,
    pub capabilities: AgentProviderCapabilitiesDto,
    pub management_schema: AgentBackendManagementSchemaDto,
    pub installation: AgentBackendInstallationDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOptionDto {
    pub id: String,
    pub model: String,
    pub display_name: String,
    pub description: String,
    pub is_default: bool,
    pub hidden: bool,
    pub supported_reasoning_efforts: Vec<ReasoningEffortOptionDto>,
    pub default_reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acp_agent: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEffortOptionDto {
    pub reasoning_effort: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilitySnapshotDto {
    pub provider: Provider,
    pub agent_id: String,
    pub availability: String,
    pub negotiated: Option<Value>,
    pub effective_capabilities: Option<AgentProviderCapabilitiesDto>,
    #[serde(default)]
    pub toolbox_items: Vec<ToolboxItemDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDto {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub remote_codex: String,
    pub capabilities: Value,
    pub enabled: bool,
    pub source: Option<String>,
    pub available: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEventEnvelope {
    #[serde(rename = "type")]
    pub event_type: ThreadEventType,
    pub thread_id: String,
    pub timestamp: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisorConnectedEnvelope {
    #[serde(rename = "type")]
    pub event_type: String,
    pub timestamp: String,
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn truncate_title(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= 15 {
        normalized
    } else {
        normalized.chars().take(15).collect()
    }
}
