//! Account-scoped navigation. References never grant access to the underlying thread.
use super::*;

pub(super) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS relay_thread_navigation (
        user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
        device_id TEXT NOT NULL REFERENCES relay_devices(id) ON DELETE CASCADE,
        thread_id TEXT NOT NULL, title TEXT NOT NULL, workspace_label TEXT NOT NULL, workspace_id TEXT,
        favorite INTEGER NOT NULL DEFAULT 0, visited_at TEXT NOT NULL,
        PRIMARY KEY(user_id,device_id,thread_id));",
    )?;
    let has_read_marker: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('relay_thread_navigation') WHERE name='read_completed_at')", [], |row| row.get(0),
    )?;
    if !has_read_marker {
        conn.execute_batch(
            "ALTER TABLE relay_thread_navigation ADD COLUMN read_completed_at TEXT",
        )?;
    }
    // The account's own shortcut order; NULL until the user first reorders, so
    // shortcuts keep listing by recency until then.
    let has_favorite_order: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('relay_thread_navigation') WHERE name='favorite_order')", [], |row| row.get(0),
    )?;
    if !has_favorite_order {
        conn.execute_batch(
            "ALTER TABLE relay_thread_navigation ADD COLUMN favorite_order INTEGER",
        )?;
    }
    Ok(())
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/relay/account/workbench",
            get(load).post(save).delete(remove),
        )
        .route(
            "/relay/account/workbench/shortcuts",
            axum::routing::put(reorder),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShortcutOrder {
    order: Vec<RemoveReference>,
}

/// Puts the account's shortcuts in the given order. Only the account's own
/// favorite references change; anything else in the list is ignored.
async fn reorder(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    Json(body): Json<ShortcutOrder>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    if body.order.len() > 200
        || body.order.iter().any(|reference| {
            Uuid::parse_str(&reference.thread_id).is_err() || reference.device_id.len() > 1024
        })
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match set_shortcut_order(&conn, &user.id, &body.order).and_then(|_| snapshot(&conn, &user.id)) {
        Ok(value) => Json(value).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn set_shortcut_order(conn: &Connection, user: &str, order: &[RemoveReference]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for (index, reference) in order.iter().enumerate() {
        tx.execute(
            "UPDATE relay_thread_navigation SET favorite_order=?4
            WHERE user_id=?1 AND device_id=?2 AND thread_id=?3 AND favorite=1",
            params![user, reference.device_id, reference.thread_id, index as i64],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoveReference {
    device_id: String,
    thread_id: String,
}

async fn remove(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    Json(reference): Json<RemoveReference>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    // Removing an account's own bookmark does not mutate the remote thread or
    // another user's navigation, even when access to that device was revoked.
    match remove_reference(&conn, &user.id, &reference).and_then(|_| snapshot(&conn, &user.id)) {
        Ok(value) => Json(value).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn remove_reference(conn: &Connection, user: &str, reference: &RemoveReference) -> Result<()> {
    conn.execute(
        "DELETE FROM relay_thread_navigation WHERE user_id=?1 AND device_id=?2 AND thread_id=?3",
        params![user, reference.device_id, reference.thread_id],
    )?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Visit {
    device_id: String,
    thread_id: String,
    title: String,
    workspace_label: String,
    workspace_id: Option<String>,
    favorite: Option<bool>,
    read_completed_at: Option<String>,
}

fn upsert(conn: &Connection, user: &str, visit: &Visit) -> Result<()> {
    // A new shortcut goes after the ordered ones; one that is unstarred leaves the order.
    conn.execute("INSERT INTO relay_thread_navigation(user_id,device_id,thread_id,title,workspace_label,favorite,visited_at,workspace_id,read_completed_at,favorite_order)
        VALUES (?1,?2,?3,?4,?5,COALESCE(?6,0),?7,?8,?9,
          CASE WHEN ?6=1 THEN (SELECT MAX(favorite_order)+1 FROM relay_thread_navigation WHERE user_id=?1 AND favorite=1) END)
        ON CONFLICT(user_id,device_id,thread_id) DO UPDATE SET title=excluded.title,
        workspace_label=excluded.workspace_label,workspace_id=excluded.workspace_id,favorite=COALESCE(?6,relay_thread_navigation.favorite),visited_at=excluded.visited_at,
        favorite_order=CASE WHEN ?6=0 THEN NULL
          WHEN ?6=1 AND relay_thread_navigation.favorite=0 THEN (SELECT MAX(favorite_order)+1 FROM relay_thread_navigation WHERE user_id=?1 AND favorite=1)
          ELSE relay_thread_navigation.favorite_order END,
        read_completed_at=CASE WHEN ?9 IS NOT NULL AND (relay_thread_navigation.read_completed_at IS NULL OR julianday(?9)>julianday(relay_thread_navigation.read_completed_at)) THEN ?9 ELSE relay_thread_navigation.read_completed_at END",
        params![user,visit.device_id,visit.thread_id,visit.title,visit.workspace_label,visit.favorite,now_rfc3339(),visit.workspace_id,visit.read_completed_at])?;
    conn.execute("DELETE FROM relay_thread_navigation WHERE user_id=?1 AND favorite=0 AND rowid NOT IN
        (SELECT rowid FROM relay_thread_navigation WHERE user_id=?1 ORDER BY visited_at DESC LIMIT 100)", [user])?;
    Ok(())
}

fn accessible(
    conn: &Connection,
    user: &str,
    device: &str,
    thread: &str,
    workspace: Option<&str>,
) -> bool {
    effective_access(conn, user, device, Some(thread), None).is_some()
        || workspace.is_some_and(|workspace| {
            effective_access(conn, user, device, Some(thread), Some(workspace))
                .is_some_and(|access| access.scope == "workspace")
        })
}

fn snapshot(conn: &Connection, user: &str) -> Result<Value> {
    let mut stmt = conn.prepare("SELECT n.device_id,n.thread_id,n.title,n.workspace_label,n.favorite,n.visited_at,d.name,n.workspace_id,n.read_completed_at,n.favorite_order
        FROM relay_thread_navigation n JOIN relay_devices d ON d.id=n.device_id WHERE n.user_id=?1 ORDER BY n.visited_at DESC LIMIT 300")?;
    let rows = stmt.query_map([user], |r| Ok(json!({
        "deviceId":r.get::<_,String>(0)?, "threadId":r.get::<_,String>(1)?, "title":r.get::<_,String>(2)?,
        "workspaceLabel":r.get::<_,String>(3)?, "favorite":r.get::<_,bool>(4)?, "visitedAt":r.get::<_,String>(5)?, "deviceName":r.get::<_,String>(6)?, "workspaceId":r.get::<_,Option<String>>(7)?, "readCompletedAt":r.get::<_,Option<String>>(8)?,
        "favoriteOrder":r.get::<_,Option<i64>>(9)?
    })))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let threads: Vec<Value> = rows
        .into_iter()
        .filter(|r| {
            accessible(
                conn,
                user,
                r["deviceId"].as_str().unwrap_or(""),
                r["threadId"].as_str().unwrap_or(""),
                r["workspaceId"].as_str(),
            )
        })
        .collect();
    let mut stmt = conn.prepare("SELECT e.id,e.device_id,e.payload,e.created_at FROM relay_push_events e
        WHERE e.user_id=?1 OR EXISTS (SELECT 1 FROM relay_thread_navigation n WHERE n.user_id=?1 AND n.device_id=e.device_id)
        ORDER BY e.created_at DESC")?;
    let events = stmt
        .query_map([user], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let notifications: Vec<Value> = events.into_iter().filter_map(|(id,device,raw,at)| {
        let event: Value = serde_json::from_str(&raw).ok()?;
        if !notification_scope::allowed(conn,user,&device,&event).ok()? { return None; }
        let href = event["url"].as_str()?;
        let thread = href.rsplit('/').next()?;
        let reference = threads.iter().find(|r| r["deviceId"]==device && r["threadId"]==thread);
        // Navigation workspace IDs are supplied by the browser, not proof that an
        // event's thread belongs to that workspace. Require an independently
        // verifiable device/thread grant before exposing server-side events.
        if !accessible(conn,user,&device,thread,None) { return None; }
        let title = reference
            .and_then(|r|r["title"].as_str()).unwrap_or("Thread");
        let failed = event["body"].as_str().unwrap_or("").contains("failed");
        Some(json!({"id":id,"title":format!("{title} {}",if failed {"failed"} else {"completed"}),"href":href,
            "occurredAt":chrono::DateTime::from_timestamp_millis(at)?.to_rfc3339()}))
    }).take(100).collect();
    Ok(json!({"threads":threads,"notifications":notifications}))
}

async fn load(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    match snapshot(&conn, &user.id) {
        Ok(value) => Json(value).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn save(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    Json(visit): Json<Visit>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    if Uuid::parse_str(&visit.thread_id).is_err()
        || visit
            .read_completed_at
            .as_ref()
            .is_some_and(|at| chrono::DateTime::parse_from_rfc3339(at).is_err())
        || visit.title.len() > 1024
        || visit.workspace_label.len() > 1024
        || visit
            .workspace_id
            .as_ref()
            .is_some_and(|id| id.len() > 1024)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if !accessible(
        &conn,
        &user.id,
        &visit.device_id,
        &visit.thread_id,
        visit.workspace_id.as_deref(),
    ) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let count: i64 = conn.query_row("SELECT count(*) FROM relay_thread_navigation WHERE user_id=?1 AND favorite=1 AND NOT(device_id=?2 AND thread_id=?3)",params![user.id,visit.device_id,visit.thread_id],|r|r.get(0)).unwrap_or(300);
    if visit.favorite == Some(true) && count >= 200 {
        return StatusCode::CONFLICT.into_response();
    }
    match upsert(&conn, &user.id, &visit).and_then(|_| snapshot(&conn, &user.id)) {
        Ok(value) => Json(value).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removing_reference_is_scoped_to_account_device_and_thread() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE relay_thread_navigation(user_id TEXT,device_id TEXT,thread_id TEXT);
            INSERT INTO relay_thread_navigation VALUES ('a','mac','one'),('a','wsl','one'),('b','mac','one'),('a','mac','two');").unwrap();
        remove_reference(
            &conn,
            "a",
            &RemoveReference {
                device_id: "mac".into(),
                thread_id: "one".into(),
            },
        )
        .unwrap();
        let rows: Vec<(String, String, String)> = conn
            .prepare("SELECT * FROM relay_thread_navigation ORDER BY user_id,device_id,thread_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                ("a".into(), "mac".into(), "two".into()),
                ("a".into(), "wsl".into(), "one".into()),
                ("b".into(), "mac".into(), "one".into())
            ]
        );
    }
    #[test]
    fn navigation_and_notifications_disappear_when_thread_sharing_is_revoked() {
        let store = RelayStore::open(PathBuf::from(":memory:"), "workbench-test".into()).unwrap();
        let conn = store.conn.try_lock().unwrap();
        conn.execute_batch("INSERT INTO relay_users VALUES ('owner','o@test','owner','user',1,NULL,'now','salt','hash'),('reader','r@test','reader','user',1,NULL,'now','salt','hash'),('stranger','s@test','stranger','user',1,NULL,'now','salt','hash');
            INSERT INTO relay_devices VALUES ('mac','owner','Mac',NULL,'hash','preview','now');
            INSERT INTO relay_shares(id,owner_user_id,target_user_id,device_id,thread_id,created_at) VALUES ('share','owner','reader','mac','thread','now');").unwrap();
        conn.execute(
            "INSERT INTO relay_notification_preferences VALUES ('reader',?1)",
            [json!({"scope":"all"}).to_string()],
        )
        .unwrap();
        let visit = Visit {
            device_id: "mac".into(),
            thread_id: "thread".into(),
            title: "Shared review".into(),
            workspace_label: "project".into(),
            workspace_id: Some("project".into()),
            favorite: Some(true),
            read_completed_at: None,
        };
        upsert(&conn, "reader", &visit).unwrap();
        conn.execute(
            "INSERT INTO relay_push_events VALUES ('event','mac','owner',?1,?2)",
            params![
                json!({"url":"/devices/mac/threads/thread","body":"completed"}).to_string(),
                chrono::Utc::now().timestamp_millis()
            ],
        )
        .unwrap();
        let allowed = snapshot(&conn, "reader").unwrap();
        assert_eq!(allowed["threads"].as_array().unwrap().len(), 1);
        assert_eq!(allowed["notifications"].as_array().unwrap().len(), 1);
        assert!(snapshot(&conn, "stranger").unwrap()["threads"]
            .as_array()
            .unwrap()
            .is_empty());
        conn.execute(
            "UPDATE relay_shares SET revoked_at='now' WHERE id='share'",
            [],
        )
        .unwrap();
        let revoked = snapshot(&conn, "reader").unwrap();
        assert!(revoked["threads"].as_array().unwrap().is_empty());
        assert!(revoked["notifications"].as_array().unwrap().is_empty());
        conn.execute_batch("INSERT INTO relay_access_grants(id,owner_user_id,target_user_id,device_id,scope,workspace_id,workspace_access,created_at) VALUES ('workspace-grant','owner','reader','mac','workspace','project','read','now');").unwrap();
        assert!(accessible(
            &conn,
            "reader",
            "mac",
            "thread",
            Some("project")
        ));
        assert!(!accessible(
            &conn,
            "reader",
            "mac",
            "thread",
            Some("other-project")
        ));
        assert_eq!(
            snapshot(&conn, "reader").unwrap()["threads"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        // A browser-supplied workspace ID must not authorize reading events
        // for a thread whose workspace the relay cannot independently verify.
        assert!(snapshot(&conn, "reader").unwrap()["notifications"]
            .as_array()
            .unwrap()
            .is_empty());
        conn.execute("UPDATE relay_users SET enabled=0 WHERE id='reader'", [])
            .unwrap();
        assert!(snapshot(&conn, "reader").unwrap()["threads"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn navigation_favorites_survive_visits_and_are_account_and_device_scoped() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE relay_users(id TEXT PRIMARY KEY); CREATE TABLE relay_devices(id TEXT PRIMARY KEY); INSERT INTO relay_users VALUES ('alice'),('bob'); INSERT INTO relay_devices VALUES ('mac'),('wsl');").unwrap();
        ensure_schema(&conn).unwrap();
        let mut v = Visit {
            device_id: "mac".into(),
            thread_id: "thread".into(),
            title: "Review".into(),
            workspace_label: "project".into(),
            workspace_id: Some("project".into()),
            favorite: Some(true),
            read_completed_at: Some("2026-09-19T12:00:00Z".into()),
        };
        upsert(&conn, "alice", &v).unwrap();
        v.favorite = None;
        v.read_completed_at = None;
        upsert(&conn, "alice", &v).unwrap();
        upsert(&conn, "bob", &v).unwrap();
        v.device_id = "wsl".into();
        upsert(&conn, "alice", &v).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM relay_thread_navigation", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 3);
        let favorites:i64=conn.query_row("SELECT count(*) FROM relay_thread_navigation WHERE favorite=1 AND user_id='alice' AND device_id='mac'",[],|r|r.get(0)).unwrap();
        assert_eq!(favorites, 1);
        let marker: Option<String> = conn.query_row("SELECT read_completed_at FROM relay_thread_navigation WHERE user_id='alice' AND device_id='mac'", [], |r| r.get(0)).unwrap();
        assert_eq!(marker.as_deref(), Some("2026-09-19T12:00:00Z"));
        let other_marker: Option<String> = conn.query_row("SELECT read_completed_at FROM relay_thread_navigation WHERE user_id='bob' AND device_id='mac'", [], |r| r.get(0)).unwrap();
        assert!(other_marker.is_none());
        ensure_schema(&conn).unwrap(); // Reopening an upgraded database preserves the marker.
    }
    #[test]
    fn shortcut_order_is_the_accounts_own_and_new_shortcuts_go_last() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE relay_users(id TEXT PRIMARY KEY); CREATE TABLE relay_devices(id TEXT PRIMARY KEY); INSERT INTO relay_users VALUES ('alice'),('bob'); INSERT INTO relay_devices VALUES ('mac');").unwrap();
        ensure_schema(&conn).unwrap();
        let visit = |thread: &str, favorite: Option<bool>| Visit {
            device_id: "mac".into(),
            thread_id: thread.into(),
            title: thread.into(),
            workspace_label: "project".into(),
            workspace_id: None,
            favorite,
            read_completed_at: None,
        };
        for thread in ["a", "b", "c"] {
            upsert(&conn, "alice", &visit(thread, Some(true))).unwrap();
        }
        upsert(&conn, "bob", &visit("a", Some(true))).unwrap();
        let order = |user: &str| -> Vec<(String, Option<i64>)> {
            conn.prepare("SELECT thread_id,favorite_order FROM relay_thread_navigation WHERE user_id=?1 AND favorite=1 ORDER BY thread_id")
                .unwrap()
                .query_map([user], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        // Before any reorder, shortcuts carry no order and list by recency.
        assert!(order("alice")
            .iter()
            .all(|(_, position)| position.is_none()));
        let reference = |thread: &str| RemoveReference {
            device_id: "mac".into(),
            thread_id: thread.into(),
        };
        set_shortcut_order(
            &conn,
            "alice",
            &[
                reference("c"),
                reference("a"),
                reference("b"),
                reference("missing"),
            ],
        )
        .unwrap();
        assert_eq!(
            order("alice"),
            vec![
                ("a".into(), Some(1)),
                ("b".into(), Some(2)),
                ("c".into(), Some(0))
            ]
        );
        assert_eq!(order("bob"), vec![("a".into(), None)]);
        // A visit keeps the position; a new shortcut goes last; unstarring leaves the order.
        upsert(&conn, "alice", &visit("a", None)).unwrap();
        upsert(&conn, "alice", &visit("d", Some(true))).unwrap();
        upsert(&conn, "alice", &visit("b", Some(false))).unwrap();
        assert_eq!(
            order("alice"),
            vec![
                ("a".into(), Some(1)),
                ("c".into(), Some(0)),
                ("d".into(), Some(3))
            ]
        );
        let unstarred: Option<i64> = conn.query_row("SELECT favorite_order FROM relay_thread_navigation WHERE user_id='alice' AND thread_id='b'", [], |r| r.get(0)).unwrap();
        assert!(unstarred.is_none());
    }
}
