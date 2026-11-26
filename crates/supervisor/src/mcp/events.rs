//! Durable milestones only: no streamed text or raw tool arguments.
use super::{store, tools::Grant};
use anyhow::Result;
use pockymoe_runtime::Supervisor;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, OnceLock},
};

pub fn start(state: &Arc<Supervisor>) {
    static STARTED: OnceLock<Mutex<HashSet<std::path::PathBuf>>> = OnceLock::new();
    if !STARTED
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(state.config.database_url.clone())
    {
        return;
    }
    let weak = Arc::downgrade(state);
    let mut rx = state.bus.subscribe();
    tokio::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let kind = serde_json::to_value(&event.event_type).unwrap_or_default();
            let name = kind.as_str().unwrap_or("");
            if !matches!(
                name,
                "thread.turn.completed"
                    | "thread.turn.failed"
                    | "thread.request.created"
                    | "thread.request.resolved"
            ) && !event.payload["reason"]
                .as_str()
                .is_some_and(|r| r.starts_with("pr") || r.starts_with("pullRequest"))
            {
                continue;
            }
            let Some(state) = weak.upgrade() else { break };
            let _ = store::transaction(&state, |db| {
                db.execute_batch("CREATE TABLE IF NOT EXISTS milestones(id INTEGER PRIMARY KEY AUTOINCREMENT, thread TEXT NOT NULL, value TEXT NOT NULL)")?;
                db.execute(
                    "INSERT INTO milestones(thread,value) VALUES(?,?)",
                    rusqlite::params![
                        event.thread_id,
                        milestone(&kind, &event.thread_id, &event.timestamp, &event.payload)
                            .to_string()
                    ],
                )?;
                db.execute("DELETE FROM milestones WHERE id <= (SELECT COALESCE(MAX(id),0)-1000 FROM milestones)",[])?;
                Ok(())
            });
        }
    });
}
/// Identifiers and outcome only: never message text, tool input or output.
fn milestone(kind: &Value, thread: &str, time: &str, payload: &Value) -> Value {
    let mut value = json!({"type":kind,"threadId":thread,"time":time});
    for key in ["turnId", "requestId", "status", "reason"] {
        if let Some(v) = payload[key].as_str().filter(|v| v.len() <= 200) {
            value[key] = json!(v);
        }
    }
    let request = &payload["request"];
    for (from, to) in [
        ("id", "requestId"),
        ("turnId", "turnId"),
        ("kind", "requestKind"),
    ] {
        if let Some(v) = request[from].as_str().filter(|v| v.len() <= 200) {
            value[to] = json!(v);
        }
    }
    value
}
pub fn poll(state: &Supervisor, grant: &Grant, cursor: &str) -> Result<Value> {
    let cursor = if cursor.is_empty() {
        0
    } else {
        cursor.parse::<i64>()?
    };
    store::transaction(state, |db| {
        db.execute_batch("CREATE TABLE IF NOT EXISTS milestones(id INTEGER PRIMARY KEY AUTOINCREMENT, thread TEXT NOT NULL, value TEXT NOT NULL)")?;
        let mut stmt =
            db.prepare("SELECT id,thread,value FROM milestones WHERE id>? ORDER BY id LIMIT 1000")?;
        let rows = stmt.query_map([cursor], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut next = cursor;
        let mut events = vec![];
        for r in rows {
            let (id, thread, value) = r?;
            next = id;
            if let Ok(t) = state.get_thread(&thread) {
                if grant.allows_workspace(&t.workspace_id) {
                    events.push(serde_json::from_str::<Value>(&value)?);
                }
            }
        }
        Ok(json!({"cursor":next.to_string(),"events":events}))
    })
}
