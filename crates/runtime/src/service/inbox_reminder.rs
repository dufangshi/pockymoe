//! Delayed, one-shot reminders for untouched passive mail. Mail stays in the
//! inbox; the reminder goes through the normal durable continuation queue.
use super::*;
use crate::interaction::enqueue;
use pockymoe_protocol::ThreadEventEnvelope;
use std::sync::atomic::Ordering;

const UNSEEN: &str = "key GLOB ?1
    AND json_extract(value,'$.acknowledgedAt') IS NULL
    AND json_extract(value,'$.supersededBy') IS NULL
    AND json_extract(value,'$.readAt') IS NULL
    AND json_extract(value,'$.remindedAt') IS NULL
    AND json_extract(value,'$.remindAfter') IS NOT NULL";

impl Supervisor {
    pub async fn inbox_reminder_tick(&self, now: &str) -> Result<()> {
        if self.update_draining.load(Ordering::SeqCst) {
            return Ok(());
        }
        // Serialize with turn admission: an idle snapshot alone is insufficient
        // while a new prompt is acquiring its live owner.
        let live = self.live.lock().await;
        let candidates = self.db.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT json_extract(value,'$.threadId') FROM kv
                 WHERE key GLOB 'cli:inbox:*'
                   AND json_extract(value,'$.readAt') IS NULL
                   AND json_extract(value,'$.remindedAt') IS NULL
                   AND json_extract(value,'$.acknowledgedAt') IS NULL
                   AND json_extract(value,'$.supersededBy') IS NULL
                   AND julianday(json_extract(value,'$.remindAfter')) <= julianday(?1)",
            )?;
            let ids = stmt
                .query_map([now], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(ids)
        })?;
        for thread in candidates {
            if live.contains_key(&thread) {
                continue;
            }
            let Ok(dto) = self.get_thread(&thread) else {
                continue;
            };
            if self.ensure_prompt_allowed(&dto).is_err() {
                continue;
            }
            let queued = self.db.with(|conn| {
                let tx = conn.unchecked_transaction()?;
                let idle: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM threads t WHERE id=?1 AND closed_at IS NULL
                     AND status NOT IN ('running','recovering')
                     AND NOT EXISTS(SELECT 1 FROM thread_turns r WHERE r.thread_id=t.id AND r.status='inProgress')
                     AND NOT EXISTS(SELECT 1 FROM thread_pending_steers p WHERE p.thread_id=t.id))",
                    [&thread], |r| r.get(0),
                )?;
                if !idle { return Ok(false); }
                let cooldown = format!("cli:inbox-reminder:{thread}");
                let cooling: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM kv WHERE key=?1 AND julianday(value)+10.0/1440.0 > julianday(?2))",
                    params![cooldown, now], |r| r.get(0),
                )?;
                if cooling { return Ok(false); }
                let mailbox = format!("cli:inbox:{thread}:*");
                let mut stmt = tx.prepare(&format!(
                    "SELECT value FROM kv WHERE {UNSEEN} ORDER BY json_extract(value,'$.createdAt')"
                ))?;
                let mail = stmt.query_map([&mailbox], |r| r.get::<_, String>(0))?
                    .map(|raw| Ok(serde_json::from_str::<Value>(&raw?)?))
                    .collect::<Result<Vec<_>>>()?;
                drop(stmt);
                // Recheck after acquiring the transaction in case the receiver
                // read/acknowledged the overdue item while candidates were found.
                let due: bool = tx.query_row(&format!(
                    "SELECT EXISTS(SELECT 1 FROM kv WHERE {UNSEEN} AND julianday(json_extract(value,'$.remindAfter')) <= julianday(?2))"
                ), params![mailbox, now], |r| r.get(0))?;
                if !due { return Ok(false); }
                let summaries = mail.iter().take(5).map(|m| format!("- {}: {}",
                    m["kind"].as_str().unwrap_or("status"),
                    m["subject"].as_str().unwrap_or("Inbox message").chars().take(120).collect::<String>()
                )).collect::<Vec<_>>().join("\n");
                let prompt = format!(
                    "[Pockymoe inbox reminder: unread mail has waited at least 10 minutes]\n{} messages are waiting in your inbox.\n{summaries}\n\nRead them with `pockymoe inbox` and `pockymoe inbox read MESSAGE_ID`. Handle questions and results within your existing task scope; acknowledge only messages you have handled. Reading is not acknowledgement. This is one batched reminder, not a new assignment.", mail.len()
                );
                let pending = Uuid::new_v4().to_string();
                enqueue(&tx, &pending, &thread, &prompt, None, now)?;
                tx.execute(&format!(
                    "UPDATE kv SET value=json_set(value,'$.remindedAt',?2) WHERE {UNSEEN}"
                ), params![mailbox, now])?;
                tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![cooldown, now])?;
                tx.commit()?;
                Ok(true)
            })?;
            if queued {
                self.bus.emit(ThreadEventEnvelope {
                    event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
                    thread_id: thread,
                    timestamp: now.into(),
                    payload: json!({"reason":"pending_steer_updated"}),
                });
            }
        }
        Ok(())
    }
}
