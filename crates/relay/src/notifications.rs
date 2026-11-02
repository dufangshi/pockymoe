//! Account-owned Web Push subscriptions and a durable, bounded delivery outbox.
use super::*;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use url::Url;
use web_push::{ContentEncoding, SubscriptionInfo, VapidSignatureBuilder, WebPushMessageBuilder};
const DAY: i64 = 86_400_000;

pub(super) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS relay_push_subscriptions (
      id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
      session_hash TEXT NOT NULL REFERENCES relay_auth_sessions(token_hash) ON DELETE CASCADE,
      subscription TEXT NOT NULL, created_at INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS relay_push_user ON relay_push_subscriptions(user_id);
    CREATE TABLE IF NOT EXISTS relay_push_events (
      id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES relay_devices(id) ON DELETE CASCADE,
      user_id TEXT NOT NULL, payload TEXT NOT NULL, created_at INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS relay_push_deliveries (
      event_id TEXT NOT NULL REFERENCES relay_push_events(id) ON DELETE CASCADE,
      subscription_id TEXT NOT NULL REFERENCES relay_push_subscriptions(id) ON DELETE CASCADE,
      attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL DEFAULT 0,
      PRIMARY KEY(event_id,subscription_id));",
    )?;
    live_activity::ensure_schema(conn)?;
    let existing: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM relay_settings WHERE key='webPushVapidPem')",
        [],
        |r| r.get(0),
    )?;
    if !existing {
        let group = openssl::ec::EcGroup::from_curve_name(openssl::nid::Nid::X9_62_PRIME256V1)?;
        let key = openssl::ec::EcKey::generate(&group)?;
        let mut context = openssl::bn::BigNumContext::new()?;
        let public = key.public_key().to_bytes(
            &group,
            openssl::ec::PointConversionForm::UNCOMPRESSED,
            &mut context,
        )?;
        let tx = conn.unchecked_transaction()?;
        set_relay_setting(
            &tx,
            "webPushVapidPem",
            &String::from_utf8(key.private_key_to_pem()?)?,
        )?;
        set_relay_setting(&tx, "webPushVapidPublic", &URL_SAFE_NO_PAD.encode(public))?;
        tx.commit()?;
    }
    Ok(())
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/relay/account/notifications/native/live-activity",
            axum::routing::put(register_live_activity),
        )
        .route("/relay/account/notifications", get(settings))
        .route(
            "/relay/account/notifications/subscription",
            post(subscribe).delete(unsubscribe),
        )
        .route(
            "/relay/account/notifications/native",
            post(subscribe_native).delete(unsubscribe),
        )
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveActivityRegistration {
    id: String,
    token: String,
}
async fn register_live_activity(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    Json(body): Json<LiveActivityRegistration>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let session = security::token_hash(&extract_session_token(&headers, &query).unwrap());
    let raw: Option<String> = conn.query_row("SELECT subscription FROM relay_push_subscriptions WHERE id=?1 AND user_id=?2 AND session_hash=?3",params![body.id,user.id,session],|r|r.get(0)).optional().ok().flatten();
    let ios = raw
        .and_then(|raw| serde_json::from_str::<apns::Subscription>(&raw).ok())
        .is_some_and(|sub| sub.native_platform == "apns" && sub.app_kind == apns::AppKind::Ios);
    let saved = ios
        && (|| -> Result<()> {
            let tx = conn.unchecked_transaction()?;
            live_activity::register(&tx, &body.id, &body.token)?;
            tx.commit()?;
            Ok(())
        })()
        .is_ok();
    if !saved {
        return failure(
            StatusCode::BAD_REQUEST,
            "Invalid Live Activity registration.",
        );
    }
    Json(json!({"ok":true})).into_response()
}
fn failure(status: StatusCode, message: &str) -> Response {
    (status, Json(ApiError::new("notifications", message))).into_response()
}
async fn settings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let result = (|| -> Result<Value> {
        let key: String = conn.query_row(
            "SELECT value FROM relay_settings WHERE key='webPushVapidPublic'",
            [],
            |r| r.get(0),
        )?;
        let mut stmt = conn.prepare("SELECT id FROM relay_push_subscriptions p JOIN relay_auth_sessions s ON s.token_hash=p.session_hash WHERE p.user_id=?1 AND s.expires_at>?2")?;
        let ids = stmt
            .query_map(
                params![user.id, chrono::Utc::now().timestamp_millis()],
                |r| r.get::<_, String>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(
            json!({"publicKey":key,"subscriptionIds":ids,"scope":"ownedDevices","nativePushAvailable":apns::configured(),"nativePushKinds":apns::configured_kinds()}),
        )
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(_) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Unable to load notification settings.",
        ),
    }
}
// Never allow authenticated users to turn the Relay into a general HTTP proxy.
fn validate_subscription(sub: &SubscriptionInfo) -> Result<()> {
    let url = Url::parse(&sub.endpoint)?;
    let host = url.host_str().unwrap_or("");
    anyhow::ensure!(
        url.scheme() == "https"
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && sub.endpoint.len() <= 4096,
        "Invalid push endpoint"
    );
    anyhow::ensure!(
        host == "fcm.googleapis.com"
            || host.ends_with(".push.services.mozilla.com")
            || host.ends_with(".notify.windows.com")
            || host.ends_with(".push.apple.com"),
        "Unsupported browser push service"
    );
    let public = URL_SAFE_NO_PAD.decode(&sub.keys.p256dh)?;
    anyhow::ensure!(
        public.len() == 65 && public[0] == 4 && URL_SAFE_NO_PAD.decode(&sub.keys.auth)?.len() == 16,
        "Invalid subscription keys"
    );
    Ok(())
}
async fn subscribe(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    Json(sub): Json<SubscriptionInfo>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    if validate_subscription(&sub).is_err() {
        return failure(
            StatusCode::BAD_REQUEST,
            "Invalid or unsupported browser push subscription.",
        );
    }
    let id = security::token_hash(&sub.endpoint);
    let session_hash = security::token_hash(&extract_session_token(&headers, &query).unwrap());
    let result = (|| -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM relay_push_subscriptions WHERE user_id=?1 AND id!=?2",
            params![user.id, id],
            |r| r.get(0),
        )?;
        anyhow::ensure!(count < 20, "Too many subscriptions");
        // Delete first so pending messages from a previous account cannot follow
        // a browser subscription when it is explicitly rebound after login.
        tx.execute("DELETE FROM relay_push_subscriptions WHERE id=?1", [&id])?;
        tx.execute(
            "INSERT INTO relay_push_subscriptions VALUES (?1,?2,?3,?4,?5)",
            params![
                id,
                user.id,
                session_hash,
                serde_json::to_string(&sub)?,
                chrono::Utc::now().timestamp_millis()
            ],
        )?;
        tx.commit()?;
        Ok(())
    })();
    match result {
        Ok(()) => Json(json!({"id":id})).into_response(),
        Err(_) => failure(
            StatusCode::BAD_REQUEST,
            "Unable to save subscription; remove unused browser subscriptions and retry.",
        ),
    }
}
#[derive(Deserialize)]
struct Remove {
    id: String,
}

