mod adapter;
mod capabilities;
pub(crate) mod catalog;
mod claude_background;
mod claude_completion;
mod claude_lifecycle;
mod claude_tasks;
mod claude_usage;
pub(crate) use claude_usage::legacy_background_reply_phases;
/// The Claude JSONL billing reader, shared with the native Claude runtime.
pub(crate) use claude_usage::ClaudeUsageReader;
mod codex_bridge;
pub(crate) mod codex_models;
pub(crate) mod codex_permissions;
pub(crate) mod context;
pub use codex_bridge::run as run_codex_app_server_bridge;
mod deepseek;
pub use deepseek::visible_models as visible_deepseek_models;
pub(crate) mod dependencies;
pub(crate) mod elicitation;
pub(crate) use elicitation::{
    questions as elicitation_questions, response as elicitation_response,
};
mod grok;
pub mod mcp_servers;
pub(crate) use grok::billing_usage as grok_billing_usage;
pub(crate) mod mapper;
mod working_locations;
pub(crate) use mapper::TurnMapper;
mod modes;
pub(crate) mod prompt;
pub(crate) mod rpc;
pub(crate) mod runtime;
mod terminal;
pub(crate) mod upstream_models;

pub use adapter::{
    adapter_for, ClaudeAdapter, CodexAdapter, CursorAdapter, DeepSeekAdapter, GrokAdapter,
    HarnessAdapter, StandardAdapter,
};
pub(crate) use capabilities::parse_commands;
pub use capabilities::{NegotiatedCaps, NegotiatedCommand};
pub use catalog::{
    augment_path, builtin_agents, classify_availability, command_available, parse_command_models,
    sign_in_environment, AcpAgentDef,
};
pub use dependencies::cleanup_installations as cleanup_adapter_installations;
pub(crate) use modes::{has_collaboration_option, resolve_collaboration_config_value};
pub use modes::{parse_available_modes, ProductSessionPolicy};
pub(crate) use modes::{permission_questions, select_permission_option, PermissionChoice};
pub use runtime::AcpRuntime;

mod usage;
pub(crate) use usage::CodexUsageReader;

pub(crate) use catalog::resolve_executable;
pub use catalog::{child_path, child_path_with};

pub(crate) use runtime::emit_mapped;

pub(crate) use runtime::resume_session_identity;
