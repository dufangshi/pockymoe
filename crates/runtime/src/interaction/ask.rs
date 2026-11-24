//! `thread ask`: a delegate mails a passive question and blocks until the reply
//! (`--in-reply-to` that question) arrives. Because the asker is blocked, the
//! question also reaches an idle ancestor promptly instead of at the 10-minute
//! reminder (docs/thread-interaction.md, "Asking and waiting for the answer").
use super::{
    agents::{clip, is_descendant},
    enqueue,
    inbox::{self, MailFilter},
    SendInput, SendMode,
};
use crate::Supervisor;
use anyhow::{ensure, Context, Result};
use pockymoe_protocol::{now_rfc3339, ThreadEventEnvelope};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

/// Questions per asker that may wait for prompt delivery at once.
const MAX_PENDING_ASKS: i64 = 3;
const DEFAULT_ASK_WAIT: Duration = Duration::from_secs(600);
const QUESTION_EXCERPT: usize = 8000;

pub fn clamp_ask_wait(seconds: Option<u64>) -> Duration {
    seconds
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_ASK_WAIT)
        .min(super::agents::MAX_WAIT)
}

fn ask_key(recipient: &str, message: &str) -> String {
    format!("cli:ask:{recipient}:{message}")
}

/// Decides, in the acceptance transaction, whether a new `ask` question is
/// delivered promptly. Only an ancestor gets it promptly, and only one
/// unanswered prompt question per asker and recipient at a time, so a
/// delegate cannot turn questions into a stream of turns on its parent.
pub(super) fn register(
    tx: &Connection,
    recipient: &str,
    asker: &str,
    message: &str,
) -> Result<(&'static str, Option<&'static str>)> {
    let key = format!("cli:inbox:{recipient}:{message}");
    tx.execute(
        "UPDATE kv SET value=json_set(value,'$.ask',json('true')) WHERE key=?1",
        [&key],
    )?;
    if !is_descendant(tx, recipient, asker)? {
        return Ok((
            "passive",
            Some("the recipient is not your ancestor, so the question waits in its inbox"),
        ));
    }
    let open: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM kv q WHERE q.key GLOB ?1
           AND json_extract(q.value,'$.id') != ?3
           AND json_extract(q.value,'$.fromThreadId')=?2
           AND json_extract(q.value,'$.askDelivery')='prompt'
           AND json_extract(q.value,'$.acknowledgedAt') IS NULL
           AND NOT EXISTS (SELECT 1 FROM kv r WHERE r.key GLOB ?4
               AND json_extract(r.value,'$.inReplyTo')=json_extract(q.value,'$.id')))",
        params![
            format!("cli:inbox:{recipient}:*"),
            asker,
            message,
            format!("cli:inbox:{asker}:*")
        ],
        |r| r.get(0),
    )?;
    if open {
        return Ok((
            "passive",
            Some("an earlier question you asked it promptly is still unanswered, so this one waits in its inbox"),
        ));
    }
    let pending: i64 = tx.query_row(
        "SELECT COUNT(*) FROM kv WHERE key GLOB 'cli:ask:*' AND json_extract(value,'$.from')=?1",
        [asker],
        |r| r.get(0),
    )?;
    if pending >= MAX_PENDING_ASKS {
        return Ok((
            "passive",
            Some("you already have 3 questions waiting for prompt delivery"),
        ));
    }
    tx.execute(
        "UPDATE kv SET value=json_set(value,'$.askDelivery','prompt') WHERE key=?1",
        [&key],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO kv(key,value) VALUES(?1,?2)",
        params![
            ask_key(recipient, message),
            json!({"from":asker,"createdAt":now_rfc3339()}).to_string()
        ],
    )?;
    Ok(("prompt", None))
}

