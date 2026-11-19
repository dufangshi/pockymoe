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

/// Attach up to 20 visits per listed resource in one query. Cross joining the
/// listed IDs first lets SQLite stop each recent-activity index scan at 20.
pub(super) fn attach(conn: &Connection, resource_type: &str, resources: &mut [Value]) {
    if resources.is_empty() {
        return;
    }
    let resource_key = if resource_type == "share" {
        "shareId"
    } else {
        "grantId"
    };
    let ids: Vec<_> = resources
        .iter()
        .map(|r| r["id"].as_str().unwrap_or_default())
        .collect();
    let load = || -> rusqlite::Result<std::collections::HashMap<String, Vec<Value>>> {
        let mut stmt = conn.prepare_cached(
            "SELECT a.resource_id,a.id,a.user_id,a.username,a.kind,a.accessed_at
             FROM json_each(?1) listed CROSS JOIN relay_share_activity a
             WHERE a.rowid IN (
               SELECT rowid FROM relay_share_activity
               WHERE resource_type=?2 AND resource_id=listed.value
               ORDER BY accessed_at DESC LIMIT 20
             ) ORDER BY a.resource_id,a.accessed_at DESC",
        )?;
        let mut rows = stmt.query(params![json!(ids).to_string(), resource_type])?;
        let mut events = std::collections::HashMap::<String, Vec<Value>>::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let event = json!({
                (resource_key): id, "id": row.get::<_, String>(1)?,
                "userId": row.get::<_, String>(2)?, "username": row.get::<_, String>(3)?,
                "kind": row.get::<_, String>(4)?, "accessedAt": row.get::<_, String>(5)?
            });
            events.entry(id).or_default().push(event);
        }
        Ok(events)
    };
    let mut by_id = load().unwrap_or_default();
    for resource in resources {
        let events = by_id
            .remove(resource["id"].as_str().unwrap_or_default())
            .unwrap_or_default();
        if let Some(last) = events.first() {
            resource["lastAccessedAt"] = last["accessedAt"].clone();
            resource["lastAccessedByUsername"] = last["username"].clone();
        } else {
            resource["lastAccessedAt"] = Value::Null;
            resource["lastAccessedByUsername"] = Value::Null;
        }
        resource["accessEvents"] = Value::Array(events);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_activity_is_scoped_ordered_and_limited_per_resource() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        for (kind, id) in [("share", "a"), ("grant", "a"), ("share", "b")] {
            for i in 0..25 {
                let at = format!("2026-11-19T09:{i:02}:00.000Z");
                conn.execute(
                    "INSERT INTO relay_share_activity VALUES (?1,?2,?3,'user',?4,'open_thread',?5)",
                    params![format!("{kind}-{id}-{i}"), id, kind, kind, at],
                )
                .unwrap();
            }
        }
        let mut rows = [json!({"id":"b"}), json!({"id":"none"}), json!({"id":"a"})];
        attach(&conn, "share", &mut rows);
        for row in [&rows[0], &rows[2]] {
            let events = row["accessEvents"].as_array().unwrap();
            assert_eq!(events.len(), 20);
            assert_eq!(events[0]["shareId"], row["id"]);
            assert_eq!(events[0]["accessedAt"], row["lastAccessedAt"]);
            assert_eq!(events[19]["accessedAt"], "2026-11-19T09:05:00.000Z");
            assert_eq!(row["lastAccessedByUsername"], "share");
        }
        assert_eq!(rows[1]["accessEvents"], json!([]));
        assert_eq!(rows[1]["lastAccessedAt"], Value::Null);
    }
}
