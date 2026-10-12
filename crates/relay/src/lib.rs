mod account;
#[cfg(test)]
mod account_tests;
mod admin;
mod apns;
mod auth_api;
mod auth_factors;
mod device_tokens;
mod notifications;
mod oauth;
mod peer;
mod preferences;
mod preview;
mod public_links;
mod route_acl;
mod security;
mod share_activity;
mod share_presence;
mod store;
mod thread_groups;
#[cfg(test)]
mod trust_tests;
#[cfg(test)]
mod web_origin_tests;
mod workbench;
use admin::*;
use route_acl::*;
use store::*;
pub use store::{
    inspect_relay_migration, migrate_relay_data_dir, migrate_relay_data_dir_with_options,
    RelayMigrationOptions, RelayMigrationReport,
};

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use axum::body::{to_bytes, Body, Bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::http::{header, HeaderMap, Method, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, delete, get, patch, post};
use axum::{Json, Router};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use pockymoe_protocol::{now_rfc3339, ApiError};
use rand::{rngs::OsRng, RngCore};
use rusqlite::backup::Backup;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use scrypt::{scrypt, Params as ScryptParams};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use oauth::{ExternalIdentity, OAuthConfig, OAuthProvider};

struct DeviceSocket {
    tx: tokio::sync::mpsc::Sender<String>,
    connection_id: Uuid,
    connected_at: String,
    last_heartbeat_at: String,
}

struct PendingDeviceRequest {
    device_id: String,
    connection_id: Uuid,
    peer_device_id: Option<String>,
    tx: tokio::sync::oneshot::Sender<Value>,
}

struct ClientSocket {
    tx: tokio::sync::mpsc::Sender<String>,
    device_id: String,
    supervisor_connection_id: Uuid,
    user_id: String,
    thread_id: Option<String>,
    attached_shell_id: Option<String>,
    session_token: String,
}

struct AppState {
    store: RelayStore,
    sockets: RwLock<HashMap<String, DeviceSocket>>,
    clients: RwLock<HashMap<String, ClientSocket>>,
    pending: StdMutex<HashMap<String, PendingDeviceRequest>>,
    web_dist: Option<PathBuf>,
    legacy_supervisor_token: Option<String>,
    oauth: OAuthConfig,
    oauth_client: reqwest::Client,
    admission: security::Admission,
    preview: preview::Hub,
}

pub async fn serve() -> Result<()> {
    let data_dir =
        std::env::var("POCKYMOE_RELAY_DATA_DIR").unwrap_or_else(|_| ".local/relay-server".into());
    let admin_username = std::env::var("POCKYMOE_ADMIN_USERNAME")
        .map_err(|_| anyhow::anyhow!("POCKYMOE_ADMIN_USERNAME is required"))?;
    let admin_username = normalize_username(&admin_username);
    let admin_password = std::env::var("POCKYMOE_ADMIN_PASSWORD")
        .map_err(|_| anyhow::anyhow!("POCKYMOE_ADMIN_PASSWORD is required"))?;
    let admin_email = std::env::var("POCKYMOE_ADMIN_EMAIL")
        .unwrap_or_else(|_| format!("{admin_username}@relay.local"))
        .trim()
        .to_ascii_lowercase();
    if admin_username.len() < 3 {
        bail!("POCKYMOE_ADMIN_USERNAME must contain at least 3 supported characters");
    }
    if admin_password.len() < 8 || (admin_username == "admin" && admin_password == "admin") {
        bail!("POCKYMOE_ADMIN_PASSWORD must be at least 8 characters and cannot use the default admin credential");
    }
    if !admin_email.contains('@') {
        bail!("POCKYMOE_ADMIN_EMAIL must be a valid email address");
    }
    let session_secret = security::session_secret(FsPath::new(&data_dir))?;
    let auto_migrate = std::env::var("POCKYMOE_RELAY_AUTO_MIGRATE")
        .ok()
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true"));
    let store = RelayStore::open_data_dir(FsPath::new(&data_dir), session_secret, auto_migrate)?;
    {
        let conn = store.conn.lock().await;
        let exists: Option<String> = conn
            .query_row(
                "SELECT id FROM relay_users WHERE username=?1",
                params![admin_username],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            let (password_salt, password_hash) = hash_password(&admin_password)?;
            conn.execute(
                "INSERT INTO relay_users
                 (id,email,username,password_hash,password_salt,role,enabled,last_seen_at,created_at)
                 VALUES (?1,?2,?3,?4,?5,'admin',1,NULL,?6)",
                params![
                    Uuid::new_v4().to_string(),
                    admin_email,
                    admin_username,
                    password_hash,
                    password_salt,
                    now_rfc3339()
                ],
            )?;
        }
    }
    let web_dist = std::env::var("POCKYMOE_RELAY_WEB_DIST_DIR")
        .ok()
        .map(PathBuf::from);
    let legacy_supervisor_token = std::env::var("POCKYMOE_RELAY_SUPERVISOR_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let registration_password = std::env::var("POCKYMOE_RELAY_REGISTRATION_PASSWORD")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let registration_enabled = std::env::var("POCKYMOE_RELAY_REGISTRATION_ENABLED")
        .ok()
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(true);
    {
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT OR IGNORE INTO relay_settings(key,value) VALUES ('registrationEnabled',?1)",
            params![if registration_enabled {
                "true"
            } else {
                "false"
            }],
        )?;
        if let Some(password) = registration_password.as_ref() {
            conn.execute(
                "INSERT OR IGNORE INTO relay_settings(key,value) VALUES ('registrationPassword',?1)",
                params![password],
            )?;
        }
    }
    let oauth = OAuthConfig::from_env()?;
    {
        let conn = store.conn.lock().await;
        for (key, available) in [
            (
                "googleAuthEnabled",
                oauth.initially_enabled(OAuthProvider::Google),
            ),
            (
                "githubAuthEnabled",
                oauth.initially_enabled(OAuthProvider::Github),
            ),
        ] {
            conn.execute(
                "INSERT OR IGNORE INTO relay_settings(key,value) VALUES (?1,?2)",
                params![key, if available { "true" } else { "false" }],
            )?;
        }
        conn.execute(
            "INSERT OR IGNORE INTO relay_settings(key,value) VALUES ('emailVerificationEnabled','false')",
            [],
        )?;
    }
    let state = Arc::new(AppState {
        store,
        sockets: RwLock::new(HashMap::new()),
        clients: RwLock::new(HashMap::new()),
        pending: StdMutex::new(HashMap::new()),
        web_dist,
        legacy_supervisor_token,
        oauth,
        oauth_client: reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?,
        admission: security::Admission::default(),
        preview: preview::Hub::from_env()?,
    });
    notifications::start(&state)?;
    let app = Router::new()
        .merge(auth_api::routes())
        .merge(notifications::routes())
        .merge(workbench::routes())
        .merge(preferences::routes())
        .merge(preview::routes())
        .route("/healthz", get(healthz))
        .route("/relay/auth/register", post(register))
        .route("/relay/auth/login", post(login))
        .route("/relay/auth/logout", post(logout))
        .route("/relay/auth/session", get(session))
        .route("/relay/auth/oauth/{provider}/start", get(oauth_start))
        .route("/relay/auth/oauth/{provider}/callback", get(oauth_callback))
        .route("/relay/account", patch(update_account))
        .route("/relay/account/password", patch(update_account_password))
        .route("/relay/portal", get(portal))
        .route("/relay/access", get(relay_access))
        .route("/relay/devices", get(list_devices).post(create_device))
        .route("/relay/devices/{device_id}", delete(delete_device))
        .route(
            "/relay/thread-links",
            get(public_links::list).post(public_links::create),
        )
        .route(
            "/relay/public-links/{id}",
            get(public_links::read).delete(public_links::revoke),
        )
        .route("/relay/shares", get(list_shares).post(create_share))
        .route(
            "/relay/shares/{share_id}",
            patch(update_share).delete(revoke_share),
        )
        .route("/relay/grants", get(list_grants).post(create_grant))
        .route(
            "/relay/grants/{grant_id}",
            patch(update_grant).delete(revoke_grant),
        )
        .route("/relay/admin", get(relay_admin))
        .route(
            "/relay/admin/settings/registration",
            patch(update_registration_settings),
        )
        .route(
            "/relay/admin/users/{user_id}",
            patch(set_user_enabled).delete(admin_delete_user),
        )
        .route(
            "/relay/admin/users/{user_id}/reset-password",
            post(admin_reset_password),
        )
        .route(
            "/relay/admin/registrations/{request_id}/approve",
            post(approve_registration),
        )
        .route(
            "/relay/admin/registrations/{request_id}/reject",
            post(reject_registration),
        )
        .route("/relay/devices/{device_id}/api/{*rest}", any(device_api))
        .route("/relay/api/{*rest}", any(relay_api_compat))
        .route("/relay/devices/{device_id}/healthz", get(device_healthz))
        .route("/relay/devices/{device_id}/presence", get(device_presence))
        .route("/supervisor/tunnel", get(supervisor_tunnel))
        .route("/relay/devices/{device_id}/ws", get(client_ws))
        .route("/relay/ws", get(client_ws_compat))
        .fallback(spa_fallback)
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security::browser_security,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            preview::middleware,
        ))
        .with_state(state);
    let host = std::env::var("POCKYMOE_RELAY_HOST")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("HOST")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "0.0.0.0".into());
    let port: u16 = std::env::var("POCKYMOE_RELAY_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .or_else(|| std::env::var("PORT").ok().and_then(|v| v.parse().ok()))
        .unwrap_or(8788);
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!("relay listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn legacy_sha256(password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hex::encode(hasher.finalize())
}

fn normalize_username(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|value| value.is_ascii_alphanumeric() || matches!(value, '_' | '-'))
        .collect()
}

// Anonymous credential work must not serialize database access or grow the
// blocking pool without bound. Waiting requests hold no database connection.
static ANONYMOUS_PASSWORD_WORK: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

fn hash_password(password: &str) -> Result<(String, String)> {
    let mut salt = [0_u8; 16];
    OsRng.fill_bytes(&mut salt);
    let salt = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(salt);
    Ok((salt.clone(), node_scrypt_hash(password, &salt)?))
}

#[cfg(test)]
struct PasswordLockProbe {
    secret: String,
    connection: std::sync::Weak<tokio::sync::Mutex<Connection>>,
    result: tokio::sync::oneshot::Sender<bool>,
}
#[cfg(test)]
static PASSWORD_LOCK_PROBES: std::sync::Mutex<Vec<PasswordLockProbe>> =
    std::sync::Mutex::new(Vec::new());

fn node_scrypt_hash(password: &str, salt: &str) -> Result<String> {
    #[cfg(test)]
    {
        let probe = {
            let mut probes = PASSWORD_LOCK_PROBES.lock().unwrap();
            probes
                .iter()
                .position(|p| p.secret == password)
                .map(|i| probes.remove(i))
        };
        if let Some(probe) = probe {
            let unlocked = probe
                .connection
                .upgrade()
                .is_some_and(|conn| conn.try_lock().is_ok());
            let _ = probe.result.send(unlocked);
        }
    }
    let params = ScryptParams::new(14, 8, 1, 32)?;
    let mut output = [0_u8; 32];
    scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut output)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(output))
}

fn verify_password(password: &str, salt: &str, expected: &str) -> bool {
    let actual = if salt == LEGACY_SHA256_SALT {
        legacy_sha256(password)
    } else {
        match node_scrypt_hash(password, salt) {
            Ok(hash) => hash,
            Err(_) => return false,
        }
    };
    actual.as_bytes().ct_eq(expected.as_bytes()).into()
}

fn hash_device_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionPayload {
    user_id: String,
    expires_at: u64,
    nonce: String,
}

fn create_session(conn: &Connection, session_secret: &str, user_id: &str) -> Result<String> {
    let mut nonce = [0_u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let payload = SessionPayload {
        user_id: user_id.to_string(),
        expires_at: now + 14 * 24 * 60 * 60 * 1000,
        nonce: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce),
    };
    let payload =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload)?);
    let mut mac = Hmac::<Sha256>::new_from_slice(session_secret.as_bytes())?;
    mac.update(payload.as_bytes());
    let signature =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    let token = format!("{payload}.{signature}");
    conn.execute(
        "DELETE FROM relay_auth_sessions WHERE expires_at<=?1",
        params![now],
    )?;
    conn.execute("INSERT INTO relay_auth_sessions(token_hash,user_id,created_at,expires_at) VALUES (?1,?2,?3,?4)",
        params![security::token_hash(&token), user_id, now, now + 14 * 24 * 60 * 60 * 1000])?;
    Ok(token)
}

fn verify_session(session_secret: &str, token: &str) -> Option<SessionPayload> {
    let mut parts = token.split('.');
    let payload = parts.next()?;
    let signature = parts.next()?;
    if parts.next().is_some() || payload.is_empty() || signature.is_empty() {
        return None;
    }
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature)
        .ok()?;
    let mut mac = Hmac::<Sha256>::new_from_slice(session_secret.as_bytes()).ok()?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&signature).ok()?;
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let payload: SessionPayload = serde_json::from_slice(&payload_bytes).ok()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    (payload.expires_at > now && !payload.user_id.is_empty() && !payload.nonce.is_empty())
        .then_some(payload)
}

#[derive(Deserialize, Default)]
struct TokenQuery {
    token: Option<String>,
    #[serde(rename = "deviceToken")]
    device_token: Option<String>,
    #[serde(rename = "relaySession")]
    relay_session: Option<String>,
}

impl TokenQuery {
    fn session_token(&self) -> Option<String> {
        self.relay_session
            .clone()
            .filter(|value| !value.is_empty())
            .or_else(|| self.token.clone().filter(|value| !value.is_empty()))
    }

    fn device_token(&self) -> Option<String> {
        self.device_token
            .clone()
            .filter(|value| !value.is_empty())
            .or_else(|| self.token.clone().filter(|value| !value.is_empty()))
    }
}

#[derive(Clone)]
struct UserRow {
    id: String,
    email: String,
    username: String,
    role: String,
    enabled: i64,
    created_at: String,
    avatar_url: Option<String>,
}

fn user_json(user: &UserRow) -> Value {
    json!({
        "id": user.id,
        "email": user.email,
        "username": user.username,
        "role": user.role,
        "enabled": user.enabled == 1,
        "createdAt": user.created_at,
        "avatarUrl": user.avatar_url
    })
}

fn relay_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM relay_settings WHERE key=?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

fn registration_settings(conn: &Connection, oauth: &OAuthConfig) -> Value {
    let enabled = relay_setting(conn, "registrationEnabled").as_deref() != Some("false");
    let registration_password_configured = relay_setting(conn, "registrationPassword").is_some();
    json!({
        "enabled": enabled,
        "registrationPassword": Value::Null,
        "registrationPasswordConfigured": registration_password_configured,
        "approvalRequired": relay_setting(conn, "registrationApprovalRequired").as_deref() == Some("true"),
        "googleAuthEnabled": relay_setting(conn, "googleAuthEnabled").as_deref() == Some("true"),
        "githubAuthEnabled": relay_setting(conn, "githubAuthEnabled").as_deref() == Some("true"),
        "emailVerificationEnabled": relay_setting(conn, "emailVerificationEnabled").as_deref() == Some("true"),
        "googleAuthAvailable": oauth.available(OAuthProvider::Google),
        "githubAuthAvailable": oauth.available(OAuthProvider::Github),
        "emailVerificationAvailable": false
    })
}

fn session_json(conn: &Connection, user: Option<&UserRow>, oauth: &OAuthConfig) -> Value {
    let settings = registration_settings(conn, oauth);
    json!({
        "authenticated": user.is_some(),
        "account": {"mode":"legacy","setupRequired":false,"recovery":false,"hasVault":false,"accountKeyId":null},
        "user": user.map(user_json),
        "registrationEnabled": settings.get("enabled").cloned().unwrap_or(Value::Bool(true)),
        "registrationSettings": settings
    })
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    {
        let value = value.trim();
        if let Some(token) = value
            .strip_prefix("Bearer ")
            .or_else(|| value.strip_prefix("bearer "))
        {
            let token = token.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

fn relay_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let (name, value) = cookie.trim().split_once('=')?;
                (name == "remote_codex_relay_session" && !value.is_empty())
                    .then(|| value.to_string())
            })
        })
}

