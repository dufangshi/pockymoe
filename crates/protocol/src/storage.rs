//! Disk use of the Supervisor database and history cleanup.
use serde::{Deserialize, Serialize};

/// What a cleanup removes from a command's text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandTextCleanup {
    /// Keep the whole command.
    #[default]
    Keep,
    /// Keep its first `commandTextChars` characters.
    Truncate,
    /// Keep only that a command ran, and when.
    Remove,
}

pub const DEFAULT_COMMAND_TEXT_CHARS: u32 = 512;

/// What a cleanup removes from turns that ended (never from running ones).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCleanupDto {
    /// Command output (and the command's input details).
    #[serde(default = "yes")]
    pub command_output: bool,
    #[serde(default)]
    pub command_text: CommandTextCleanup,
    #[serde(default = "default_command_text_chars")]
    pub command_text_chars: u32,
    /// The diffs of file changes; their paths and line counts stay.
    #[serde(default)]
    pub file_diffs: bool,
}

fn yes() -> bool {
    true
}
fn default_command_text_chars() -> u32 {
    DEFAULT_COMMAND_TEXT_CHARS
}

impl Default for StorageCleanupDto {
    fn default() -> Self {
        Self {
            command_output: true,
            command_text: CommandTextCleanup::Keep,
            command_text_chars: DEFAULT_COMMAND_TEXT_CHARS,
            file_diffs: false,
        }
    }
}

/// Automatic cleanup: one age for everything it removes. `None` turns it off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageSettingsDto {
    /// Clean up turns that ended this many days ago (0.13.28 called it
    /// `commandOutputRetentionDays`).
    #[serde(alias = "commandOutputRetentionDays")]
    pub retention_days: Option<u32>,
    #[serde(flatten)]
    pub cleanup: StorageCleanupDto,
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
    /// The part of `bytes` held by file changes (their diffs).
    #[serde(default)]
    pub file_change_bytes: u64,
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
    #[serde(default)]
    pub file_change_bytes: u64,
    /// Command outputs and diffs already removed by cleanup.
    pub pruned_items: u64,
    pub pruned_bytes: u64,
    pub thread_count: u64,
    pub conversation_count: u64,
    pub threads: Vec<StorageThreadDto>,
    pub offset: u64,
    pub next_offset: Option<u64>,
}

/// A cleanup or compaction running in the background, or the last one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageJobDto {
    /// `prune`, `dedupe` (the one-time removal of input copies of commands)
    /// or `compact`.
    pub action: String,
    /// `running`, `done` or `failed`.
    pub state: String,
    /// Items to look at (prune); 0 until counted, and for compaction.
    pub total: u64,
    pub processed: u64,
    /// Items changed and bytes removed so far (prune).
    pub items: u64,
    pub bytes: u64,
    /// Database file size before and after (compact).
    pub before_bytes: Option<u64>,
    pub after_bytes: Option<u64>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    /// The conversation a manual cleanup targets; `None` for the whole device.
    #[serde(default)]
    pub thread_id: Option<String>,
}

/// What the Disk section shows before and after a scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageOverviewDto {
    pub device_name: String,
    /// The last scan's page; `None` until a scan runs (only on request).
    pub scan: Option<StorageScanDto>,
    /// The running or last cleanup/compaction since the Supervisor started.
    #[serde(default)]
    pub job: Option<StorageJobDto>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_accept_the_old_age_name_with_options_and_default_the_rest() {
        let settings: StorageSettingsDto = serde_json::from_str(
            r#"{"commandOutputRetentionDays":7,"commandText":"truncate","commandTextChars":64}"#,
        )
        .unwrap();
        assert_eq!(settings.retention_days, Some(7));
        assert!(settings.cleanup.command_output);
        assert_eq!(settings.cleanup.command_text, CommandTextCleanup::Truncate);
        assert_eq!(settings.cleanup.command_text_chars, 64);
        assert!(!settings.cleanup.file_diffs);
        let current: StorageSettingsDto =
            serde_json::from_str(r#"{"retentionDays":null,"fileDiffs":true}"#).unwrap();
        assert_eq!(current.retention_days, None);
        assert!(current.cleanup.file_diffs);
        assert_eq!(
            serde_json::to_value(&current).unwrap()["retentionDays"],
            serde_json::Value::Null
        );
    }
}
