pub mod acp;
pub mod actor;
mod child_process;
pub mod config;
pub mod db;
pub mod device_metrics;
pub mod fake;
pub mod file_documents;
pub mod files;
pub mod history;
pub mod host_hooks;
pub mod host_messaging;
pub mod host_mode;
pub mod import_id;
pub mod interaction;
pub mod local_access;
mod local_notifications;
pub mod local_sessions;
pub mod management;
mod node_runtime;
pub mod plugins;
pub mod publications;
pub mod restart;
pub mod service;
pub mod thread_images;
pub mod upstreams;

pub use actor::{EventBus, SharedRuntime};
pub use config::RuntimeConfig;
pub use db::Database;
pub use restart::{RestartStrategy, HOST_MANAGED_RESTART_EXIT_CODE};
pub use service::{bootstrap_runtimes, Supervisor, UploadedPromptAttachment};

use anyhow::Result;
use std::sync::Arc;

pub async fn boot() -> Result<Arc<Supervisor>> {
    crate::acp::augment_path();
    let config = RuntimeConfig::from_env()?;
    let db = Database::open(&config.database_url)?;
    let runtimes = bootstrap_runtimes(&config);
    for runtime in &runtimes {
        runtime.start().await?;
    }
    Ok(Arc::new(Supervisor::new(config, db, runtimes)))
}

mod usage;

mod harness_processes;
mod native_subagents;
mod usage_history;

mod pricing;

mod relay_notifications;
pub mod subscription;