fn extract_session_token(headers: &HeaderMap, query: &TokenQuery) -> Option<String> {
    if let Some(token) = headers.get("x-pockymoe-resolved-session") {
        return token
            .to_str()
            .ok()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
    }
    bearer_token(headers)
        .or_else(|| query.session_token())
        .or_else(|| {
            if headers
                .get("x-remote-codex-auth-realm")
                .is_some_and(|v| v == "admin")
            {
                security::cookie(headers, "remote_codex_relay_admin_session")
            } else {
                relay_cookie(headers)
            }
        })
}

fn load_user_by_id(conn: &Connection, user_id: &str) -> Option<UserRow> {
    conn.query_row(
        "SELECT id, email, username, role, enabled, created_at, (SELECT avatar_data_url FROM relay_user_profiles WHERE user_id=relay_users.id) FROM relay_users WHERE id=?1",
        params![user_id],
        |row| {
            Ok(UserRow {
                id: row.get(0)?,
                email: row.get(1)?,
                username: row.get(2)?,
                role: row.get(3)?,
                enabled: row.get(4)?,
                created_at: row.get(5)?,
                avatar_url: row.get(6)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

fn load_user_by_session(conn: &Connection, session_secret: &str, token: &str) -> Option<UserRow> {
    let payload = verify_session(session_secret, token)?;
    let live: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM relay_auth_sessions WHERE token_hash=?1 AND user_id=?2 AND expires_at>?3)",
        params![security::token_hash(token), payload.user_id, chrono::Utc::now().timestamp_millis()],
        |row| row.get(0),
    ).ok()?;
    live.then(|| load_user_by_id(conn, &payload.user_id))
        .flatten()
        .filter(|user| user.enabled == 1)
}

fn authenticated_user(
    conn: &Connection,
    session_secret: &str,
    headers: &HeaderMap,
    query: &TokenQuery,
) -> Option<UserRow> {
    let user = extract_session_token(headers, query)
        .and_then(|token| load_user_by_session(conn, session_secret, &token))
        .filter(|user| user.role != "admin");
    if let Some(user) = user.as_ref() {
        let _ = conn.execute(
            "UPDATE relay_users SET last_seen_at=?1 WHERE id=?2",
            params![now_rfc3339(), user.id],
        );
    }
    user
}

fn authenticated_admin_user(
    conn: &Connection,
    session_secret: &str,
    headers: &HeaderMap,
    _query: &TokenQuery,
) -> Option<UserRow> {
    let credential = if headers.contains_key("x-pockymoe-resolved-session") {
        extract_session_token(headers, &TokenQuery::default())
    } else {
        bearer_token(headers)
            .or_else(|| security::cookie(headers, "remote_codex_relay_admin_session"))
    };
    let user = credential
        .and_then(|token| load_user_by_session(conn, session_secret, &token))
        .filter(|user| user.role == "admin");
    if let Some(user) = user.as_ref() {
        let _ = conn.execute(
            "UPDATE relay_users SET last_seen_at=?1 WHERE id=?2",
            params![now_rfc3339(), user.id],
        );
    }
    user
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "code": "unauthorized", "message": "login required" })),
    )
        .into_response()
}

