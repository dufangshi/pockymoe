use crate::Supervisor;
use anyhow::{ensure, Result};
use pockymoe_protocol::now_rfc3339;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

// Explicit partial indexes keep periodic work independent of acknowledged history,
// even when SQLite would otherwise prefer the generic kv key index.
pub(crate) const REMINDER_CANDIDATES: &str = "SELECT DISTINCT json_extract(value,'$.threadId')
    FROM kv INDEXED BY mailbox_open_idx
    WHERE key GLOB 'cli:inbox:*' AND json_extract(value,'$.acknowledgedAt') IS NULL
      AND json_extract(value,'$.readAt') IS NULL
      AND json_extract(value,'$.remindedAt') IS NULL
      AND json_extract(value,'$.supersededBy') IS NULL
      AND julianday(json_extract(value,'$.remindAfter')) <= julianday(?1)";

pub(super) const OPEN_QUESTIONS: &str = "SELECT json_extract(q.value,'$.id'),
    COALESCE(json_extract(q.value,'$.subject'),''), json_extract(q.value,'$.threadId')
    FROM kv q INDEXED BY mailbox_open_idx
    WHERE q.key GLOB 'cli:inbox:*' AND json_extract(q.value,'$.acknowledgedAt') IS NULL
      AND json_extract(q.value,'$.fromThreadId')=?1
      AND json_extract(q.value,'$.kind')='question'
      AND NOT EXISTS (SELECT 1 FROM kv r INDEXED BY mailbox_reply_idx
          WHERE r.key GLOB 'cli:inbox:*'
            AND json_extract(r.value,'$.threadId')=?1
            AND json_extract(r.value,'$.inReplyTo') IS NOT NULL
            AND json_extract(r.value,'$.inReplyTo')=json_extract(q.value,'$.id'))";

/// Keep handled mail for 30 days for read/--all. Preserve replies that still
/// resolve an unacknowledged question, or deleting them could re-block a wake.
/// Run at the first reminder tick and then hourly, using a durable cooldown.
pub(crate) fn cleanup_acknowledged(conn: &Connection, now: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let cooling: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM kv WHERE key='cli:mailbox-cleanup'
         AND julianday(value)+1.0/24.0 > julianday(?1))",
        [now],
        |r| r.get(0),
    )?;
    if cooling {
        return Ok(());
    }
    tx.execute(
        "DELETE FROM kv INDEXED BY mailbox_ack_age_idx
         WHERE key GLOB 'cli:inbox:*' AND json_extract(value,'$.acknowledgedAt') IS NOT NULL
           AND julianday(json_extract(value,'$.acknowledgedAt')) < julianday(?1)-30
           AND NOT EXISTS (SELECT 1 FROM kv q INDEXED BY mailbox_open_idx
               WHERE q.key GLOB 'cli:inbox:*' AND json_extract(q.value,'$.acknowledgedAt') IS NULL
                 AND json_extract(q.value,'$.kind')='question'
                 AND json_extract(q.value,'$.fromThreadId')=json_extract(kv.value,'$.threadId')
                 AND json_extract(q.value,'$.id')=json_extract(kv.value,'$.inReplyTo'))",
        [now],
    )?;
    tx.execute(
        "INSERT INTO kv(key,value) VALUES('cli:mailbox-cleanup',?1)
        ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [now],
    )?;
    tx.commit()?;
    Ok(())
}

