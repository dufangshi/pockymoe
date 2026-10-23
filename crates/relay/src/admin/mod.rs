//! Relay administration API (docs/relay-admin.md). Every route requires the
//! relay admin session. Reads cover the overview, users, devices, shares,
//! sessions and security events, and the system; mutations are registration
//! review and settings, user enable/reset/delete and sessions, disconnecting a
//! device tunnel and revoking a share. Admin views never carry thread titles
//! or conversation content.
use super::*;

mod audit;
mod common;
mod devices;
#[cfg(test)]
mod load_tests;
mod overview;
mod server;
mod shares;
mod stats;
mod system;
#[cfg(test)]
mod tests;
mod users;

pub(crate) use common::{
    compare_versions, day_of, rfc3339_from_ms, DAY_MS, HOUR_MS, MINUTE_MS, RELAY_VERSION,
};

/// In-memory state of the admin console: when this process started and the
/// slow system checks it caches.
pub(crate) struct AdminState {
    pub(crate) started_at: String,
    integrity: StdMutex<Option<(std::time::Instant, String)>>,
    storage: StdMutex<Option<(std::time::Instant, Value)>>,
    /// People and devices active on each finished day (they never change).
    active_days: StdMutex<HashMap<String, (i64, i64)>>,
    /// The slow storage checks run one at a time, off the async threads.
    measuring: tokio::sync::Mutex<()>,
}

impl Default for AdminState {
    fn default() -> Self {
        Self {
            started_at: now_rfc3339(),
            integrity: StdMutex::new(None),
            storage: StdMutex::new(None),
            active_days: StdMutex::new(HashMap::new()),
            measuring: tokio::sync::Mutex::new(()),
        }
    }
}

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/relay/admin", get(relay_admin))
        .route("/relay/admin/overview", get(overview::overview))
        .route("/relay/admin/search", get(search))
        .route("/relay/admin/users", get(users::list))
        .route(
            "/relay/admin/users/{user_id}",
            get(users::detail)
                .patch(set_user_enabled)
                .delete(admin_delete_user),
        )
        .route(
            "/relay/admin/users/{user_id}/sessions",
            delete(users::end_sessions),
        )
        .route(
            "/relay/admin/users/{user_id}/reset-password",
            post(admin_reset_password),
        )
        .route("/relay/admin/registrations", get(users::registrations))
        .route(
            "/relay/admin/registrations/{request_id}/approve",
            post(approve_registration),
        )
        .route(
            "/relay/admin/registrations/{request_id}/reject",
            post(reject_registration),
        )
        .route("/relay/admin/devices", get(devices::list))
        .route("/relay/admin/devices/{device_id}", get(devices::detail))
        .route(
            "/relay/admin/devices/{device_id}/disconnect",
            post(devices::disconnect),
        )
        .route("/relay/admin/shares", get(shares::list))
        .route("/relay/admin/shares/{kind}/{id}", delete(shares::revoke))
        .route("/relay/admin/public-links", get(shares::public_links))
        .route("/relay/admin/previews", get(shares::previews))
        .route("/relay/admin/security", get(audit::summary))
        .route("/relay/admin/security/events", get(audit::events))
        .route("/relay/admin/sessions", get(audit::sessions))
        .route(
            "/relay/admin/sessions/{session_id}",
            delete(audit::end_session),
        )
        .route("/relay/admin/system", get(system::system))
        .route("/relay/admin/actions", get(stats::actions))
        .route("/relay/admin/performance", get(stats::performance_handler))
        .route(
            "/relay/admin/performance/devices",
            get(stats::performance_devices),
        )
        .route("/relay/admin/server", get(server::server))
        .route(
            "/relay/admin/settings/retention",
            get(stats::retention).patch(stats::update_retention),
        )
        .route(
            "/relay/admin/settings/registration",
            get(registration_settings_read).patch(update_registration_settings),
        )
}

async fn registration_settings_read(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match common::admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let settings = registration_settings(&conn, &state.oauth);
    Json(json!({
        "registrationEnabled": settings.get("enabled").cloned().unwrap_or(Value::Bool(true)),
        "settings": settings
    }))
    .into_response()
}

