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
    // A delegate that mails its own result as its last act and then ends its
    // turn would be reported twice: by that mail and by the automatic turn
    // notification seconds later. The notification then only costs its
    // receiver a wake or a reminder turn, so it is stored as superseded by the
    // delegate's own report (kept for `inbox read` and `list --all`).
    let superseded_by: Option<String> = match from {
        Some(sender) if envelope.kind == "result" && text.starts_with(TURN_NOTIFICATION) => conn
            .query_row(
                "SELECT json_extract(value,'$.id') FROM kv WHERE key GLOB ?1
                   AND json_extract(value,'$.fromThreadId')=?2
                   AND json_extract(value,'$.kind')='result'
                   AND json_extract(value,'$.supersededBy') IS NULL
                   AND substr(json_extract(value,'$.text'),1,?4) != ?5
                   AND julianday(json_extract(value,'$.createdAt')) >= julianday(?3) - ?6
                 ORDER BY json_extract(value,'$.createdAt') DESC LIMIT 1",
                params![
                    format!("{}*", prefix(thread)),
                    sender,
                    now,
                    TURN_NOTIFICATION.len() as i64,
                    TURN_NOTIFICATION,
                    OWN_REPORT_WINDOW_SECONDS / 86_400.0
                ],
                |r| r.get(0),
            )
            .optional()?,
        _ => None,
    };
    conn.execute("INSERT INTO kv(key,value) VALUES(?1,?2)", params![key(thread,id),json!({"id":id,"threadId":thread,"fromThreadId":from,"text":text,"createdAt":now,"acknowledgedAt":null,"readAt":null,"remindAfter":remind_after.to_rfc3339(),"remindedAt":null,"subject":envelope.subject,"kind":envelope.kind,"inReplyTo":envelope.in_reply_to,"topicKey":envelope.topic_key,"supersededBy":superseded_by}).to_string()])?;
    Ok(())
}
/// The first line of an automatic turn-completion notification.
pub(crate) const TURN_NOTIFICATION: &str = "[Pockymoe turn notification]";
/// A result the same sender mailed this recently is its own report of the turn
/// the notification announces.
const OWN_REPORT_WINDOW_SECONDS: f64 = 60.0;
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
    AND json_extract(value,'$.id') NOT IN (SELECT value FROM json_each(?4))
    AND (?5 IS NULL OR json_extract(value,'$.inReplyTo')=?5)";

/// What `inbox wait` and a mail wake match: unacknowledged, current mail of a
/// mailbox, optionally from given threads, of given kinds, or answering one
/// message. `exclude` holds ids already seen (`--new`).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailFilter {
    #[serde(default)]
    pub from_thread_ids: Vec<String>,
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
}

impl MailFilter {
    pub(super) fn validate(&self) -> Result<()> {
        validate_kinds(&self.kinds)?;
        ensure!(
            self.from_thread_ids.len() <= 20,
            "filter on at most 20 sender threads"
        );
        if let Some(id) = &self.in_reply_to {
            ensure!(
                !id.trim().is_empty() && id.len() <= 128,
                "inReplyTo must be a message id"
            );
        }
        Ok(())
    }
}

/// Matching mail, oldest first, at most `limit`.
pub(super) fn matching(
    conn: &Connection,
    thread: &str,
    filter: &MailFilter,
    exclude: &[String],
    limit: usize,
) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT value FROM kv WHERE {FILTERED_UNREAD}
         ORDER BY json_extract(value,'$.createdAt'), json_extract(value,'$.id') LIMIT ?6"
    ))?;
    let rows = stmt
        .query_map(
            params![
                format!("{}*", prefix(thread)),
                serde_json::to_string(&filter.from_thread_ids)?,
                serde_json::to_string(&filter.kinds)?,
                serde_json::to_string(exclude)?,
                filter.in_reply_to,
                limit as i64
            ],
            |r| r.get::<_, String>(0),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.iter()
        .map(|raw| Ok(serde_json::from_str::<Value>(raw)?))
        .collect()
}

