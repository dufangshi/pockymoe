//! Relay administration API: the admin summary, registration approval,
//! and user enable/reset/delete.
//! Split out of lib.rs so the privileged surface sits in one reviewable file.
use super::*;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdminQuery {
    token: Option<String>,
    relay_session: Option<String>,
    days: Option<u32>,
}

impl AdminQuery {
    fn token_query(&self) -> TokenQuery {
        TokenQuery {
            token: self.token.clone(),
            device_token: None,
            relay_session: self.relay_session.clone(),
        }
    }
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

/// Threads, agent threads (threads with a parent) and workspaces per device,
/// from the IDs devices report in heartbeats. No titles exist in this table.
pub(crate) fn lineage_counts(conn: &Connection) -> HashMap<String, (i64, i64, i64)> {
    conn.prepare(
        "SELECT device_id,COUNT(*),COUNT(parent_thread_id),COUNT(DISTINCT workspace_id)
         FROM relay_thread_lineage GROUP BY device_id",
    )
    .and_then(|mut stmt| {
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get(1)?, row.get(2)?, row.get(3)?),
            ))
        })?
        .collect()
    })
    .unwrap_or_default()
}

/// Admin views never carry thread titles: they come from conversation content
/// and only the owner and recipient see them on their own pages.
pub(crate) fn without_thread_title(mut share: Value) -> Value {
    if let Some(object) = share.as_object_mut() {
        object.insert("threadTitle".into(), Value::Null);
    }
    share
}