/// Users and devices whose name, email or ID matches, for the console's search.
async fn search(
    headers: HeaderMap,
    Query(query): Query<common::ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match common::admin_read(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    common::blocking(|| {
        let Some(search) = query.search() else {
            return Json(json!({ "users": [], "devices": [] })).into_response();
        };
        let pattern = common::like_pattern(&search);
        let users: Vec<Value> = conn
            .prepare(
                "SELECT id,username,email,role,enabled FROM relay_users
                 WHERE lower(username) LIKE ?1 ESCAPE '\\' OR lower(email) LIKE ?1 ESCAPE '\\' OR id=?2
                 ORDER BY username LIMIT 6",
            )
            .and_then(|mut stmt| {
                stmt.query_map(params![pattern, search], |row| {
                    Ok(json!({
                        "id": row.get::<_, String>(0)?,
                        "username": row.get::<_, String>(1)?,
                        "email": row.get::<_, String>(2)?,
                        "role": row.get::<_, String>(3)?,
                        "enabled": row.get::<_, i64>(4)? == 1,
                    }))
                })?
                .collect()
            })
            .unwrap_or_default();
        let devices: Vec<Value> = conn
            .prepare(
                "SELECT d.id,d.name,u.username,u.id,u.email FROM relay_devices d JOIN relay_users u ON u.id=d.owner_user_id
                 WHERE lower(d.name) LIKE ?1 ESCAPE '\\' OR d.id=?2 ORDER BY d.name LIMIT 6",
            )
            .and_then(|mut stmt| {
                stmt.query_map(params![pattern, search], |row| {
                    Ok(json!({
                        "id": row.get::<_, String>(0)?,
                        "name": row.get::<_, String>(1)?,
                        "ownerUsername": row.get::<_, String>(2)?,
                        "ownerUserId": row.get::<_, String>(3)?,
                        "ownerEmail": row.get::<_, String>(4)?,
                    }))
                })?
                .collect()
            })
            .unwrap_or_default();
        Json(json!({ "users": users, "devices": devices })).into_response()
    })
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdminQuery {
    days: Option<u32>,
}

/// The tunnel times the relay holds in memory for each online device.
pub(crate) async fn tunnel_times(state: &AppState) -> HashMap<String, (String, String)> {
    state
        .sockets
        .read()
        .await
        .iter()
        .map(|(id, socket)| {
            (
                id.clone(),
                (
                    socket.connected_at.clone(),
                    socket.last_heartbeat_at.clone(),
                ),
            )
        })
        .collect()
}

/// Admin views never carry thread titles: they come from conversation content
/// and only the owner and recipient see them on their own pages.
pub(crate) fn without_thread_title(mut share: Value) -> Value {
    if let Some(object) = share.as_object_mut() {
        object.insert("threadTitle".into(), Value::Null);
    }
    share
}

/// Rows per list of the legacy summary.
const LEGACY_ROWS: i64 = 500;

pub(crate) async fn relay_admin(
    headers: HeaderMap,
    Query(query): Query<AdminQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let tunnels = tunnel_times(&state).await;
    let (conn, _) = match common::admin_read(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    common::blocking(|| {
        // Kept in the response for older Web builds; it never filtered anything.
        let conversation_window_days = query.days.unwrap_or(7).clamp(1, 365);
        // Bounded: a relay with tens of thousands of accounts answers with the
        // first LEGACY_ROWS of each list and says so (`truncated`); the console
        // pages through its own routes.
        let mut stmt = match conn.prepare(
            "SELECT u.id,u.email,u.username,u.role,u.enabled,u.last_seen_at,u.created_at,
                    (SELECT COUNT(*) FROM relay_devices d WHERE d.owner_user_id=u.id)
             FROM relay_users u ORDER BY u.created_at ASC LIMIT ?1",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let users: Vec<Value> = stmt
            .query_map([LEGACY_ROWS], |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "email": row.get::<_, String>(1)?,
                    "username": row.get::<_, String>(2)?,
                    "role": row.get::<_, String>(3)?,
                    "enabled": row.get::<_, i64>(4)? == 1,
                    "lastSeenAt": row.get::<_, Option<String>>(5)?,
                    "createdAt": row.get::<_, String>(6)?,
                    "deviceCount": row.get::<_, i64>(7)?,
                    // The relay records no conversations; older Web builds read this.
                    "conversationCount": 0
                }))
            })
            .ok()
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default();
        drop(stmt);

        let mut stmt = match conn.prepare(
            "SELECT d.id,d.owner_user_id,d.name,d.token,d.token_preview,d.created_at,u.username,u.email,
                    (SELECT COUNT(*) FROM relay_thread_lineage l WHERE l.device_id=d.id),
                    (SELECT COUNT(parent_thread_id) FROM relay_thread_lineage l WHERE l.device_id=d.id),
                    (SELECT COUNT(DISTINCT workspace_id) FROM relay_thread_lineage l WHERE l.device_id=d.id)
             FROM relay_devices d JOIN relay_users u ON u.id=d.owner_user_id ORDER BY d.created_at ASC LIMIT ?1",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let devices: Vec<Value> = stmt
            .query_map([LEGACY_ROWS], |row| {
                let id: String = row.get(0)?;
                let tunnel = tunnels.get(&id);
                let (threads, agent_threads, workspaces): (i64, i64, i64) =
                    (row.get(8)?, row.get(9)?, row.get(10)?);
                Ok(json!({
                    "id": id,
                    "ownerUserId": row.get::<_, String>(1)?,
                    "name": row.get::<_, String>(2)?,
                    "token": row.get::<_, Option<String>>(3)?,
                    "tokenPreview": row.get::<_, String>(4)?,
                    "connected": tunnel.is_some(),
                    "connectedAt": tunnel.map(|(connected, _)| connected),
                    "lastHeartbeatAt": tunnel.map(|(_, heartbeat)| heartbeat),
                    "createdAt": row.get::<_, String>(5)?,
                    "ownerUsername": row.get::<_, String>(6)?,
                    "ownerEmail": row.get::<_, String>(7)?,
                    "ipAddress": Value::Null,
                    "threadCount": threads,
                    "agentThreadCount": agent_threads,
                    "workspaceCount": workspaces,
                    "workspaces": [],
                    "threads": []
                }))
            })
            .ok()
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default();
        drop(stmt);

        let user_ids: Vec<String> = users
            .iter()
            .filter_map(|user| user.get("id").and_then(Value::as_str).map(str::to_string))
            .collect();
        // Owners with shares or grants only, instead of two queries per account.
        let owners_with = |table: &str| -> Vec<String> {
            conn.prepare(&format!("SELECT DISTINCT owner_user_id FROM {table}"))
                .and_then(|mut stmt| stmt.query_map([], |row| row.get(0))?.collect())
                .unwrap_or_default()
        };
        let share_owners: std::collections::HashSet<String> =
            owners_with("relay_shares").into_iter().collect();
        let grant_owners: std::collections::HashSet<String> =
            owners_with("relay_access_grants").into_iter().collect();
        let shares: Vec<Value> = user_ids
            .iter()
            .filter(|id| share_owners.contains(*id))
            .flat_map(|id| relay_shares_for(&conn, "owner_user_id", id))
            .take(LEGACY_ROWS as usize)
            .map(without_thread_title)
            .collect();
        let grants: Vec<Value> = user_ids
            .iter()
            .filter(|id| grant_owners.contains(*id))
            .flat_map(|id| relay_grants_for(&conn, "owner_user_id", id))
            .take(LEGACY_ROWS as usize)
            .map(without_thread_title)
            .collect();
        let truncated = [users.len(), devices.len(), shares.len(), grants.len()]
            .iter()
            .any(|len| *len as i64 >= LEGACY_ROWS);
        let pending_registrations = if table_exists(&conn, "relay_pending_registrations") {
            conn.prepare(
                "SELECT id,email,username,created_at,provider FROM relay_pending_registrations
                 WHERE status='pending' ORDER BY created_at ASC",
            )
            .ok()
            .and_then(|mut stmt| {
                stmt.query_map([], |row| {
                    Ok(json!({
                        "id": row.get::<_, String>(0)?,
                        "email": row.get::<_, String>(1)?,
                        "username": row.get::<_, String>(2)?,
                        "createdAt": row.get::<_, String>(3)?,
                        "provider": row.get::<_, String>(4)?
                    }))
                })
                .ok()
                .map(|rows| rows.flatten().collect::<Vec<_>>())
            })
            .unwrap_or_default()
        } else {
            Vec::new()
        };
        let settings = registration_settings(&conn, &state.oauth);
        let registration_enabled = settings
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        Json(json!({
            "users": users,
            "devices": devices,
            "shares": shares,
            "grants": grants,
            "pendingRegistrations": pending_registrations,
            "settings": settings,
            "conversationWindowDays": conversation_window_days,
            "registrationEnabled": registration_enabled,
            "truncated": truncated
        }))
        .into_response()
    })
}

pub(crate) fn set_relay_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO relay_settings(key,value) VALUES (?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub(crate) async fn update_registration_settings(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let conn = state.store.conn.lock().await;
    let Some(admin) =
        authenticated_admin_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let before = registration_settings(&conn, &state.oauth);
    if body.get("googleAuthEnabled").and_then(Value::as_bool) == Some(true)
        && !state.oauth.available(OAuthProvider::Google)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "Google OAuth credentials are not configured.",
            )),
        )
            .into_response();
    }
    if body.get("githubAuthEnabled").and_then(Value::as_bool) == Some(true)
        && !state.oauth.available(OAuthProvider::Github)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "GitHub OAuth credentials are not configured.",
            )),
        )
            .into_response();
    }
    if body
        .get("emailVerificationEnabled")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "Email verification is not configured in the Rust relay.",
            )),
        )
            .into_response();
    }
    if let Some(enabled) = body.get("enabled").and_then(Value::as_bool) {
        let _ = set_relay_setting(
            &conn,
            "registrationEnabled",
            if enabled { "true" } else { "false" },
        );
    }
    if let Some(approval) = body.get("approvalRequired").and_then(Value::as_bool) {
        let _ = set_relay_setting(
            &conn,
            "registrationApprovalRequired",
            if approval { "true" } else { "false" },
        );
    }
    for (key, value) in [
        (
            "googleAuthEnabled",
            body.get("googleAuthEnabled").and_then(Value::as_bool),
        ),
        (
            "githubAuthEnabled",
            body.get("githubAuthEnabled").and_then(Value::as_bool),
        ),
        (
            "emailVerificationEnabled",
            body.get("emailVerificationEnabled")
                .and_then(Value::as_bool),
        ),
    ] {
        if let Some(value) = value {
            let _ = set_relay_setting(&conn, key, if value { "true" } else { "false" });
        }
    }
    if let Some(password) = body.get("registrationPassword") {
        match password {
            Value::Null => {
                let _ = conn.execute(
                    "DELETE FROM relay_settings WHERE key='registrationPassword'",
                    [],
                );
            }
            Value::String(password) if password.trim().len() >= 8 => {
                let _ = set_relay_setting(&conn, "registrationPassword", password.trim());
            }
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiError::new(
                        "bad_request",
                        "Registration password must be at least 8 characters",
                    )),
                )
                    .into_response();
            }
        }
    }
    let settings = registration_settings(&conn, &state.oauth);
    // Which settings changed, as before → after; the invite code itself is never logged.
    let mut changes = serde_json::Map::new();
    for key in [
        "enabled",
        "approvalRequired",
        "googleAuthEnabled",
        "githubAuthEnabled",
        "emailVerificationEnabled",
    ] {
        if before[key] != settings[key] {
            changes.insert(key.into(), json!([before[key], settings[key]]));
        }
    }
    match body.get("registrationPassword") {
        Some(Value::Null) => {
            changes.insert("inviteCode".into(), json!("removed"));
        }
        Some(Value::String(_)) => {
            changes.insert("inviteCode".into(), json!("set"));
        }
        _ => {}
    }
    if !changes.is_empty() {
        log_action(
            &conn,
            &admin,
            "settings.registration",
            None,
            Value::Object(changes),
        );
    }
    Json(json!({
        "registrationEnabled": settings.get("enabled").cloned().unwrap_or(Value::Bool(true)),
        "settings": settings
    }))
    .into_response()
}

