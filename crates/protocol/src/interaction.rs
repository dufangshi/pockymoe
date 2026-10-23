use serde::{Deserialize, Serialize};

pub const THREAD_INTERACTION_SKILL: &str =
    include_str!("../../../skills/thread-interaction/SKILL.md");

/// Topic guides behind the short skill: `pockymoe guide TOPIC`.
/// (name, one-line summary, text)
pub const THREAD_INTERACTION_GUIDES: &[(&str, &str, &str)] = &[
    (
        "preview",
        "Show a local web app to the user through a private preview",
        include_str!("../../../skills/thread-interaction/guides/preview.md"),
    ),
    (
        "delegate",
        "Create, wait for, close and clean up delegates; lineage, roles, worktrees",
        include_str!("../../../skills/thread-interaction/guides/delegate.md"),
    ),
    (
        "messaging",
        "Labels, delivery choice, batches, receipts for thread send",
        include_str!("../../../skills/thread-interaction/guides/messaging.md"),
    ),
    (
        "inbox",
        "When and how to read, wait for and acknowledge mail",
        include_str!("../../../skills/thread-interaction/guides/inbox.md"),
    ),
    (
        "tasks",
        "The lineage task board and harness-native timers",
        include_str!("../../../skills/thread-interaction/guides/tasks.md"),
    ),
    (
        "devices",
        "Threads, files and mail on your other devices",
        include_str!("../../../skills/thread-interaction/guides/devices.md"),
    ),
    (
        "transcript",
        "Reading thread history (and pockymoe_session_id), retrying sends safely",
        include_str!("../../../skills/thread-interaction/guides/transcript.md"),
    ),
    (
        "connection",
        "Credentials, connection discovery and failure handling",
        include_str!("../../../skills/thread-interaction/guides/connection.md"),
    ),
    (
        "automation",
        "Durable hooks: schedules, completion triggers, scripts",
        include_str!("../../../skills/thread-interaction/guides/automation.md"),
    ),
];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSendInput {
    pub text: String,
    #[serde(default = "inbox_delivery")]
    pub delivery: String,
    #[serde(default = "inbox_delivery")]
    pub notify_delivery: String,
    #[serde(default)]
    pub from_thread_id: Option<String>,
    #[serde(default)]
    pub notify_on_complete: bool,
    #[serde(default)]
    pub client_request_id: Option<String>,
    /// One-line summary. Lets a receiver triage from a listing or an unread notice
    /// without opening the message, and gives a sender something to name work by.
    #[serde(default)]
    pub subject: Option<String>,
    /// What the message is for: result, question, status or task. Freeform text
    /// alone forces the receiver to read everything to find out whether anything is
    /// expected of it. Defaults to `status` - the least demanding reading.
    #[serde(default)]
    pub kind: Option<String>,
    /// Message this answers, so an exchange correlates instead of relying on prose.
    #[serde(default)]
    pub in_reply_to: Option<String>,
    /// Why handling this at the next checkpoint would cause harm or wasted work.
    /// Required for peer direct/steer; omitted fields preserve legacy retry hashes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt_reason: Option<String>,
    /// Opt in to replacing older full status snapshots from this sender on this
    /// topic. Never used to merge results, questions, tasks or incremental patches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_key: Option<String>,
}

/// Accepted `kind` values. `question` is the only one that implies the sender is
/// waiting on the receiver; the rest are informational, which keeps the passive
/// inbox the right default rather than something to escalate around.
pub const MESSAGE_KINDS: [&str; 4] = ["result", "question", "status", "task"];

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTranscriptQuery {
    pub limit: Option<u32>,
    pub before_turn_id: Option<String>,
    pub turn_id: Option<String>,
    pub item_id: Option<String>,
    pub view: Option<String>,
    pub offset: Option<u32>,
    pub text_offset: Option<u32>,
    #[serde(default)]
    pub raw: bool,
}

fn inbox_delivery() -> String {
    "inbox".into()
}