pub(crate) async fn relay_admin(
    headers: HeaderMap,
    Query(query): Query<AdminQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let tunnels = tunnel_times(&state).await;
    let conn = state.store.conn.lock().await;
    if authenticated_admin_user(
        &conn,
        &state.store.session_secret,
        &headers,
        &query.token_query(),
    )
    .is_none()
    {
        return unauthorized();
    }
    // Kept in the response for older Web builds; it never filtered anything.
    let conversation_window_days = query.days.unwrap_or(7).clamp(1, 365);
    let mut stmt = match conn.prepare(
        "SELECT u.id,u.email,u.username,u.role,u.enabled,u.last_seen_at,u.created_at,
                (SELECT COUNT(*) FROM relay_devices d WHERE d.owner_user_id=u.id)
         FROM relay_users u ORDER BY u.created_at ASC",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let users: Vec<Value> = stmt
        .query_map([], |row| {
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

    let lineage = lineage_counts(&conn);
    let mut stmt = match conn.prepare(
        "SELECT d.id,d.owner_user_id,d.name,d.token,d.token_preview,d.created_at,u.username,u.email
         FROM relay_devices d JOIN relay_users u ON u.id=d.owner_user_id ORDER BY d.created_at ASC",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let devices: Vec<Value> = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let tunnel = tunnels.get(&id);
            let (threads, agent_threads, workspaces) =
                lineage.get(&id).copied().unwrap_or_default();
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
    let shares: Vec<Value> = user_ids
        .iter()
        .flat_map(|id| relay_shares_for(&conn, "owner_user_id", id))
        .map(without_thread_title)
        .collect();
    let grants: Vec<Value> = user_ids
        .iter()
        .flat_map(|id| relay_grants_for(&conn, "owner_user_id", id))
        .map(without_thread_title)
        .collect();
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
        "registrationEnabled": registration_enabled
    }))
    .into_response()
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
    if authenticated_admin_user(&conn, &state.store.session_secret, &headers, &query).is_none() {
        return unauthorized();
    }
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
    Json(json!({
        "registrationEnabled": settings.get("enabled").cloned().unwrap_or(Value::Bool(true)),
        "settings": settings
    }))
    .into_response()
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
    if authenticated_admin_user(&conn, &state.store.session_secret, &headers, &query).is_none() {
        return unauthorized();
    }
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
    if authenticated_admin_user(&conn, &state.store.session_secret, &headers, &query).is_none() {
        return unauthorized();
    }
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
    if authenticated_admin_user(&conn, &state.store.session_secret, &headers, &query).is_none() {
        return unauthorized();
    }
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
        Ok(1) => Json(json!({ "id": request_id })).into_response(),
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn body(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers
    }

    #[tokio::test]
    async fn legacy_summary_reports_tunnel_times_lineage_and_no_thread_titles() {
        let (state, dir) = crate::tests::test_app_state("admin-legacy-summary");
        let token = {
            let conn = state.store.conn.lock().await;
            conn.execute_batch(
                "INSERT INTO relay_users VALUES ('admin','admin@x','admin','admin',1,NULL,'2026-01-01T00:00:00Z','s','h');
                 INSERT INTO relay_users VALUES ('owner','o@x','owner','user',1,NULL,'2026-01-01T00:00:00Z','s','h');
                 INSERT INTO relay_users VALUES ('guest','g@x','guest','user',1,NULL,'2026-01-01T00:00:00Z','s','h');
                 INSERT INTO relay_devices VALUES ('dev','owner','Laptop',NULL,'th','rcd_x...y','2026-01-02T00:00:00Z');
                 INSERT INTO relay_thread_lineage(device_id,thread_id,parent_thread_id,workspace_id) VALUES
                   ('dev','t1',NULL,'w1'),('dev','t2','t1','w1'),('dev','t3',NULL,'w2');
                 INSERT INTO relay_shares(id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,thread_id,thread_title,thread_access,workspace_access,created_at)
                   VALUES ('share','owner','owner','guest','guest','dev','Laptop','t1','Secret plans','read','none','2026-01-03T00:00:00Z');
                 INSERT INTO relay_access_grants(id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,scope,thread_access,workspace_access,created_at)
                   VALUES ('grant','owner','owner','guest','guest','dev','Laptop','device','control','write','2026-01-03T00:00:00Z');",
            )
            .unwrap();
            create_session(&conn, &state.store.session_secret, "admin").unwrap()
        };
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        state.sockets.write().await.insert(
            "dev".into(),
            DeviceSocket {
                tx,
                connection_id: Uuid::new_v4(),
                connected_at: "2026-01-04T00:00:00Z".into(),
                last_heartbeat_at: "2026-01-04T00:01:00Z".into(),
            },
        );
        let value = body(
            relay_admin(
                bearer(&token),
                Query(AdminQuery::default()),
                State(state.clone()),
            )
            .await
            .into_response(),
        )
        .await;
        let device = &value["devices"][0];
        assert_eq!(device["connectedAt"], "2026-01-04T00:00:00Z");
        assert_eq!(device["lastHeartbeatAt"], "2026-01-04T00:01:00Z");
        assert_eq!(device["threadCount"], 3);
        assert_eq!(device["agentThreadCount"], 1);
        assert_eq!(device["workspaceCount"], 2);
        assert_eq!(value["shares"][0]["threadTitle"], Value::Null);
        assert_eq!(value["grants"][0]["scope"], "device");
        let owner = value["users"]
            .as_array()
            .unwrap()
            .iter()
            .find(|user| user["id"] == "owner")
            .unwrap();
        assert_eq!(owner["deviceCount"], 1);
        let unauthorized = relay_admin(
            HeaderMap::new(),
            Query(AdminQuery::default()),
            State(state.clone()),
        )
        .await
        .into_response();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn rejecting_a_registration_deletes_its_pending_vault() {
        let (state, dir) = crate::tests::test_app_state("admin-reject-vault");
        let token = {
            let conn = state.store.conn.lock().await;
            conn.execute_batch(
                "INSERT INTO relay_users VALUES ('admin','admin@x','admin','admin',1,NULL,'2026-01-01T00:00:00Z','s','h');
                 INSERT INTO relay_pending_registrations(id,email,username,password_salt,password_hash,created_at)
                   VALUES ('req','n@x','newbie','s','h','2026-01-01T00:00:00Z');
                 INSERT INTO relay_pending_account_vaults(request_id,user_id,vault_json,recovery_salt,recovery_hash)
                   VALUES ('req','u','{}','s','h');",
            )
            .unwrap();
            create_session(&conn, &state.store.session_secret, "admin").unwrap()
        };
        let response = reject_registration(
            Path("req".into()),
            bearer(&token),
            Query(TokenQuery::default()),
            State(state.clone()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let conn = state.store.conn.lock().await;
        let vaults: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM relay_pending_account_vaults",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let status: String = conn
            .query_row(
                "SELECT status FROM relay_pending_registrations WHERE id='req'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((vaults, status.as_str()), (0, "rejected"));
        drop(conn);
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