async fn subscribe_native(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    Json(input): Json<Value>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let Ok(sub) = serde_json::from_value::<apns::Subscription>(input.clone()) else {
        return failure(StatusCode::BAD_REQUEST, "Invalid native push subscription.");
    };
    let live_token = input["liveActivityToken"].as_str();
    if live_token
        .is_some_and(|token| sub.app_kind != apns::AppKind::Ios || !apns::valid_token(token))
    {
        return failure(StatusCode::BAD_REQUEST, "Invalid Live Activity token.");
    }
    if !apns::configured_for(sub.app_kind) {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "APNs is not configured on this relay.",
        );
    }
    if apns::validate(&sub).is_err() {
        return failure(StatusCode::BAD_REQUEST, "Invalid native push subscription.");
    }
    let id = apns::subscription_id(&sub);
    let session = security::token_hash(&extract_session_token(&headers, &query).unwrap());
    let result = (|| -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        if sub.app_kind == apns::AppKind::Ios {
            let legacy = security::token_hash(&format!(
                "apns:{}:{}",
                sub.sandbox,
                sub.device_token.to_ascii_lowercase()
            ));
            tx.execute("DELETE FROM relay_push_subscriptions WHERE id=?1", [legacy])?;
        }
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM relay_push_subscriptions WHERE user_id=?1 AND id!=?2",
            params![user.id, id],
            |r| r.get(0),
        )?;
        anyhow::ensure!(count < 20, "Too many subscriptions");
        tx.execute("DELETE FROM relay_push_subscriptions WHERE id=?1 AND (user_id!=?2 OR session_hash!=?3)", params![id,user.id,session])?;
        tx.execute(
            "INSERT INTO relay_push_subscriptions VALUES (?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET subscription=excluded.subscription",
            params![
                id,
                user.id,
                session,
                serde_json::to_string(&sub)?,
                chrono::Utc::now().timestamp_millis()
            ],
        )?;
        if let Some(token) = live_token {
            live_activity::register(&tx, &id, token)?;
        }
        tx.commit()?;
        Ok(())
    })();
    match result {
        Ok(()) => Json(json!({"id":id})).into_response(),
        Err(_) => failure(
            StatusCode::BAD_REQUEST,
            "Unable to save native push subscription.",
        ),
    }
}
async fn unsubscribe(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    Json(body): Json<Remove>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    match conn.execute(
        "DELETE FROM relay_push_subscriptions WHERE id=?1 AND user_id=?2",
        params![body.id, user.id],
    ) {
        Ok(_) => Json(json!({"ok":true})).into_response(),
        Err(_) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Unable to disable notifications.",
        ),
    }
}