fn with_session_cookie(mut response: Response, token: &str) -> Response {
    let value = format!(
        "remote_codex_relay_session={token}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}",
        14 * 24 * 60 * 60
    );
    if let Ok(value) = value.parse() {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

fn with_cleared_session_cookie(mut response: Response) -> Response {
    if let Ok(value) =
        "remote_codex_relay_session=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0".parse()
    {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

fn preview_token(token: &str) -> String {
    if token.len() <= 11 {
        return token.to_string();
    }
    format!(
        "{}...{}",
        &token[..7.min(token.len())],
        &token[token.len().saturating_sub(4)..]
    )
}

struct DeviceJsonInput<'a> {
    id: &'a str,
    owner_user_id: &'a str,
    name: &'a str,
    created_at: &'a str,
    connected: bool,
    connected_at: Option<&'a str>,
    last_heartbeat_at: Option<&'a str>,
    token: Option<&'a str>,
    token_preview: Option<&'a str>,
}

fn device_json(input: DeviceJsonInput<'_>) -> Value {
    json!({
        "id": input.id,
        "ownerUserId": input.owner_user_id,
        "name": input.name,
        "token": input.token,
        "tokenPreview": input.token_preview
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| input.token.map(preview_token))
            .unwrap_or_else(|| "••••".into()),
        "connected": input.connected,
        "connectedAt": input.connected_at,
        "lastHeartbeatAt": input.last_heartbeat_at,
        "createdAt": input.created_at
    })
}

async fn healthz(State(state): State<Arc<AppState>>) -> Json<Value> {
    let sockets = state.sockets.read().await;
    let connected = sockets.len();
    let primary = sockets.values().next();
    Json(json!({
        "status": "ok",
        "timestamp": now_rfc3339(),
        "connectedSupervisors": connected,
        "supervisorConnected": connected > 0,
        "supervisorConnectedAt": primary.map(|socket| socket.connected_at.as_str()),
        "lastSupervisorHeartbeatAt": primary.map(|socket| socket.last_heartbeat_at.as_str()),
        "supervisorCount": connected
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterInput {
    user_id: String,
    email: String,
    username: String,
    auth_key: String,
    recovery_auth_key: String,
    vault: Value,
    registration_password: Option<String>,
}

async fn register(State(state): State<Arc<AppState>>, Json(body): Json<RegisterInput>) -> Response {
    let username = normalize_username(&body.username);
    let email = body.email.trim().to_ascii_lowercase();
    let id = body.user_id.clone();
    if !account::canonical_user_id(&id)
        || body.vault["userId"].as_str() != Some(&id)
        || username.len() < 3
        || username.len() > 80
        || email.len() > 320
        || !email.contains('@')
        || !account::valid_key(&body.auth_key)
        || !account::valid_key(&body.recovery_auth_key)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("bad_request", "Invalid registration fields")),
        )
            .into_response();
    }
    if let Err(error) = account::validate_vault(&body.vault, &id) {
        return error.into_response();
    }
    if body.vault["revision"] != 1 {
        return (
            StatusCode::CONFLICT,
            Json(ApiError::new(
                "vault_conflict",
                "First vault revision must be 1",
            )),
        )
            .into_response();
    }
    let Ok(password_work) = ANONYMOUS_PASSWORD_WORK.acquire().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let auth_key = body.auth_key.clone();
    let recovery_key = body.recovery_auth_key.clone();
    let hashes = tokio::task::spawn_blocking(move || -> Result<_> {
        Ok((hash_password(&auth_key)?, hash_password(&recovery_key)?))
    })
    .await;
    drop(password_work);
    let Ok(Ok(((salt, hash), (recovery_salt, recovery_hash)))) = hashes else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let conn = state.store.conn.lock().await;
    if relay_setting(&conn, "registrationEnabled").as_deref() == Some("false") {
        return (
            StatusCode::FORBIDDEN,
            Json(ApiError::new(
                "forbidden",
                "Registration is currently disabled",
            )),
        )
            .into_response();
    }
    if let Some(expected) = relay_setting(&conn, "registrationPassword") {
        if body.registration_password.as_deref().unwrap_or_default() != expected {
            return (
                StatusCode::FORBIDDEN,
                Json(ApiError::new("forbidden", "Invalid registration password")),
            )
                .into_response();
        }
    }
    let id_taken = conn.query_row("SELECT EXISTS(SELECT 1 FROM relay_users WHERE id=?1 UNION ALL SELECT 1 FROM relay_pending_registrations WHERE id=?1 UNION ALL SELECT 1 FROM relay_pending_account_vaults WHERE user_id=?1)",[&id],|r|r.get::<_,bool>(0));
    match id_taken {
        Ok(true) => {
            return (
                StatusCode::CONFLICT,
                Json(ApiError::new("user_id_taken", "User id already exists")),
            )
                .into_response()
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Ok(false) => {}
    }
    let Ok(tx) = conn.unchecked_transaction() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if relay_setting(&tx, "registrationApprovalRequired").as_deref() == Some("true") {
        let result = (|| -> Result<Value> {
            let request = insert_pending_registration(
                &tx,
                &email,
                &username,
                &salt,
                &hash,
                "password",
                None,
                Some(&id),
            )?;
            let request_id = request["id"]
                .as_str()
                .context("Pending request missing id")?;
            if request_id != id {
                anyhow::bail!("Pending request identity mismatch");
            }
            // Only a request created by this registration may receive its vault.
            tx.execute("INSERT INTO relay_pending_account_vaults(request_id,user_id,vault_json,recovery_salt,recovery_hash) VALUES (?1,?2,?3,?4,?5)",params![request_id,id,serde_json::to_string(&body.vault)?,recovery_salt,recovery_hash])?;
            Ok(request)
        })();
        return match result {
            Ok(request) if tx.commit().is_ok() => (
                StatusCode::ACCEPTED,
                Json(json!({"pendingApproval":true,"request":request})),
            )
                .into_response(),
            _ => (
                StatusCode::CONFLICT,
                Json(ApiError::new("conflict", "Registration already exists")),
            )
                .into_response(),
        };
    }
    let result = (|| -> Result<()> {
        tx.execute("INSERT INTO relay_users(id,email,username,password_salt,password_hash,role,enabled,created_at) VALUES (?1,?2,?3,'','','user',1,?4)",params![id,email,username,now_rfc3339()])?;
        account::store_vault(&tx, &id, &body.vault)
            .map_err(|_| anyhow::anyhow!("Invalid vault"))?;
        tx.execute("INSERT INTO relay_account_auth(user_id,auth_salt,auth_hash,recovery_salt,recovery_hash) VALUES (?1,?2,?3,?4,?5)",params![id,salt,hash,recovery_salt,recovery_hash])?;
        Ok(())
    })();
    if result.is_err() || tx.commit().is_err() {
        return (
            StatusCode::CONFLICT,
            Json(ApiError::new("conflict", "User already exists")),
        )
            .into_response();
    }
    let Some(user) = load_user_by_id(&conn, &id) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    auth_api::login_response(&conn, &state, &user, &HeaderMap::new(), true, false)
        .unwrap_or_else(IntoResponse::into_response)
}

fn insert_pending_registration(
    conn: &Connection,
    email: &str,
    username: &str,
    password_salt: &str,
    password_hash: &str,
    provider: &str,
    provider_subject: Option<&str>,
    requested_user_id: Option<&str>,
) -> Result<Value> {
    let user_exists = conn
        .query_row(
            "SELECT 1 FROM relay_users WHERE email=?1 OR username=?2 LIMIT 1",
            params![email, username],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if user_exists {
        bail!("A user with that email or username already exists.");
    }
    let existing = conn
        .query_row(
            "SELECT id,email,username,created_at,provider
             FROM relay_pending_registrations
             WHERE status='pending' AND (email=?1 OR username=?2)
             ORDER BY created_at DESC LIMIT 1",
            params![email, username],
            |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "email": row.get::<_, String>(1)?,
                    "username": row.get::<_, String>(2)?,
                    "createdAt": row.get::<_, String>(3)?,
                    "provider": row.get::<_, String>(4)?
                }))
            },
        )
        .optional()?;
    if existing.is_some() {
        bail!("Registration already exists");
    }
    let id = requested_user_id
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let created_at = now_rfc3339();
    conn.execute(
        "INSERT INTO relay_pending_registrations(
           id,email,username,password_salt,password_hash,created_at,status,
           reviewed_at,reviewed_by_user_id,provider,provider_subject
         ) VALUES (?1,?2,?3,?4,?5,?6,'pending',NULL,NULL,?7,?8)",
        params![
            id,
            email,
            username,
            password_salt,
            password_hash,
            created_at,
            provider,
            provider_subject
        ],
    )?;
    Ok(json!({
        "id": id,
        "email": email,
        "username": username,
        "createdAt": created_at,
        "provider": provider
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginInput {
    identifier: Option<String>,
    username: Option<String>,
    login: Option<String>,
    password: Option<String>,
    auth_key: Option<String>,
    #[serde(default)]
    upgrade: bool,
}

async fn login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<LoginInput>,
) -> Response {
    let ident = body
        .identifier
        .or(body.username)
        .or(body.login)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let key_login = body.auth_key.is_some();
    let secret = body.auth_key.or(body.password).unwrap_or_default();
    if secret.len() > 1024
        || ident.len() > 320
        || !state.admission.allow(format!("login:{ident}"), 30, 300)
    {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    if key_login && !account::valid_key(&secret) {
        return unauthorized();
    }
    static PASSWORD_WORK: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let Ok(_permit) = PASSWORD_WORK.try_acquire() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let conn = state.store.conn.lock().await;
    let row: Option<(String,String,String)> = conn.query_row(
        "SELECT id,password_salt,password_hash FROM relay_users WHERE (username=?1 OR email=?1) AND enabled=1",[ident],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().ok().flatten();
    let row = row.and_then(|(id,salt,hash)| {
        let user = load_user_by_id(&conn,&id)?;
        if !key_login && body.upgrade && account::has_vault(&conn,&id) {return None;}
        if user.role == "admin" { return (!key_login).then_some((id,salt,hash)); }
        if !key_login && account::reset_available(&conn,&id) { return Some((id,salt,hash)); }
        if account::oauth_only(&conn,&id) { return None; }
        if key_login {
            conn.query_row("SELECT auth_salt,auth_hash FROM relay_account_auth WHERE user_id=?1 AND auth_hash IS NOT NULL",[&id],|r|Ok((id.clone(),r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional().ok().flatten()
        } else if body.upgrade && account::legacy_password_allowed(&conn,&id) { Some((id,salt,hash)) } else { None }
    });
    drop(conn);
    let verified = tokio::task::spawn_blocking(move || {
        let proof = row
            .as_ref()
            .map(|(_, salt, hash)| verify_password(&secret, salt, hash))
            .unwrap_or_else(|| {
                // Spend the same slow work for unknown or unavailable credentials.
                let _ = node_scrypt_hash(&secret, "pockymoe-invalid-login");
                false
            });
        let upgrade_hash = if proof
            && row
                .as_ref()
                .is_some_and(|(_, salt, _)| salt == LEGACY_SHA256_SALT)
        {
            hash_password(&secret).ok()
        } else {
            None
        };
        row.filter(|_| proof).map(|row| (row, upgrade_hash))
    })
    .await
    .ok()
    .flatten();
    let conn = state.store.conn.lock().await;
    let Some(((id, salt, hash), upgrade_hash)) = verified else {
        return unauthorized();
    };
    // Recheck the exact credential after the slow hash; reset/disable may have raced it.
    let table_check = if key_login {
        "SELECT EXISTS(SELECT 1 FROM relay_account_auth a JOIN relay_users u ON u.id=a.user_id WHERE a.user_id=?1 AND a.auth_salt=?2 AND a.auth_hash=?3 AND u.enabled=1 AND a.oauth_only=0)"
    } else {
        "SELECT EXISTS(SELECT 1 FROM relay_users WHERE id=?1 AND password_salt=?2 AND password_hash=?3 AND enabled=1)"
    };
    if !conn
        .query_row(table_check, params![id, salt, hash], |r| {
            r.get::<_, bool>(0)
        })
        .unwrap_or(false)
    {
        return unauthorized();
    }
    let Some(user) = load_user_by_id(&conn, &id) else {
        return unauthorized();
    };
    let reset = !key_login && account::reset_available(&conn, &id);
    if !key_login && !reset && user.role != "admin" && !account::legacy_password_allowed(&conn, &id)
    {
        return unauthorized();
    }
    if let Some((new_salt, new_hash)) = upgrade_hash {
        if conn
            .execute(
                "UPDATE relay_users SET password_salt=?2,password_hash=?3 WHERE id=?1",
                params![id, new_salt, new_hash],
            )
            .is_err()
        {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    let mode = if reset {
        account::RESET
    } else {
        account::login_mode(&conn, &user)
    };
    let has_factors = auth_factors::has_factors(&conn, &id);
    if has_factors && (reset || !auth_factors::trusted(&conn, &id, &headers)) {
        auth_api::challenge_response_mode(&conn, &user, &headers, mode)
            .unwrap_or_else(IntoResponse::into_response)
    } else {
        auth_api::login_response_mode(
            &conn,
            &state,
            &user,
            &headers,
            !has_factors || mode == account::SETUP,
            false,
            mode,
        )
        .unwrap_or_else(IntoResponse::into_response)
    }
}

async fn logout(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let conn = state.store.conn.lock().await;
    if let Some(token) = extract_session_token(&headers, &query) {
        if security::revoke_session(&conn, &token).is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    let mut response =
        with_cleared_session_cookie(Json(session_json(&conn, None, &state.oauth)).into_response());
    if headers
        .get("x-remote-codex-auth-realm")
        .is_some_and(|v| v == "admin")
    {
        response.headers_mut().insert(
            header::SET_COOKIE,
            "remote_codex_relay_admin_session=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0"
                .parse()
                .unwrap(),
        );
    }
    response
}

async fn session(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let conn = state.store.conn.lock().await;
    let token = extract_session_token(&headers, &query);
    let user = token
        .as_deref()
        .and_then(|token| load_user_by_session(&conn, &state.store.session_secret, token));
    Json(match (user, token) {
        (Some(user), Some(token)) => account::session_json(&conn, &user, &state.oauth, &token),
        _ => session_json(&conn, None, &state.oauth),
    })
}

async fn oauth_start(
    Path(provider): Path<String>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let Some(provider) = OAuthProvider::parse(&provider) else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", "OAuth provider was not found")),
        )
            .into_response();
    };
    let enabled = {
        let conn = state.store.conn.lock().await;
        relay_setting(
            &conn,
            if provider == OAuthProvider::Google {
                "googleAuthEnabled"
            } else {
                "githubAuthEnabled"
            },
        )
        .as_deref()
            == Some("true")
    };
    if !enabled || !state.oauth.available(provider) {
        return (
            StatusCode::FORBIDDEN,
            Json(ApiError::new(
                "forbidden",
                format!("{} authentication is disabled.", provider.display_name()),
            )),
        )
            .into_response();
    }
    let callback = state.oauth.callback_url(&headers, provider);
    let result = {
        let conn = state.store.conn.lock().await;
        oauth::sign_state(provider, &state.store.session_secret).and_then(|signed| {
            let (browser, verifier) = security::start_oauth(&conn, &signed, &callback)?;
            let target = state
                .oauth
                .authorization_url(provider, &callback, &signed, &verifier)?;
            Ok((target, browser))
        })
    };
    match result {
        Ok((target, browser)) => {
            let mut response = Redirect::temporary(&target).into_response();
            response.headers_mut().append(header::SET_COOKIE, format!("remote_codex_oauth={browser}; HttpOnly; Secure; SameSite=Lax; Path=/relay/auth/oauth; Max-Age=600").parse().unwrap());
            response
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
struct OAuthCallbackQuery {
    code: String,
    state: String,
}

async fn oauth_callback(
    Path(provider): Path<String>,
    headers: HeaderMap,
    Query(query): Query<OAuthCallbackQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let Some(provider) = OAuthProvider::parse(&provider) else {
        return Redirect::temporary(
            &state
                .oauth
                .browser_url("/relay-portal?oauthError=OAuth%20provider%20is%20invalid."),
        )
        .into_response();
    };
    if !oauth::verify_state(&query.state, provider, &state.store.session_secret) {
        return Redirect::temporary(&state.oauth.browser_url(
            "/relay-portal?oauthError=OAuth%20request%20expired%20or%20was%20invalid.",
        ))
        .into_response();
    }
    let pending = {
        let conn = state.store.conn.lock().await;
        security::cookie(&headers, "remote_codex_oauth")
            .and_then(|browser| security::consume_oauth(&conn, &query.state, &browser))
    };
    let Some((verifier, callback)) = pending else {
        return oauth_error_redirect(
            &state.oauth,
            "OAuth request expired or belongs to another browser.",
        );
    };
    let identity = match state
        .oauth
        .fetch_identity(
            &state.oauth_client,
            provider,
            &query.code,
            &callback,
            &verifier,
        )
        .await
    {
        Ok(identity) => identity,
        Err(error) => {
            tracing::warn!(error = %error, provider = provider.as_str(), "OAuth identity lookup failed");
            return oauth_error_redirect(&state.oauth, "OAuth authentication failed.");
        }
    };
    let outcome = {
        let conn = state.store.conn.lock().await;
        authenticate_external_identity(&conn, &state.store.session_secret, &identity)
    };
    match outcome {
        Ok(OAuthOutcome::Pending) => {
            Redirect::temporary(&state.oauth.browser_url("/relay-portal?oauthPending=1"))
                .into_response()
        }
        Ok(OAuthOutcome::Login(token)) => {
            let conn = state.store.conn.lock().await;
            let Some(user) = load_user_by_session(&conn, &state.store.session_secret, &token)
            else {
                return unauthorized();
            };
            if security::revoke_session(&conn, &token).is_err() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            let has_factors = auth_factors::has_factors(&conn, &user.id);
            let result = if has_factors && !auth_factors::trusted(&conn, &user.id, &headers) {
                auth_api::challenge_response(&conn, &user, &headers)
            } else {
                auth_api::login_response(&conn, &state, &user, &headers, true, false)
            };
            let mut response = match result {
                Ok(r) => r,
                Err(e) => return e.into_response(),
            };
            *response.status_mut() = StatusCode::SEE_OTHER;
            response.headers_mut().insert(
                header::LOCATION,
                state.oauth.browser_url("/relay-portal").parse().unwrap(),
            );
            *response.body_mut() = Body::empty();
            response
        }
        Err(message) => oauth_error_redirect(&state.oauth, &message),
    }
}

enum OAuthOutcome {
    Login(String),
    Pending,
}

fn authenticate_external_identity(
    conn: &Connection,
    session_secret: &str,
    identity: &ExternalIdentity,
) -> std::result::Result<OAuthOutcome, String> {
    let linked: Option<String> = conn
        .query_row(
            "SELECT user_id FROM relay_user_identities
             WHERE provider=?1 AND provider_subject=?2",
            params![identity.provider.as_str(), identity.subject],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "OAuth identity lookup failed.".to_string())?;
    if let Some(user_id) = linked {
        let enabled: Option<i64> = conn
            .query_row(
                "SELECT enabled FROM relay_users WHERE id=?1",
                params![user_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| "OAuth account lookup failed.".to_string())?;
        if enabled != Some(1) {
            return Err("This relay account is disabled.".into());
        }
        let token = create_session(conn, session_secret, &user_id)
            .map_err(|_| "OAuth session creation failed.".to_string())?;
        let _ = conn.execute(
            "UPDATE relay_users SET last_seen_at=?1 WHERE id=?2",
            params![now_rfc3339(), user_id],
        );
        return Ok(OAuthOutcome::Login(token));
    }
    if relay_setting(conn, "registrationEnabled").as_deref() == Some("false") {
        return Err("Registration is currently disabled.".into());
    }
    let email_exists = conn
        .query_row(
            "SELECT 1 FROM relay_users WHERE email=?1",
            params![identity.email],
            |_| Ok(()),
        )
        .optional()
        .map_err(|_| "OAuth account lookup failed.".to_string())?
        .is_some();
    if email_exists {
        return Err(
            "An account with this email already exists. Sign in with its current method before linking OAuth."
                .into(),
        );
    }
    let username = available_username(conn, &identity.username)
        .map_err(|_| "OAuth username allocation failed.".to_string())?;
    let (password_salt, password_hash) = (String::new(), String::new());
    if relay_setting(conn, "registrationApprovalRequired").as_deref() == Some("true") {
        insert_pending_registration(
            conn,
            &identity.email,
            &username,
            &password_salt,
            &password_hash,
            identity.provider.as_str(),
            Some(&identity.subject),
            None,
        )
        .map_err(|error| error.to_string())?;
        return Ok(OAuthOutcome::Pending);
    }
    let user_id = Uuid::new_v4().to_string();
    let now = now_rfc3339();
    let tx = conn
        .unchecked_transaction()
        .map_err(|_| "OAuth account creation failed.".to_string())?;
    tx.execute(
        "INSERT INTO relay_users
         (id,email,username,password_hash,password_salt,role,enabled,last_seen_at,created_at)
         VALUES (?1,?2,?3,?4,?5,'user',1,?6,?6)",
        params![
            user_id,
            identity.email,
            username,
            password_hash,
            password_salt,
            now
        ],
    )
    .map_err(|_| "OAuth account creation failed.".to_string())?;
    tx.execute(
        "INSERT INTO relay_user_identities
         (id,user_id,provider,provider_subject,provider_email,created_at)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![
            Uuid::new_v4().to_string(),
            user_id,
            identity.provider.as_str(),
            identity.subject,
            identity.email,
            now
        ],
    )
    .map_err(|_| "OAuth identity creation failed.".to_string())?;
    tx.execute(
        "INSERT INTO relay_account_auth(user_id,oauth_only) VALUES (?1,1)",
        [&user_id],
    )
    .map_err(|_| "OAuth account creation failed.".to_string())?;
    tx.commit()
        .map_err(|_| "OAuth account creation failed.".to_string())?;
    let token = create_session(conn, session_secret, &user_id)
        .map_err(|_| "OAuth session creation failed.".to_string())?;
    Ok(OAuthOutcome::Login(token))
}

fn available_username(conn: &Connection, value: &str) -> rusqlite::Result<String> {
    let mut base = normalize_username(value);
    base.truncate(48);
    if base.is_empty() {
        base = "user".into();
    }
    if base.len() < 3 {
        base.push_str("user");
    }
    let mut candidate = base.clone();
    let mut suffix = 1_u32;
    loop {
        let taken = conn
            .query_row(
                "SELECT 1 FROM relay_users WHERE username=?1
                 UNION ALL
                 SELECT 1 FROM relay_pending_registrations
                 WHERE username=?1 AND status='pending' LIMIT 1",
                params![candidate],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !taken {
            return Ok(candidate);
        }
        let prefix: String = base.chars().take(42).collect();
        candidate = format!("{prefix}-{suffix}");
        suffix += 1;
    }
}

fn oauth_error_redirect(oauth: &OAuthConfig, message: &str) -> Response {
    let encoded: String = url::form_urlencoded::byte_serialize(message.as_bytes()).collect();
    Redirect::temporary(&oauth.browser_url(&format!("/relay-portal?oauthError={encoded}")))
        .into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateAccountInput {
    username: Option<String>,
    // Missing preserves the image; null explicitly restores the default avatar.
    #[serde(default)]
    avatar_url: Value,
}

fn avatar_data(input: &Value) -> Result<Option<&str>> {
    if input.is_null() {
        return Ok(None);
    }
    let value = input
        .as_str()
        .context("Avatar must be an uploaded raster image")?;
    if value.len() > 131072 {
        bail!("Avatar must be no larger than 128 KiB after resizing");
    }
    let (mime, encoded) = value.split_once(",").context("Invalid avatar data")?;
    use base64::{engine::general_purpose::STANDARD, Engine};
    let bytes = STANDARD
        .decode(encoded)
        .context("Invalid avatar encoding")?;
    let valid = match mime {
        "data:image/png;base64" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "data:image/jpeg;base64" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "data:image/webp;base64" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
        _ => false,
    };
    if !valid {
        bail!("Upload a PNG, JPEG or WebP avatar; remote URLs and SVG are not supported");
    }
    Ok(Some(value))
}

async fn update_account(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Json(input): Json<Value>,
) -> impl IntoResponse {
    let mut conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let Ok(body) = serde_json::from_value::<UpdateAccountInput>(input.clone()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("bad_request", "Invalid profile fields")),
        )
            .into_response();
    };
    let username = body
        .username
        .as_deref()
        .map(normalize_username)
        .unwrap_or_else(|| user.username.clone());
    if username.len() < 3 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "Username must be at least 3 characters",
            )),
        )
            .into_response();
    }
    let avatar = match avatar_data(&body.avatar_url) {
        Ok(avatar) => avatar,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiError::new("bad_request", error.to_string())),
            )
                .into_response()
        }
    };
    let result = (|| -> Result<()> {
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE relay_users SET username=?1 WHERE id=?2",
            params![username, user.id],
        )?;
        if input.get("avatarUrl").is_some() {
            tx.execute("INSERT INTO relay_user_profiles(user_id,avatar_data_url) VALUES(?1,?2) ON CONFLICT(user_id) DO UPDATE SET avatar_data_url=excluded.avatar_data_url", params![user.id,avatar])?;
        }
        tx.commit()?;
        Ok(())
    })();
    if let Err(error) = result {
        tracing::warn!(%error, "profile update failed");
        return (
            StatusCode::CONFLICT,
            Json(ApiError::new(
                "conflict",
                "Profile could not be saved; username may already be in use",
            )),
        )
            .into_response();
    }
    let updated = load_user_by_id(&conn, &user.id).unwrap_or(user);
    Json(user_json(&updated)).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdatePasswordInput {
    current_password: String,
    new_password: String,
    verification_token: Option<String>,
}

async fn update_account_password(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdatePasswordInput>,
) -> impl IntoResponse {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    if account::has_vault(&conn, &user.id) || account::oauth_only(&conn, &user.id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "Change the password through the account vault",
            )),
        )
            .into_response();
    }
    let current_token = extract_session_token(&headers, &query).unwrap_or_default();
    if body.new_password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "Password must be at least 8 characters",
            )),
        )
            .into_response();
    }
    let password: Option<(String, String)> = conn
        .query_row(
            "SELECT password_salt,password_hash FROM relay_users WHERE id=?1",
            params![user.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    let Some((salt, hash)) = password else {
        return unauthorized();
    };
    if !verify_password(&body.current_password, &salt, &hash) {
        return (
            StatusCode::FORBIDDEN,
            Json(ApiError::new("forbidden", "Current password is incorrect")),
        )
            .into_response();
    }
    if auth_factors::has_factors(&conn, &user.id)
        && !auth_factors::consume_password_grant(
            &conn,
            &user.id,
            &current_token,
            body.verification_token.as_deref().unwrap_or_default(),
        )
    {
        return (StatusCode::FORBIDDEN,Json(json!({"code":"reauthentication_required","message":"Verify your identity before changing your password"}))).into_response();
    }
    let Ok((salt, hash)) = hash_password(&body.new_password) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if conn
        .execute(
            "UPDATE relay_users SET password_salt=?1,password_hash=?2 WHERE id=?3",
            params![salt, hash, user.id],
        )
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let Ok(token) = create_session(&conn, &state.store.session_secret, &user.id) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if auth_factors::mark_strong(&conn, &token).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    with_session_cookie(Json(user_json(&user)).into_response(), &token)
}

async fn list_user_devices(state: &AppState, user_id: &str) -> Vec<Value> {
    let connected = state.sockets.read().await;
    let conn = state.store.conn.lock().await;
    let mut stmt = conn
        .prepare(
            "SELECT id,owner_user_id,name,created_at,token,token_preview
             FROM relay_devices
             WHERE owner_user_id=?1
             ORDER BY created_at ASC",
        )
        .expect("stmt");
    stmt.query_map(params![user_id], |row| {
        let id: String = row.get(0)?;
        let owner: String = row.get(1)?;
        let name: String = row.get(2)?;
        let created_at: String = row.get(3)?;
        let _token: Option<String> = row.get(4)?;
        let token_preview: Option<String> = row.get(5)?;
        Ok(device_json(DeviceJsonInput {
            id: &id,
            owner_user_id: &owner,
            name: &name,
            created_at: &created_at,
            connected: connected.contains_key(&id),
            connected_at: connected
                .get(&id)
                .map(|socket| socket.connected_at.as_str()),
            last_heartbeat_at: connected
                .get(&id)
                .map(|socket| socket.last_heartbeat_at.as_str()),
            token: None,
            token_preview: token_preview.as_deref(),
        }))
    })
    .ok()
    .map(|rows| rows.filter_map(|row| row.ok()).collect())
    .unwrap_or_default()
}

async fn list_devices(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let user = {
        let conn = state.store.conn.lock().await;
        authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    };
    let Some(user) = user else {
        return unauthorized();
    };
    Json(json!({ "devices": list_user_devices(&state, &user.id).await })).into_response()
}

#[derive(Deserialize, Default)]
#[allow(dead_code)]
struct AccessQuery {
    token: Option<String>,
    #[serde(rename = "deviceToken")]
    device_token: Option<String>,
    #[serde(rename = "deviceId")]
    device_id: Option<String>,
    #[serde(rename = "threadId")]
    thread_id: Option<String>,
    #[serde(rename = "workspaceId")]
    workspace_id: Option<String>,
}

impl AccessQuery {
    fn token_query(&self) -> TokenQuery {
        TokenQuery {
            token: self.token.clone(),
            device_token: None,
            relay_session: None,
        }
    }
}

use pockymoe_protocol::access::{owner_access, EffectiveAccess};

fn access_json(access: &EffectiveAccess) -> Value {
    json!({
        "kind": access.kind,
        "grantId": access.grant_id,
        "shareId": access.share_id,
        "scope": access.scope,
        "threadAccess": access.thread_access,
        "workspaceAccess": access.workspace_access,
        "workspaceId": access.workspace_id,
        "workspaceScope": access.workspace_scope,
        "canCreateThreads": access.can_create_threads
    })
}

fn effective_access(
    conn: &Connection,
    user_id: &str,
    device_id: &str,
    thread_id: Option<&str>,
    workspace_id: Option<&str>,
) -> Option<EffectiveAccess> {
    if !load_user_by_id(conn, user_id).is_some_and(|user| user.enabled == 1) {
        return None;
    }
    let owned = conn
        .query_row(
            "SELECT 1 FROM relay_devices WHERE id=?1 AND owner_user_id=?2",
            params![device_id, user_id],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some();
    if owned {
        return Some(owner_access());
    }

    let now = now_rfc3339();
    let share = if let Some(thread_id) = thread_id {
        conn.query_row(
            "SELECT id,thread_id,thread_access,workspace_access,workspace_id
             FROM relay_shares
             WHERE target_user_id=?1 AND device_id=?2 AND (thread_id=?3 OR thread_id IN (
               WITH RECURSIVE ancestors(id,depth) AS (
                 SELECT parent_thread_id,1 FROM relay_thread_lineage WHERE device_id=?2 AND thread_id=?3
                 UNION ALL SELECT t.parent_thread_id,a.depth+1 FROM relay_thread_lineage t JOIN ancestors a ON t.thread_id=a.id AND t.device_id=?2 WHERE a.depth<3
               ) SELECT id FROM ancestors))
               AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?4)
             ORDER BY created_at DESC LIMIT 1",
            params![user_id, device_id, thread_id, now],
            |row| {
                Ok(EffectiveAccess {
                    kind: "shared",
                    grant_id: None,
                    share_id: Some(row.get(0)?),
                    scope: "thread".to_string(),
                    thread_id: Some(thread_id.to_string()),
                    thread_access: row.get(2)?,
                    workspace_access: row.get(3)?,
                    workspace_id: row.get(4)?,
                    workspace_scope: Some("selected".to_string()),
                    workspace_ids: Vec::new(),
                    can_create_threads: false,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
    } else if let Some(workspace_id) = workspace_id {
        conn.query_row(
            "SELECT id,thread_id,thread_access,workspace_access,workspace_id
             FROM relay_shares
             WHERE target_user_id=?1 AND device_id=?2 AND workspace_id=?3
               AND workspace_access<>'none' AND revoked_at IS NULL
               AND (expires_at IS NULL OR expires_at>?4)
             ORDER BY created_at DESC LIMIT 1",
            params![user_id, device_id, workspace_id, now],
            |row| {
                Ok(EffectiveAccess {
                    kind: "shared",
                    grant_id: None,
                    share_id: Some(row.get(0)?),
                    scope: "thread".to_string(),
                    thread_id: Some(row.get(1)?),
                    thread_access: row.get(2)?,
                    workspace_access: row.get(3)?,
                    workspace_id: row.get(4)?,
                    workspace_scope: Some("selected".to_string()),
                    workspace_ids: Vec::new(),
                    can_create_threads: false,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
    } else {
        None
    };
    if let Some(share) = share {
        if let Some(access) = pockymoe_protocol::access::merge_access(
            false,
            Some(share),
            &[],
            thread_id,
            workspace_id,
        ) {
            return Some(access);
        }
    }

    let mut stmt = conn
        .prepare(
            "SELECT id,scope,thread_id,workspace_id,workspace_scope,workspace_ids,
                    thread_access,workspace_access,can_create_threads
             FROM relay_access_grants
             WHERE target_user_id=?1 AND device_id=?2 AND revoked_at IS NULL
               AND (expires_at IS NULL OR expires_at>?3)
             ORDER BY CASE scope WHEN 'device' THEN 3 WHEN 'workspace' THEN 2 ELSE 1 END DESC,
                      created_at DESC",
        )
        .ok()?;
    let rows = stmt
        .query_map(params![user_id, device_id, now], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })
        .ok()?;
    let mut matching = Vec::new();
    for row in rows.flatten() {
        let (
            id,
            scope,
            grant_thread,
            grant_workspace,
            workspace_scope,
            workspace_ids,
            thread_access,
            workspace_access,
            can_create_threads,
        ) = row;
        let selected: Vec<String> = serde_json::from_str(&workspace_ids).unwrap_or_default();
        let grant = EffectiveAccess {
            kind: "shared",
            grant_id: Some(id),
            share_id: None,
            scope,
            thread_id: grant_thread,
            thread_access,
            workspace_access,
            workspace_id: grant_workspace,
            workspace_scope: Some(workspace_scope),
            workspace_ids: selected,
            can_create_threads: can_create_threads == 1,
        };
        // Database lookup supplies lineage evidence; the matching rule is shared.
        let lineage = grant
            .thread_id
            .as_deref()
            .filter(|parent| {
                thread_id
                    .is_some_and(|target| thread_groups::contains(conn, device_id, parent, target))
            })
            .map(|parent| vec![parent.to_owned()])
            .unwrap_or_default();
        if pockymoe_protocol::access::grant_matches(&grant, thread_id, workspace_id, &lineage) {
            matching.push(grant);
        }
    }
    pockymoe_protocol::access::merge_access(false, None, &matching, thread_id, workspace_id)
}

fn grantee_registration_request(method: &Method, path: &str, headers: &HeaderMap) -> bool {
    let encrypted = ["x-rcd-key", "x-rcd-request", "x-rcd-enc"]
        .iter()
        .all(|name| {
            headers
                .get(*name)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| !value.is_empty())
        });
    encrypted && *method == Method::POST && path.split('?').next() == Some("/api/transport/browser")
}

/// Registration has no resource id: any live row lets the device decide whether
/// the assertion belongs to one of its grants. Never use this for ordinary routes.
fn registration_access(conn: &Connection, user: &str, device: &str) -> Option<EffectiveAccess> {
    if !load_user_by_id(conn, user).is_some_and(|user| user.enabled == 1) {
        return None;
    }
    conn.query_row(
        "SELECT id,'share', 'thread',thread_id,workspace_id,thread_access,workspace_access
         FROM relay_shares WHERE target_user_id=?1 AND device_id=?2
           AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?3)
         UNION ALL
         SELECT id,'grant',scope,thread_id,workspace_id,thread_access,workspace_access
         FROM relay_access_grants WHERE target_user_id=?1 AND device_id=?2
           AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?3)
         LIMIT 1",
        params![user, device, now_rfc3339()],
        |row| {
            let id: String = row.get(0)?;
            let share = row.get::<_, String>(1)? == "share";
            Ok(EffectiveAccess {
                kind: "shared",
                share_id: share.then(|| id.clone()),
                grant_id: (!share).then_some(id),
                scope: row.get(2)?,
                thread_id: row.get(3)?,
                workspace_id: row.get(4)?,
                thread_access: row.get(5)?,
                workspace_access: row.get(6)?,
                workspace_scope: None,
                workspace_ids: Vec::new(),
                can_create_threads: false,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

fn device_request_access(
    conn: &Connection,
    user: &str,
    device: &str,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> Option<EffectiveAccess> {
    effective_access(
        conn,
        user,
        device,
        resource_id_from_path(path, "threads").as_deref(),
        resource_id_from_path(path, "workspaces").as_deref(),
    )
    .or_else(|| {
        grantee_registration_request(method, path, headers)
            .then(|| registration_access(conn, user, device))
            .flatten()
    })
}

async fn relay_access(
    headers: HeaderMap,
    Query(query): Query<AccessQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let user = {
        let conn = state.store.conn.lock().await;
        authenticated_user(
            &conn,
            &state.store.session_secret,
            &headers,
            &query.token_query(),
        )
    };
    let Some(user) = user else {
        return unauthorized();
    };
    let Some(device_id) = query.device_id.filter(|value| !value.is_empty()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "code": "bad_request", "message": "deviceId is required" })),
        )
            .into_response();
    };
    let conn = state.store.conn.lock().await;
    if let Some(access) = effective_access(
        &conn,
        &user.id,
        &device_id,
        query.thread_id.as_deref(),
        query.workspace_id.as_deref(),
    ) {
        return Json(access_json(&access)).into_response();
    }
    (
        StatusCode::FORBIDDEN,
        Json(json!({ "code": "forbidden", "message": "Device access is not allowed." })),
    )
        .into_response()
}

fn relay_shares_for(conn: &Connection, column: &str, user_id: &str) -> Vec<Value> {
    let sql = format!(
        "SELECT id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,
                thread_id,thread_title,workspace_id,workspace_label,label,thread_access,workspace_access,
                created_at,revoked_at,expires_at
         FROM relay_shares WHERE {column}=?1 AND revoked_at IS NULL
           AND (expires_at IS NULL OR expires_at>?2) ORDER BY created_at DESC"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    stmt.query_map(params![user_id, now_rfc3339()], |row| {
        let activity = share_activity::fields(conn, "share", &row.get::<_, String>(0)?);
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "ownerUserId": row.get::<_, String>(1)?,
            "ownerUsername": row.get::<_, Option<String>>(2)?.unwrap_or_else(|| "unknown".to_string()),
            "targetUserId": row.get::<_, String>(3)?,
            "targetUsername": row.get::<_, Option<String>>(4)?.unwrap_or_else(|| "unknown".to_string()),
            "deviceId": row.get::<_, String>(5)?,
            "deviceName": row.get::<_, Option<String>>(6)?.unwrap_or_else(|| "Pockymoe device".to_string()),
            "threadId": row.get::<_, String>(7)?,
            "threadTitle": row.get::<_, Option<String>>(8)?,
            "workspaceId": row.get::<_, Option<String>>(9)?,
            "workspaceLabel": row.get::<_, Option<String>>(10)?,
            "label": row.get::<_, Option<String>>(11)?,
            "threadAccess": row.get::<_, String>(12)?,
            "workspaceAccess": row.get::<_, String>(13)?,
            "createdAt": row.get::<_, String>(14)?,
            "revokedAt": row.get::<_, Option<String>>(15)?,
            "expiresAt": row.get::<_, Option<String>>(16)?,
            "lastAccessedAt": activity.0,
            "lastAccessedByUsername": activity.1,
            "accessEvents": activity.2
        }))
    })
    .ok()
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

fn relay_grants_for(conn: &Connection, column: &str, user_id: &str) -> Vec<Value> {
    let sql = format!(
        "SELECT id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,
                scope,thread_id,thread_title,workspace_id,workspace_label,workspace_scope,workspace_ids,
                label,thread_access,workspace_access,can_create_threads,created_at,revoked_at,expires_at
         FROM relay_access_grants WHERE {column}=?1 AND revoked_at IS NULL
           AND (expires_at IS NULL OR expires_at>?2) ORDER BY created_at DESC"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    stmt.query_map(params![user_id, now_rfc3339()], |row| {
        let workspace_ids: String = row.get(13)?;
        let activity = share_activity::fields(conn, "grant", &row.get::<_, String>(0)?);
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "ownerUserId": row.get::<_, String>(1)?,
            "ownerUsername": row.get::<_, Option<String>>(2)?.unwrap_or_else(|| "unknown".to_string()),
            "targetUserId": row.get::<_, String>(3)?,
            "targetUsername": row.get::<_, Option<String>>(4)?.unwrap_or_else(|| "unknown".to_string()),
            "deviceId": row.get::<_, String>(5)?,
            "deviceName": row.get::<_, Option<String>>(6)?.unwrap_or_else(|| "Pockymoe device".to_string()),
            "scope": row.get::<_, String>(7)?,
            "threadId": row.get::<_, Option<String>>(8)?,
            "threadTitle": row.get::<_, Option<String>>(9)?,
            "workspaceId": row.get::<_, Option<String>>(10)?,
            "workspaceLabel": row.get::<_, Option<String>>(11)?,
            "workspaceScope": row.get::<_, String>(12)?,
            "workspaceIds": serde_json::from_str::<Vec<String>>(&workspace_ids).unwrap_or_default(),
            "label": row.get::<_, Option<String>>(14)?,
            "threadAccess": row.get::<_, String>(15)?,
            "workspaceAccess": row.get::<_, String>(16)?,
            "canCreateThreads": row.get::<_, i64>(17)? == 1,
            "createdAt": row.get::<_, String>(18)?,
            "revokedAt": row.get::<_, Option<String>>(19)?,
            "expiresAt": row.get::<_, Option<String>>(20)?,
            "lastAccessedAt": activity.0,
            "lastAccessedByUsername": activity.1,
            "accessEvents": activity.2
        }))
    })
    .ok()
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

async fn portal(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let user = {
        let conn = state.store.conn.lock().await;
        authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    };
    let Some(user) = user else {
        return unauthorized();
    };
    let (shared_with_me, shared_by_me, grants_with_me, grants_by_me) = {
        let sockets = state.sockets.read().await;
        let connected = sockets.keys().cloned().collect();
        let conn = state.store.conn.lock().await;
        let mut lists = [
            relay_shares_for(&conn, "target_user_id", &user.id),
            relay_shares_for(&conn, "owner_user_id", &user.id),
            relay_grants_for(&conn, "target_user_id", &user.id),
            relay_grants_for(&conn, "owner_user_id", &user.id),
        ];
        for list in &mut lists {
            share_presence::enrich(&conn, &connected, list);
        }
        let [incoming, outgoing, incoming_grants, outgoing_grants] = lists;
        (incoming, outgoing, incoming_grants, outgoing_grants)
    };
    let shared_devices_with_me: Vec<Value> = grants_with_me
        .iter()
        .filter(|grant| grant.get("scope").and_then(Value::as_str) == Some("device"))
        .cloned()
        .collect();
    let mut shared_threads_with_me = shared_with_me.clone();
    shared_threads_with_me.extend(
        grants_with_me
            .iter()
            .filter(|grant| grant.get("scope").and_then(Value::as_str) != Some("device"))
            .cloned(),
    );
    Json(json!({
        "user": user_json(&user),
        "devices": list_user_devices(&state, &user.id).await,
        "sharedWithMe": shared_with_me,
        "sharedByMe": shared_by_me,
        "sharedDevicesWithMe": shared_devices_with_me,
        "sharedThreadsWithMe": shared_threads_with_me,
        "grantsByMe": grants_by_me
    }))
    .into_response()
}

#[derive(Deserialize)]
struct CreateDeviceInput {
    name: String,
}

async fn create_device(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateDeviceInput>,
) -> impl IntoResponse {
    let user = {
        let conn = state.store.conn.lock().await;
        authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    };
    let Some(user) = user else {
        return unauthorized();
    };
    {
        let conn = state.store.conn.lock().await;
        let token = extract_session_token(&headers, &query).unwrap_or_default();
        if !auth_factors::recent(&conn, &token) {
            return (
                StatusCode::FORBIDDEN,
                Json(ApiError::new(
                    "reauthentication_required",
                    "Authenticate again to create a device",
                )),
            )
                .into_response();
        }
    }
    let name = body.name.trim();
    if name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("bad_request", "Device name is required")),
        )
            .into_response();
    }
    let id = Uuid::new_v4().to_string();
    let mut token_bytes = [0_u8; 24];
    OsRng.fill_bytes(&mut token_bytes);
    let token = format!(
        "rcd_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_bytes)
    );
    let created_at = now_rfc3339();
    let token_preview = preview_token(&token);
    let token_hash = hash_device_token(&token);
    {
        let conn = state.store.conn.lock().await;
        let Ok(tx) = conn.unchecked_transaction() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        if tx
            .execute(
                "INSERT INTO relay_devices
                 (id,owner_user_id,name,token,token_hash,token_preview,created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    id,
                    user.id,
                    name,
                    Option::<String>::None,
                    token_hash,
                    token_preview.clone(),
                    created_at
                ],
            )
            .map_err(anyhow::Error::from)
            .and_then(|_| device_tokens::save(&tx, &state.store.session_secret, &id, &token))
            .and_then(|_| tx.commit().map_err(anyhow::Error::from))
            .is_err()
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("internal", "Failed to create device")),
            )
                .into_response();
        }
    }
    Json(json!({
        "device": device_json(DeviceJsonInput {
            id: &id,
            owner_user_id: &user.id,
            name,
            created_at: &created_at,
            connected: false,
            connected_at: None,
            last_heartbeat_at: None,
            token: Some(&token),
            token_preview: Some(&token_preview),
        }),
        "token": token
    }))
    .into_response()
}

