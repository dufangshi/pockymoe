//! Device-side durable lifecycle outbox. Relay ACK follows durable fan-out.
use crate::Supervisor;
use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub(crate) fn record(
    conn: &Connection,
    thread: &str,
    turn: &str,
    status: &str,
    now: &str,
) -> Result<()> {
    if !matches!(status, "completed" | "failed") {
        return Ok(());
    }
    conn.execute(
        "INSERT OR IGNORE INTO kv(key,value) VALUES (?1,?2)",
        params![
            format!("relay:notice:{turn}"),
            json!({"threadId":thread,"turnId":turn,"status":status,"occurredAt":now}).to_string()
        ],
    )?;
    Ok(())
}
pub(crate) fn record_request(
    conn: &Connection,
    event: &remote_codex_protocol::ThreadEventEnvelope,
) -> Result<()> {
    let request = &event.payload["request"];
    let (Some(id), Some(turn)) = (request["id"].as_str(), request["turnId"].as_str()) else {
        return Ok(());
    };
    // Only a live turn can be waiting. Ignore delayed/replayed creation events.
    let waiting: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM thread_turns WHERE id=?1 AND thread_id=?2 AND status IN ('inProgress','recovering'))", params![turn,event.thread_id], |r|r.get(0))?;
    if waiting {
        conn.execute("INSERT OR IGNORE INTO kv(key,value) VALUES (?1,?2)", params![format!("relay:notice:{id}"),json!({"threadId":event.thread_id,"turnId":turn,"requestId":id,"status":"awaiting_input","occurredAt":event.timestamp}).to_string()])?;
    }
    Ok(())
}

impl Supervisor {
    pub fn pending_relay_notifications(&self) -> Result<Vec<Value>> {
        self.db.with(|conn| {
            let cutoff=(chrono::Utc::now()-chrono::Duration::days(1)).to_rfc3339();
            conn.execute("DELETE FROM kv WHERE key GLOB 'relay:notice:*' AND json_extract(value,'$.occurredAt')<?1",[cutoff])?;
            let mut stmt=conn.prepare("SELECT value FROM kv WHERE key GLOB 'relay:notice:*' ORDER BY key LIMIT 100")?;
            let rows=stmt.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().map(|raw|Ok(serde_json::from_str(&raw)?)).collect()
        })
    }
    pub fn acknowledge_relay_notification(&self, event_key: &str) -> Result<()> {
        anyhow::ensure!(
            !event_key.is_empty() && event_key.len() <= 256,
            "Invalid notification ID"
        );
        self.db.with(|conn| {
            conn.execute(
                "DELETE FROM kv WHERE key=?1",
                [format!("relay:notice:{event_key}")],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use remote_codex_protocol::ThreadEventEnvelope;

    #[test]
    fn awaiting_input_outbox_is_durable_deduplicated_and_live_only() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = crate::Database::open(&dir.path().join("isolated.sqlite"))?;
        let event = ThreadEventEnvelope {
            event_type: "thread.request.created".into(),
            thread_id: "thread".into(),
            timestamp: remote_codex_protocol::now_rfc3339(),
            payload: json!({"request":{"id":"perm-request","turnId":"turn","kind":"permissionRequest","title":"Private command"}}),
        };
        db.with(|conn| {
            conn.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES ('turn','thread','inProgress',1)", [])?;
            record_request(conn, &event)?;
            record_request(conn, &event)?;
            let count: i64 = conn.query_row("SELECT count(*) FROM kv WHERE key GLOB 'relay:notice:*'", [], |r|r.get(0))?;
            assert_eq!(count, 1);
            let raw: String = conn.query_row("SELECT value FROM kv WHERE key='relay:notice:perm-request'", [], |r|r.get(0))?;
            let value: Value = serde_json::from_str(&raw)?;
            assert_eq!(value["status"], "awaiting_input");
            assert_eq!(value["requestId"], "perm-request");
            assert!(!raw.contains("Private command"));
            // A terminal notice for the same turn is a distinct event.
            record(conn, "thread", "turn", "completed", &event.timestamp)?;
            assert_eq!(conn.query_row("SELECT count(*) FROM kv WHERE key GLOB 'relay:notice:*'", [], |r|r.get::<_,i64>(0))?, 2);
            conn.execute("DELETE FROM kv WHERE key='relay:notice:perm-request'", [])?;
            conn.execute("UPDATE thread_turns SET status='completed'", [])?;
            record_request(conn, &event)?;
            assert_eq!(conn.query_row("SELECT count(*) FROM kv WHERE key='relay:notice:perm-request'", [], |r|r.get::<_,i64>(0))?, 0);
            Ok(())
        })
    }
}
