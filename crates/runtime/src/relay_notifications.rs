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
    if !matches!(status, "completed" | "failed" | "interrupted") {
        return Ok(());
    }
    let workspace: String = conn.query_row(
        "SELECT workspace_id FROM threads WHERE id=?1",
        [thread],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO kv(key,value) VALUES (?1,?2)",
        params![
            format!("relay:notice:{turn}"),
            json!({"threadId":thread,"turnId":turn,"workspaceId":workspace,"status":status,"occurredAt":now}).to_string()
        ],
    )?;
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
    pub fn acknowledge_relay_notification(&self, turn: &str) -> Result<()> {
        uuid::Uuid::parse_str(turn)?;
        self.db.with(|conn| {
            conn.execute(
                "DELETE FROM kv WHERE key=?1",
                [format!("relay:notice:{turn}")],
            )?;
            Ok(())
        })
    }
}