/// Records an admin change: who, what, on which object and when. The detail
/// holds only setting names and values, counts and states, never secrets.
pub(crate) fn log_action(
    conn: &Connection,
    admin: &UserRow,
    action: &str,
    target: Option<(&str, &str, &str)>,
    detail: Value,
) {
    let detail = (!detail.is_null()).then(|| detail.to_string());
    if conn
        .execute(
            "INSERT INTO relay_admin_actions(admin_user_id,admin_username,action,target_type,target_id,target_label,detail,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                admin.id,
                admin.username,
                action,
                target.map(|t| t.0),
                target.map(|t| t.1),
                target.map(|t| t.2),
                detail,
                common::now_ms()
            ],
        )
        .is_err()
    {
        tracing::warn!(action, "Admin action log write failed");
    }
}

#[derive(Deserialize)]
pub(crate) struct SetUserEnabledInput {
    enabled: bool,
}

pub(crate) async fn set_user_enabled(
    Path(user_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetUserEnabledInput>,
) -> impl IntoResponse {
    let conn = state.store.conn.lock().await;
    let Some(admin) =
        authenticated_admin_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let Some(user) = load_user_by_id(&conn, &user_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", "User not found")),
        )
            .into_response();
    };
    if user.role == "admin" && !body.enabled {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "The admin user cannot be disabled",
            )),
        )
            .into_response();
    }
    if conn
        .execute(
            "UPDATE relay_users SET enabled=?1 WHERE id=?2",
            params![body.enabled as i64, user_id],
        )
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if (user.enabled == 1) != body.enabled {
        log_action(
            &conn,
            &admin,
            if body.enabled {
                "user.enabled"
            } else {
                "user.disabled"
            },
            Some(("user", &user.id, &user.username)),
            Value::Null,
        );
    }
    let updated = load_user_by_id(&conn, &user_id).unwrap_or(user);
    Json(user_json(&updated)).into_response()
}

