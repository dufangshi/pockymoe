//! The standalone owner uses the same SQLite kv as local access credentials.
use crate::Supervisor;
use anyhow::{ensure, Result};
use pockymoe_protocol::{should_notify_turn, NotificationSettings};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

const SETTINGS: &str = "local:owner:notifications";
fn subscription_key(thread: &str, turn: &str) -> String {
    format!("local:owner:turn-subscription:{thread}:{turn}")
}
fn settings(conn: &Connection) -> Result<NotificationSettings> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM kv WHERE key=?1", [SETTINGS], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(raw
        .map(|v| serde_json::from_str(&v))
        .transpose()?
        .unwrap_or_default())
}

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
    let key = subscription_key(thread, turn);
    let subscribed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM kv WHERE key=?1)",
        [&key],
        |r| r.get(0),
    )?;
    conn.execute("DELETE FROM kv WHERE key=?1", [key])?;
    if status == "interrupted" {
        return Ok(());
    }
    let workspace: String = conn.query_row(
        "SELECT workspace_id FROM threads WHERE id=?1",
        [thread],
        |r| r.get(0),
    )?;
    let event = json!({"id":turn,"threadId":thread,"workspaceId":workspace,"status":status,"subscribed":subscribed,"occurredAt":now});
    conn.execute(
        "INSERT OR IGNORE INTO kv(key,value) VALUES (?1,?2)",
        params![format!("local:owner:notice:{turn}"), event.to_string()],
    )?;
    Ok(())
}

impl Supervisor {
    pub fn local_notification_settings(&self) -> Result<NotificationSettings> {
        self.db.with(settings)
    }
    pub fn save_local_notification_settings(&self, value: &NotificationSettings) -> Result<()> {
        ensure!(value.valid(), "Invalid notification settings");
        self.db.set_kv(SETTINGS, &serde_json::to_string(value)?)
    }
    pub fn local_turn_subscription(&self, thread: &str, turn: &str) -> Result<bool> {
        self.db.with(|conn| {
            Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM kv k JOIN thread_turns t ON t.thread_id=?1 AND t.id=?2 AND t.status='inProgress' WHERE k.key=?3)", params![thread,turn,subscription_key(thread,turn)], |r| r.get(0))?)
        })
    }
    pub fn set_local_turn_subscription(
        &self,
        thread: &str,
        turn: &str,
        subscribed: bool,
    ) -> Result<()> {
        uuid::Uuid::parse_str(thread)?;
        uuid::Uuid::parse_str(turn)?;
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM thread_turns WHERE thread_id=?1 AND id=?2 AND status='inProgress')", params![thread,turn], |r| r.get(0))?;
            ensure!(active, "This turn has already ended");
            let key = subscription_key(thread,turn);
            if subscribed {
                tx.execute("INSERT OR IGNORE INTO kv(key,value) VALUES (?1,'true')", [key])?;
            } else {
                tx.execute("DELETE FROM kv WHERE key=?1", [key])?;
            }
            tx.commit()?;
            Ok(())
        })
    }
    pub fn local_notifications(&self) -> Result<Vec<Value>> {
        self.db.with(|conn| {
            let cutoff = (chrono::Utc::now()-chrono::Duration::days(1)).to_rfc3339();
            conn.execute("DELETE FROM kv WHERE key GLOB 'local:owner:notice:*' AND json_extract(value,'$.occurredAt')<?1",[cutoff])?;
            // Keep subscriptions while a disconnected turn is being recovered.
            conn.execute("DELETE FROM kv WHERE key GLOB 'local:owner:turn-subscription:*' AND NOT EXISTS(SELECT 1 FROM thread_turns t WHERE t.status IN ('inProgress','recovering') AND kv.key='local:owner:turn-subscription:'||t.thread_id||':'||t.id)",[])?;
            let settings = settings(conn)?;
            let mut stmt = conn.prepare("SELECT k.value,t.title FROM kv k JOIN threads t ON t.id=json_extract(k.value,'$.threadId') WHERE k.key GLOB 'local:owner:notice:*' ORDER BY json_extract(k.value,'$.occurredAt') DESC")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows.into_iter().filter_map(|(raw,title)| {
                let e: Value = serde_json::from_str(&raw).ok()?;
                if !should_notify_turn(&settings,"local",e["workspaceId"].as_str(),e["subscribed"]==true,e["status"].as_str()?) { return None; }
                Some(json!({"id":e["id"],"title":title,"href":format!("/threads/{}",e["threadId"].as_str()?),"occurredAt":e["occurredAt"]}))
            }).take(100).collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_subscription_is_consumed_by_its_turn_only_and_survives_notice_reads() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE kv(key TEXT PRIMARY KEY,value TEXT); CREATE TABLE threads(id TEXT,workspace_id TEXT); INSERT INTO threads VALUES ('thread','work');").unwrap();
        conn.execute(
            "INSERT INTO kv VALUES (?1,'true')",
            [subscription_key("thread", "one")],
        )
        .unwrap();
        record(&conn, "thread", "other", "completed", "now").unwrap();
        assert!(conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM kv WHERE key=?1)",
                [subscription_key("thread", "one")],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
        record(&conn, "thread", "one", "failed", "now").unwrap();
        let raw: String = conn
            .query_row(
                "SELECT value FROM kv WHERE key='local:owner:notice:one'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let event: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(event["workspaceId"], "work");
        assert_eq!(event["subscribed"], true);
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM kv WHERE key GLOB 'local:owner:turn-subscription:*'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        conn.execute(
            "INSERT INTO kv VALUES (?1,'true')",
            [subscription_key("thread", "interrupted")],
        )
        .unwrap();
        record(&conn, "thread", "interrupted", "interrupted", "now").unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM kv WHERE key GLOB 'local:owner:notice:*'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
    }
}