// The device identity comes only from the authenticated tunnel, never the payload.
pub(super) fn accept(conn: &Connection, device: &str, event: &Value) -> Result<String> {
    let thread = event["threadId"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing thread"))?;
    let turn = event["turnId"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing turn"))?;
    Uuid::parse_str(thread)?;
    Uuid::parse_str(turn)?;
    let status = event["status"].as_str().unwrap_or("");
    anyhow::ensure!(
        ["completed", "failed", "interrupted", "awaiting_input"].contains(&status),
        "Unsupported notification event"
    );
    let event_key = if status == "awaiting_input" {
        let request = event["requestId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing request"))?;
        anyhow::ensure!(
            !request.is_empty()
                && request.len() <= 256
                && request
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b)),
            "Invalid request ID"
        );
        request
    } else {
        turn
    };
    let at = chrono::DateTime::parse_from_rfc3339(event["occurredAt"].as_str().unwrap_or(""))?
        .timestamp_millis();
    let now = chrono::Utc::now().timestamp_millis();
    anyhow::ensure!(at <= now + 60_000, "Future notification");
    if at < now - DAY {
        return Ok(event_key.into());
    } // Expired events are acknowledged, not replayed.
    let tx = conn.unchecked_transaction()?;
    let owner: String = tx.query_row(
        "SELECT owner_user_id FROM relay_devices WHERE id=?1",
        [device],
        |r| r.get(0),
    )?;
    let id = format!("{device}:{event_key}");
    if status != "awaiting_input" {
        tx.execute("UPDATE relay_turn_subscriptions SET ended_at=?4 WHERE device_id=?1 AND thread_id=?2 AND turn_id=?3 AND ended_at IS NULL",params![device,thread,turn,now])?;
    }
    let payload = json!({"title":"Pockymoe","body":if status=="completed" {"A thread turn completed. Click to view."} else if status=="awaiting_input" {"A thread needs your input. Click to respond."} else {"A thread turn failed. Click to view."},"url":format!("/devices/{device}/threads/{thread}"),"tag":id,"deviceId":device,"userId":owner,"occurredAt":at,"threadId":thread,"turnId":turn,"status":status,"workspaceId":event["workspaceId"]});
    if tx.execute(
        "INSERT OR IGNORE INTO relay_push_events VALUES (?1,?2,?3,?4,?5)",
        params![id, device, owner, payload.to_string(), at],
    )? > 0
        && notification_scope::allowed(&tx, &owner, device, &payload)?
    {
        tx.execute("INSERT INTO relay_push_deliveries(event_id,subscription_id) SELECT ?1,p.id FROM relay_push_subscriptions p JOIN relay_auth_sessions s ON s.token_hash=p.session_hash JOIN relay_users u ON u.id=p.user_id WHERE p.user_id=?2 AND p.created_at<=?3 AND s.expires_at>?4 AND u.enabled=1",params![id,owner,at,now])?;
    }
    tx.commit()?;
    Ok(event_key.into())
}