fn prefix(thread: &str) -> String {
    format!("cli:inbox:{thread}:")
}
fn key(thread: &str, id: &str) -> String {
    format!("{}{id}", prefix(thread))
}
pub(crate) struct Envelope<'a> {
    pub subject: Option<&'a str>,
    pub kind: &'a str,
    pub in_reply_to: Option<&'a str>,
    pub topic_key: Option<&'a str>,
}
pub(crate) fn store(
    conn: &Connection,
    thread: &str,
    id: &str,
    from: Option<&str>,
    text: &str,
    now: &str,
    envelope: Envelope<'_>,
) -> Result<()> {
    let remind_after = chrono::DateTime::parse_from_rfc3339(now)? + chrono::Duration::minutes(10);
    // subject/kind ride on the stored record so `inbox list`, which returns it
    // verbatim, lets a receiver triage without opening anything.
    conn.execute("INSERT INTO kv(key,value) VALUES(?1,?2)", params![key(thread,id),json!({"id":id,"threadId":thread,"fromThreadId":from,"text":text,"createdAt":now,"acknowledgedAt":null,"readAt":null,"remindAfter":remind_after.to_rfc3339(),"remindedAt":null,"subject":envelope.subject,"kind":envelope.kind,"inReplyTo":envelope.in_reply_to,"topicKey":envelope.topic_key,"supersededBy":null}).to_string()])?;
    Ok(())
}
/// The sender explicitly opted into full snapshots. Retain old mail for read/all,
/// but do not repeatedly present obsolete status as work requiring attention.
pub(super) fn supersede_status(
    conn: &Connection,
    thread: &str,
    id: &str,
    from: Option<&str>,
    topic: &str,
) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE kv SET value=json_set(value,'$.supersededBy',?2)
         WHERE key GLOB ?1 AND json_extract(value,'$.id') != ?2
           AND json_extract(value,'$.fromThreadId') IS ?3
           AND json_extract(value,'$.topicKey')=?4
           AND json_extract(value,'$.kind')='status'
           AND json_extract(value,'$.inReplyTo') IS NULL
           AND json_extract(value,'$.acknowledgedAt') IS NULL
           AND json_extract(value,'$.supersededBy') IS NULL",
        params![format!("{}*", prefix(thread)), id, from, topic],
    )?)
}

const FILTERED_UNREAD: &str = "key GLOB ?1
    AND json_extract(value,'$.acknowledgedAt') IS NULL
    AND json_extract(value,'$.supersededBy') IS NULL
    AND (?2='[]' OR json_extract(value,'$.fromThreadId') IN (SELECT value FROM json_each(?2)))
    AND (?3='[]' OR COALESCE(json_extract(value,'$.kind'),'status') IN (SELECT value FROM json_each(?3)))
    AND json_extract(value,'$.id') NOT IN (SELECT value FROM json_each(?4))";

fn string_filter(query: &Value, field: &str) -> Result<Vec<String>> {
    if query[field].is_null() {
        return Ok(vec![]);
    }
    let values = query[field]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("{field} must be an array of strings"))?;
    values
        .iter()
        .map(|v| {
            v.as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("{field} must contain nonempty strings"))
        })
        .collect()
}
fn validate_kinds(kinds: &[String]) -> Result<()> {
    ensure!(
        kinds
            .iter()
            .all(|k| pockymoe_protocol::MESSAGE_KINDS.contains(&k.as_str())),
        "kinds must contain only result, question, status or task"
    );
    Ok(())
}
fn mark_read(conn: &Connection, thread: &str, message: &mut Value, now: &str) -> Result<()> {
    if message["readAt"].is_null() {
        conn.execute(
            "UPDATE kv SET value=json_set(value,'$.readAt',?2) WHERE key=?1 AND json_extract(value,'$.readAt') IS NULL",
            params![key(thread, message["id"].as_str().unwrap_or_default()), now],
        )?;
        message["readAt"] = json!(now);
    }
    Ok(())
}
fn get(conn: &Connection, thread: &str, id: &str) -> Result<Value> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM kv WHERE key=?1",
            [key(thread, id)],
            |r| r.get(0),
        )
        .optional()?;
    Ok(serde_json::from_str(&raw.ok_or_else(|| {
        anyhow::anyhow!("inbox message not found")
    })?)?)
}

