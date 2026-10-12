use crate::Supervisor;
use anyhow::{ensure, Result};
use pockymoe_protocol::now_rfc3339;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

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