fn encrypted_request(
    pem: &str,
    sub: &SubscriptionInfo,
    payload: &[u8],
) -> Result<web_push::WebPushMessage> {
    let mut signature = VapidSignatureBuilder::from_pem(std::io::Cursor::new(pem), sub)?;
    signature.add_claim("sub", "https://github.com/dufangshi/pockymoe");
    let mut message = WebPushMessageBuilder::new(sub);
    message.set_payload(ContentEncoding::Aes128Gcm, payload);
    message.set_vapid_signature(signature.build()?);
    message.set_ttl(86400);
    Ok(message.build()?)
}

pub(super) fn start(state: &Arc<AppState>) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let weak = Arc::downgrade(state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let Some(state) = weak.upgrade() else { break };
            if live_activity::deliver_batch(&state, &http).await.is_err() {
                tracing::warn!("Live Activity delivery deferred");
            }
            if let Err(error) = deliver_batch(&state, &http).await {
                tracing::warn!(%error,"Push delivery batch deferred");
            }
        }
    });
    Ok(())
}
async fn deliver_batch(state: &AppState, http: &reqwest::Client) -> Result<()> {
    let now = chrono::Utc::now().timestamp_millis();
    let (pem, jobs) = {
        let conn = state.store.conn.lock().await;
        conn.execute(
            "DELETE FROM relay_push_events WHERE created_at<?1",
            [now - DAY],
        )?;
        conn.execute(
            "DELETE FROM relay_turn_subscriptions WHERE ended_at<?1",
            [now - DAY],
        )?;
        conn.execute("DELETE FROM relay_push_subscriptions WHERE session_hash NOT IN (SELECT token_hash FROM relay_auth_sessions WHERE expires_at>?1)",[now])?;
        let pem: String = conn.query_row(
            "SELECT value FROM relay_settings WHERE key='webPushVapidPem'",
            [],
            |r| r.get(0),
        )?;
        let mut stmt=conn.prepare("SELECT d.event_id,d.subscription_id,p.subscription,e.payload,d.attempts FROM relay_push_deliveries d JOIN relay_push_events e ON e.id=d.event_id JOIN relay_push_subscriptions p ON p.id=d.subscription_id JOIN relay_devices v ON v.id=e.device_id JOIN relay_users u ON u.id=p.user_id WHERE d.next_at<=?1 AND e.user_id=p.user_id AND v.owner_user_id=p.user_id AND u.enabled=1 ORDER BY e.created_at LIMIT 20")?;
        let jobs = stmt
            .query_map([now], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, u32>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        (pem, jobs)
    };
    for (event, id, raw, payload, attempts) in jobs {
        // Recheck before sending: unsubscribe/logout may happen during a batch.
        if !state.store.conn.lock().await.query_row("SELECT EXISTS(SELECT 1 FROM relay_push_subscriptions p JOIN relay_auth_sessions s ON s.token_hash=p.session_hash JOIN relay_push_events e ON e.id=?3 JOIN relay_devices d ON d.id=e.device_id WHERE p.id=?1 AND s.expires_at>?2 AND p.subscription=?4 AND p.user_id=e.user_id AND d.owner_user_id=p.user_id)",params![id,chrono::Utc::now().timestamp_millis(),event,raw],|r|r.get::<_,bool>(0))? { continue; }
        let payload_value: Value = serde_json::from_str(&payload)?;
        let device = payload_value["deviceId"]
            .as_str()
            .unwrap_or_else(|| event.rsplit_once(':').map(|(d, _)| d).unwrap_or(""));
        let user = payload_value["userId"].as_str().unwrap_or("");
        if !notification_scope::allowed(
            &*state.store.conn.lock().await,
            user,
            device,
            &payload_value,
        )? {
            state.store.conn.lock().await.execute(
                "DELETE FROM relay_push_deliveries WHERE event_id=?1 AND subscription_id=?2",
                params![event, id],
            )?;
            continue;
        }
        let result = async {
            if let Ok(sub) = serde_json::from_str::<apns::Subscription>(&raw) {
                return apns::deliver(http, &sub, &payload).await;
            }
            let sub: SubscriptionInfo = serde_json::from_str(&raw)?;
            validate_subscription(&sub)?;
            let message = encrypted_request(&pem, &sub, payload.as_bytes())?;
            let request = web_push::request_builder::build_request::<Vec<u8>>(message);
            let mut send = http.post(request.uri().to_string());
            for (key, value) in request.headers() {
                send = send.header(key.as_str(), value.as_bytes());
            }
            Ok::<_, anyhow::Error>(send.body(request.into_body()).send().await?.status())
        }
        .await;
        let conn = state.store.conn.lock().await;
        match result {
            Ok(status) if status == StatusCode::GONE || status == StatusCode::NOT_FOUND => {
                state.metrics.push("expired");
                conn.execute("DELETE FROM relay_push_subscriptions WHERE id=?1", [id])?;
            }
            Ok(status) if status.is_success() => {
                state.metrics.push("sent");
                conn.execute(
                    "DELETE FROM relay_push_deliveries WHERE event_id=?1 AND subscription_id=?2",
                    params![event, id],
                )?;
            }
            _ => {
                state.metrics.push("failed");
                // Do not log push URLs or keys. Retry transport errors and service
                // throttling with bounded backoff; expiration bounds total retention.
                let delay = 15_000i64 * (1i64 << attempts.min(8));
                conn.execute("UPDATE relay_push_deliveries SET attempts=attempts+1,next_at=?3 WHERE event_id=?1 AND subscription_id=?2",params![event,id,now+delay])?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn awaiting_input_accepts_request_tags_and_deduplicates() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("push-input-{}", Uuid::new_v4()));
        let store = RelayStore::open(dir.join("db"), "secret".into())?;
        let conn = store.conn.try_lock().unwrap();
        conn.execute("INSERT INTO relay_users VALUES ('owner','owner','owner','user',1,NULL,'now','salt','hash')", [])?;
        let session = create_session(&conn, "secret", "owner")?;
        conn.execute("INSERT INTO relay_devices VALUES ('device','owner','device',NULL,'device','device','now')", [])?;
        conn.execute(
            "INSERT INTO relay_push_subscriptions VALUES ('sub','owner',?1,'{}',?2)",
            params![
                security::token_hash(&session),
                chrono::Utc::now().timestamp_millis() - 1000
            ],
        )?;
        let mut event = json!({"threadId":Uuid::new_v4(),"turnId":Uuid::new_v4(),"requestId":"perm-request-1","status":"awaiting_input","occurredAt":now_rfc3339(),"body":"private text ignored","deviceId":"spoofed"});
        // Default subscribed policy still applies; request creation does not end it.
        conn.execute(
            "INSERT INTO relay_turn_subscriptions VALUES ('owner','device',?1,?2,NULL)",
            params![
                event["threadId"].as_str().unwrap(),
                event["turnId"].as_str().unwrap()
            ],
        )?;
        assert_eq!(accept(&conn, "device", &event)?, "perm-request-1");
        accept(&conn, "device", &event)?;
        assert_eq!(
            conn.query_row("SELECT count(*) FROM relay_push_deliveries", [], |r| r
                .get::<_, i64>(0))?,
            1
        );
        let payload: String =
            conn.query_row("SELECT payload FROM relay_push_events", [], |r| r.get(0))?;
        let payload: Value = serde_json::from_str(&payload)?;
        assert_eq!(payload["tag"], "device:perm-request-1");
        assert_eq!(payload["deviceId"], "device");
        assert_eq!(
            payload["body"],
            "A thread needs your input. Click to respond."
        );
        assert!(conn.query_row(
            "SELECT ended_at IS NULL FROM relay_turn_subscriptions",
            [],
            |r| r.get::<_, bool>(0)
        )?);
        event["status"] = json!("completed");
        accept(&conn, "device", &event)?;
        assert_eq!(
            conn.query_row("SELECT count(*) FROM relay_push_events", [], |r| r
                .get::<_, i64>(0))?,
            2
        );
        assert!(!conn.query_row(
            "SELECT ended_at IS NULL FROM relay_turn_subscriptions",
            [],
            |r| r.get::<_, bool>(0)
        )?);
        assert!(!notification_scope::allowed(
            &conn, "owner", "device", &payload
        )?);
        event["status"] = json!("awaiting_input");
        event["requestId"] = Value::Null;
        assert!(accept(&conn, "device", &event).is_err());
        drop(conn);
        drop(store);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn owner_fanout_deduplicates_across_devices_and_revocation_cascades() {
        let dir = std::env::temp_dir().join(format!("push-test-{}", Uuid::new_v4()));
        let store = RelayStore::open(dir.join("db"), "secret".into()).unwrap();
        let conn = store.conn.try_lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        for user in ["owner", "guest"] {
            conn.execute(
                "INSERT INTO relay_users VALUES (?1,?1,?1,'user',1,NULL,'now','salt','hash')",
                [user],
            )
            .unwrap();
            let token = create_session(&conn, "secret", user).unwrap();
            for browser in 0..2 {
                conn.execute(
                    "INSERT INTO relay_push_subscriptions VALUES (?1,?2,?3,'{}',?4)",
                    params![
                        format!("{user}-{browser}"),
                        user,
                        security::token_hash(&token),
                        now - 1000
                    ],
                )
                .unwrap();
            }
        }
        conn.execute(
            "INSERT INTO relay_notification_preferences VALUES ('owner',?1)",
            [json!({"scope":"all"}).to_string()],
        )
        .unwrap();
        let thread = Uuid::new_v4().to_string();
        for device in ["device-a", "device-b"] {
            conn.execute(
                "INSERT INTO relay_devices VALUES (?1,'owner',?1,NULL,?1,?1,'now')",
                [device],
            )
            .unwrap();
            conn.execute("INSERT INTO relay_access_grants(id,owner_user_id,target_user_id,device_id,scope,created_at) VALUES (?1,'owner','guest',?1,'device','now')",[device]).unwrap();
            let event = json!({"threadId":thread,"turnId":Uuid::new_v4().to_string(),"status":"completed","occurredAt":now_rfc3339()});
            accept(&conn, device, &event).unwrap();
            accept(&conn, device, &event).unwrap();
        }
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT count(*) FROM relay_push_deliveries"), 4);
        assert_eq!(
            count("SELECT count(*) FROM relay_push_deliveries WHERE subscription_id LIKE 'guest%'"),
            0
        );
        conn.execute("DELETE FROM relay_auth_sessions WHERE user_id='owner'", [])
            .unwrap();
        assert_eq!(count("SELECT count(*) FROM relay_push_deliveries"), 0);
        let old = json!({"threadId":thread,"turnId":Uuid::new_v4().to_string(),"status":"completed","occurredAt":(chrono::Utc::now()-chrono::Duration::days(2)).to_rfc3339()});
        accept(&conn, "device-a", &old).unwrap();
        assert_eq!(count("SELECT count(*) FROM relay_push_events"), 2);
        drop(conn);
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn encrypted_payload_roundtrips_and_endpoint_is_not_an_http_proxy() {
        let dir = std::env::temp_dir().join(format!("push-crypto-{}", Uuid::new_v4()));
        let store = RelayStore::open(dir.join("db"), "secret".into()).unwrap();
        let conn = store.conn.try_lock().unwrap();
        let pem: String = conn
            .query_row(
                "SELECT value FROM relay_settings WHERE key='webPushVapidPem'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let (keys, auth) = ece::generate_keypair_and_auth_secret().unwrap();
        let public = keys.pub_as_raw().unwrap();
        let mut sub = SubscriptionInfo::new(
            "https://fcm.googleapis.com/fcm/send/test".into(),
            URL_SAFE_NO_PAD.encode(public),
            URL_SAFE_NO_PAD.encode(auth),
        );
        validate_subscription(&sub).unwrap();
        let message = encrypted_request(&pem, &sub, b"private payload").unwrap();
        let payload = message.payload.unwrap();
        assert_eq!(
            ece::decrypt(&keys.raw_components().unwrap(), &auth, &payload.content).unwrap(),
            b"private payload"
        );
        assert!(payload
            .crypto_headers
            .iter()
            .any(|(k, v)| *k == "Authorization" && v.starts_with("vapid ")));
        for endpoint in [
            "http://fcm.googleapis.com/a",
            "https://127.0.0.1/a",
            "https://fcm.googleapis.com.evil.test/a",
            "https://fcm.googleapis.com:8443/a",
        ] {
            sub.endpoint = endpoint.into();
            assert!(validate_subscription(&sub).is_err());
        }
        drop(conn);
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod mobile_tests {
    use super::*;
    #[tokio::test]
    async fn live_activity_registration_requires_matching_owner_session_and_ios() -> Result<()> {
        let (state, dir) = crate::tests::test_app_state("live-registration");
        let owner;
        let other;
        let second_session;
        {
            let conn = state.store.conn.lock().await;
            for user in ["owner", "other"] {
                conn.execute(
                    "INSERT INTO relay_users VALUES (?1,?1,?1,'user',1,NULL,'now','salt','hash')",
                    [user],
                )?;
            }
            owner = create_session(&conn, &state.store.session_secret, "owner")?;
            other = create_session(&conn, &state.store.session_secret, "other")?;
            second_session = create_session(&conn, &state.store.session_secret, "owner")?;
            for (id, kind) in [("ios", "ios"), ("desktop", "macos-desktop")] {
                conn.execute("INSERT INTO relay_push_subscriptions VALUES (?1,'owner',?2,?3,?4)",params![id,security::token_hash(&owner),json!({"nativePlatform":"apns","appKind":kind,"deviceToken":"ab".repeat(32),"relayOrigin":"https://relay.test"}).to_string(),chrono::Utc::now().timestamp_millis()])?;
            }
        }
        for (session, id, expected) in [
            (&owner, "ios", StatusCode::OK),
            (&other, "ios", StatusCode::BAD_REQUEST),
            (&second_session, "ios", StatusCode::BAD_REQUEST),
            (&owner, "desktop", StatusCode::BAD_REQUEST),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::AUTHORIZATION, format!("Bearer {session}").parse()?);
            let response = register_live_activity(
                State(state.clone()),
                headers,
                Query(TokenQuery {
                    token: None,
                    device_token: None,
                    relay_session: None,
                }),
                Json(LiveActivityRegistration {
                    id: id.into(),
                    token: "cd".repeat(32),
                }),
            )
            .await;
            assert_eq!(response.status(), expected);
        }
        drop(state);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