pub(super) fn store_from_peer(
    conn: &Connection,
    thread: &str,
    id: &str,
    sender: &super::RemoteSender,
    text: &str,
    now: &str,
    envelope: Envelope<'_>,
) -> Result<()> {
    store(
        conn,
        thread,
        id,
        sender.thread_id.as_deref(),
        text,
        now,
        envelope,
    )?;
    let reply_to = sender
        .thread_id
        .as_ref()
        .map(|from| format!("{}/{from}", sender.device_id));
    conn.execute(
        "UPDATE kv SET value=json_set(value,'$.fromDeviceId',?1,'$.fromDeviceName',?2,'$.replyTo',?3) WHERE key=?4",
        params![sender.device_id, sender.device_name, reply_to, key(thread, id)],
    )?;
    Ok(())
}
impl Supervisor {
    /// Waiting questions and tasks first, so newer status cannot hide a blocker.
    /// Showing what is waiting is what makes a passive inbox workable: a bare count
    /// tells an agent nothing about whether looking now is worth interrupting itself.
    pub fn inbox_unread_digest(&self, thread: &str, limit: usize) -> Result<Vec<(String, String)>> {
        self.db.with(|c| {
            let mut stmt = c.prepare(
                "SELECT value FROM kv WHERE key GLOB ?1
                   AND json_extract(value,'$.acknowledgedAt') IS NULL
                   AND json_extract(value,'$.supersededBy') IS NULL
                 ORDER BY CASE json_extract(value,'$.kind')
                   WHEN 'question' THEN 0 WHEN 'task' THEN 1 WHEN 'result' THEN 2 ELSE 3 END,
                   json_extract(value,'$.createdAt') DESC, json_extract(value,'$.id') DESC LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(params![format!("{}*", prefix(thread)), limit as i64], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows
                .iter()
                .filter_map(|raw| serde_json::from_str::<Value>(raw).ok())
                .map(|m| {
                    let kind = m["kind"].as_str().unwrap_or("status").to_string();
                    let subject = m["subject"].as_str().map(str::to_owned).unwrap_or_else(|| {
                        // Pre-envelope mail, and senders that skipped --subject.
                        let text = m["text"].as_str().unwrap_or("");
                        let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                        line.chars().take(60).collect()
                    });
                    (kind, subject)
                })
                .collect())
        })
    }
    /// Blocks until unacknowledged mail matching the filters is waiting, then returns
    /// it with text inline (bounded). `only_new` ignores mail that was already
    /// waiting, for a caller deliberately deferring something it has seen.
    pub async fn inbox_wait(
        &self,
        thread: &str,
        from: &[String],
        kinds: &[String],
        only_new: bool,
        timeout: std::time::Duration,
    ) -> Result<Value> {
        self.get_thread(thread)?;
        validate_kinds(kinds)?;
        let started = std::time::Instant::now();
        let deadline = started + timeout;
        let mut events = self.bus.subscribe();
        let mailbox = format!("{}*", prefix(thread));
        let from_json = serde_json::to_string(from)?;
        let kinds_json = serde_json::to_string(kinds)?;
        // Capture every matching id, not just the first page. Exclude them in SQL
        // before LIMIT, otherwise --new is starved by a backlog of old messages.
        let seen: Vec<String> = if only_new {
            self.db.with(|c| {
                let mut stmt = c.prepare(&format!(
                    "SELECT json_extract(value,'$.id') FROM kv WHERE {FILTERED_UNREAD}"
                ))?;
                let ids = stmt
                    .query_map(params![mailbox, from_json, kinds_json, "[]"], |r| r.get(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(ids)
            })?
        } else {
            vec![]
        };
        let seen_json = serde_json::to_string(&seen)?;
        let unread = || {
            self.db.with(|c| {
                let mut stmt = c.prepare(&format!(
                    "SELECT value FROM kv WHERE {FILTERED_UNREAD}
                     ORDER BY json_extract(value,'$.createdAt'), json_extract(value,'$.id') LIMIT 200"
                ))?;
                let rows = stmt
                    .query_map(params![mailbox, from_json, kinds_json, seen_json], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let mut messages = rows.iter().map(|raw| serde_json::from_str::<Value>(raw)).collect::<std::result::Result<Vec<_>, _>>()?;
                let now = now_rfc3339();
                for message in &mut messages { mark_read(c, thread, message, &now)?; }
                Ok(messages)
            })
        };
        loop {
            let matching: Vec<Value> = unread()?
                .into_iter()
                .map(|mut m| {
                    let text = m["text"].as_str().unwrap_or("").to_string();
                    m["textLength"] = json!(text.chars().count());
                    m["text"] = json!(super::agents::clip(&text, 4000));
                    m
                })
                .collect();
            if !matching.is_empty() || !super::agents::pause(&mut events, deadline).await {
                let found = !matching.is_empty();
                return Ok(json!({
                    "threadId": thread,
                    "timedOut": !found,
                    "waitedSeconds": started.elapsed().as_secs(),
                    "messages": matching,
                    "next": if found {
                        "Handle these, then acknowledge them with `pockymoe inbox ack ID...`; unacknowledged mail satisfies the next wait immediately."
                    } else {
                        "Nothing arrived. Wait again, or check `pockymoe thread tree` for delegates that are blocked or finished without writing."
                    },
                }));
            }
        }
    }

    pub fn inbox_unread_count(&self, thread: &str) -> Result<i64> {
        self.db.with(|c| Ok(c.query_row("SELECT count(*) FROM kv WHERE key GLOB ?1 AND json_extract(value,'$.acknowledgedAt') IS NULL AND json_extract(value,'$.supersededBy') IS NULL",[format!("{}*",prefix(thread))],|r|r.get(0))?))
    }
    pub fn inbox_list(&self, thread: &str, query: &Value) -> Result<Value> {
        self.get_thread(thread)?;
        let limit = query["limit"].as_u64().unwrap_or(20).clamp(1, 100) as usize;
        let all = query["all"].as_bool().unwrap_or(false);
        let from = string_filter(query, "fromThreadIds")?;
        let kinds = string_filter(query, "kinds")?;
        validate_kinds(&kinds)?;
        let from_json = serde_json::to_string(&from)?;
        let kinds_json = serde_json::to_string(&kinds)?;
        self.db.with(|c| {
            let before=query["before"].as_str().map(|id|get(c,thread,id)).transpose()?;
            let date=before.as_ref().and_then(|v|v["createdAt"].as_str());
            let id=query["before"].as_str();
            let mut stmt=c.prepare("SELECT value FROM kv WHERE key GLOB ?1
                AND (?2 OR (json_extract(value,'$.acknowledgedAt') IS NULL AND json_extract(value,'$.supersededBy') IS NULL))
                AND (?3 IS NULL OR (json_extract(value,'$.createdAt'),json_extract(value,'$.id')) < (?3,?4))
                AND (?6='[]' OR json_extract(value,'$.fromThreadId') IN (SELECT value FROM json_each(?6)))
                AND (?7='[]' OR COALESCE(json_extract(value,'$.kind'),'status') IN (SELECT value FROM json_each(?7)))
                ORDER BY json_extract(value,'$.createdAt') DESC,json_extract(value,'$.id') DESC LIMIT ?5")?;
            let raw=stmt.query_map(params![format!("{}*",prefix(thread)),all,date,id,limit+1,from_json,kinds_json],|r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
            let has_more=raw.len()>limit;
            let mut messages=raw.iter().take(limit).map(|r|serde_json::from_str::<Value>(r)).collect::<std::result::Result<Vec<_>,_>>()?;
            let next=if has_more {messages.last().map(|m|m["id"].clone())} else {None};
            for message in &mut messages {
                let text=message["text"].as_str().unwrap_or("");
                let count=text.chars().count();
                message["preview"]=json!(text.chars().take(240).collect::<String>());
                message["textLength"]=json!(count);
                message.as_object_mut().unwrap().remove("text");
            }
            messages.reverse();
            Ok(json!({"threadId":thread,"messages":messages,"nextBefore":next,"readCommand":format!("pockymoe inbox read MESSAGE_ID --thread {thread}"),"acknowledgement":"Reading does not acknowledge a message. Use inbox ack after handling it."}))
        })
    }
    pub fn inbox_read(&self, thread: &str, query: &Value) -> Result<Value> {
        self.get_thread(thread)?;
        let id = query["messageId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("messageId required"))?;
        let offset = query["textOffset"].as_u64().unwrap_or(0) as usize;
        self.db.with(|c| {
            let mut message = get(c, thread, id)?;
            let chars: Vec<char> = message["text"].as_str().unwrap_or("").chars().collect();
            let end = offset.saturating_add(8192).min(chars.len());
            ensure!(offset <= chars.len(), "textOffset exceeds message length");
            mark_read(c, thread, &mut message, &now_rfc3339())?;
            message["text"] = json!(chars[offset..end].iter().collect::<String>());
            message["nextTextOffset"] = if end < chars.len() {
                json!(end)
            } else {
                Value::Null
            };
            Ok(message)
        })
    }
    pub fn inbox_ack(&self, thread: &str, query: &Value) -> Result<Value> {
        self.get_thread(thread)?;
        let ids = query["messageIds"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("messageIds required"))?;
        ensure!(
            !ids.is_empty() && ids.len() <= 100,
            "acknowledge 1 to 100 messages"
        );
        self.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            let now = now_rfc3339();
            for id in ids {
                let id = id
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("invalid message ID"))?;
                let mut message = get(&tx, thread, id)?;
                if message["acknowledgedAt"].is_null() {
                    message["acknowledgedAt"] = json!(now);
                    tx.execute(
                        "UPDATE kv SET value=?1 WHERE key=?2",
                        params![message.to_string(), key(thread, id)],
                    )?;
                }
            }
            tx.commit()?;
            Ok(json!({"threadId":thread,"acknowledged":ids}))
        })
    }
}

#[cfg(test)]
mod mailbox_tests {
    use super::*;
    use rusqlite::StatementStatus;
    use std::time::Instant;

    fn database() -> (tempfile::TempDir, crate::Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::Database::open(&dir.path().join("mail.sqlite")).unwrap();
        (dir, db)
    }

    fn insert(conn: &Connection, id: &str, fields: Value) {
        let mut mail = json!({"id":id,"threadId":"receiver","fromThreadId":"sender",
            "kind":"status","acknowledgedAt":null,"readAt":null,"remindedAt":null,
            "supersededBy":null,"remindAfter":"2026-01-01T00:10:00Z","text":"x".repeat(1100)});
        mail.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        conn.execute(
            "INSERT INTO kv(key,value) VALUES(?1,?2)",
            params![
                key(mail["threadId"].as_str().unwrap(), id),
                mail.to_string()
            ],
        )
        .unwrap();
    }

    fn scan(conn: &Connection, sql: &str, parameter: &str) -> (usize, i32, std::time::Duration) {
        let mut stmt = conn.prepare(sql).unwrap();
        let start = Instant::now();
        let count = stmt.query_map([parameter], |_| Ok(())).unwrap().count();
        (
            count,
            stmt.get_status(StatementStatus::VmStep),
            start.elapsed(),
        )
    }

    #[test]
    fn mailbox_scans_skip_acknowledged_history() {
        let (_dir, db) = database();
        db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            for n in 0..5000 {
                insert(&tx, &format!("old-{n}"), json!({"acknowledgedAt":"2026-01-02T00:00:00Z"}));
            }
            insert(&tx, "question", json!({"kind":"question"}));
            insert(&tx, "answered", json!({"kind":"question"}));
            // An acknowledged reply must still resolve a pending question.
            insert(&tx, "reply", json!({"threadId":"sender","inReplyTo":"answered",
                "acknowledgedAt":"2026-01-02T00:00:00Z"}));
            tx.commit()?;
            let old_reminder = "SELECT DISTINCT json_extract(value,'$.threadId') FROM kv
                WHERE key GLOB 'cli:inbox:*' AND json_extract(value,'$.readAt') IS NULL
                AND json_extract(value,'$.remindedAt') IS NULL
                AND json_extract(value,'$.acknowledgedAt') IS NULL
                AND json_extract(value,'$.supersededBy') IS NULL
                AND julianday(json_extract(value,'$.remindAfter')) <= julianday(?1)";
            let old_questions = "SELECT json_extract(q.value,'$.id'), COALESCE(json_extract(q.value,'$.subject'),''), json_extract(q.value,'$.threadId')
                FROM kv q WHERE q.key GLOB 'cli:inbox:*'
                AND json_extract(q.value,'$.fromThreadId')=?1 AND json_extract(q.value,'$.kind')='question'
                AND json_extract(q.value,'$.acknowledgedAt') IS NULL
                AND NOT EXISTS (SELECT 1 FROM kv r WHERE r.key GLOB 'cli:inbox:' || ?1 || ':*'
                AND json_extract(r.value,'$.inReplyTo')=json_extract(q.value,'$.id'))";
            // Reproduce the pre-index scans too, so the benchmark compares the
            // same temp database and output without planner help from new indexes.
            conn.execute_batch("DROP INDEX mailbox_open_idx; DROP INDEX mailbox_reply_idx; DROP INDEX mailbox_ack_age_idx;")?;
            let before_reminder = scan(conn, old_reminder, "2026-01-03T00:00:00Z");
            let before_questions = scan(conn, old_questions, "sender");
            conn.execute_batch(include_str!("mailbox_indexes.sql"))?;
            let after_reminder = scan(conn, REMINDER_CANDIDATES, "2026-01-03T00:00:00Z");
            let after_questions = scan(conn, OPEN_QUESTIONS, "sender");
            eprintln!("5001 acknowledged / 2 open messages; (rows, VM steps, elapsed):\nreminder before={before_reminder:?}, after={after_reminder:?}\nquestions before={before_questions:?}, after={after_questions:?}");
            assert_eq!(before_reminder.0, after_reminder.0);
            assert_eq!(before_questions.0, 1);
            assert_eq!(after_questions.0, 1);
            assert!(after_reminder.1 < 200, "{after_reminder:?}");
            assert!(after_questions.1 < 200, "{after_questions:?}");
            assert!(before_reminder.1 > after_reminder.1 * 100);
            assert!(before_questions.1 > after_questions.1 * 100);
            // Index maintenance follows acknowledgement immediately.
            conn.execute("UPDATE kv SET value=json_set(value,'$.acknowledgedAt','2026-01-03T00:00:00Z') WHERE key=?1", [key("receiver", "question")])?;
            assert_eq!(scan(conn, OPEN_QUESTIONS, "sender").0, 0);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn mailbox_retention_preserves_recent_unhandled_and_needed_replies() {
        let (_dir, db) = database();
        db.with(|conn| {
            insert(conn, "expired", json!({"acknowledgedAt":"2026-01-01T00:00:00Z"}));
            insert(conn, "boundary", json!({"acknowledgedAt":"2026-01-02T00:00:00Z"}));
            insert(conn, "recent", json!({"acknowledgedAt":"2026-01-31T00:00:00Z"}));
            insert(conn, "unhandled", json!({"readAt":"2026-01-01T00:00:00Z","supersededBy":"new"}));
            insert(conn, "question", json!({"kind":"question"}));
            insert(conn, "reply", json!({"threadId":"sender","inReplyTo":"question","acknowledgedAt":"2026-01-01T00:00:00Z"}));
            // Non-mail KV records need not contain JSON.
            conn.execute("INSERT INTO kv(key,value) VALUES('other','not json')", [])?;
            cleanup_acknowledged(conn, "2026-02-01T00:00:00Z")?;
            assert!(get(conn, "receiver", "expired").is_err());
            for id in ["boundary", "recent", "unhandled", "question"] {
                assert!(get(conn, "receiver", id).is_ok(), "lost {id}");
            }
            assert!(get(conn, "sender", "reply").is_ok());
            assert_eq!(scan(conn, OPEN_QUESTIONS, "sender").0, 0);
            conn.execute("UPDATE kv SET value=json_set(value,'$.acknowledgedAt','2026-02-01T00:00:00Z') WHERE key=?1", [key("receiver", "question")])?;
            cleanup_acknowledged(conn, "2026-02-01T00:30:00Z")?;
            assert!(get(conn, "sender", "reply").is_ok(), "hourly cooldown");
            cleanup_acknowledged(conn, "2026-02-01T01:00:00Z")?;
            assert!(get(conn, "sender", "reply").is_err());
            assert!(get(conn, "receiver", "boundary").is_err());
            assert!(get(conn, "receiver", "question").is_ok());
            Ok(())
        }).unwrap();
    }
}
