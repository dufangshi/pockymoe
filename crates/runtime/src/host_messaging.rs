//! Optional host transport. The runtime knows only its own envelope and inbox.
use crate::{interaction::inbox, Supervisor};
use anyhow::Result;
use pockymoe_protocol::{ThreadEventEnvelope, ThreadSendInput};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::sync::Arc;

pub trait HostMessaging: Send + Sync {
    /// Called after common envelope validation, before any internal delivery.
    /// The implementation owns durable retry handling and returns the usual receipt.
    fn send(&self, state: &Supervisor, recipient: &str, input: ThreadSendInput) -> Result<Value>;
}

/// Already authenticated host mail. Extra metadata is display data, never runtime policy.
pub struct InboundMail<'a> {
    pub thread_id: &'a str,
    pub message_id: &'a str,
    pub from_thread_id: Option<&'a str>,
    pub text: &'a str,
    pub created_at: &'a str,
    pub subject: Option<&'a str>,
    pub kind: &'a str,
    pub in_reply_to: Option<&'a str>,
    pub metadata: Value,
}

/// Use the caller's transaction so transport checkpoints and inbox mail commit together.
pub fn store_inbound(conn: &Connection, mail: InboundMail<'_>) -> Result<()> {
    inbox::store(
        conn,
        mail.thread_id,
        mail.message_id,
        mail.from_thread_id,
        mail.text,
        mail.created_at,
        inbox::Envelope {
            subject: mail.subject,
            kind: mail.kind,
            in_reply_to: mail.in_reply_to,
            topic_key: None,
        },
    )?;
    conn.execute(
        "UPDATE kv SET value=json_set(value,'$.host',json(?2)) WHERE key=?1",
        params![
            format!("cli:inbox:{}:{}", mail.thread_id, mail.message_id),
            mail.metadata.to_string()
        ],
    )?;
    Ok(())
}

impl Supervisor {
    pub fn set_host_messaging(&self, hook: Arc<dyn HostMessaging>) {
        *self.interaction.host_messaging.write().unwrap() = Some(hook);
    }

    /// Call only after the inbound transaction commits; wakes existing inbox waits.
    pub fn host_inbox_committed(&self, thread: &str) {
        self.bus.emit(ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
            thread_id: thread.into(),
            timestamp: pockymoe_protocol::now_rfc3339(),
            payload: json!({"reason":"inbox_updated"}),
        });
    }
}