async fn delete_device(
    Path(device_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let user = {
        let conn = state.store.conn.lock().await;
        authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    };
    let Some(user) = user else {
        return unauthorized();
    };
    let conn = state.store.conn.lock().await;
    let deleted = conn
        .execute(
            "DELETE FROM relay_devices WHERE id=?1 AND owner_user_id=?2",
            params![device_id, user.id],
        )
        .unwrap_or(0);
    if deleted == 0 {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", "Device not found")),
        )
            .into_response();
    }
    drop(conn);
    state.sockets.write().await.remove(&device_id);
    Json(json!({ "id": device_id })).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateShareInput {
    #[serde(alias = "targetUsername")]
    target_identifier: Option<String>,
    device_id: Option<String>,
    thread_id: Option<String>,
    thread_title: Option<String>,
    workspace_id: Option<String>,
    workspace_label: Option<String>,
    label: Option<String>,
    thread_access: Option<String>,
    workspace_access: Option<String>,
    expires_at: Option<String>,
}

async fn create_share(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateShareInput>,
) -> impl IntoResponse {
    let conn = state.store.conn.lock().await;
    let Some(owner) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let target_identifier = body
        .target_identifier
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let device_id = body.device_id.unwrap_or_default();
    let thread_id = body.thread_id.unwrap_or_default();
    if target_identifier.is_empty() || device_id.is_empty() || thread_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "targetIdentifier, deviceId and threadId are required",
            )),
        )
            .into_response();
    }
    let device_name: Option<String> = conn
        .query_row(
            "SELECT name FROM relay_devices WHERE id=?1 AND owner_user_id=?2",
            params![device_id, owner.id],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let Some(device_name) = device_name else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", "Device not found")),
        )
            .into_response();
    };
    let target: Option<(String, String)> = conn
        .query_row(
            "SELECT id,username FROM relay_users
             WHERE enabled=1 AND (username=?1 OR email=?1)",
            params![target_identifier],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    let Some((target_id, target_username)) = target else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", "Target user not found")),
        )
            .into_response();
    };
    if target_id == owner.id {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "You cannot share a session with yourself",
            )),
        )
            .into_response();
    }
    let thread_access = normalize_thread_access(body.thread_access.as_deref());
    let workspace_access = normalize_workspace_access(body.workspace_access.as_deref());
    let id = Uuid::new_v4().to_string();
    let created_at = now_rfc3339();
    if conn
        .execute(
            "INSERT INTO relay_shares
         (id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,
          thread_id,thread_title,workspace_id,workspace_label,label,thread_access,workspace_access,
          created_at,revoked_at,expires_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,NULL,?16)",
            params![
                id,
                owner.id,
                owner.username,
                target_id,
                target_username,
                device_id,
                device_name,
                thread_id,
                body.thread_title,
                body.workspace_id,
                body.workspace_label,
                body.label,
                thread_access,
                workspace_access,
                created_at,
                body.expires_at,
            ],
        )
        .is_err()
    {
        return (
            StatusCode::CONFLICT,
            Json(ApiError::new("conflict", "Share could not be created")),
        )
            .into_response();
    }
    Json(json!({
        "id": id,
        "ownerUserId": owner.id,
        "ownerUsername": owner.username,
        "targetUserId": target_id,
        "targetUsername": target_username,
        "deviceId": device_id,
        "deviceName": device_name,
        "threadId": thread_id,
        "threadTitle": body.thread_title,
        "workspaceId": body.workspace_id,
        "workspaceLabel": body.workspace_label,
        "label": body.label,
        "threadAccess": thread_access,
        "workspaceAccess": workspace_access,
        "createdAt": created_at,
        "revokedAt": Value::Null,
        "expiresAt": body.expires_at
    }))
    .into_response()
}

