//! Disk use of the Supervisor database and the command-output retention policy.
use serde::{Deserialize, Serialize};

/// `None` keeps command output forever.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageSettingsDto {
    /// Remove a command's output once its turn ended this many days ago.
    /// The command itself, its status and its input stay.
    pub command_output_retention_days: Option<u32>,
}

/// One conversation: a root thread with its agent threads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageThreadDto {
    pub thread_id: String,
    pub title: String,
    pub workspace_label: Option<String>,
    /// Stored history of the conversation, in bytes.
    pub bytes: u64,
    /// The part of `bytes` held by command executions.
    pub command_bytes: u64,
    pub items: u64,
    /// Agent threads counted in this row.
    pub agent_threads: u64,
    pub last_activity_at: Option<String>,
}

/// A scan computed on request; pages read from the same result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageScanDto {
    pub scanned_at: String,
    pub device_name: String,
    /// Database file plus its write-ahead log.
    pub database_bytes: u64,
    /// Free pages inside the file: reused by new data, returned by compacting.
    pub free_bytes: u64,
    pub history_bytes: u64,
    pub command_bytes: u64,
    /// Command outputs already removed by the retention policy.
    pub pruned_items: u64,
    pub pruned_bytes: u64,
    pub thread_count: u64,
    pub conversation_count: u64,
    pub threads: Vec<StorageThreadDto>,
    pub offset: u64,
    pub next_offset: Option<u64>,
}

/// What the Disk section shows before and after a scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageOverviewDto {
    pub device_name: String,
    /// The last scan's page; `None` until a scan runs (only on request).
    pub scan: Option<StorageScanDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoragePruneResultDto {
    pub items: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCompactResultDto {
    pub before_bytes: u64,
    pub after_bytes: u64,
}
