mod adapter;
mod capabilities;
pub(crate) mod catalog;
mod claude_background;
mod claude_completion;
mod claude_lifecycle;
mod claude_tasks;
mod claude_usage;
pub(crate) use claude_usage::legacy_background_reply_phases;
mod codex_bridge;
mod codex_models;
mod codex_permissions;
pub use codex_bridge::run as run_codex_app_server_bridge;
mod deepseek;
pub(crate) mod dependencies;
mod elicitation;
mod grok;
pub mod mcp_servers;
pub(crate) use grok::billing_usage as grok_billing_usage;
pub(crate) mod mapper;
pub(crate) use mapper::TurnMapper;
mod modes;
mod prompt;
pub(crate) mod rpc;
mod runtime;
mod terminal;
mod upstream_models;

pub use adapter::{
    adapter_for, ClaudeAdapter, CodexAdapter, CursorAdapter, DeepSeekAdapter, GrokAdapter,
    HarnessAdapter, StandardAdapter,
};
pub use capabilities::NegotiatedCaps;
pub use catalog::{
    augment_path, builtin_agents, classify_availability, command_available, parse_command_models,
};
pub use modes::{parse_available_modes, ProductSessionPolicy};
pub use runtime::AcpRuntime;

mod usage;

pub(crate) use catalog::{child_path, resolve_executable};