// Omitted PATCH fields preserve the row; explicit null restores nullable values.
// This distinction is needed when rolling back an edit after a device grant fails.
fn nullable_patch_string<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateShareInput {
    #[serde(default, deserialize_with = "nullable_patch_string")]
    thread_title: Option<Option<String>>,
    thread_access: Option<String>,
    workspace_access: Option<String>,
    #[serde(default, deserialize_with = "nullable_patch_string")]
    workspace_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable_patch_string")]
    workspace_label: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable_patch_string")]
    label: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable_patch_string")]
    expires_at: Option<Option<String>>,
}

async fn update_share(
    Path(share_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateShareInput>,
) -> impl IntoResponse {
    let conn = state.store.conn.lock().await;
    let Some(owner) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let owned = conn
        .query_row(
            "SELECT 1 FROM relay_shares WHERE id=?1 AND owner_user_id=?2 AND revoked_at IS NULL",
            params![share_id, owner.id],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some();
    if !owned {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", "Share not found")),
        )
            .into_response();
    }
    if let Some(thread_title) = body.thread_title {
        let _ = conn.execute(
            "UPDATE relay_shares SET thread_title=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![thread_title, share_id, owner.id],
        );
    }
    if let Some(access) = body.thread_access {
        let access = normalize_thread_access(Some(&access));
        let _ = conn.execute(
            "UPDATE relay_shares SET thread_access=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![access, share_id, owner.id],
        );
    }
    if let Some(access) = body.workspace_access {
        let access = normalize_workspace_access(Some(&access));
        let _ = conn.execute(
            "UPDATE relay_shares SET workspace_access=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![access, share_id, owner.id],
        );
    }
    if let Some(workspace_id) = body.workspace_id {
        let _ = conn.execute(
            "UPDATE relay_shares SET workspace_id=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![workspace_id, share_id, owner.id],
        );
    }
    if let Some(workspace_label) = body.workspace_label {
        let _ = conn.execute(
            "UPDATE relay_shares SET workspace_label=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![workspace_label, share_id, owner.id],
        );
    }
    if let Some(label) = body.label {
        let _ = conn.execute(
            "UPDATE relay_shares SET label=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![label, share_id, owner.id],
        );
    }
    if let Some(expires_at) = body.expires_at {
        let _ = conn.execute(
            "UPDATE relay_shares SET expires_at=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![expires_at, share_id, owner.id],
        );
    }
    Json(json!({ "id": share_id, "ok": true })).into_response()
}

async fn revoke_share(
    Path(share_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let conn = state.store.conn.lock().await;
    let Some(owner) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let revoked_at = now_rfc3339();
    let changed = conn
        .execute(
            "UPDATE relay_shares SET revoked_at=?1 WHERE id=?2 AND owner_user_id=?3",
            params![revoked_at, share_id, owner.id],
        )
        .unwrap_or(0);
    if changed == 0 {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", "Share not found")),
        )
            .into_response();
    }
    Json(json!({ "id": share_id, "revokedAt": revoked_at })).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateGrantInput {
    #[serde(alias = "targetUsername")]
    target_identifier: Option<String>,
    device_id: Option<String>,
    scope: Option<String>,
    thread_id: Option<String>,
    workspace_id: Option<String>,
    workspace_scope: Option<String>,
    workspace_ids: Option<Vec<String>>,
    label: Option<String>,
    thread_access: Option<String>,
    workspace_access: Option<String>,
    can_create_threads: Option<bool>,
    expires_at: Option<String>,
}

async fn create_grant(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateGrantInput>,
) -> impl IntoResponse {
    let conn = state.store.conn.lock().await;
    let Some(owner) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let target_identifier = body
        .target_identifier
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let device_id = body.device_id.unwrap_or_default();
    let scope = normalize_scope(body.scope.as_deref());
    if target_identifier.is_empty() || device_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "targetIdentifier and deviceId are required",
            )),
        )
            .into_response();
    }
    if (scope == "thread" && body.thread_id.as_deref().unwrap_or_default().is_empty())
        || (scope == "workspace" && body.workspace_id.as_deref().unwrap_or_default().is_empty())
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "The selected grant scope requires its resource id",
            )),
        )
            .into_response();
    }
    let device_name: Option<String> = conn
        .query_row(
            "SELECT name FROM relay_devices WHERE id=?1 AND owner_user_id=?2",
            params![device_id, owner.id],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let Some(device_name) = device_name else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", "Device not found")),
        )
            .into_response();
    };
    let target: Option<(String, String)> = conn
        .query_row(
            "SELECT id,username FROM relay_users WHERE enabled=1 AND (username=?1 OR email=?1)",
            params![target_identifier],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    let Some((target_id, target_username)) = target else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", "Target user not found")),
        )
            .into_response();
    };
    if target_id == owner.id {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "bad_request",
                "You cannot grant access to yourself",
            )),
        )
            .into_response();
    }
    let id = Uuid::new_v4().to_string();
    let created_at = now_rfc3339();
    let thread_access = normalize_thread_access(body.thread_access.as_deref());
    let workspace_access = normalize_workspace_access(body.workspace_access.as_deref());
    let workspace_scope = match body.workspace_scope.as_deref() {
        Some("selected") => "selected",
        _ => "all",
    };
    let workspace_ids = serde_json::to_string(&body.workspace_ids.clone().unwrap_or_default())
        .unwrap_or_else(|_| "[]".to_string());
    if conn.execute(
        "INSERT INTO relay_access_grants
         (id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,
          scope,thread_id,thread_title,workspace_id,workspace_label,workspace_scope,workspace_ids,label,
          thread_access,workspace_access,can_create_threads,created_at,revoked_at,expires_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,NULL,?10,NULL,?11,?12,?13,?14,?15,?16,?17,NULL,?18)",
        params![
            id,
            owner.id,
            owner.username,
            target_id,
            target_username,
            device_id,
            device_name,
            scope,
            body.thread_id,
            body.workspace_id,
            workspace_scope,
            workspace_ids,
            body.label,
            thread_access,
            workspace_access,
            body.can_create_threads.unwrap_or(false) as i64,
            created_at,
            body.expires_at,
        ],
    ).is_err() {
        return (
            StatusCode::CONFLICT,
            Json(ApiError::new("conflict", "Grant could not be created")),
        )
            .into_response();
    }
    Json(json!({ "id": id })).into_response()
}

async fn list_grants(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let conn = state.store.conn.lock().await;
    let Some(owner) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let mut stmt = conn
        .prepare(
            "SELECT id,target_username,device_id,scope,thread_id,thread_access,created_at
                  FROM relay_access_grants WHERE owner_user_id=?1 AND revoked_at IS NULL",
        )
        .expect("stmt");
    let grants: Vec<Value> = stmt
        .query_map(params![owner.id], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "targetUsername": row.get::<_, String>(1)?,
                "deviceId": row.get::<_, String>(2)?,
                "scope": row.get::<_, String>(3)?,
                "threadId": row.get::<_, Option<String>>(4)?,
                "threadAccess": row.get::<_, String>(5)?,
                "createdAt": row.get::<_, String>(6)?
            }))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    Json(json!({ "grants": grants })).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateGrantInput {
    thread_access: Option<String>,
    workspace_access: Option<String>,
    can_create_threads: Option<bool>,
    workspace_scope: Option<String>,
    workspace_ids: Option<Vec<String>>,
    #[serde(default, deserialize_with = "nullable_patch_string")]
    label: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable_patch_string")]
    expires_at: Option<Option<String>>,
}

async fn update_grant(
    Path(grant_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateGrantInput>,
) -> impl IntoResponse {
    let conn = state.store.conn.lock().await;
    let Some(owner) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let owned = conn
        .query_row(
            "SELECT 1 FROM relay_access_grants WHERE id=?1 AND owner_user_id=?2 AND revoked_at IS NULL",
            params![grant_id, owner.id],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some();
    if !owned {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", "Grant not found")),
        )
            .into_response();
    }
    if let Some(access) = body.thread_access {
        let access = normalize_thread_access(Some(&access));
        let _ = conn.execute(
            "UPDATE relay_access_grants SET thread_access=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![access, grant_id, owner.id],
        );
    }
    if let Some(access) = body.workspace_access {
        let access = normalize_workspace_access(Some(&access));
        let _ = conn.execute(
            "UPDATE relay_access_grants SET workspace_access=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![access, grant_id, owner.id],
        );
    }
    if let Some(can) = body.can_create_threads {
        let _ = conn.execute(
            "UPDATE relay_access_grants SET can_create_threads=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![can as i64, grant_id, owner.id],
        );
    }
    if let Some(scope) = body.workspace_scope {
        let scope = if scope == "selected" {
            "selected"
        } else {
            "all"
        };
        let _ = conn.execute(
            "UPDATE relay_access_grants SET workspace_scope=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![scope, grant_id, owner.id],
        );
    }
    if let Some(ids) = body.workspace_ids {
        let ids = serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string());
        let _ = conn.execute(
            "UPDATE relay_access_grants SET workspace_ids=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![ids, grant_id, owner.id],
        );
    }
    if let Some(label) = body.label {
        let _ = conn.execute(
            "UPDATE relay_access_grants SET label=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![label, grant_id, owner.id],
        );
    }
    if let Some(expires_at) = body.expires_at {
        let _ = conn.execute(
            "UPDATE relay_access_grants SET expires_at=?1 WHERE id=?2 AND owner_user_id=?3 AND revoked_at IS NULL",
            params![expires_at, grant_id, owner.id],
        );
    }
    Json(json!({ "id": grant_id, "ok": true })).into_response()
}

async fn revoke_grant(
    Path(grant_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let conn = state.store.conn.lock().await;
    let Some(owner) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let revoked_at = now_rfc3339();
    let changed = conn
        .execute(
            "UPDATE relay_access_grants SET revoked_at=?1 WHERE id=?2 AND owner_user_id=?3",
            params![revoked_at, grant_id, owner.id],
        )
        .unwrap_or(0);
    if changed == 0 {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", "Grant not found")),
        )
            .into_response();
    }
    Json(json!({ "id": grant_id, "revokedAt": revoked_at })).into_response()
}

fn normalize_thread_access(value: Option<&str>) -> String {
    if value == Some("read") {
        "read".to_string()
    } else {
        "control".to_string()
    }
}

fn normalize_workspace_access(value: Option<&str>) -> String {
    match value {
        Some("read") => "read".to_string(),
        Some("write") => "write".to_string(),
        _ => "none".to_string(),
    }
}