/// The recipient will hear the question another way, or never should.
fn settled(tx: &Connection, recipient: &str, asker: &str, message: &str) -> Result<bool> {
    let Ok(question) = inbox::get(tx, recipient, message) else {
        return Ok(true);
    };
    if !question["acknowledgedAt"].is_null() || !question["readAt"].is_null() {
        return Ok(true);
    }
    let answered: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM kv WHERE key GLOB ?1 AND json_extract(value,'$.inReplyTo')=?2)",
        params![format!("cli:inbox:{asker}:*"), message],
        |r| r.get(0),
    )?;
    if answered {
        return Ok(true);
    }
    // A delegate wake covering the asker already reports it as blocked.
    let covered: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM kv w, json_each(json_extract(w.value,'$.members')) m
         WHERE w.key GLOB ?1 AND m.value=?2)",
        params![format!("cli:wake:{recipient}:*"), asker],
        |r| r.get(0),
    )?;
    let closed: bool = tx
        .query_row(
            "SELECT closed_at IS NOT NULL FROM threads WHERE id=?1",
            [recipient],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(true);
    Ok(covered || closed)
}

impl Supervisor {
    /// Mails `target` a question from `asker` and blocks until a reply to it
    /// arrives or `timeout` passes. A retry with the same request id resumes
    /// the wait on the same question.
    #[allow(clippy::too_many_arguments)]
    pub async fn ask_thread(
        &self,
        target: &str,
        asker: &str,
        subject: &str,
        text: String,
        request_id: Option<String>,
        timeout: Duration,
        agent: bool,
    ) -> Result<Value> {
        ensure!(target != asker, "a thread cannot ask itself");
        let input = SendInput {
            text,
            delivery: "inbox".into(),
            notify_delivery: "inbox".into(),
            from_thread_id: Some(asker.into()),
            notify_on_complete: false,
            client_request_id: request_id,
            subject: Some(subject.into()),
            kind: Some("question".into()),
            in_reply_to: None,
            interrupt_reason: None,
            topic_key: None,
            retry_failed: false,
        };
        let receipt = self.send_to_thread_impl(
            target,
            input,
            SendMode {
                consult_host: true,
                agent,
                ask: true,
                ..Default::default()
            },
        )?;
        let question = receipt["messageId"]
            .as_str()
            .context("the question was not stored")?
            .to_owned();
        let filter = MailFilter {
            in_reply_to: Some(question.clone()),
            ..Default::default()
        };
        let waited = self
            .inbox_wait_matching(asker, &filter, false, timeout)
            .await?;
        let replies = waited["messages"].as_array().cloned().unwrap_or_default();
        let mut result = json!({
            "threadId": target,
            "questionId": question,
            "promptDelivery": receipt["promptDelivery"],
            "waitedSeconds": waited["waitedSeconds"],
        });
        if let Some(note) = receipt.get("promptDeliveryNote") {
            result["promptDeliveryNote"] = note.clone();
        }
        match replies.first() {
            Some(reply) => {
                result["answered"] = json!(true);
                result["reply"] = reply.clone();
                if replies.len() > 1 {
                    result["replies"] = json!(replies);
                }
                result["next"] = json!(format!(
                    "Act on the answer. It is marked read, not acknowledged: `pockymoe inbox ack {}` once handled.",
                    reply["id"].as_str().unwrap_or("MESSAGE_ID")
                ));
            }
            None => {
                result["answered"] = json!(false);
                result["timedOut"] = json!(true);
                result["next"] = json!(format!(
                    "No answer yet. The question stays open in the recipient's inbox (its waits on you report you as blocked). Wait again with `pockymoe inbox wait --in-reply-to {question} --timeout 600` (or rerun this ask with the same --request-id), or register `pockymoe inbox wait --in-reply-to {question} --wake` and end your turn."
                ));
            }
        }
        Ok(result)
    }

