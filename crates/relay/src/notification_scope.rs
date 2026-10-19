//! Account preferences and exact turn subscriptions; references never grant access.
use super::*;
use pockymoe_protocol::{should_notify_turn, NotificationSettings};

pub(super) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS relay_notification_preferences (
        user_id TEXT PRIMARY KEY REFERENCES relay_users(id) ON DELETE CASCADE,
        settings TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS relay_turn_subscriptions (
        user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
        device_id TEXT NOT NULL REFERENCES relay_devices(id) ON DELETE CASCADE,
        thread_id TEXT NOT NULL, turn_id TEXT NOT NULL, ended_at INTEGER,
        PRIMARY KEY(user_id,device_id,thread_id,turn_id));",
    )?;
    Ok(())
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/relay/account/notifications/preferences",
            get(load).put(save),
        )
        .route(
            "/relay/account/notifications/turns/{device}/{thread}/{turn}",
            get(subscription).put(subscribe),
        )
}
pub(super) fn settings(conn: &Connection, user: &str) -> Result<NotificationSettings> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT settings FROM relay_notification_preferences WHERE user_id=?1",
            [user],
            |r| r.get(0),
        )
        .optional()?;
    Ok(raw
        .map(|v| serde_json::from_str(&v))
        .transpose()?
        .unwrap_or_default())
}
fn response(value: Value) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response()
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
    match settings(&conn, &user.id) {
        Ok(value) => response(json!(value)),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
async fn save(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    Json(value): Json<NotificationSettings>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    if !value.valid() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if conn.execute("INSERT INTO relay_notification_preferences VALUES (?1,?2) ON CONFLICT(user_id) DO UPDATE SET settings=excluded.settings", params![user.id,json!(value).to_string()]).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    response(json!(value))
}

fn accessible(conn: &Connection, user: &str, device: &str, thread: &str) -> bool {
    // Like existing workbench events, a browser-supplied workspace cannot prove access.
    effective_access(conn, user, device, Some(thread), None).is_some()
}
async fn subscription(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    Path((device, thread, turn)): Path<(String, String, String)>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    if !accessible(&conn, &user.id, &device, &thread) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match conn.query_row("SELECT EXISTS(SELECT 1 FROM relay_turn_subscriptions WHERE user_id=?1 AND device_id=?2 AND thread_id=?3 AND turn_id=?4 AND ended_at IS NULL)",params![user.id,device,thread,turn],|r|r.get::<_,bool>(0)) {
        Ok(subscribed) => response(json!({"subscribed":subscribed})),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Subscribe {
    subscribed: bool,
}
async fn subscribe(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    Path((device, thread, turn)): Path<(String, String, String)>,
    Json(body): Json<Subscribe>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    if Uuid::parse_str(&thread).is_err() || Uuid::parse_str(&turn).is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if !accessible(&conn, &user.id, &device, &thread) {
        return StatusCode::FORBIDDEN.into_response();
    }
    // The same lock serializes this write with event acceptance. A late click
    // cannot subscribe a finished turn or change an already accepted event.
    let ended = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM relay_push_events WHERE id=?1)",
        [format!("{device}:{turn}")],
        |r| r.get::<_, bool>(0),
    );
    match ended {
        Ok(true) => {
            return (
                StatusCode::CONFLICT,
                Json(ApiError::new("turn_ended", "这一轮已经结束。")),
            )
                .into_response()
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        _ => (),
    }
    let result = if body.subscribed {
        conn.execute("INSERT OR IGNORE INTO relay_turn_subscriptions(user_id,device_id,thread_id,turn_id) VALUES (?1,?2,?3,?4)",params![user.id,device,thread,turn])
    } else {
        conn.execute("DELETE FROM relay_turn_subscriptions WHERE user_id=?1 AND device_id=?2 AND thread_id=?3 AND turn_id=?4 AND ended_at IS NULL",params![user.id,device,thread,turn])
    };
    match result {
        Ok(_) => response(json!({"subscribed":body.subscribed})),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub(super) fn allowed(conn: &Connection, user: &str, device: &str, event: &Value) -> Result<bool> {
    let settings = settings(conn, user)?;
    let subscribed = if let (Some(thread), Some(turn)) =
        (event["threadId"].as_str(), event["turnId"].as_str())
    {
        // Ended rows are frozen eligibility evidence, not active subscriptions.
        conn.query_row("SELECT EXISTS(SELECT 1 FROM relay_turn_subscriptions WHERE user_id=?1 AND device_id=?2 AND thread_id=?3 AND turn_id=?4)",params![user,device,thread,turn],|r|r.get(0))?
    } else {
        false
    };
    let status = event["status"].as_str().unwrap_or_else(|| {
        if event["body"].as_str().unwrap_or("").contains("failed") {
            "failed"
        } else {
            "completed"
        }
    });
    Ok(should_notify_turn(
        &settings,
        device,
        event["workspaceId"].as_str(),
        subscribed,
        status,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn account_policy_persists_and_exact_turn_subscriptions_gate_push_and_workbench() {
        let (state, dir) = crate::tests::test_app_state("notification-scope");
        let (owner, other_browser, stranger) = {
            let conn = state.store.conn.lock().await;
            conn.execute_batch("INSERT INTO relay_users VALUES ('owner','o@test','owner','user',1,NULL,'now','salt','hash'),('stranger','s@test','stranger','user',1,NULL,'now','salt','hash');
              INSERT INTO relay_devices VALUES ('a','owner','A',NULL,'a','a','now'),('b','owner','B',NULL,'b','b','now');").unwrap();
            let owner = create_session(&conn, &state.store.session_secret, "owner").unwrap();
            conn.execute(
                "INSERT INTO relay_push_subscriptions VALUES ('browser','owner',?1,'{}',?2)",
                params![
                    security::token_hash(&owner),
                    chrono::Utc::now().timestamp_millis() - 1000
                ],
            )
            .unwrap();
            (
                owner,
                create_session(&conn, &state.store.session_secret, "owner").unwrap(),
                create_session(&conn, &state.store.session_secret, "stranger").unwrap(),
            )
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = routes()
            .merge(workbench::routes())
            .with_state(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        let prefs = format!("{base}/relay/account/notifications/preferences");
        assert_eq!(
            client.get(&prefs).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(&prefs)
                .bearer_auth(&owner)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()["scope"],
            "subscribed"
        );
        let thread = Uuid::new_v4().to_string();
        let turn = Uuid::new_v4().to_string();
        let sub = format!("{base}/relay/account/notifications/turns/a/{thread}/{turn}");
        assert_eq!(
            client
                .put(&sub)
                .bearer_auth(&stranger)
                .json(&json!({"subscribed":true}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            client
                .put(&sub)
                .bearer_auth(&owner)
                .json(&json!({"subscribed":true,"userId":"stranger"}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(client
            .put(&sub)
            .bearer_auth(&owner)
            .json(&json!({"subscribed":true}))
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
        assert_eq!(
            client
                .get(&sub)
                .bearer_auth(&other_browser)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()["subscribed"],
            true
        );
        // Cancellation is persisted, and subscribing again still binds just this turn.
        client
            .put(&sub)
            .bearer_auth(&other_browser)
            .json(&json!({"subscribed":false}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        assert_eq!(
            client
                .get(&sub)
                .bearer_auth(&owner)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()["subscribed"],
            false
        );
        client
            .put(&sub)
            .bearer_auth(&owner)
            .json(&json!({"subscribed":true}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        {
            let conn = state.store.conn.lock().await;
            for (device, turn_id, status, workspace) in [
                ("a", Uuid::new_v4().to_string(), "completed", Some("w")),
                ("b", turn.clone(), "failed", None),
                ("a", turn.clone(), "failed", Some("w")),
            ] {
                let event = json!({"threadId":thread,"turnId":turn_id,"status":status,"occurredAt":now_rfc3339(),"workspaceId":workspace});
                notifications::accept(&conn, device, &event).unwrap();
                notifications::accept(&conn, device, &event).unwrap();
            }
            assert_eq!(
                conn.query_row("SELECT count(*) FROM relay_push_deliveries", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
        assert_eq!(
            client
                .get(&sub)
                .bearer_auth(&other_browser)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()["subscribed"],
            false
        );
        assert_eq!(
            client
                .put(&sub)
                .bearer_auth(&owner)
                .json(&json!({"subscribed":true}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        {
            let conn = state.store.conn.lock().await;
            // A busy unselected workspace cannot evict an eligible older notice.
            for _ in 0..105 {
                notifications::accept(&conn, "a", &json!({"threadId":thread,"turnId":Uuid::new_v4().to_string(),"status":"completed","occurredAt":now_rfc3339(),"workspaceId":"other"})).unwrap();
            }
        }
        let snapshot = client
            .get(format!("{base}/relay/account/workbench"))
            .bearer_auth(&other_browser)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(snapshot["notifications"].as_array().unwrap().len(), 1);
        // Workspace IDs are trusted only from the device event, and scoped by device.
        let selected = json!({"scope":"selected","devices":[],"workspaces":[{"deviceId":"b","workspaceId":"w","name":"W","deviceName":"B"}]});
        client
            .put(&prefs)
            .bearer_auth(&other_browser)
            .json(&selected)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        assert_eq!(
            client
                .get(&prefs)
                .bearer_auth(&owner)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap(),
            selected
        );
        assert_eq!(
            client
                .get(&prefs)
                .bearer_auth(&stranger)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()["scope"],
            "subscribed"
        );
        {
            let conn = state.store.conn.lock().await;
            for workspace in [None, Some("other"), Some("w")] {
                notifications::accept(&conn,"b",&json!({"threadId":thread,"turnId":Uuid::new_v4().to_string(),"status":"completed","occurredAt":now_rfc3339(),"workspaceId":workspace})).unwrap();
            }
            assert_eq!(
                conn.query_row("SELECT count(*) FROM relay_push_deliveries", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                2
            );
            let payload: String = conn.query_row("SELECT payload FROM relay_push_events WHERE device_id='b' AND json_extract(payload,'$.workspaceId')='w'",[],|r|r.get(0)).unwrap();
            assert!(allowed(
                &conn,
                "owner",
                "b",
                &serde_json::from_str(&payload).unwrap()
            )
            .unwrap());
            let interrupted = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO relay_turn_subscriptions VALUES ('owner','b',?1,?2,NULL)",
                params![thread, interrupted],
            )
            .unwrap();
            notifications::accept(&conn,"b",&json!({"threadId":thread,"turnId":interrupted,"status":"interrupted","occurredAt":now_rfc3339(),"workspaceId":"w"})).unwrap();
            assert!(!conn
                .query_row(
                    "SELECT ended_at IS NULL FROM relay_turn_subscriptions WHERE turn_id=?1",
                    [interrupted],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap());
            assert_eq!(
                conn.query_row("SELECT count(*) FROM relay_push_deliveries", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                2
            );
        }
        server.abort();
        let _ = server.await;
        drop(state);
        let reopened =
            RelayStore::open(dir.join("relay-store.sqlite"), "test-secret".into()).unwrap();
        assert_eq!(
            json!(settings(&*reopened.conn.lock().await, "owner").unwrap()),
            selected
        );
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
