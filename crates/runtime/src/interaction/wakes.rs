//! One-shot wakes a thread registers for itself: on its delegates settling or
//! needing attention (`thread wait --wake`), or on matching mail arriving in its
//! inbox (`inbox wait --wake`). Both kinds share `cli:wake:CALLER:ID` records,
//! the cap and the delivery: steered into a running turn that takes steers,
//! otherwise one queued continuation whose prompt origin is `system`.
use super::{
    agents::{clip, is_descendant, wake_task_context},
    coordination, enqueue,
    inbox::{self, MailFilter},
};
use crate::Supervisor;
use anyhow::{ensure, Result};
use pockymoe_protocol::{
    now_rfc3339, ThreadEventEnvelope, ThreadPendingMailWakeDto, ThreadPendingWakeDto,
    ThreadWakeMemberDto,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

/// Pending wakes of either kind per caller.
const MAX_PENDING_WAKES: usize = 3;
const WAKE_EXCERPT: usize = 1500;
const MAIL_PREVIEWS: usize = 5;
const MAIL_EXCERPT: usize = 600;

fn pending_count(conn: &Connection, caller: &str) -> Result<usize> {
    let pending: i64 = conn.query_row(
        "SELECT COUNT(*) FROM kv WHERE key GLOB ?1",
        [format!("cli:wake:{caller}:*")],
        |r| r.get(0),
    )?;
    Ok(pending as usize)
}

fn ensure_below_cap(conn: &Connection, caller: &str) -> Result<()> {
    let pending = pending_count(conn, caller)?;
    ensure!(
        pending < MAX_PENDING_WAKES,
        "you already have {pending} pending wakes (thread and mail wakes together); that is the limit"
    );
    Ok(())
}

/// A sender's name as the caller knows it: a local thread's name or title, or
/// the device name its mail carried.
fn sender_name(conn: &Connection, caller: &str, thread: &str) -> Result<Option<String>> {
    let local: Option<Option<String>> = conn
        .query_row(
            "SELECT COALESCE(agent_name,title) FROM threads WHERE id=?1",
            [thread],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(name) = local {
        return Ok(name);
    }
    Ok(conn
        .query_row(
            "SELECT json_extract(value,'$.fromDeviceName') FROM kv WHERE key GLOB ?1
             AND json_extract(value,'$.fromThreadId')=?2
             AND json_extract(value,'$.fromDeviceName') IS NOT NULL LIMIT 1",
            params![format!("cli:inbox:{caller}:*"), thread],
            |r| r.get(0),
        )
        .optional()?)
}

/// How long a mail wake waits unless its caller says otherwise, and at most.
const MAIL_WAKE_LIFETIME: Duration = Duration::from_secs(24 * 3600);
const MAIL_WAKE_MAX_LIFETIME: Duration = Duration::from_secs(7 * 24 * 3600);

/// How much of a question a wake report quotes.
const QUESTION_EXCERPT: usize = 1500;

/// Unanswered, unacknowledged questions `member` mailed to `caller`: id,
/// subject and text, oldest first.
fn open_questions_to(
    conn: &Connection,
    caller: &str,
    member: &str,
) -> Result<Vec<(String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT json_extract(q.value,'$.id'), COALESCE(json_extract(q.value,'$.subject'),''),
                COALESCE(json_extract(q.value,'$.text'),'')
         FROM kv q WHERE q.key GLOB ?1
           AND json_extract(q.value,'$.fromThreadId')=?2
           AND json_extract(q.value,'$.kind')='question'
           AND json_extract(q.value,'$.acknowledgedAt') IS NULL
           AND NOT EXISTS (SELECT 1 FROM kv r WHERE r.key GLOB ?3
               AND json_extract(r.value,'$.inReplyTo')=json_extract(q.value,'$.id'))
         ORDER BY json_extract(q.value,'$.createdAt') LIMIT 5",
    )?;
    let rows = stmt
        .query_map(
            params![
                format!("cli:inbox:{caller}:*"),
                member,
                format!("cli:inbox:{member}:*")
            ],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn wake_members(conn: &Connection, ids: &[String]) -> Result<Vec<ThreadWakeMemberDto>> {
    let mut members = Vec::new();
    for id in ids {
        let row: Option<(Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT COALESCE(agent_name,title),status FROM threads WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (name, status) = row.unwrap_or_default();
        members.push(ThreadWakeMemberDto {
            thread_id: id.clone(),
            name,
            status,
        });
    }
    Ok(members)
}

impl Supervisor {
    /// One-shot: when every listed delegate settles (or one blocks or stalls),
    /// queue a single turn on the caller carrying their outcomes. The only path
    /// by which delegates wake their parent, and only because the parent asked.
    pub fn register_wake(&self, caller: &str, ids: &[String]) -> Result<Value> {
        ensure!(
            !ids.is_empty() && ids.len() <= 20,
            "wake on 1 to 20 threads"
        );
        let caller_thread = self.get_thread(caller)?;
        self.ensure_prompt_allowed(&caller_thread)?;
        let id = Uuid::new_v4().to_string();
        self.db.with(|c| {
            for member in ids {
                ensure!(
                    is_descendant(c, caller, member)?,
                    "you can only wake on your own delegates; {member} is not one"
                );
            }
            ensure_below_cap(c, caller)?;
            c.execute(
                "INSERT INTO kv(key,value) VALUES(?1,?2)",
                params![
                    format!("cli:wake:{caller}:{id}"),
                    json!({"members":ids,"createdAt":now_rfc3339(),"taskTurnId":caller_thread.active_turn_id.as_deref().map(|turn| crate::service::peer_work::root_turn(c, caller, turn)).transpose()?}).to_string()
                ],
            )?;
            Ok(())
        })?;
        self.emit_wake_registered(caller);
        Ok(
            json!({"registered":true,"wakeId":id,"threadId":caller,"members":ids,"behavior":"When all of these settle or one blocks or stalls, their report joins your running turn if your harness takes steers, or is queued as your next turn. You may end your turn now."}),
        )
    }

    /// `thread wait --wake`: registers the wake unless every member has already
    /// settled. Then there is nothing to wait for, and a wake would only replay
    /// their last results a moment later, so the caller gets those results now,
    /// in the turn it is still running, with a warning for each member that
    /// holds an unread task from the caller in its inbox (mailed, never queued).
    pub async fn wake_or_report(&self, caller: &str, ids: &[String]) -> Result<Value> {
        ensure!(
            !ids.is_empty() && ids.len() <= 20,
            "wake on 1 to 20 threads"
        );
        self.db.with(|c| {
            for member in ids {
                ensure!(
                    is_descendant(c, caller, member)?,
                    "you can only wake on your own delegates; {member} is not one"
                );
            }
            Ok(())
        })?;
        let now = self.wait_threads(ids, false, Duration::ZERO).await?;
        if now["allSettled"] != true {
            return self.register_wake(caller, ids);
        }
        let unstarted = self.db.with(|c| {
            let mut waiting = Vec::new();
            for member in ids {
                let count: i64 = c.query_row(
                    "SELECT COUNT(*) FROM kv WHERE key GLOB ?1
                       AND json_extract(value,'$.fromThreadId')=?2
                       AND json_extract(value,'$.kind')='task'
                       AND json_extract(value,'$.readAt') IS NULL
                       AND json_extract(value,'$.acknowledgedAt') IS NULL",
                    params![format!("cli:inbox:{member}:*"), caller],
                    |r| r.get(0),
                )?;
                if count > 0 {
                    waiting.push(json!({"threadId": member, "unreadTasksFromYou": count}));
                }
            }
            Ok(waiting)
        })?;
        let next = if unstarted.is_empty() {
            "No wake was registered: these threads have already settled and nothing is queued for them, so their last results are here. If you meant to wait for new work, queue it first (`pockymoe thread send NAME --kind task --delivery queue ...`) and then register the wake."
        } else {
            "No wake was registered: these threads are idle. A task you sent sits unread in a delegate's inbox (`unstartedTasks`): inbox delivery never starts a turn. Send it again with `--delivery queue` (the mailed copy stays in its inbox; say so in the new message or it may be read as a second task), then register the wake."
        };
        Ok(json!({
            "registered": false,
            "allSettled": true,
            "threadId": caller,
            "threads": now["threads"],
            "unstartedTasks": unstarted,
            "next": next,
        }))
    }

    /// One-shot: when mail matching `filter` that was not already waiting
    /// arrives in the caller's inbox, deliver its previews like a delegate wake.
    /// Works for mail from any device, so it is how a caller waits for work it
    /// sent to another device.
    ///
    /// A mail wake expires (24 hours unless `lifetime` says otherwise, at most
    /// 7 days): mail that never comes must not hold one of the caller's three
    /// wakes forever. Expiry leaves a passive status note in the caller's inbox.
    pub fn register_mail_wake(
        &self,
        caller: &str,
        filter: &MailFilter,
        lifetime: Option<Duration>,
    ) -> Result<Value> {
        filter.validate()?;
        let lifetime = lifetime
            .unwrap_or(MAIL_WAKE_LIFETIME)
            .clamp(Duration::from_secs(1), MAIL_WAKE_MAX_LIFETIME);
        let expires = (chrono::Utc::now()
            + chrono::Duration::from_std(lifetime).unwrap_or(chrono::Duration::days(1)))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let caller_thread = self.get_thread(caller)?;
        self.ensure_prompt_allowed(&caller_thread)?;
        let id = Uuid::new_v4().to_string();
        self.db.with(|c| {
            ensure_below_cap(c, caller)?;
            // `--new` semantics: mail already waiting is the caller's to read now.
            let seen = inbox::matching_ids(c, caller, filter)?;
            c.execute(
                "INSERT INTO kv(key,value) VALUES(?1,?2)",
                params![
                    format!("cli:wake:{caller}:{id}"),
                    json!({"kind":"mail","filter":filter,"seen":seen,"createdAt":now_rfc3339(),"expiresAt":expires,"taskTurnId":caller_thread.active_turn_id.as_deref().map(|turn| crate::service::peer_work::root_turn(c, caller, turn)).transpose()?}).to_string()
                ],
            )?;
            Ok(())
        })?;
        self.emit_wake_registered(caller);
        Ok(json!({
            "wakeId": id, "threadId": caller, "kind": "mail", "filter": filter, "expiresAt": expires,
            "behavior": "When new mail matching this filter arrives in your inbox, its previews join your running turn if your harness takes steers, or are queued as your next turn. Mail already waiting does not fire it. If nothing matches before expiresAt, the wake is dropped and a status note is left in your inbox. You may end your turn now.",
        }))
    }

    /// The caller's wakes that have not fired, and optionally cancels some:
    /// `cancel` names wake ids, `all` every one. A cancelled wake never fires.
    pub fn manage_wakes(&self, caller: &str, cancel: &[String], all: bool) -> Result<Value> {
        let mut cancelled = Vec::new();
        self.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            let ids: Vec<String> = if all {
                let mut stmt = tx.prepare("SELECT key FROM kv WHERE key GLOB ?1")?;
                let keys = stmt
                    .query_map([format!("cli:wake:{caller}:*")], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                keys.iter()
                    .filter_map(|key| key.rsplit(':').next().map(str::to_owned))
                    .collect()
            } else {
                cancel.to_vec()
            };
            for id in ids {
                ensure!(
                    id.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-'),
                    "invalid wake id {id}"
                );
                if tx.execute(
                    "DELETE FROM kv WHERE key=?1",
                    [format!("cli:wake:{caller}:{id}")],
                )? > 0
                {
                    cancelled.push(id);
                }
            }
            tx.commit()?;
            Ok(())
        })?;
        if !cancelled.is_empty() {
            self.emit_wake_registered(caller);
        }
        Ok(json!({
            "threadId": caller,
            "cancelled": cancelled,
            "pendingWakes": self.pending_wakes(caller)?,
            "limit": MAX_PENDING_WAKES,
        }))
    }

    /// Drops mail wakes past their `expiresAt` and tells each caller in its inbox.
    fn expire_mail_wakes(&self) -> Result<()> {
        let now = now_rfc3339();
        let expired = self.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            let rows: Vec<(String, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT key,value FROM kv WHERE key GLOB 'cli:wake:*'
                       AND json_extract(value,'$.kind')='mail'
                       AND json_extract(value,'$.expiresAt') IS NOT NULL
                       AND julianday(json_extract(value,'$.expiresAt')) <= julianday(?1)",
                )?;
                let rows = stmt
                    .query_map([&now], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                rows
            };
            let mut callers = Vec::new();
            for (key, raw) in rows {
                let caller = key.split(':').nth(2).unwrap_or_default().to_string();
                let registered: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
                let filter: MailFilter =
                    serde_json::from_value(registered["filter"].clone()).unwrap_or_default();
                // Mail that matches is the wake's to report, not to expire.
                let seen: Vec<String> =
                    serde_json::from_value(registered["seen"].clone()).unwrap_or_default();
                if !inbox::matching(&tx, &caller, &filter, &seen, 1)?.is_empty() {
                    continue;
                }
                tx.execute("DELETE FROM kv WHERE key=?1", [&key])?;
                let exists = tx
                    .query_row("SELECT 1 FROM threads WHERE id=?1", [&caller], |_| Ok(()))
                    .optional()?
                    .is_some();
                if exists {
                    inbox::store(
                        &tx,
                        &caller,
                        &Uuid::new_v4().to_string(),
                        None,
                        &format!(
                            "The mail wake you registered at {} expired: no matching mail arrived (filter: {}). If you still depend on it, check the sender (`pockymoe thread status ID`, `pockymoe outbox`) and register the wake again.",
                            registered["createdAt"].as_str().unwrap_or("an earlier time"),
                            registered["filter"]
                        ),
                        &now,
                        inbox::Envelope {
                            subject: Some("Mail wake expired"),
                            kind: "status",
                            in_reply_to: None,
                            topic_key: None,
                        },
                    )?;
                    callers.push(caller);
                }
            }
            tx.commit()?;
            Ok(callers)
        })?;
        for caller in expired {
            self.emit_wake_registered(&caller);
        }
        Ok(())
    }

    fn emit_wake_registered(&self, caller: &str) {
        self.bus.emit(ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
            thread_id: caller.into(),
            timestamp: now_rfc3339(),
            payload: json!({"reason":"wake_registered"}),
        });
    }

    /// Wakes `thread` registered that have not fired, for its detail view.
    pub fn pending_wakes(&self, thread: &str) -> Result<Vec<ThreadPendingWakeDto>> {
        self.db.with(|c| {
            let mut stmt = c.prepare("SELECT key,value FROM kv WHERE key GLOB ?1 ORDER BY key")?;
            let rows = stmt
                .query_map([format!("cli:wake:{thread}:*")], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut wakes = Vec::new();
            for (key, raw) in rows {
                let Ok(record) = serde_json::from_str::<Value>(&raw) else {
                    continue;
                };
                let id = key.rsplit(':').next().unwrap_or_default().to_string();
                let created_at = record["createdAt"].as_str().map(str::to_owned);
                if record["kind"] == "mail" {
                    let filter: MailFilter =
                        serde_json::from_value(record["filter"].clone()).unwrap_or_default();
                    let mut from = Vec::new();
                    for sender in &filter.from_thread_ids {
                        let local = wake_members(c, std::slice::from_ref(sender))?.remove(0);
                        from.push(ThreadWakeMemberDto {
                            name: match local.name {
                                Some(name) => Some(name),
                                None => sender_name(c, thread, sender)?,
                            },
                            ..local
                        });
                    }
                    // Clients from before `kind` list members: the local senders.
                    let members = from
                        .iter()
                        .filter(|m| m.status.is_some())
                        .cloned()
                        .collect();
                    wakes.push(ThreadPendingWakeDto {
                        id,
                        created_at,
                        members,
                        kind: "mail".into(),
                        mail: Some(ThreadPendingMailWakeDto {
                            from,
                            kinds: filter.kinds,
                            in_reply_to: filter.in_reply_to,
                        }),
                    });
                    continue;
                }
                let ids: Vec<String> = record["members"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                wakes.push(ThreadPendingWakeDto {
                    id,
                    created_at,
                    members: wake_members(c, &ids)?,
                    kind: "threads".into(),
                    mail: None,
                });
            }
            wakes.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            Ok(wakes)
        })
    }

    /// The report of a delegate wake that is ready to fire, the members whose
    /// stall it reports and the questions to the caller it quotes.
    async fn delegate_wake_report(
        &self,
        caller: &str,
        registered: &Value,
    ) -> Result<Option<(String, Vec<String>, Vec<String>)>> {
        let members: Vec<String> =
            serde_json::from_value(registered["members"].clone()).unwrap_or_default();
        let timers = self.interaction.timers();
        let mut states = Vec::new();
        let mut fresh_stalls = Vec::new();
        for member in &members {
            let Ok(state) = self.member_state(member).await else {
                continue;
            };
            if state["state"] == "stalled" {
                // A stall fires each caller's wake once; a wake registered again
                // after hearing about it waits for the delegate to change.
                let told = self.db.with(|c| {
                    Ok(coordination::stall(c, member, timers)?
                        .is_some_and(|record| coordination::stall_reported_to(&record, caller)))
                })?;
                if !told {
                    fresh_stalls.push(member.clone());
                }
            }
            states.push(state);
        }
        let blocked = states.iter().any(|s| s["state"] == "blocked");
        let ready =
            states.iter().all(|s| s["settled"] == true) || blocked || !fresh_stalls.is_empty();
        if !ready {
            return Ok(None);
        }
        // A blocked member's questions to the caller are quoted in full, with the
        // command that answers them, so the wake turn can reply at once.
        let mut asked = Vec::new();
        let mut questions = std::collections::HashMap::new();
        for s in states.iter().filter(|s| s["state"] == "blocked") {
            let Some(member) = s["threadId"].as_str() else {
                continue;
            };
            let open = self.db.with(|c| open_questions_to(c, caller, member))?;
            asked.extend(open.iter().map(|q| q.0.clone()));
            questions.insert(member.to_string(), open);
        }
        let lines = states
            .iter()
            .map(|s| {
                let label = s["name"].as_str().map_or_else(
                    || s["threadId"].as_str().unwrap_or("").to_string(),
                    |n| format!("{n} ({})", s["threadId"].as_str().unwrap_or("")),
                );
                let mut line = format!("- {label}: {}", s["state"].as_str().unwrap_or(""));
                if let Some(waiting) = s["waitingOn"].as_array() {
                    let waiting = waiting
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("; ");
                    line.push_str(&format!(", waiting on: {waiting}"));
                }
                let target = s["name"]
                    .as_str()
                    .or(s["threadId"].as_str())
                    .unwrap_or("ID");
                for (id, subject, text) in s["threadId"]
                    .as_str()
                    .and_then(|member| questions.get(member))
                    .into_iter()
                    .flatten()
                {
                    let body = if text.chars().count() > QUESTION_EXCERPT {
                        format!(
                            "{}\n  [Truncated; read it all with `pockymoe inbox read {id}`]",
                            clip(text, QUESTION_EXCERPT)
                        )
                    } else {
                        text.clone()
                    };
                    line.push_str(&format!(
                        "\n  question {id} | {subject}:\n  {}\n  answer with: pockymoe thread send {target} --kind result --in-reply-to {id} --subject \"...\" --text \"...\"",
                        body.replace('\n', "\n  ")
                    ));
                }
                if let Some(failures) = s["consecutiveFailures"].as_i64().filter(|n| *n > 0) {
                    line.push_str(&format!(", {failures} failed turns in a row"));
                }
                if let Some(outcome) = coordination::outcome_line(&s["outcome"]) {
                    line.push_str(&format!("\n  outcome: {}", clip(&outcome, WAKE_EXCERPT)));
                }
                // A stalled delegate's last closing message belongs to an earlier
                // task; next to the stall it reads as this task's result.
                if let Some(message) = s["lastTurn"]["closingMessage"]
                    .as_str()
                    .filter(|_| s["state"] != "stalled")
                {
                    line.push_str(&format!(
                        "\n  closing message: {}",
                        clip(message, WAKE_EXCERPT)
                    ));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n");
        use crate::service::peer_work::{WAKE_BLOCKED, WAKE_NOTE, WAKE_SETTLED, WAKE_STALLED};
        let header = if blocked {
            WAKE_BLOCKED
        } else if !fresh_stalls.is_empty() {
            WAKE_STALLED
        } else {
            WAKE_SETTLED
        };
        let hint = if fresh_stalls.is_empty() {
            String::new()
        } else {
            format!("\n\n{}", coordination::STALLED_NEXT)
        };
        Ok(Some((
            format!(
                "{header}{WAKE_NOTE}{lines}{hint}\n\nVerify results before acting on them. Full transcripts: pockymoe transcript ID --limit 1"
            ),
            fresh_stalls,
            asked,
        )))
    }

    /// The report of a mail wake whose mail has arrived.
    fn mail_wake_report(&self, caller: &str, registered: &Value) -> Result<Option<String>> {
        let filter: MailFilter =
            serde_json::from_value(registered["filter"].clone()).unwrap_or_default();
        let seen: Vec<String> =
            serde_json::from_value(registered["seen"].clone()).unwrap_or_default();
        self.db.with(|c| {
            let mail = inbox::matching(c, caller, &filter, &seen, 50)?;
            if mail.is_empty() {
                return Ok(None);
            }
            let mut lines = Vec::new();
            for message in mail.iter().take(MAIL_PREVIEWS) {
                let from = message["fromThreadId"].as_str();
                let sender = match from {
                    Some(thread) => {
                        let name = sender_name(c, caller, thread)?;
                        let address = match message["replyTo"].as_str() {
                            Some(remote) => remote.to_string(),
                            None => thread.to_string(),
                        };
                        match name {
                            Some(name) => format!("{name} [{address}]"),
                            None => address,
                        }
                    }
                    None => "the owner".into(),
                };
                let text = message["text"].as_str().unwrap_or("").replace('\n', " ");
                lines.push(format!(
                    "- {} \"{}\" from {sender} · message {}\n  {}",
                    message["kind"].as_str().unwrap_or("status"),
                    message["subject"].as_str().unwrap_or("(no subject)"),
                    message["id"].as_str().unwrap_or(""),
                    clip(text.trim(), MAIL_EXCERPT)
                ));
            }
            let more = if mail.len() > MAIL_PREVIEWS {
                format!(
                    "\n{} more matching messages are waiting: `pockymoe inbox list`.",
                    mail.len() - MAIL_PREVIEWS
                )
            } else {
                String::new()
            };
            Ok(Some(format!(
                "{}{}{}{more}\n\nRead each in full with `pockymoe inbox read MESSAGE_ID`; acknowledge what you handled with `pockymoe inbox ack MESSAGE_ID...`. Reply with `pockymoe thread send SENDER --in-reply-to MESSAGE_ID` (for another device's mail, SENDER is its replyTo).",
                crate::service::peer_work::WAKE_MAIL,
                crate::service::peer_work::WAKE_MAIL_NOTE,
                lines.join("\n")
            )))
        })
    }

    pub(super) async fn fire_wakes(&self) -> Result<()> {
        self.expire_mail_wakes()?;
        let wakes: Vec<(String, String)> = self.db.with(|c| {
            let mut stmt = c.prepare("SELECT key,value FROM kv WHERE key GLOB 'cli:wake:*'")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        for (key, raw) in wakes {
            let caller = key.split(':').nth(2).unwrap_or_default().to_string();
            let Ok(registered) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            let (text, stalls, asked) = if registered["kind"] == "mail" {
                match self.mail_wake_report(&caller, &registered)? {
                    Some(text) => (text, Vec::new(), Vec::new()),
                    None => continue,
                }
            } else {
                match self.delegate_wake_report(&caller, &registered).await? {
                    Some(report) => report,
                    None => continue,
                }
            };
            // A running turn that takes steers hears the report now, e.g. Claude
            // waiting on a native subagent; otherwise it is the caller's next turn.
            let steer_turn = self.get_thread(&caller).ok().and_then(|thread| {
                let steer = self.runtime(thread.provider).is_ok_and(|runtime| {
                    runtime
                        .negotiated_caps(thread.agent_id.as_deref())
                        .turns
                        .steer
                });
                thread
                    .active_turn_id
                    .filter(|_| steer && thread.status == "running")
            });
            let now = now_rfc3339();
            let pending = Uuid::new_v4().to_string();
            let queued = self.db.with(|c| {
                let tx = c.unchecked_transaction()?;
                // Another tick may have fired it meanwhile.
                if tx
                    .query_row("SELECT 1 FROM kv WHERE key=?1", [&key], |_| Ok(()))
                    .optional()?
                    .is_none()
                {
                    return Ok(false);
                }
                let exists = tx
                    .query_row("SELECT 1 FROM threads WHERE id=?1", [&caller], |_| Ok(()))
                    .optional()?
                    .is_some();
                if exists {
                    enqueue(&tx, &pending, &caller, &text, None, &now)?;
                    tx.execute(
                        "INSERT INTO kv(key,value) VALUES(?1,'system')",
                        [format!("cli:prompt-origin:{pending}")],
                    )?;
                    match &steer_turn {
                        // Held for that turn, so the queue worker cannot start it too.
                        Some(turn) => {
                            tx.execute(
                                "UPDATE thread_pending_steers SET delivery='cli-steer',turn_id=?2 WHERE id=?1",
                                params![pending, turn],
                            )?;
                        }
                        None => wake_task_context(&tx, &caller, &pending, &registered, &text)?,
                    }
                    for member in &stalls {
                        coordination::mark_stall_reported(&tx, member, &caller)?;
                    }
                    // The report quotes these questions: do not also deliver
                    // them as prompt turns of their own (`thread ask`).
                    for question in &asked {
                        tx.execute(
                            "DELETE FROM kv WHERE key=?1",
                            [format!("cli:ask:{caller}:{question}")],
                        )?;
                        // Quoted in full, so it is read: the 10-minute reminder
                        // must not start a turn for it again.
                        tx.execute(
                            "UPDATE kv SET value=json_set(value,'$.readAt',?2)
                             WHERE key=?1 AND json_extract(value,'$.readAt') IS NULL",
                            params![format!("cli:inbox:{caller}:{question}"), now],
                        )?;
                    }
                }
                tx.execute("DELETE FROM kv WHERE key=?1", [&key])?;
                tx.commit()?;
                Ok(exists)
            })?;
            if queued && steer_turn.is_some() {
                if let Err(error) = self.steer_pending_prompt(&caller, &pending).await {
                    // A wake must not be lost: the turn ended or refused it, so
                    // it becomes the next turn, nested under the original task.
                    tracing::info!(%error, thread_id = %caller, "wake queued instead of steered");
                    self.db.with(|c| {
                        let tx = c.unchecked_transaction()?;
                        if tx.execute(
                            "UPDATE thread_pending_steers SET delivery='continuation',turn_id='' WHERE id=?1 AND delivery='cli-steer'",
                            [&pending],
                        )? > 0
                        {
                            wake_task_context(&tx, &caller, &pending, &registered, &text)?;
                        }
                        tx.commit()?;
                        Ok(())
                    })?;
                }
            }
            self.bus.emit(ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
                thread_id: caller,
                timestamp: now,
                payload: json!({"reason":"pending_steer_updated"}),
            });
        }
        Ok(())
    }
}