pub(crate) async fn admin_delete_user(
    Path(user_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let conn = state.store.conn.lock().await;
    let Some(admin) =
        authenticated_admin_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let Some(user) = load_user_by_id(&conn, &user_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", "User not found")),
        )
            .into_response();
    };
    if user.role == "admin" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "The admin user cannot be deleted",
            )),
        )
            .into_response();
    }
    if conn
        .execute("DELETE FROM relay_users WHERE id=?1", params![user_id])
        .is_err()
    {
        return (
            StatusCode::CONFLICT,
            Json(ApiError::new(
                "conflict",
                "User still owns relay resources that must be reassigned",
            )),
        )
            .into_response();
    }
    log_action(
        &conn,
        &admin,
        "user.deleted",
        Some(("user", &user.id, &user.username)),
        Value::Null,
    );
    Json(json!({ "id": user_id })).into_response()
}

#[derive(Deserialize)]
pub(crate) struct ResetPasswordInput {
    password: String,
}

pub(crate) async fn admin_reset_password(
    Path(user_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<ResetPasswordInput>,
) -> impl IntoResponse {
    let conn = state.store.conn.lock().await;
    let Some(admin) =
        authenticated_admin_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let Some(user) = load_user_by_id(&conn, &user_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", "User not found")),
        )
            .into_response();
    };
    if user.role == "admin" || body.password.len() < 12 || body.password.len() > 1024 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "Only non-admin users can be reset to a password of at least 12 characters",
            )),
        )
            .into_response();
    }
    let oauth_only = crate::account::oauth_only(&conn, &user_id);
    let Ok((salt, hash)) = hash_password(&body.password) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let result = (|| -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE relay_users SET password_salt=?1,password_hash=?2 WHERE id=?3",
            params![salt, hash, user_id],
        )?;
        // Every non-admin reset is a one-time temporary password, including
        // accounts that still need first-vault setup with legacy upgrade disabled.
        tx.execute("INSERT INTO relay_account_auth(user_id,oauth_only,reset_pending,reset_consumed) VALUES (?1,?2,1,0)
            ON CONFLICT(user_id) DO UPDATE SET reset_pending=1,reset_consumed=0,auth_salt=NULL,auth_hash=NULL",params![user_id,oauth_only])?;
        tx.commit()?;
        Ok(())
    })();
    if result.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    log_action(
        &conn,
        &admin,
        "user.password_reset",
        Some(("user", &user.id, &user.username)),
        Value::Null,
    );
    Json(user_json(&user)).into_response()
}

