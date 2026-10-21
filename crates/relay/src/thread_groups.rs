//! Device-authenticated parent edges and workspace IDs only; no
//! conversation/model/file data. The workspace ID lets a workspace grant reach
//! the conversations in that workspace.
use super::*;

pub(super) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS relay_thread_lineage(device_id TEXT NOT NULL,thread_id TEXT NOT NULL,parent_thread_id TEXT,PRIMARY KEY(device_id,thread_id));CREATE INDEX IF NOT EXISTS relay_lineage_parent ON relay_thread_lineage(device_id,parent_thread_id);")?;
    let has_workspace = conn
        .prepare("PRAGMA table_info(relay_thread_lineage)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .flatten()
        .any(|column| column == "workspace_id");
    if !has_workspace {
        conn.execute(
            "ALTER TABLE relay_thread_lineage ADD COLUMN workspace_id TEXT",
            [],
        )?;
    }
    Ok(())
}

pub(super) fn replace(conn: &Connection, device: &str, rows: &Value) -> Result<()> {
    let rows = rows
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("invalid lineage snapshot"))?;
    anyhow::ensure!(rows.len() <= 100_000, "lineage snapshot is too large");
    for row in rows {
        Uuid::parse_str(
            row["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("invalid thread id"))?,
        )?;
        if let Some(parent) = row["parentThreadId"].as_str() {
            Uuid::parse_str(parent)?;
        }
        if let Some(workspace) = row["workspaceId"].as_str() {
            anyhow::ensure!(
                !workspace.is_empty()
                    && workspace.len() <= 256
                    && !workspace.contains(['/', '?', '#', '%', '\\', '\n', '\r']),
                "invalid workspace id"
            );
        }
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM relay_thread_lineage WHERE device_id=?1",
        [device],
    )?;
    for row in rows {
        tx.execute("INSERT INTO relay_thread_lineage(device_id,thread_id,parent_thread_id,workspace_id) VALUES(?1,?2,?3,?4)",params![device,row["id"].as_str(),row["parentThreadId"].as_str(),row["workspaceId"].as_str()])?;
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn contains(conn: &Connection, device: &str, parent: &str, target: &str) -> bool {
    if parent == target {
        return true;
    }
    conn.query_row("WITH RECURSIVE ancestors(id,depth) AS (SELECT parent_thread_id,1 FROM relay_thread_lineage WHERE device_id=?1 AND thread_id=?2 UNION ALL SELECT t.parent_thread_id,a.depth+1 FROM relay_thread_lineage t JOIN ancestors a ON t.thread_id=a.id AND t.device_id=?1 WHERE a.depth<3) SELECT EXISTS(SELECT 1 FROM ancestors WHERE id=?3)",params![device,target,parent],|r|r.get(0)).unwrap_or(false)
}

/// The workspaces of a thread and of its ancestors up to three levels, nearest
/// first, as the device last reported them. Empty for unknown threads and for
/// devices that do not report workspaces yet.
pub(super) fn workspaces(conn: &Connection, device: &str, thread: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare(
        "WITH RECURSIVE chain(id,depth) AS (SELECT ?2,0 UNION ALL SELECT l.parent_thread_id,c.depth+1 FROM relay_thread_lineage l JOIN chain c ON l.thread_id=c.id AND l.device_id=?1 WHERE c.depth<3 AND l.parent_thread_id IS NOT NULL)
         SELECT l.workspace_id FROM chain c JOIN relay_thread_lineage l ON l.device_id=?1 AND l.thread_id=c.id WHERE l.workspace_id IS NOT NULL ORDER BY c.depth",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![device, thread], |row| row.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lineage_is_device_scoped_directional_and_replaced_atomically() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        let root = Uuid::new_v4().to_string();
        let child = Uuid::new_v4().to_string();
        let grandchild = Uuid::new_v4().to_string();
        let other = Uuid::new_v4().to_string();
        replace(&conn,"device",&json!([{"id":root},{"id":child,"parentThreadId":root},{"id":grandchild,"parentThreadId":child},{"id":other}])).unwrap();
        assert!(contains(&conn, "device", &root, &child));
        assert!(contains(&conn, "device", &root, &grandchild));
        assert!(!contains(&conn, "device", &child, &root));
        assert!(!contains(&conn, "device", &root, &other));
        assert!(!contains(&conn, "another-device", &root, &child));
        assert!(replace(&conn, "device", &json!([{"id":"invalid"}])).is_err());
        assert!(contains(&conn, "device", &root, &child));
        replace(&conn, "device", &json!([{"id":root}])).unwrap();
        assert!(!contains(&conn, "device", &root, &child));
    }

    #[test]
    fn workspaces_follow_the_thread_then_its_ancestors() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        // An older table gains the column once; a second run is a no-op.
        ensure_schema(&conn).unwrap();
        let [root, child, grandchild, other] = [(); 4].map(|_| Uuid::new_v4().to_string());
        replace(
            &conn,
            "device",
            &json!([
                {"id":root,"workspaceId":"shared"},
                {"id":child,"parentThreadId":root,"workspaceId":"elsewhere"},
                {"id":grandchild,"parentThreadId":child},
                {"id":other,"workspaceId":"private"}
            ]),
        )
        .unwrap();
        assert_eq!(workspaces(&conn, "device", &root), ["shared"]);
        assert_eq!(workspaces(&conn, "device", &child), ["elsewhere", "shared"]);
        assert_eq!(
            workspaces(&conn, "device", &grandchild),
            ["elsewhere", "shared"]
        );
        assert_eq!(workspaces(&conn, "device", &other), ["private"]);
        assert!(workspaces(&conn, "another-device", &root).is_empty());
        assert!(workspaces(&conn, "device", &Uuid::new_v4().to_string()).is_empty());
        for bad in ["", "a/b", "a?b", "a%2fb"] {
            assert!(replace(&conn, "device", &json!([{"id":root,"workspaceId":bad}])).is_err());
        }
        assert_eq!(workspaces(&conn, "device", &root), ["shared"]);
    }
}