/// Ids of every matching message now, for `--new`.
pub(super) fn matching_ids(
    conn: &Connection,
    thread: &str,
    filter: &MailFilter,
) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT json_extract(value,'$.id') FROM kv WHERE {FILTERED_UNREAD}"
    ))?;
    let ids = stmt
        .query_map(
            params![
                format!("{}*", prefix(thread)),
                serde_json::to_string(&filter.from_thread_ids)?,
                serde_json::to_string(&filter.kinds)?,
                "[]",
                filter.in_reply_to
            ],
            |r| r.get(0),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(ids)
}

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
pub(super) fn mark_read(
    conn: &Connection,
    thread: &str,
    message: &mut Value,
    now: &str,
) -> Result<()> {
    if message["readAt"].is_null() {
        conn.execute(
            "UPDATE kv SET value=json_set(value,'$.readAt',?2) WHERE key=?1 AND json_extract(value,'$.readAt') IS NULL",
            params![key(thread, message["id"].as_str().unwrap_or_default()), now],
        )?;
        message["readAt"] = json!(now);
    }
    Ok(())
}
pub(super) fn get(conn: &Connection, thread: &str, id: &str) -> Result<Value> {
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
        let filter = MailFilter {
            from_thread_ids: from.to_vec(),
            kinds: kinds.to_vec(),
            in_reply_to: None,
        };
        self.inbox_wait_matching(thread, &filter, only_new, timeout)
            .await
    }

    /// `inbox_wait` with every filter, including the reply to one message.
    pub async fn inbox_wait_matching(
        &self,
        thread: &str,
        filter: &MailFilter,
        only_new: bool,
        timeout: std::time::Duration,
    ) -> Result<Value> {
        self.get_thread(thread)?;
        filter.validate()?;
        let started = std::time::Instant::now();
        let deadline = started + timeout;
        let mut events = self.bus.subscribe();
        // Capture every matching id, not just the first page. Exclude them in SQL
        // before LIMIT, otherwise --new is starved by a backlog of old messages.
        let seen: Vec<String> = if only_new {
            self.db.with(|c| matching_ids(c, thread, filter))?
        } else {
            vec![]
        };
        let unread = || {
            self.db.with(|c| {
                let mut messages = matching(c, thread, filter, &seen, 200)?;
                let now = now_rfc3339();
                for message in &mut messages {
                    mark_read(c, thread, message, &now)?;
                }
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
    fn a_turn_notification_after_the_senders_own_fresh_result_is_stored_superseded() {
        let (_dir, db) = database();
        db.with(|conn| {
            let result = |kind| Envelope {
                subject: Some("done"),
                kind,
                in_reply_to: None,
                topic_key: None,
            };
            let note =
                format!("{TURN_NOTIFICATION}\nThread worker, turn t ended with status completed.");
            let superseded = |id: &str| -> Option<String> {
                get(conn, "lead", id).unwrap()["supersededBy"]
                    .as_str()
                    .map(str::to_owned)
            };
            // The worker mails its result, then its turn ends four seconds later.
            store(
                conn,
                "lead",
                "own",
                Some("worker"),
                "remote.txt is ready",
                "2026-01-01T00:00:20Z",
                result("result"),
            )?;
            store(
                conn,
                "lead",
                "auto",
                Some("worker"),
                &note,
                "2026-01-01T00:00:24Z",
                result("result"),
            )?;
            assert_eq!(superseded("auto").as_deref(), Some("own"));
            assert_eq!(superseded("own"), None);
            // A result from long before the turn ended is not that turn's report.
            store(
                conn,
                "lead",
                "late",
                Some("worker"),
                &note,
                "2026-01-01T00:05:00Z",
                result("result"),
            )?;
            assert_eq!(superseded("late"), None);
            // Another sender's result, a status and an earlier notification do not count.
            store(
                conn,
                "lead",
                "other",
                Some("other-worker"),
                "mine is ready",
                "2026-01-01T00:09:50Z",
                result("result"),
            )?;
            store(
                conn,
                "lead",
                "status",
                Some("worker"),
                "halfway",
                "2026-01-01T00:09:55Z",
                result("status"),
            )?;
            store(
                conn,
                "lead",
                "again",
                Some("worker"),
                &note,
                "2026-01-01T00:10:00Z",
                result("result"),
            )?;
            assert_eq!(superseded("again"), None);
            store(
                conn,
                "lead",
                "twice",
                Some("worker"),
                &note,
                "2026-01-01T00:10:05Z",
                result("result"),
            )?;
            assert_eq!(
                superseded("twice"),
                None,
                "a notification never supersedes another"
            );
            Ok(())
        })
        .unwrap();
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