pub(crate) async fn approve_registration(
    Path(request_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(admin) =
        authenticated_admin_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let record: Option<(String, String, String, String, String, Option<String>)> = conn
        .query_row(
            "SELECT email,username,password_salt,password_hash,provider,provider_subject
             FROM relay_pending_registrations WHERE id=?1 AND status='pending'",
            params![request_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .ok()
        .flatten();
    let Some((email, username, salt, hash, provider, provider_subject)) = record else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new(
                "not_found",
                "Pending registration was not found.",
            )),
        )
            .into_response();
    };
    let duplicate = conn
        .query_row(
            "SELECT 1 FROM relay_users WHERE email=?1 OR username=?2 LIMIT 1",
            params![email, username],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some();
    if duplicate {
        return (
            StatusCode::CONFLICT,
            Json(ApiError::new(
                "conflict",
                "A user with that email or username already exists.",
            )),
        )
            .into_response();
    }
    let pending_vault: Option<(String,String,String,String)> = conn.query_row(
        "SELECT user_id,vault_json,recovery_salt,recovery_hash FROM relay_pending_account_vaults WHERE request_id=?1",[&request_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().ok().flatten();
    let user_id = pending_vault
        .as_ref()
        .map(|p| p.0.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let now = now_rfc3339();
    let result = (|| -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO relay_users
             (id,email,username,password_hash,password_salt,role,enabled,last_seen_at,created_at)
             VALUES (?1,?2,?3,?4,?5,'user',1,NULL,?6)",
            params![user_id, email, username, hash, salt, now],
        )?;
        if matches!(provider.as_str(), "google" | "github") {
            if let Some(subject) = provider_subject.as_deref() {
                tx.execute(
                    "INSERT INTO relay_user_identities
                     (id,user_id,provider,provider_subject,provider_email,created_at)
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    params![
                        Uuid::new_v4().to_string(),
                        user_id,
                        provider,
                        subject,
                        email,
                        now
                    ],
                )?;
            }
        }
        if let Some((_, raw, recovery_salt, recovery_hash)) = &pending_vault {
            tx.execute(
                "UPDATE relay_users SET password_salt='',password_hash='' WHERE id=?1",
                [&user_id],
            )?;
            let vault: Value = serde_json::from_str(raw)?;
            crate::account::store_vault(&tx, &user_id, &vault)
                .map_err(|_| anyhow::anyhow!("Invalid pending vault"))?;
            tx.execute("INSERT INTO relay_account_auth(user_id,auth_salt,auth_hash,recovery_salt,recovery_hash) VALUES (?1,?2,?3,?4,?5)",params![user_id,salt,hash,recovery_salt,recovery_hash])?;
        } else if matches!(provider.as_str(), "google" | "github") {
            tx.execute(
                "INSERT INTO relay_account_auth(user_id,oauth_only) VALUES (?1,1)",
                [&user_id],
            )?;
        }
        tx.execute(
            "DELETE FROM relay_pending_account_vaults WHERE request_id=?1",
            [&request_id],
        )?;
        tx.execute(
            "UPDATE relay_pending_registrations
             SET status='approved',reviewed_at=?1,reviewed_by_user_id=?2 WHERE id=?3",
            params![now, admin.id, request_id],
        )?;
        tx.commit()?;
        Ok(())
    })();
    if result.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let Some(user) = load_user_by_id(&conn, &user_id) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    log_action(
        &conn,
        &admin,
        "registration.approved",
        Some(("user", &user.id, &user.username)),
        json!({ "provider": provider }),
    );
    Json(user_json(&user)).into_response()
}

pub(crate) async fn reject_registration(
    Path(request_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(admin) =
        authenticated_admin_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let username: Option<String> = conn
        .query_row(
            "SELECT username FROM relay_pending_registrations WHERE id=?1",
            [&request_id],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    // A rejected request keeps its row for the review history; the encrypted
    // vault it carried has no owner any more and goes.
    let result = (|| -> rusqlite::Result<usize> {
        let tx = conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE relay_pending_registrations
             SET status='rejected',reviewed_at=?1,reviewed_by_user_id=?2
             WHERE id=?3 AND status='pending'",
            params![now_rfc3339(), admin.id, request_id],
        )?;
        if changed == 1 {
            tx.execute(
                "DELETE FROM relay_pending_account_vaults WHERE request_id=?1",
                [&request_id],
            )?;
        }
        tx.commit()?;
        Ok(changed)
    })();
    match result {
        Ok(1) => {
            log_action(
                &conn,
                &admin,
                "registration.rejected",
                Some((
                    "registration",
                    &request_id,
                    username.as_deref().unwrap_or_default(),
                )),
                Value::Null,
            );
            Json(json!({ "id": request_id })).into_response()
        }
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(ApiError::new(
                "not_found",
                "Pending registration was not found.",
            )),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
