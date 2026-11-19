//! Shared-resource visits: successful authorized resource reads/writes, excluding owner previews.
use super::*;

pub(super) fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS relay_share_activity (
            id TEXT PRIMARY KEY, resource_id TEXT NOT NULL, resource_type TEXT NOT NULL,
            user_id TEXT NOT NULL, username TEXT NOT NULL, kind TEXT NOT NULL, accessed_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS relay_share_activity_recent
            ON relay_share_activity(resource_type, resource_id, accessed_at DESC);",
    )
}

fn request_kind(method: &Method, path: &str) -> Option<&'static str> {
    let path = path.split('?').next().unwrap_or(path);
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    match (method, parts.as_slice()) {
        (&Method::GET, ["api", "workspaces"]) => Some("open_device"),
        (&Method::GET, ["api", "threads", _]) => Some("open_thread"),
        (&Method::POST, ["api", "threads", "start"]) => Some("create_thread"),
        (&Method::POST, ["api", "threads", _, "prompt" | "steer"]) => Some("send_prompt"),
        (&Method::GET, ["api", "workspaces", _, "files", "preview" | "raw" | "download"]) => {
            Some("read_workspace_file")
        }
        (
            &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE,
            ["api", "workspaces", _, "files", ..],
        ) => Some("write_workspace_file"),
        _ => None,
    }
}

pub(super) async fn record_response(
    state: &AppState,
    access: &EffectiveAccess,
    user_id: &str,
    method: &Method,
    path: &str,
    status: StatusCode,
) {
    if !status.is_success() || access.kind != "shared" {
        return;
    }
    let Some(kind) = request_kind(method, path) else {
        return;
    };
    let conn = state.store.conn.lock().await;
    let Some(user) = load_user_by_id(&conn, user_id) else {
        return;
    };
    let resource = access
        .share_id
        .as_deref()
        .map(|id| ("share", id))
        .or_else(|| access.grant_id.as_deref().map(|id| ("grant", id)));
    if let Some((resource_type, resource_id)) = resource {
        if let Err(error) = record(
            &conn,
            resource_type,
            resource_id,
            user_id,
            &user.username,
            kind,
            &now_rfc3339(),
        ) {
            tracing::warn!(%error, "Unable to record shared access");
        }
    }
}

fn record(
    conn: &Connection,
    resource_type: &str,
    resource_id: &str,
    user_id: &str,
    username: &str,
    kind: &str,
    at: &str,
) -> rusqlite::Result<()> {
    // Coalesce initial fetches/reconnects and detail refreshes. A continuously
    // open page contributes at most one sample per kind every five minutes.
    let recent: Option<(String, String)> = conn.query_row(
        "SELECT id,accessed_at FROM relay_share_activity WHERE resource_type=?1 AND resource_id=?2 AND user_id=?3 AND kind=?4 ORDER BY accessed_at DESC LIMIT 1",
        params![resource_type, resource_id, user_id, kind], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    if recent.is_some_and(|(_, previous)| {
        chrono::DateTime::parse_from_rfc3339(&previous)
            .ok()
            .zip(chrono::DateTime::parse_from_rfc3339(at).ok())
            .is_some_and(|(previous, current)| (current - previous).num_seconds() < 300)
    }) {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO relay_share_activity VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            Uuid::new_v4().to_string(),
            resource_id,
            resource_type,
            user_id,
            username,
            kind,
            at
        ],
    )?;
    // Bounded per-share history; no paths, content, IP addresses or credentials are stored.
    conn.execute("DELETE FROM relay_share_activity WHERE resource_type=?1 AND resource_id=?2 AND id NOT IN (SELECT id FROM relay_share_activity WHERE resource_type=?1 AND resource_id=?2 ORDER BY accessed_at DESC LIMIT 100)", params![resource_type, resource_id])?;
    Ok(())
}

pub(super) fn fields(
    conn: &Connection,
    resource_type: &str,
    resource_id: &str,
) -> (Value, Value, Vec<Value>) {
    let events = (|| -> rusqlite::Result<Vec<Value>> {
        let mut stmt = conn.prepare_cached("SELECT id,user_id,username,kind,accessed_at FROM relay_share_activity WHERE resource_type=?1 AND resource_id=?2 ORDER BY accessed_at DESC LIMIT 20")?;
        let rows = stmt.query_map(params![resource_type, resource_id], |row| Ok(json!({
            "id": row.get::<_, String>(0)?, "userId": row.get::<_, String>(1)?,
            "username": row.get::<_, String>(2)?, "kind": row.get::<_, String>(3)?,
            "accessedAt": row.get::<_, String>(4)?,
            (if resource_type == "share" { "shareId" } else { "grantId" }): resource_id,
        })))?;
        rows.collect()
    })().unwrap_or_default();
    (
        events
            .first()
            .map(|e| e["accessedAt"].clone())
            .unwrap_or(Value::Null),
        events
            .first()
            .map(|e| e["username"].clone())
            .unwrap_or(Value::Null),
        events,
    )
}