    /// Delivers `ask` questions to idle recipients as one queued turn each.
    /// A busy recipient gets it at its next idle moment; one that reads or
    /// answers it first, or a wake that reports the asker, makes it unneeded.
    pub(super) async fn deliver_asks(&self) -> Result<()> {
        let records: Vec<(String, String)> = self.db.with(|c| {
            let mut stmt = c.prepare("SELECT key,value FROM kv WHERE key GLOB 'cli:ask:*'")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        if records.is_empty()
            || self
                .update_draining
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(());
        }
        // Serialize with turn admission, like the inbox reminder.
        let live = self.live.lock().await;
        for (key, raw) in records {
            let mut parts = key.splitn(4, ':').skip(2);
            let (Some(recipient), Some(message)) = (parts.next(), parts.next()) else {
                continue;
            };
            let record: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
            let Some(asker) = record["from"].as_str() else {
                self.db
                    .with(|c| Ok(c.execute("DELETE FROM kv WHERE key=?1", [&key])?))?;
                continue;
            };
            if live.contains_key(recipient) {
                continue;
            }
            let allowed = self
                .get_thread(recipient)
                .is_ok_and(|thread| self.ensure_prompt_allowed(&thread).is_ok());
            let pending = Uuid::new_v4().to_string();
            let delivered = self.db.with(|c| {
                let tx = c.unchecked_transaction()?;
                if settled(&tx, recipient, asker, message)? || !allowed {
                    tx.execute("DELETE FROM kv WHERE key=?1", [&key])?;
                    tx.commit()?;
                    return Ok(false);
                }
                let idle: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM threads t WHERE id=?1
                     AND status NOT IN ('running','recovering')
                     AND NOT EXISTS(SELECT 1 FROM thread_turns r WHERE r.thread_id=t.id AND r.status='inProgress')
                     AND NOT EXISTS(SELECT 1 FROM thread_pending_steers p WHERE p.thread_id=t.id))",
                    [recipient],
                    |r| r.get(0),
                )?;
                if !idle {
                    return Ok(false);
                }
                let mut question = inbox::get(&tx, recipient, message)?;
                let name: Option<String> = tx
                    .query_row(
                        "SELECT COALESCE(agent_name,title) FROM threads WHERE id=?1",
                        [asker],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                let text = question["text"].as_str().unwrap_or("");
                let body = if text.chars().count() > QUESTION_EXCERPT {
                    format!(
                        "{}\n[Truncated; read it all with `pockymoe inbox read {message}`]",
                        clip(text, QUESTION_EXCERPT)
                    )
                } else {
                    text.to_string()
                };
                let subject = question["subject"]
                    .as_str()
                    .map(|s| format!(" | {s}"))
                    .unwrap_or_default();
                // The peer header the receiver and the Web already read; the
                // urgency line says why it did not wait in the inbox.
                let prompt = format!(
                    "[Pockymoe question from thread {asker}{subject}]\nImmediate handling needed: {} is blocked in `pockymoe thread ask` until you reply with --kind result --in-reply-to {message}\n{body}",
                    name.as_deref().unwrap_or("the sender")
                );
                let now = now_rfc3339();
                enqueue(&tx, &pending, recipient, &prompt, None, &now)?;
                tx.execute(
                    "INSERT INTO kv(key,value) VALUES(?1,'peer')",
                    [format!("cli:prompt-origin:{pending}")],
                )?;
                tx.execute(
                    "INSERT OR REPLACE INTO kv(key,value) VALUES(?1,?2)",
                    params![super::control::sender_key(&pending), asker],
                )?;
                // Its text is in the prompt; the reminder need not repeat it.
                inbox::mark_read(&tx, recipient, &mut question, &now)?;
                tx.execute("DELETE FROM kv WHERE key=?1", [&key])?;
                tx.commit()?;
                Ok(true)
            })?;
            if delivered {
                self.bus.emit(ThreadEventEnvelope {
                    event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
                    thread_id: recipient.into(),
                    timestamp: now_rfc3339(),
                    payload: json!({"reason":"pending_steer_updated"}),
                });
            }
        }
        Ok(())
    }
}
