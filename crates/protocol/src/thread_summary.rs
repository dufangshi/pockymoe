//! Compact persisted public conversation, separate from the tool transcript.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSummaryQuery {
    pub since: Option<String>,
    pub recent_seconds: Option<u64>,
    pub turns: Option<u32>,
    pub max_chars: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThreadSummaryRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadSummaryMessage {
    /// First public emission/acceptance, or null when the saved item lacks it.
    pub time: Option<String>,
    pub role: ThreadSummaryRole,
    pub text: String,
    /// True only when maxChars removed the beginning of this message.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadSummaryDto {
    pub messages: Vec<ThreadSummaryMessage>,
    /// maxChars removed text; selection by time/turn filters is not truncation.
    pub truncated: bool,
}