fn normalize_scope(value: Option<&str>) -> String {
    match value {
        Some("workspace") => "workspace".to_string(),
        Some("device") => "device".to_string(),
        _ => "thread".to_string(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresenceQuery {
    #[serde(flatten)]
    auth: TokenQuery,
    thread_id: Option<String>,
    workspace_id: Option<String>,
}

async fn device_presence(
    Path(device_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<PresenceQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let allowed = {
        let conn = state.store.conn.lock().await;
        authenticated_user(&conn, &state.store.session_secret, &headers, &query.auth)
            .and_then(|user| {
                effective_access(
                    &conn,
                    &user.id,
                    &device_id,
                    query.thread_id.as_deref(),
                    query.workspace_id.as_deref(),
                )
            })
            .is_some()
    };
    if !allowed {
        return unauthorized();
    }
    // A presence probe never drops the tunnel. Only return reachability,
    // including to thread-only guests.
    let response = forward_device_with_timeout(
        state,
        device_id,
        "GET".into(),
        "/healthz".into(),
        None,
        None,
        json!({}),
        Duration::from_secs(5),
    )
    .await;
    (
        [("cache-control", "no-store")],
        Json(json!({
            "connected": response.status().is_success()
        })),
    )
        .into_response()
}

async fn device_healthz(
    Path(device_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let allowed = {
        let conn = state.store.conn.lock().await;
        authenticated_user(&conn, &state.store.session_secret, &headers, &query)
            .and_then(|user| effective_access(&conn, &user.id, &device_id, None, None))
            .is_some()
    };
    if !allowed {
        return unauthorized();
    }
    forward_device(
        state,
        device_id,
        "GET".into(),
        "/healthz".into(),
        None,
        None,
        json!({}),
    )
    .await
}

async fn device_api(
    Path((device_id, rest)): Path<(String, String)>,
    method: Method,
    uri: Uri,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let path = relay_api_target_path(&rest, &uri);
    let resolved = {
        let conn = state.store.conn.lock().await;
        authenticated_user(&conn, &state.store.session_secret, &headers, &query).and_then(|user| {
            device_request_access(&conn, &user.id, &device_id, &method, &path, &headers)
                .map(|access| (user.id, access))
        })
    };
    let Some((user_id, access)) = resolved else {
        return unauthorized();
    };
    if !relay_target_allowed(&path)
        || !(access_allows(&access, &method, &path)
            || grantee_registration_request(&method, &path, &headers))
    {
        return (
            StatusCode::FORBIDDEN,
            Json(ApiError::new(
                "forbidden",
                "This relay session does not allow that operation",
            )),
        )
            .into_response();
    }
    let mut forwarded_headers = [
        header::CONTENT_TYPE,
        header::ACCEPT,
        header::RANGE,
        "x-rcd-key".parse().unwrap(),
        "x-rcd-request".parse().unwrap(),
        "x-rcd-enc".parse().unwrap(),
        "x-rcd-sealed".parse().unwrap(),
        "x-rcd-resource".parse().unwrap(),
        "x-rcd-session".parse().unwrap(),
        "x-rcd-signature".parse().unwrap(),
    ]
    .into_iter()
    .filter_map(|name| {
        headers
            .get(&name)
            .and_then(|value| value.to_str().ok())
            .map(|value| (name.as_str().to_string(), Value::String(value.to_string())))
    })
    .collect::<serde_json::Map<String, Value>>();
    forwarded_headers.insert("x-rcd-file-actor".into(), json!(user_id));
    let (body, body_encoding) = encode_relay_request_body(&body);
    let response = forward_device(
        state.clone(),
        device_id.clone(),
        method.as_str().to_string(),
        path.clone(),
        body,
        body_encoding,
        Value::Object(forwarded_headers),
    )
    .await;
    share_activity::record_response(&state, &access, &user_id, &method, &path, response.status())
        .await;
    response
}

async fn relay_api_compat(
    Path(rest): Path<String>,
    method: Method,
    uri: Uri,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let path = relay_api_target_path(&rest, &uri);
    let mut connected_device_ids: Vec<String> =
        state.sockets.read().await.keys().cloned().collect();
    {
        let conn = state.store.conn.lock().await;
        if let Ok(mut stmt) = conn.prepare("SELECT id FROM relay_devices ORDER BY created_at") {
            if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) {
                for id in rows.flatten() {
                    if !connected_device_ids.contains(&id) {
                        connected_device_ids.push(id);
                    }
                }
            }
        };
    }
    let resolved = {
        let conn = state.store.conn.lock().await;
        let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
        else {
            return unauthorized();
        };
        connected_device_ids.iter().find_map(|device_id| {
            device_request_access(&conn, &user.id, device_id, &method, &path, &headers)
                .map(|access| (device_id.clone(), user.id.clone(), access))
        })
    };
    let Some((device_id, user_id, access)) = resolved else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError::new(
                "service_unavailable",
                "No accessible supervisor is connected to this relay",
            )),
        )
            .into_response();
    };
    if !relay_target_allowed(&path)
        || !(access_allows(&access, &method, &path)
            || grantee_registration_request(&method, &path, &headers))
    {
        return (
            StatusCode::FORBIDDEN,
            Json(ApiError::new(
                "forbidden",
                "This relay session does not allow that operation",
            )),
        )
            .into_response();
    }
    let mut forwarded_headers = [
        header::CONTENT_TYPE,
        header::ACCEPT,
        header::RANGE,
        "x-rcd-key".parse().unwrap(),
        "x-rcd-request".parse().unwrap(),
        "x-rcd-enc".parse().unwrap(),
        "x-rcd-sealed".parse().unwrap(),
        "x-rcd-resource".parse().unwrap(),
        "x-rcd-session".parse().unwrap(),
        "x-rcd-signature".parse().unwrap(),
    ]
    .into_iter()
    .filter_map(|name| {
        headers
            .get(&name)
            .and_then(|value| value.to_str().ok())
            .map(|value| (name.as_str().to_string(), Value::String(value.to_string())))
    })
    .collect::<serde_json::Map<String, Value>>();
    forwarded_headers.insert("x-rcd-file-actor".into(), json!(user_id));
    let (body, body_encoding) = encode_relay_request_body(&body);
    let response = forward_device(
        state.clone(),
        device_id.clone(),
        method.as_str().to_string(),
        path.clone(),
        body,
        body_encoding,
        Value::Object(forwarded_headers),
    )
    .await;
    share_activity::record_response(&state, &access, &user_id, &method, &path, response.status())
        .await;
    response
}

async fn internal_forward_json(
    state: &Arc<AppState>,
    device_id: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value> {
    let response = forward_device(
        state.clone(),
        device_id.to_string(),
        method.to_string(),
        path.to_string(),
        body.as_ref().map(Value::to_string),
        None,
        if body.is_some() {
            json!({ "content-type": "application/json" })
        } else {
            json!({})
        },
    )
    .await;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 32 * 1024 * 1024).await?;
    let payload: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        json!({ "message": String::from_utf8_lossy(&bytes).chars().take(300).collect::<String>() })
    });
    if !status.is_success() {
        let message = payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("supervisor bootstrap request failed");
        bail!("supervisor bootstrap request failed with {status}: {message}");
    }
    Ok(payload)
}

fn relay_api_target_path(rest: &str, uri: &Uri) -> String {
    // Axum's wildcard Path has already percent-decoded `rest`. Preserve the
    // wire spelling: the browser and device bind this exact path into HPKE AAD.
    let raw_rest = uri.path().split_once("/api/").map(|(_, rest)| rest);
    let mut path = format!("/api/{}", raw_rest.unwrap_or(rest));
    if let Some(raw) = uri.query() {
        let filtered: Vec<&str> = raw
            .split('&')
            .filter(|part| {
                let key = part.split('=').next().unwrap_or("");
                key != "token" && key != "deviceToken" && key != "relaySession"
            })
            .collect();
        if !filtered.is_empty() {
            path.push('?');
            path.push_str(&filtered.join("&"));
        }
    }
    path
}

fn encode_relay_request_body(body: &[u8]) -> (Option<String>, Option<String>) {
    if body.is_empty() {
        (None, None)
    } else {
        match String::from_utf8(body.to_vec()) {
            Ok(body) => (Some(body), None),
            Err(error) => (
                Some(base64::engine::general_purpose::STANDARD.encode(error.into_bytes())),
                Some("base64".to_string()),
            ),
        }
    }
}

async fn forward_device(
    state: Arc<AppState>,
    device_id: String,
    method: String,
    path: String,
    body: Option<String>,
    body_encoding: Option<String>,
    headers: Value,
) -> axum::response::Response {
    forward_device_with_timeout(
        state,
        device_id,
        method,
        path,
        body,
        body_encoding,
        headers,
        Duration::from_secs(30),
    )
    .await
}

struct PendingRequestGuard<'a> {
    pending: &'a StdMutex<HashMap<String, PendingDeviceRequest>>,
    request_id: &'a str,
}

impl Drop for PendingRequestGuard<'_> {
    fn drop(&mut self) {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(self.request_id);
    }
}

#[allow(clippy::too_many_arguments)]
async fn forward_device_with_timeout(
    state: Arc<AppState>,
    device_id: String,
    method: String,
    path: String,
    body: Option<String>,
    body_encoding: Option<String>,
    headers: Value,
    response_timeout: Duration,
) -> axum::response::Response {
    let mut request_payload = json!({
        "method": method,
        "path": path,
        "headers": headers,
        "body": body
    });
    if let Some(body_encoding) = body_encoding {
        request_payload["bodyEncoding"] = Value::String(body_encoding);
    }
    match forward_device_payload_with_timeout(
        state,
        device_id,
        request_payload,
        None,
        response_timeout,
    )
    .await
    {
        Ok(value) => forwarded_device_response(value),
        Err(DeviceForwardError::Busy) => StatusCode::TOO_MANY_REQUESTS.into_response(),
        Err(DeviceForwardError::Offline) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "code": "service_unavailable", "message": "device is offline" })),
        )
            .into_response(),
        Err(DeviceForwardError::Timeout) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(json!({ "code": "timeout", "message": "device did not respond" })),
        )
            .into_response(),
    }
}

enum DeviceForwardError {
    Offline,
    Timeout,
    Busy,
}

async fn forward_device_payload_with_timeout(
    state: Arc<AppState>,
    device_id: String,
    request_payload: Value,
    peer: Option<peer::Caller>,
    response_timeout: Duration,
) -> Result<Value, DeviceForwardError> {
    let request_id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let _pending_guard = PendingRequestGuard {
        pending: &state.pending,
        request_id: &request_id,
    };
    let mut frame = json!({
        "type": "relay.request",
        "timestamp": now_rfc3339(),
        "requestId": request_id,
        "deviceId": device_id,
        "payload": request_payload
    });
    if let Some(caller) = peer.as_ref() {
        frame["peer"] = caller.identity();
    }
    let payload = frame.to_string();
    let sent = {
        let sockets = state.sockets.read().await;
        if let Some(caller) = peer.as_ref() {
            if !sockets
                .get(&caller.device_id)
                .is_some_and(|socket| socket.connection_id == caller.connection_id)
            {
                return Err(DeviceForwardError::Offline);
            }
        }
        if let Some(socket) = sockets.get(&device_id) {
            // Register against the exact connection while it is still current.
            // A replaced connection must only cancel its own pending requests.
            let mut pending = state.pending.lock().unwrap_or_else(|e| e.into_inner());
            if pending.len() >= 256
                || peer.as_ref().is_some_and(|caller| {
                    pending
                        .values()
                        .filter(|request| {
                            request.peer_device_id.as_deref() == Some(caller.device_id.as_str())
                        })
                        .count()
                        >= peer::MAX_IN_FLIGHT
                })
            {
                return Err(DeviceForwardError::Busy);
            }
            pending.insert(
                request_id.clone(),
                PendingDeviceRequest {
                    device_id: device_id.clone(),
                    connection_id: socket.connection_id,
                    peer_device_id: peer.as_ref().map(|caller| caller.device_id.clone()),
                    tx,
                },
            );
            socket.tx.try_send(payload).is_ok()
        } else {
            false
        }
    };
    if !sent {
        return Err(DeviceForwardError::Offline);
    }
    match tokio::time::timeout(response_timeout, rx).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err(DeviceForwardError::Offline),
        Err(_) => Err(DeviceForwardError::Timeout),
    }
}

fn forwarded_device_response(value: Value) -> Response {
    let status = value
        .get("statusCode")
        .and_then(Value::as_u64)
        .and_then(|status| StatusCode::from_u16(status as u16).ok())
        .unwrap_or(StatusCode::OK);
    let encoded_body = if value.get("bodyEncoding").and_then(Value::as_str) == Some("base64") {
        value.get("body").and_then(Value::as_str)
    } else {
        value.get("bodyBase64").and_then(Value::as_str)
    };
    let bytes = if let Some(encoded) = encoded_body {
        match base64::engine::general_purpose::STANDARD.decode(encoded) {
            Ok(bytes) => bytes,
            Err(_) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "code": "invalid_supervisor_response",
                        "message": "Supervisor returned an invalid binary response."
                    })),
                )
                    .into_response();
            }
        }
    } else {
        match value.get("body").cloned().unwrap_or(Value::Null) {
            Value::String(raw) => raw.into_bytes(),
            Value::Null => Vec::new(),
            other => serde_json::to_vec(&other).unwrap_or_default(),
        }
    };

    let mut response = Response::builder().status(status);
    if let Some(headers) = value.get("headers").and_then(Value::as_object) {
        for name in [
            "content-type",
            "content-disposition",
            "cache-control",
            "x-content-type-options",
            "content-security-policy",
            "referrer-policy",
            "x-rcd-encrypted",
        ] {
            if let Some(header_value) = headers.get(name).and_then(Value::as_str) {
                response = response.header(name, header_value);
            }
        }
    }
    response
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

async fn list_shares(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let conn = state.store.conn.lock().await;
    let Some(owner) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let mut stmt = conn
        .prepare(
            "SELECT id,target_username,device_id,thread_id,thread_access,created_at
                  FROM relay_shares WHERE owner_user_id=?1 AND revoked_at IS NULL",
        )
        .expect("stmt");
    let shares: Vec<Value> = stmt
        .query_map(params![owner.id], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "targetUsername": row.get::<_, String>(1)?,
                "deviceId": row.get::<_, String>(2)?,
                "threadId": row.get::<_, String>(3)?,
                "threadAccess": row.get::<_, String>(4)?,
                "createdAt": row.get::<_, String>(5)?
            }))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    Json(json!({ "shares": shares })).into_response()
}

async fn supervisor_tunnel(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let token = query
        .device_token()
        .or_else(|| bearer_token(&headers))
        .unwrap_or_default();
    if token.is_empty() {
        return unauthorized();
    }
    let device_id = {
        let conn = state.store.conn.lock().await;
        device_id_for_supervisor_token(&conn, &token, state.legacy_supervisor_token.as_deref())
    };
    let Some(device_id) = device_id else {
        return unauthorized();
    };
    {
        let conn = state.store.conn.lock().await;
        if let Err(error) = device_tokens::remember_authenticated(
            &conn,
            &state.store.session_secret,
            &device_id,
            &token,
        ) {
            tracing::warn!(%error, "Could not retain authenticated device setup credential");
        }
    }
    ws.max_message_size(128 * 1024 * 1024)
        .max_frame_size(128 * 1024 * 1024)
        .on_upgrade(move |socket| handle_supervisor(socket, state, device_id))
        .into_response()
}

