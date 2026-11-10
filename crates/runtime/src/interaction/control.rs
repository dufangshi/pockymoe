//! A thread's control over its delegates: stop a running one, and inspect or
//! withdraw the messages waiting in its queue (docs/thread-interaction.md,
//! "Stopping a delegate and its queue").
use crate::Supervisor;
use anyhow::{anyhow, bail, ensure, Result};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

/// Where a queued CLI message records the local thread that sent it.
pub(crate) fn sender_key(pending: &str) -> String {
    format!("cli:prompt-from:{pending}")
}

/// The sending thread of a queued message: the recorded one, or for messages
/// queued before it was recorded, the one named in the header the receiver reads.
fn sender_of(recorded: Option<String>, prompt: &str) -> Option<String> {
    recorded.or_else(|| {
        let header = prompt.strip_prefix("[Pockymoe ")?.lines().next()?;
        let rest = header.split_once(" from thread ")?.1;
        let id: String = rest
            .chars()
            .take_while(|c| c.is_ascii_hexdigit() || *c == '-')
            .collect();
        uuid::Uuid::parse_str(&id).ok().map(|id| id.to_string())
    })
}

struct Queued {
    id: String,
    delivery: String,
    prompt: String,
    created_at: String,
    sender: Option<String>,
}

impl Supervisor {
    /// `thread` must sit somewhere under `ancestor` in its lineage. A thread has
    /// no say over itself, its parents, its siblings or anyone else's delegates.
    pub fn ensure_delegate_of(&self, ancestor: &str, thread: &str) -> Result<()> {
        ensure!(
            ancestor != thread,
            "forbidden: a thread controls its delegates, not itself"
        );
        let mut current = thread.to_owned();
        // Lineages are shallow; the bound only guards against a corrupt loop.
        for _ in 0..64 {
            let parent: Option<String> = self.db.with(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT parent_thread_id FROM threads WHERE id=?1",
                        [&current],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .ok_or_else(|| anyhow!("thread not found"))?)
            })?;
            match parent {
                Some(parent) if parent == ancestor => return Ok(()),
                Some(parent) => current = parent,
                None => break,
            }
        }
        bail!("forbidden: only your own delegates (threads under you in the lineage) may be stopped or have their queue changed")
    }

    fn queued(&self, thread: &str) -> Result<Vec<Queued>> {
        self.get_thread(thread)?;
        self.db.with(|conn| {
            let mut query = conn.prepare(
                "SELECT p.id,p.delivery,p.submitted_prompt,p.created_at,
                        (SELECT value FROM kv WHERE key='cli:prompt-from:'||p.id)
                 FROM thread_pending_steers p WHERE p.thread_id=?1
                 ORDER BY p.created_at,p.rowid",
            )?;
            let rows = query
                .query_map([thread], |row| {
                    let prompt: String = row.get(2)?;
                    Ok(Queued {
                        id: row.get(0)?,
                        delivery: row.get(1)?,
                        sender: sender_of(row.get(4)?, &prompt),
                        prompt,
                        created_at: row.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// The messages waiting for `thread`, oldest first. `viewer` marks its own.
    pub fn queued_messages(&self, thread: &str, viewer: Option<&str>) -> Result<Value> {
        let status = self.get_thread(thread)?.status;
        let messages: Vec<Value> = self
            .queued(thread)?
            .into_iter()
            .map(|message| {
                let preview: String = message.prompt.chars().take(240).collect();
                json!({
                    "messageId": message.id,
                    // `held` never runs by itself: it targeted a turn that ended.
                    "state": match message.delivery.as_str() {
                        "cli-steer" => "held",
                        "update-resume" => "resume",
                        // Already handed to the running turn; nothing left to withdraw.
                        "steer" => "steered",
                        _ => "queued",
                    },
                    "fromThreadId": message.sender,
                    "mine": viewer.is_some() && message.sender.as_deref() == viewer,
                    "createdAt": message.created_at,
                    "preview": preview,
                    "truncated": message.prompt.chars().count() > 240,
                })
            })
            .collect();
        Ok(
            json!({"threadId":thread,"status":status,"queuedCount":messages.len(),"messages":messages}),
        )
    }

    /// Withdraws queued messages before they run. `only_from` limits the caller
    /// to what that thread sent: the owner's and other senders' messages stay.
    pub async fn remove_queued_messages(
        &self,
        thread: &str,
        ids: &[String],
        all: bool,
        only_from: Option<&str>,
    ) -> Result<Value> {
        ensure!(
            all != !ids.is_empty(),
            "name the message ids to remove, or pass all (--all) and no ids"
        );
        let queued = self.queued(thread)?;
        // A steered message is already in the running turn.
        let allowed = |message: &Queued| {
            message.delivery != "steer"
                && only_from.is_none_or(|sender| message.sender.as_deref() == Some(sender))
        };
        for id in ids {
            let message = queued
                .iter()
                .find(|message| &message.id == id)
                .ok_or_else(|| {
                    anyhow!("queued message {id} was not found; it may already have started")
                })?;
            ensure!(
                allowed(message),
                "forbidden: message {id} is already steered into the running turn, or was not sent by you; only its sender or the owner can remove a waiting message"
            );
        }
        let (mut removed, mut kept) = (Vec::new(), 0usize);
        for message in &queued {
            let chosen = if all {
                allowed(message)
            } else {
                ids.contains(&message.id)
            };
            if !chosen {
                kept += 1;
                continue;
            }
            // Gone already means a turn took it between the listing and now.
            match self.cancel_pending_steer(thread, &message.id).await {
                Ok(_) => removed.push(message.id.clone()),
                Err(_) => {}
            }
        }
        Ok(json!({"threadId":thread,"removed":removed,"remainingCount":kept}))
    }

    /// Stops the turn `thread` is running. With `clear`, the messages `only_from`
    /// may remove are withdrawn first, so the stop is not followed by stale work.
    pub async fn stop_thread_for_agent(
        &self,
        thread: &str,
        reason: Option<&str>,
        clear: bool,
        only_from: Option<&str>,
    ) -> Result<Value> {
        if let Some(reason) = reason {
            ensure!(
                !reason.trim().is_empty() && reason.chars().count() <= 500,
                "reason must be nonempty and at most 500 characters"
            );
        }
        let before = self.get_thread(thread)?;
        let cleared = if clear && !self.queued(thread)?.is_empty() {
            self.remove_queued_messages(thread, &[], true, only_from)
                .await?
        } else {
            json!({"removed":[],"remainingCount":self.queued(thread)?.len()})
        };
        let was_running = before.status == "running" || before.active_turn_id.is_some();
        if was_running {
            tracing::info!(
                thread,
                by = only_from.unwrap_or("owner"),
                reason = reason.unwrap_or(""),
                "Thread stopped through the CLI"
            );
            self.interrupt(thread).await?;
        }
        let after = self.get_thread(thread)?;
        let remaining = self.queued(thread)?.len();
        Ok(json!({
            "threadId": thread,
            "stopped": was_running,
            "stoppedTurnId": before.active_turn_id.filter(|_| was_running),
            "status": after.status,
            "removed": cleared["removed"],
            "queuedCount": remaining,
            "reason": reason,
            // What the caller most needs to know next.
            "next": if remaining > 0 {
                "Messages remain in the queue and run next, in order. Inspect them with `pockymoe thread queue`."
            } else if was_running {
                "The thread is stopped with an empty queue. Send its new instructions with `pockymoe thread send --delivery direct`."
            } else {
                "The thread was not running; nothing was interrupted."
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sender_comes_from_the_record_or_the_header_the_receiver_reads() {
        let id = "7a39ffeb-fdb7-4ed6-bb73-f3f4d805647b";
        assert_eq!(
            sender_of(Some("x".into()), "anything").as_deref(),
            Some("x")
        );
        assert_eq!(
            sender_of(
                None,
                &format!("[Pockymoe task from thread {id} | fix it]\nbody")
            )
            .as_deref(),
            Some(id)
        );
        // A person's prompt that merely quotes such a header further down is not a sender.
        assert_eq!(
            sender_of(
                None,
                &format!("please read\n[Pockymoe task from thread {id}]")
            ),
            None
        );
        assert_eq!(
            sender_of(
                None,
                "[Pockymoe task from device-1/local (device \"mac\")]\nbody"
            ),
            None
        );
    }
}