fn device_id_for_supervisor_token(
    conn: &Connection,
    token: &str,
    legacy_supervisor_token: Option<&str>,
) -> Option<String> {
    conn.query_row(
        "SELECT id FROM relay_devices WHERE token_hash=?1",
        params![hash_device_token(token)],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .or_else(|| conn.query_row("SELECT d.id FROM relay_devices d JOIN relay_device_setup_tokens s ON s.device_id=d.id WHERE s.token_hash=?1", params![hash_device_token(token)], |r| r.get(0)).optional().ok().flatten())
    .or_else(|| {
        legacy_supervisor_token
            .filter(|expected| token.as_bytes().ct_eq(expected.as_bytes()).into())
            .map(|_| "legacy-default".to_string())
    })
}

async fn handle_supervisor(socket: WebSocket, state: Arc<AppState>, device_id: String) {
    handle_supervisor_with_timeout(socket, state, device_id, Duration::from_secs(90)).await;
}

async fn handle_supervisor_with_timeout(
    socket: WebSocket,
    state: Arc<AppState>,
    device_id: String,
    receive_timeout: Duration,
) {
    let connection_id = Uuid::new_v4();
    let connected_at = now_rfc3339();
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    state.sockets.write().await.insert(
        device_id.clone(),
        DeviceSocket {
            tx: tx.clone(),
            connection_id,
            connected_at: connected_at.clone(),
            last_heartbeat_at: connected_at.clone(),
        },
    );
    let device_name = peer::device_name(&state, &device_id).await;
    let (mut sink, mut stream) = socket.split();
    let greeting = Message::Text(
        json!({
            "type": "relay.connected",
            "timestamp": connected_at,
            "deviceId": device_id,
            "deviceName": device_name,
            "portPreviewBaseUrl": state.preview.base.as_ref().map(url::Url::as_str),
            "publicBaseUrl": state.oauth.public_base_url
        })
        .to_string()
        .into(),
    );
    if !matches!(
        tokio::time::timeout(Duration::from_secs(10), sink.send(greeting)).await,
        Ok(Ok(()))
    ) {
        remove_supervisor_connection(&state, &device_id, connection_id).await;
        return;
    }
    let revocations = match account::device_revocations(&state, &device_id).await {
        Ok(frame) => frame,
        Err(_) => {
            remove_supervisor_connection(&state, &device_id, connection_id).await;
            return;
        }
    };
    if !matches!(
        tokio::time::timeout(
            Duration::from_secs(10),
            sink.send(Message::Text(revocations.into()))
        )
        .await,
        Ok(Ok(()))
    ) {
        remove_supervisor_connection(&state, &device_id, connection_id).await;
        return;
    }
    let mut last_received = tokio::time::Instant::now();
    loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(last_received + receive_timeout) => {
                tracing::warn!(%device_id, "supervisor stopped responding; marking device offline");
                break;
            }
            incoming = stream.next() => {
                last_received = tokio::time::Instant::now();
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if !state.sockets.read().await.get(&device_id).is_some_and(|socket|socket.connection_id==connection_id) {break;}
                        if let Ok(msg) = serde_json::from_str::<Value>(&text) {
                            match msg.get("type").and_then(Value::as_str) {
                                Some("peer.request") => {
                                    peer::request(state.clone(), device_id.clone(), connection_id, msg);
                                }
                                Some("peer.directory") => {
                                    peer::directory(&state, &device_id, connection_id, msg).await;
                                }
                                Some("relay.response") => {
                                    if let Some(request_id) = msg.get("requestId").and_then(Value::as_str) {
                                        let mut pending=state.pending.lock().unwrap_or_else(|p|p.into_inner());
                                        if pending.get(request_id).is_some_and(|request|request.device_id==device_id && request.connection_id==connection_id) {
                                            if let Some(request)=pending.remove(request_id) {
                                                let payload=msg.get("payload").cloned().unwrap_or(json!({}));
                                                let _=request.tx.send(payload);
                                            }
                                        }
                                    }
                                }
                                Some("relay.client.close") => {
                                    if let Some(id)=msg.get("clientId").and_then(Value::as_str) {
                                        let mut clients=state.clients.write().await;
                                        if clients.get(id).is_some_and(|c|c.device_id==device_id && c.supervisor_connection_id==connection_id) { clients.remove(id); }
                                    }
                                }
                                Some("relay.server.message") => {
                                    if let (Some(client_id), Some(payload)) = (
                                        msg.get("clientId").and_then(Value::as_str),
                                        msg.get("payload"),
                                    ) {
                                        forward_server_message_to_client(
                                            &state,
                                            &device_id,
                                            connection_id,
                                            client_id,
                                            payload,
                                        )
                                        .await;
                                    }
                                }
                                Some("relay.heartbeat") => {
                                    preview::update(&state, &device_id, connection_id, &msg["portMappings"]);
                                    if msg["threadLineage"].is_array() {
                                        if let Err(error)=thread_groups::replace(&*state.store.conn.lock().await,&device_id,&msg["threadLineage"]) { tracing::warn!(%error,"invalid device lineage"); }
                                    }
                                    // Display relay receipt time; a sleeping or clock-skewed
                                    // device cannot extend its own online lease.
                                    let timestamp = now_rfc3339();
                                    let mut sockets = state.sockets.write().await;
                                    if let Some(socket) = sockets.get_mut(&device_id) {
                                        if socket.connection_id == connection_id {
                                            socket.last_heartbeat_at = timestamp;
                                        }
                                    }
                                }
                                Some("relay.notification") => {
                                    let result = notifications::accept(&*state.store.conn.lock().await, &device_id, &msg["payload"]);
                                    if let Ok(turn_id) = result {
                                        let _ = tx.try_send(json!({"type":"relay.notification.ack","turnId":turn_id}).to_string());
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    Some(Ok(Message::Ping(payload))) => {
                        if !matches!(tokio::time::timeout(Duration::from_secs(10), sink.send(Message::Pong(payload))).await, Ok(Ok(()))) {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            outgoing = rx.recv() => {
                match outgoing {
                    Some(text) => {
                        if !matches!(tokio::time::timeout(Duration::from_secs(10),sink.send(Message::Text(text.into()))).await,Ok(Ok(()))) {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }
    remove_supervisor_connection(&state, &device_id, connection_id).await;
}

async fn remove_supervisor_connection(state: &AppState, device_id: &str, connection_id: Uuid) {
    preview::disconnected(state, device_id, connection_id);
    let mut sockets = state.sockets.write().await;
    if sockets
        .get(device_id)
        .is_some_and(|socket| socket.connection_id == connection_id)
    {
        sockets.remove(device_id);
    }
    drop(sockets);
    state.clients.write().await.retain(|_, client| {
        client.device_id != device_id || client.supervisor_connection_id != connection_id
    });
    state
        .pending
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|_, request| {
            request.device_id != device_id || request.connection_id != connection_id
        });
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ClientWsQuery {
    token: Option<String>,
    relay_session: Option<String>,
    thread_id: Option<String>,
    #[serde(rename = "channelId")]
    channel_id: Option<String>,
}

impl ClientWsQuery {
    fn token_query(&self) -> TokenQuery {
        TokenQuery {
            token: self.token.clone(),
            device_token: None,
            relay_session: self.relay_session.clone(),
        }
    }
}

async fn client_ws(
    ws: WebSocketUpgrade,
    Path(device_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<ClientWsQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let user_and_access = {
        let conn = state.store.conn.lock().await;
        authenticated_user(
            &conn,
            &state.store.session_secret,
            &headers,
            &query.token_query(),
        )
        .and_then(|user| {
            let access = effective_access(
                &conn,
                &user.id,
                &device_id,
                query.thread_id.as_deref(),
                None,
            )?;
            Some((user, access))
        })
    };
    let Some((user, access)) = user_and_access else {
        return unauthorized();
    };
    if !state.sockets.read().await.contains_key(&device_id) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError::new(
                "service_unavailable",
                "No supervisor is connected for this device",
            )),
        )
            .into_response();
    }
    let token = extract_session_token(&headers, &query.token_query()).unwrap_or_default();
    ws.on_upgrade(move |socket| {
        handle_client_socket(
            socket,
            state,
            device_id,
            user.id,
            query.thread_id,
            access,
            token,
            query.channel_id,
        )
    })
    .into_response()
}

async fn client_ws_compat(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Query(query): Query<ClientWsQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let mut device_ids: Vec<String> = state.sockets.read().await.keys().cloned().collect();
    {
        let conn = state.store.conn.lock().await;
        if let Ok(mut stmt) = conn.prepare("SELECT id FROM relay_devices ORDER BY created_at") {
            if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) {
                for id in rows.flatten() {
                    if !device_ids.contains(&id) {
                        device_ids.push(id);
                    }
                }
            }
        };
    }
    let resolved = {
        let conn = state.store.conn.lock().await;
        authenticated_user(
            &conn,
            &state.store.session_secret,
            &headers,
            &query.token_query(),
        )
        .and_then(|user| {
            device_ids.iter().find_map(|device_id| {
                let access =
                    effective_access(&conn, &user.id, device_id, query.thread_id.as_deref(), None)?;
                Some((device_id.clone(), user.clone(), access))
            })
        })
    };
    let Some((device_id, user, access)) = resolved else {
        return unauthorized();
    };
    let token = extract_session_token(&headers, &query.token_query()).unwrap_or_default();
    ws.on_upgrade(move |socket| {
        handle_client_socket(
            socket,
            state,
            device_id,
            user.id,
            query.thread_id,
            access,
            token,
            query.channel_id,
        )
    })
    .into_response()
}

async fn shared_shell_scope_allowed(
    state: &Arc<AppState>,
    device: &str,
    thread: &str,
    shell: &str,
    secure_channel: bool,
) -> bool {
    // This hint chooses a redundant routing probe; the strict device always
    // authenticates the channel and authorizes the shell against its own grants.
    let device_filters = secure_channel
        && internal_forward_json(state, device, "GET", "/api/transport/key", None)
            .await
            .ok()
            .is_some_and(|key| key["trust"]["strict"] == true);
    if device_filters {
        return true;
    }
    // Retain relay shell ownership checks for legacy and unpaired channels.
    internal_forward_json(
        state,
        device,
        "GET",
        &format!("/api/threads/{thread}/transport/shell-scope"),
        None,
    )
    .await
    .ok()
    .is_some_and(|response| {
        response["shells"]
            .as_array()
            .is_some_and(|items| items.iter().any(|s| s["id"].as_str() == Some(shell)))
    })
}

async fn handle_client_socket(
    socket: WebSocket,
    state: Arc<AppState>,
    device_id: String,
    user_id: String,
    thread_id: Option<String>,
    _initial_access: EffectiveAccess,
    session_token: String,
    secure_channel_id: Option<String>,
) {
    let client_id = Uuid::new_v4().to_string();
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let supervisor_connection_id = {
        let sockets = state.sockets.read().await;
        let Some(supervisor) = sockets.get(&device_id) else {
            return;
        };
        let supervisor_connection_id = supervisor.connection_id;
        state.clients.write().await.insert(
            client_id.clone(),
            ClientSocket {
                tx,
                device_id: device_id.clone(),
                supervisor_connection_id,
                user_id: user_id.clone(),
                thread_id: thread_id.clone(),
                attached_shell_id: None,
                session_token: session_token.clone(),
            },
        );
        supervisor_connection_id
    };
    send_to_supervisor_connection(
        &state,
        &device_id,
        supervisor_connection_id,
        json!({
            "type": "relay.client.connected",
            "timestamp": now_rfc3339(),
            "secureChannelId": secure_channel_id,
            "clientId": client_id
        }),
    )
    .await;

    let (mut sink, mut stream) = socket.split();
    let mut auth_tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            _ = auth_tick.tick() => {
                let conn = state.store.conn.lock().await;
                if load_user_by_session(&conn, &state.store.session_secret, &session_token).is_none()
                    || effective_access(&conn, &user_id, &device_id, thread_id.as_deref(), None).is_none() { break; }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(payload) = serde_json::from_str::<Value>(&text) else { continue; };
                        let message_thread = payload.get("threadId").and_then(Value::as_str);
                        if thread_id.as_deref().zip(message_thread).is_some_and(|(a,b)| a != b) { break; }
                        let kind = payload.get("type").and_then(Value::as_str).unwrap_or_default();
                        if !pockymoe_protocol::access::client_message_allowed(&owner_access(), kind) { break; }
                        let fresh_access = {
                            let conn = state.store.conn.lock().await;
                            if load_user_by_session(&conn, &state.store.session_secret, &session_token).is_none() { break; }
                            effective_access(
                                &conn,
                                &user_id,
                                &device_id,
                                thread_id.as_deref().or(message_thread),
                                None,
                            )
                        };
                        let Some(fresh_access) = fresh_access else {
                            break;
                        };
                        if !pockymoe_protocol::access::client_message_allowed(&fresh_access, kind) {
                            break;
                        }
                        if kind.starts_with("shell.") {
                            let Some(shell_id) = payload.get("shellId").and_then(Value::as_str) else { break; };
                            if kind == "shell.attach" && fresh_access.kind == "shared" {
                                let Some(scope) = thread_id.as_deref().or(message_thread) else { break; };
                                if scope.contains(['/', '?', '#']) { break; }
                                if !shared_shell_scope_allowed(&state,&device_id,scope,shell_id,secure_channel_id.is_some()).await { break; }
                            } else if kind != "shell.attach" {
                                let clients = state.clients.read().await;
                                if !clients.get(&client_id).is_some_and(|c| c.attached_shell_id.as_deref() == Some(shell_id)) { break; }
                            }
                        }
                        match payload.get("type").and_then(Value::as_str) {
                            Some("shell.attach") => {
                                if let Some(shell_id) = payload.get("shellId").and_then(Value::as_str) {
                                    if let Some(client) = state.clients.write().await.get_mut(&client_id) {
                                        client.attached_shell_id = Some(shell_id.to_string());
                                    }
                                }
                            }
                            Some("shell.detach") => {
                                if let Some(client) = state.clients.write().await.get_mut(&client_id) {
                                    client.attached_shell_id = None;
                                }
                            }
                            _ => {}
                        }
                        send_to_supervisor_connection(
                            &state,
                            &device_id,
                            supervisor_connection_id,
                            json!({
                                "type": "relay.client.message",
                                "timestamp": now_rfc3339(),
                                "clientId": client_id,
                                "payload": payload
                            }),
                        ).await;
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            outgoing = rx.recv() => {
                match outgoing {
                    Some(text) => {
                        let valid = { let conn = state.store.conn.lock().await;
                            load_user_by_session(&conn, &state.store.session_secret, &session_token).is_some() };
                        if !valid || !matches!(tokio::time::timeout(Duration::from_secs(10),sink.send(Message::Text(text.into()))).await,Ok(Ok(()))) {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }
    state.clients.write().await.remove(&client_id);
    send_to_supervisor_connection(
        &state,
        &device_id,
        supervisor_connection_id,
        json!({
            "type": "relay.client.disconnected",
            "timestamp": now_rfc3339(),
            "clientId": client_id
        }),
    )
    .await;
}

async fn send_to_supervisor_connection(
    state: &AppState,
    device_id: &str,
    connection_id: Uuid,
    message: Value,
) {
    let overflow = state
        .sockets
        .read()
        .await
        .get(device_id)
        .is_some_and(|supervisor| {
            supervisor.connection_id == connection_id
                && supervisor.tx.try_send(message.to_string()).is_err()
        });
    if overflow {
        let mut sockets = state.sockets.write().await;
        if sockets
            .get(device_id)
            .is_some_and(|s| s.connection_id == connection_id)
        {
            sockets.remove(device_id);
        }
        drop(sockets);
        state
            .clients
            .write()
            .await
            .retain(|_, c| c.device_id != device_id || c.supervisor_connection_id != connection_id);
    }
}

async fn forward_server_message_to_client(
    state: &AppState,
    device_id: &str,
    connection_id: Uuid,
    client_id: &str,
    payload: &Value,
) {
    let client = {
        let clients = state.clients.read().await;
        clients.get(client_id).and_then(|client| {
            (client.device_id == device_id && client.supervisor_connection_id == connection_id)
                .then(|| {
                    (
                        client.tx.clone(),
                        client.user_id.clone(),
                        client.thread_id.clone(),
                        client.attached_shell_id.clone(),
                        client.session_token.clone(),
                    )
                })
        })
    };
    let Some((tx, user_id, configured_thread, attached_shell, session_token)) = client else {
        return;
    };
    let event_thread = payload.get("threadId").and_then(Value::as_str);
    if !pockymoe_protocol::access::outbound_event_in_scope(
        configured_thread.as_deref(),
        attached_shell.as_deref(),
        payload,
    ) {
        return;
    }
    let authorized = {
        let conn = state.store.conn.lock().await;
        let access = effective_access(
            &conn,
            &user_id,
            device_id,
            configured_thread.as_deref().or(event_thread),
            None,
        );
        access.is_some()
            && load_user_by_session(&conn, &state.store.session_secret, &session_token).is_some()
    };
    if !authorized || tx.try_send(payload.to_string()).is_err() {
        state.clients.write().await.remove(client_id);
    }
}

const RELAY_BOOTSTRAP: &str = r#"<script>window.__POCKYMOE_BOOTSTRAP__={"mode":"relay"};</script>"#;

fn inject_bootstrap(html: &str) -> String {
    if html.contains("</head>") {
        html.replacen("</head>", &format!("{RELAY_BOOTSTRAP}</head>"), 1)
    } else {
        format!("{RELAY_BOOTSTRAP}{html}")
    }
}

fn mime_for(path: &FsPath) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    fn temporary_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("pockymoe-relay-{name}-{}", Uuid::new_v4().simple()))
    }

    #[tokio::test]
    async fn strict_encrypted_shells_skip_plaintext_probe_and_unpaired_shells_keep_scope_checks() {
        for (secure, strict, shell_present, expected) in [
            (true, true, false, true),
            (true, false, true, true),
            (true, false, false, false),
            (false, false, false, false),
            (false, true, false, false),
        ] {
            let (state, dir) = test_app_state("shell-probe");
            let (tx, mut requests) = tokio::sync::mpsc::channel(4);
            state.sockets.write().await.insert(
                "device".into(),
                DeviceSocket {
                    tx,
                    connection_id: Uuid::new_v4(),
                    connected_at: "now".into(),
                    last_heartbeat_at: "now".into(),
                },
            );
            let request_state = state.clone();
            let task = tokio::spawn(async move {
                shared_shell_scope_allowed(&request_state, "device", "thread", "shell", secure)
                    .await
            });
            if secure {
                let request: Value = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
                assert_eq!(request["payload"]["path"], "/api/transport/key");
                let pending = state
                    .pending
                    .lock()
                    .unwrap()
                    .remove(request["requestId"].as_str().unwrap())
                    .unwrap();
                pending.tx.send(json!({"statusCode":200,"headers":{"content-type":"application/json"},"body":json!({"trust":{"strict":strict}}).to_string()})).unwrap();
            }
            if !secure || !strict {
                let request: Value = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
                assert_eq!(
                    request["payload"]["path"],
                    "/api/threads/thread/transport/shell-scope"
                );
                let pending = state
                    .pending
                    .lock()
                    .unwrap()
                    .remove(request["requestId"].as_str().unwrap())
                    .unwrap();
                let response = if strict {
                    json!({"statusCode":403,"body":json!({"code":"device_strict_mode"}).to_string()})
                } else {
                    json!({"statusCode":200,"body":json!({"shells":if shell_present {json!([{"id":"shell"}])} else {json!([{"id":"other"}])}}).to_string()})
                };
                pending.tx.send(response).unwrap();
            }
            assert_eq!(task.await.unwrap(), expected);
            assert!(
                requests.try_recv().is_err(),
                "strict devices received an unexpected plaintext scope probe"
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    pub(super) fn test_app_state(name: &str) -> (Arc<AppState>, PathBuf) {
        let data_dir = temporary_test_dir(name);
        let store = RelayStore::open(
            data_dir.join("relay-store.sqlite"),
            "test-secret".to_string(),
        )
        .unwrap();
        (
            Arc::new(AppState {
                store,
                sockets: RwLock::new(HashMap::new()),
                clients: RwLock::new(HashMap::new()),
                pending: StdMutex::new(HashMap::new()),
                web_dist: None,
                legacy_supervisor_token: None,
                oauth: OAuthConfig::default(),
                oauth_client: reqwest::Client::new(),
                admission: security::Admission::default(),
                preview: preview::Hub::default(),
            }),
            data_dir,
        )
    }

    #[test]
    fn avatar_rejects_remote_active_invalid_and_oversized_content() {
        assert_eq!(avatar_data(&Value::Null).unwrap(), None);
        for value in [
            json!("https://example.test/avatar.png"),
            json!("data:image/svg+xml;base64,PHN2Zy8+"),
            json!("data:image/png;base64,aGVsbG8="),
            json!("data:image/png;base64,!!!"),
            json!(42),
            json!(format!("data:image/png;base64,{}", "A".repeat(131072))),
        ] {
            assert!(avatar_data(&value).is_err());
        }
        let image = json!("data:image/png;base64,iVBORw0KGgo=");
        assert_eq!(avatar_data(&image).unwrap(), image.as_str());
    }

    fn device_socket(connection_id: Uuid) -> (DeviceSocket, tokio::sync::mpsc::Receiver<String>) {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        (
            DeviceSocket {
                tx,
                connection_id,
                connected_at: "2026-01-01T00:00:00Z".to_string(),
                last_heartbeat_at: "2026-01-01T00:00:00Z".to_string(),
            },
            rx,
        )
    }

    fn client_socket(device_id: &str, supervisor_connection_id: Uuid) -> ClientSocket {
        let (tx, _rx) = tokio::sync::mpsc::channel(64);
        ClientSocket {
            tx,
            device_id: device_id.to_string(),
            supervisor_connection_id,
            user_id: "user".to_string(),
            thread_id: None,
            attached_shell_id: None,
            session_token: String::new(),
        }
    }

    #[tokio::test]
    async fn silent_supervisor_expires_even_with_outgoing_traffic_and_future_heartbeat() {
        use tokio_tungstenite::{connect_async, tungstenite::Message as ClientMessage};
        let (state, data_dir) = test_app_state("silent-supervisor");
        let app_state = state.clone();
        let app = Router::new().route(
            "/tunnel",
            get(move |ws: WebSocketUpgrade| async move {
                ws.on_upgrade(move |socket| {
                    handle_supervisor_with_timeout(
                        socket,
                        app_state,
                        "device".into(),
                        Duration::from_millis(250),
                    )
                })
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (mut socket, _) = connect_async(format!("ws://{address}/tunnel"))
            .await
            .unwrap();
        // relay.connected and the initial durable revocation list
        socket.next().await.unwrap().unwrap();
        socket.next().await.unwrap().unwrap();
        // A healthy device must remain online across multiple lease periods.
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            socket
                .send(ClientMessage::Text(
                    json!({"type":"relay.heartbeat"}).to_string().into(),
                ))
                .await
                .unwrap();
            socket
                .send(ClientMessage::Ping(Vec::new().into()))
                .await
                .unwrap();
            assert!(matches!(
                socket.next().await.unwrap().unwrap(),
                ClientMessage::Pong(_)
            ));
            assert!(state.sockets.read().await.contains_key("device"));
        }
        socket
            .send(ClientMessage::Text(
                json!({"type":"relay.heartbeat","timestamp":"2999-01-01T00:00:00Z"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        // Receiving a pong proves the preceding heartbeat has been processed.
        socket
            .send(ClientMessage::Ping(Vec::new().into()))
            .await
            .unwrap();
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            ClientMessage::Pong(_)
        ));
        let (connection_id, tx) = {
            let sockets = state.sockets.read().await;
            let device = sockets.get("device").unwrap();
            assert!(!device.last_heartbeat_at.starts_with("2999"));
            (device.connection_id, device.tx.clone())
        };
        state
            .clients
            .write()
            .await
            .insert("client".into(), client_socket("device", connection_id));
        let request_state = state.clone();
        let request = tokio::spawn(async move {
            forward_device_with_timeout(
                request_state,
                "device".into(),
                "GET".into(),
                "/api/transport/key".into(),
                None,
                None,
                json!({}),
                Duration::from_secs(10),
            )
            .await
        });
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            ClientMessage::Text(_)
        ));
        // Simulate a half-open connection: continue writing towards it, but no
        // supervisor frames arrive. Outbound traffic must not renew liveness.
        let writer = tokio::spawn(async move {
            while tx.send("{}".into()).await.is_ok() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let response = tokio::time::timeout(Duration::from_secs(2), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(!state.sockets.read().await.contains_key("device"));
        assert!(!state.clients.read().await.contains_key("client"));
        assert!(state.pending.lock().unwrap().is_empty());
        writer.await.unwrap();
        server.abort();
        let _ = server.await;
        drop(socket);
        drop(state);
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn stale_supervisor_cleanup_preserves_replacement_socket_and_clients() {
        let (state, data_dir) = test_app_state("connection-replacement");
        let device_id = "device";
        let old_connection_id = Uuid::new_v4();
        let new_connection_id = Uuid::new_v4();
        let (new_socket, _new_socket_rx) = device_socket(new_connection_id);
        state
            .sockets
            .write()
            .await
            .insert(device_id.to_string(), new_socket);
        {
            let mut clients = state.clients.write().await;
            clients.insert(
                "old-client".to_string(),
                client_socket(device_id, old_connection_id),
            );
            clients.insert(
                "new-client".to_string(),
                client_socket(device_id, new_connection_id),
            );
            clients.insert(
                "other-device-client".to_string(),
                client_socket("other-device", old_connection_id),
            );
        }

        let (old_tx, old_rx) = tokio::sync::oneshot::channel();
        let (new_tx, mut new_rx) = tokio::sync::oneshot::channel();
        {
            let mut pending = state.pending.lock().unwrap();
            pending.insert(
                "old".into(),
                PendingDeviceRequest {
                    device_id: device_id.into(),
                    connection_id: old_connection_id,
                    peer_device_id: None,
                    tx: old_tx,
                },
            );
            pending.insert(
                "new".into(),
                PendingDeviceRequest {
                    device_id: device_id.into(),
                    connection_id: new_connection_id,
                    peer_device_id: None,
                    tx: new_tx,
                },
            );
        }
        remove_supervisor_connection(&state, device_id, old_connection_id).await;
        assert!(old_rx.await.is_err());
        assert!(matches!(
            new_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        assert!(state.pending.lock().unwrap().contains_key("new"));

        assert_eq!(
            state
                .sockets
                .read()
                .await
                .get(device_id)
                .map(|socket| socket.connection_id),
            Some(new_connection_id)
        );
        let clients = state.clients.read().await;
        assert!(!clients.contains_key("old-client"));
        assert!(clients.contains_key("new-client"));
        assert!(clients.contains_key("other-device-client"));
        drop(clients);

        drop(state);
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn signed_sessions_round_trip_and_reject_tampering() {
        let dir = temporary_test_dir("signed-sessions");
        let store = RelayStore::open(dir.join("db"), "session-secret".into()).unwrap();
        let conn = store.conn.try_lock().unwrap();
        conn.execute("INSERT INTO relay_users VALUES ('owner','o@example.test','owner','user',1,NULL,'now','salt','hash')", []).unwrap();
        let token = create_session(&conn, "session-secret", "owner").unwrap();
        assert_eq!(
            verify_session("session-secret", &token)
                .expect("new session must verify")
                .user_id,
            "owner"
        );
        let mut tampered = token.into_bytes();
        let last = tampered.last_mut().expect("session is non-empty");
        *last = if *last == b'a' { b'b' } else { b'a' };
        assert!(
            verify_session("session-secret", std::str::from_utf8(&tampered).unwrap()).is_none()
        );
    }

    #[tokio::test]
    async fn snapshots_existing_node_database_before_first_rust_open() {
        let data_dir = temporary_test_dir("node-backup");
        std::fs::create_dir_all(&data_dir).unwrap();
        let database_path = data_dir.join("relay-store.sqlite");
        let initial = RelayStore::open(database_path.clone(), "secret".to_string()).unwrap();
        {
            let conn = initial.conn.lock().await;
            let (salt, hash) = hash_password("password-1").unwrap();
            conn.execute(
                "INSERT INTO relay_users
                 (id,email,username,role,enabled,last_seen_at,created_at,password_salt,password_hash)
                 VALUES ('user','user@example.com','user','user',1,NULL,'2026-01-01T00:00:00Z',?1,?2)",
                params![salt, hash],
            )
            .unwrap();
        }
        drop(initial);

        let plan = inspect_relay_migration(&data_dir).unwrap();
        assert_eq!(plan.source_kind, "node");
        assert!(!plan.ready_for_rust);
        let refusal = RelayStore::open_data_dir(&data_dir, "secret".to_string(), false)
            .err()
            .expect("normal startup must refuse an unapproved Node database");
        assert!(refusal.to_string().contains("relay-migrate"));
        let report = migrate_relay_data_dir(&data_dir).unwrap();
        assert!(report.ready_for_rust);
        assert_eq!(report.user_count, 1);
        let reopened = RelayStore::open_data_dir(&data_dir, "secret".to_string(), false).unwrap();
        let backup_path = data_dir.join("relay-store.pre-rust-0.12.sqlite");
        assert!(backup_path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&backup_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let backup =
            Connection::open_with_flags(backup_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            backup
                .query_row("SELECT COUNT(*) FROM relay_users", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(backup);
        drop(reopened);
        std::fs::remove_dir_all(&data_dir).unwrap();
    }

    #[test]
    fn legacy_import_fails_on_unmappable_rows_and_keeps_source() {
        let data_dir = temporary_test_dir("invalid-legacy-import");
        std::fs::create_dir_all(&data_dir).unwrap();
        let legacy_path = data_dir.join("relay.sqlite");
        let legacy = Connection::open(&legacy_path).unwrap();
        legacy
            .execute_batch(
                "
                CREATE TABLE users (
                  id TEXT PRIMARY KEY,email TEXT NOT NULL,username TEXT NOT NULL,
                  password_hash TEXT NOT NULL,role TEXT NOT NULL,enabled INTEGER NOT NULL,created_at TEXT NOT NULL
                );
                CREATE TABLE devices (
                  id TEXT PRIMARY KEY,user_id TEXT NOT NULL,name TEXT NOT NULL,token TEXT,
                  token_hash TEXT NOT NULL,token_preview TEXT,created_at TEXT NOT NULL
                );
                CREATE TABLE shares (
                  id TEXT PRIMARY KEY,owner_user_id TEXT NOT NULL,target_username TEXT NOT NULL,
                  device_id TEXT NOT NULL,thread_id TEXT,workspace_id TEXT,thread_access TEXT NOT NULL,
                  workspace_access TEXT NOT NULL,created_at TEXT NOT NULL,revoked_at TEXT
                );
                CREATE TABLE grants (
                  id TEXT PRIMARY KEY,owner_user_id TEXT NOT NULL,target_username TEXT NOT NULL,
                  device_id TEXT NOT NULL,scope TEXT NOT NULL,thread_id TEXT,workspace_id TEXT,
                  thread_access TEXT NOT NULL,workspace_access TEXT NOT NULL,
                  can_create_threads INTEGER NOT NULL,created_at TEXT NOT NULL,revoked_at TEXT
                );
                INSERT INTO users VALUES
                  ('owner','owner@example.com','owner','legacy','user',1,'2026-01-01T00:00:00Z');
                INSERT INTO devices VALUES
                  ('device','owner','Laptop','rcd_old','old-hash','rcd_old','2026-01-01T00:00:00Z');
                INSERT INTO shares VALUES
                  ('share','owner','missing-target','device','thread',NULL,'read','none','2026-01-01T00:00:00Z',NULL);
                ",
            )
            .unwrap();
        drop(legacy);

        let error = migrate_relay_data_dir(&data_dir).unwrap_err();
        assert!(error.to_string().contains("count mismatch"));
        assert!(legacy_path.exists());
        assert!(!data_dir.join("relay-store.sqlite").exists());
        std::fs::remove_dir_all(&data_dir).unwrap();
    }
}

fn safe_dist_path(dist: &FsPath, url_path: &str) -> Option<PathBuf> {
    let rel = url_path.trim_start_matches('/');
    if rel.is_empty() {
        return Some(dist.join("index.html"));
    }
    if rel.split('/').any(|segment| segment == "..") {
        return None;
    }
    Some(dist.join(rel))
}

async fn spa_fallback(State(state): State<Arc<AppState>>, req: Request) -> Response {
    // API-only relays cannot supply executable content to the trusted Web origin.
    let path = req.uri().path();
    let api = path == "/relay"
        || path.starts_with("/relay/")
        || path == "/healthz"
        || path == "/supervisor/tunnel";
    if state.oauth.api_only(req.headers()) {
        if api {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "code": "not_found", "message": "not found" })),
            )
                .into_response();
        }
        if req.method() != Method::GET && req.method() != Method::HEAD {
            return StatusCode::NOT_FOUND.into_response();
        }
        let target = format!(
            "{}{}",
            state.oauth.web_origin.as_deref().unwrap(),
            req.uri()
                .path_and_query()
                .map(|p| p.as_str())
                .unwrap_or("/")
        );
        return Redirect::temporary(&target).into_response();
    }
    let Some(dist) = state.web_dist.as_ref() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "code": "not_found", "message": "not found" })),
        )
            .into_response();
    };
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "code": "not_found", "message": "not found" })),
        )
            .into_response();
    }
    let path = req.uri().path();
    if path.starts_with("/relay/") || path == "/healthz" || path == "/supervisor/tunnel" {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "code": "not_found", "message": "not found" })),
        )
            .into_response();
    }
    let Some(candidate) = safe_dist_path(dist, path) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let has_ext = FsPath::new(path).extension().is_some();
    let file_path = if tokio::fs::metadata(&candidate)
        .await
        .map(|meta| meta.is_file())
        .unwrap_or(false)
    {
        candidate
    } else if has_ext {
        return StatusCode::NOT_FOUND.into_response();
    } else {
        dist.join("index.html")
    };
    match tokio::fs::read(&file_path).await {
        Ok(bytes) => {
            let is_html = file_path.extension().and_then(|ext| ext.to_str()) == Some("html");
            let (body, content_type) = if is_html {
                let html = String::from_utf8_lossy(&bytes);
                (
                    Body::from(inject_bootstrap(&html)),
                    "text/html; charset=utf-8",
                )
            } else {
                (Body::from(bytes), mime_for(&file_path))
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header(
                    header::CACHE_CONTROL,
                    if is_html {
                        "no-cache"
                    } else {
                        "public, max-age=31536000, immutable"
                    },
                )
                .body(body)
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "code": "not_found",
                "message": "Relay web frontend is not built."
            })),
        )
            .into_response(),
    }
}
