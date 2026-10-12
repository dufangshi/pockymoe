//! Opaque account vaults and the public account-certificate directory.
//! No account private key, password or decrypted vault enters this module.
use crate::auth_api::{auth, failure, internal, invalid, ApiResult, Failure};
use crate::{auth_factors as factors, security, AppState};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use hmac::{Hmac, Mac};
use openssl::{bn::BigNum, ecdsa::EcdsaSig, nid::Nid, pkey::PKey, sign::Verifier};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub(crate) const MAX_VAULT_BYTES: usize = 64 * 1024;
pub(crate) const FULL: &str = "full";
pub(crate) const SETUP: &str = "setup";
pub(crate) const RECOVERY: &str = "recovery";
pub(crate) const RESET: &str = "reset";

pub(crate) fn ensure_schema(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS relay_account_vaults (
        user_id TEXT PRIMARY KEY REFERENCES relay_users(id) ON DELETE CASCADE,
        vault_json TEXT NOT NULL, revision INTEGER NOT NULL,
        account_key_id TEXT NOT NULL, account_public_key TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS relay_account_auth (
        user_id TEXT PRIMARY KEY REFERENCES relay_users(id) ON DELETE CASCADE,
        auth_salt TEXT, auth_hash TEXT, recovery_salt TEXT, recovery_hash TEXT,
        oauth_only INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS relay_account_browsers (
        user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
        browser_id TEXT NOT NULL, certificate_json TEXT NOT NULL, label TEXT NOT NULL,
        created_at INTEGER NOT NULL, last_seen_at INTEGER NOT NULL,
        PRIMARY KEY(user_id,browser_id)
    );
    CREATE TABLE IF NOT EXISTS relay_account_revocations (
        user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
        account_key_id TEXT NOT NULL, kind TEXT NOT NULL, subject_id TEXT NOT NULL,
        revocation_json TEXT NOT NULL, PRIMARY KEY(user_id,account_key_id,kind,subject_id)
    );
    CREATE TABLE IF NOT EXISTS relay_pending_account_vaults (
        request_id TEXT PRIMARY KEY REFERENCES relay_pending_registrations(id) ON DELETE CASCADE,
        user_id TEXT NOT NULL UNIQUE, vault_json TEXT NOT NULL,
        recovery_salt TEXT NOT NULL, recovery_hash TEXT NOT NULL
    );",
    )?;
    if conn
        .prepare("SELECT account_mode FROM relay_auth_sessions LIMIT 0")
        .is_err()
    {
        conn.execute(
            "ALTER TABLE relay_auth_sessions ADD COLUMN account_mode TEXT NOT NULL DEFAULT 'full'",
            [],
        )?;
    }
    for (column, definition) in [
        ("reset_pending", "INTEGER NOT NULL DEFAULT 0"),
        ("reset_consumed", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        if conn
            .prepare(&format!("SELECT {column} FROM relay_account_auth LIMIT 0"))
            .is_err()
        {
            conn.execute(
                &format!("ALTER TABLE relay_account_auth ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    Ok(())
}
pub(crate) fn reset_available(conn: &Connection, user: &str) -> bool {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM relay_account_auth WHERE user_id=?1 AND reset_pending=1 AND reset_consumed=0)",[user],|r|r.get(0)).unwrap_or(false)
}
pub(crate) fn legacy_password_allowed(conn: &Connection, user: &str) -> bool {
    !has_vault(conn,user) && !conn.query_row("SELECT EXISTS(SELECT 1 FROM relay_account_auth WHERE user_id=?1 AND reset_consumed=1)",[user],|r|r.get::<_,bool>(0)).unwrap_or(true)
}
pub(crate) fn consume_reset(conn: &Connection, user: &str) -> Result<(), Failure> {
    let changed = conn.execute("UPDATE relay_account_auth SET reset_consumed=1 WHERE user_id=?1 AND reset_pending=1 AND reset_consumed=0",[user]).map_err(internal)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(failure(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Temporary password was already used",
        ))
    }
}
fn clear_reset(conn: &Connection, user: &str) -> Result<(), Failure> {
    conn.execute(
        "UPDATE relay_account_auth SET reset_consumed=CASE WHEN reset_pending=1 THEN 1 ELSE reset_consumed END,reset_pending=0 WHERE user_id=?1",
        [user],
    )
    .map_err(internal)?;
    Ok(())
}

pub(crate) fn vault(conn: &Connection, user: &str) -> Result<Option<Value>, Failure> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT vault_json FROM relay_account_vaults WHERE user_id=?1",
            [user],
            |r| r.get(0),
        )
        .optional()
        .map_err(internal)?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(internal))
        .transpose()
}
pub(crate) fn has_vault(conn: &Connection, user: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM relay_account_vaults WHERE user_id=?1)",
        [user],
        |r| r.get(0),
    )
    .unwrap_or(false)
}
pub(crate) fn oauth_only(conn: &Connection, user: &str) -> bool {
    // Existing OAuth-created accounts predate account-auth metadata. They already
    // had no user-selected login password; preserve their rows and recognize the
    // existing provider identity when no explicit authentication profile exists.
    conn.query_row(
        "SELECT COALESCE((SELECT oauth_only FROM relay_account_auth WHERE user_id=?1),
         EXISTS(SELECT 1 FROM relay_user_identities WHERE user_id=?1))",
        [user],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

pub(crate) fn login_mode(conn: &Connection, user: &crate::UserRow) -> &'static str {
    if user.role != "admin" && !has_vault(conn, &user.id) {
        SETUP
    } else {
        FULL
    }
}
pub(crate) fn session_mode(conn: &Connection, token: &str) -> String {
    conn.query_row(
        "SELECT account_mode FROM relay_auth_sessions WHERE token_hash=?1",
        [security::token_hash(token)],
        |r| r.get(0),
    )
    .unwrap_or_else(|_| SETUP.into())
}
pub(crate) fn mark_session(conn: &Connection, token: &str, mode: &str) -> Result<(), Failure> {
    conn.execute(
        "UPDATE relay_auth_sessions SET account_mode=?2 WHERE token_hash=?1",
        params![security::token_hash(token), mode],
    )
    .map_err(internal)?;
    Ok(())
}
pub(crate) fn session_json(
    conn: &Connection,
    user: &crate::UserRow,
    oauth: &crate::OAuthConfig,
    token: &str,
) -> Value {
    let mut value = crate::session_json(conn, Some(user), oauth);
    let has_vault = has_vault(conn, &user.id);
    let mode = if oauth_only(conn, &user.id) {
        "oauth"
    } else if has_vault {
        "account"
    } else {
        "legacy"
    };
    let restriction = session_mode(conn, token);
    value["account"] = json!({
        "mode":mode,"setupRequired":restriction == SETUP,
        "recovery":matches!(restriction.as_str(),RECOVERY|RESET),
        "hasVault":has_vault,
        "accountKeyId":vault(conn,&user.id).ok().flatten().map(|v|v["accountKeyId"].clone()).unwrap_or(Value::Null),
    });
    value
}

pub(crate) fn valid_key(key: &str) -> bool {
    B64.decode(key)
        .is_ok_and(|bytes| bytes.len() == 32 && B64.encode(bytes) == key)
}
fn decoded(s: &Value, len: Option<usize>) -> bool {
    s.as_str().is_some_and(|s| {
        B64.decode(s).is_ok_and(|b| {
            !b.is_empty() && len.is_none_or(|len| b.len() == len) && B64.encode(b) == s
        })
    })
}
fn public_key(key: &str) -> Option<PKey<openssl::pkey::Public>> {
    let der = B64.decode(key).ok()?;
    let key = PKey::public_key_from_der(&der).ok()?;
    let ec = key.ec_key().ok()?;
    (ec.group().curve_name() == Some(Nid::X9_62_PRIME256V1)).then_some(key)
}
pub(crate) fn validate_vault(vault: &Value, user: &str) -> Result<(), Failure> {
    if serde_json::to_vec(vault).map_err(internal)?.len() > MAX_VAULT_BYTES {
        return Err(failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "Account vault is too large",
        ));
    }
    let kdf = &vault["kdf"];
    let key = vault["accountPublicKey"].as_str().unwrap_or_default();
    if vault["version"] != 1
        || vault["userId"].as_str() != Some(user)
        || user.is_empty()
        || user.len() > 128
        || vault["revision"]
            .as_u64()
            .is_none_or(|n| n == 0 || n > i64::MAX as u64)
        || public_key(key).is_none()
        || vault["accountKeyId"].as_str()
            != B64
                .decode(key)
                .ok()
                .map(|k| B64.encode(Sha256::digest(k)))
                .as_deref()
        || kdf["name"] != "argon2id"
        || kdf["memoryKiB"].as_u64().is_none_or(|n| n < 65536)
        || kdf["iterations"].as_u64().is_none_or(|n| n < 3)
        || kdf["parallelism"].as_u64().is_none_or(|n| n < 1)
        || !decoded(&kdf["salt"], Some(16))
    {
        return Err(invalid("Invalid account vault"));
    }
    for slot in [
        &vault["slots"]["password"],
        &vault["slots"]["recovery"],
        &vault["body"],
    ] {
        if !decoded(&slot["iv"], Some(12))
            || !decoded(&slot["ciphertext"], None)
            || B64
                .decode(slot["ciphertext"].as_str().unwrap())
                .unwrap()
                .len()
                < 16
        {
            return Err(invalid("Invalid encrypted vault field"));
        }
    }
    Ok(())
}
pub(crate) fn store_vault(conn: &Connection, user: &str, value: &Value) -> Result<(), Failure> {
    conn.execute("INSERT INTO relay_account_vaults(user_id,vault_json,revision,account_key_id,account_public_key) VALUES (?1,?2,?3,?4,?5)
        ON CONFLICT(user_id) DO UPDATE SET vault_json=excluded.vault_json,revision=excluded.revision",
        params![user, serde_json::to_string(value).map_err(internal)?, value["revision"].as_i64(), value["accountKeyId"].as_str(), value["accountPublicKey"].as_str()]).map_err(internal)?;
    Ok(())
}
fn verify_auth(conn: &Connection, user: &str, key: Option<&str>) -> bool {
    let Some(key) = key.filter(|key| valid_key(key)) else {
        return false;
    };
    let hash: Option<(String, String)> = conn.query_row("SELECT auth_salt,auth_hash FROM relay_account_auth WHERE user_id=?1 AND auth_hash IS NOT NULL", [user], |r| Ok((r.get(0)?, r.get(1)?))).optional().ok().flatten();
    hash.is_some_and(|(salt, hash)| crate::verify_password(key, &salt, &hash))
}
pub(crate) fn account_public(conn: &Connection, user: &str, code: &str) -> ApiResult {
    let value = vault(conn, user)?
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, code, "Account key is not available"))?;
    Ok(Json(
        json!({"accountKeyId":value["accountKeyId"],"accountPublicKey":value["accountPublicKey"]}),
    ))
}
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/relay/auth/kdf", get(kdf))
        .route("/relay/auth/recover", post(recover))
        .route(
            "/relay/account/vault",
            get(read_vault)
                .put(write_vault)
                .delete(delete_vault)
                .layer(DefaultBodyLimit::max(MAX_VAULT_BYTES + 4096)),
        )
        .route(
            "/relay/account/browsers",
            get(browsers).post(register_browser),
        )
        .route("/relay/account/revocations", post(revoke))
        .route("/relay/devices/{id}/owner-account", get(owner_account))
        .route("/relay/shares/{id}/recipient-account", get(share_account))
        .route("/relay/grants/{id}/recipient-account", get(grant_account))
        .layer(axum::middleware::from_fn(no_store))
}
async fn no_store(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
#[derive(Deserialize)]
struct KdfQuery {
    login: String,
}
async fn kdf(
    State(state): State<Arc<AppState>>,
    Query(query): Query<KdfQuery>,
) -> Result<Response, Failure> {
    if query.login.len() > 320 {
        return Err(invalid("Invalid login"));
    }
    let login = query.login.trim().to_ascii_lowercase();
    let conn = state.store.conn.lock().await;
    let user: Option<String> = conn
        .query_row(
            "SELECT id FROM relay_users WHERE username=?1 OR email=?1",
            [&login],
            |r| r.get(0),
        )
        .optional()
        .map_err(internal)?;
    let known = user
        .as_deref()
        .map(|user| vault(&conn, user))
        .transpose()?
        .flatten();
    let mut mac =
        Hmac::<Sha256>::new_from_slice(state.store.session_secret.as_bytes()).map_err(internal)?;
    mac.update(b"pockymoe-fake-kdf-v1\n");
    mac.update(login.as_bytes());
    let salt = B64.encode(&mac.finalize().into_bytes()[..16]);
    let value = json!({"mode":if user.is_some() && (known.is_none() || user.as_deref().is_some_and(|user|reset_available(&conn,user))) { "legacy" } else { "account" },"kdf":known.map(|v| v["kdf"].clone()).unwrap_or_else(|| json!({"name":"argon2id","memoryKiB":65536,"iterations":3,"parallelism":1,"salt":salt}))});
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response())
}
async fn read_vault(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, _) = auth(&conn, &state, &headers)?;
    Ok(Json(vault(&conn, &user.id)?.ok_or_else(|| {
        failure(
            StatusCode::NOT_FOUND,
            "vault_missing",
            "Secure your account first",
        )
    })?))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultWrite {
    vault: Value,
    expected_revision: u64,
    auth_key: Option<String>,
    current_auth_key: Option<String>,
    recovery_auth_key: Option<String>,
}
async fn write_vault(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let body: VaultWrite =
        serde_json::from_slice(&body).map_err(|_| invalid("Invalid vault request"))?;
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    validate_vault(&body.vault, &user.id)?;
    let old = vault(&conn, &user.id)?;
    let revision = old
        .as_ref()
        .and_then(|v| v["revision"].as_u64())
        .unwrap_or(0);
    if body.expected_revision != revision
        || body.vault["revision"].as_u64() != revision.checked_add(1)
    {
        return Err(failure(
            StatusCode::CONFLICT,
            "vault_conflict",
            "Account vault changed; fetch it again",
        ));
    }
    let mode = session_mode(&conn, &token);
    let oauth = oauth_only(&conn, &user.id);
    if let Some(old) = &old {
        for field in ["userId", "accountKeyId", "accountPublicKey"] {
            if old[field] != body.vault[field] {
                return Err(invalid("Account key identity cannot change"));
            }
        }
    } else if mode != SETUP && !oauth {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Sign in to create your account vault",
        ));
    }
    let password_changed = old.as_ref().is_none_or(|v| {
        v["slots"]["password"] != body.vault["slots"]["password"] || v["kdf"] != body.vault["kdf"]
    });
    let recovery_changed = old
        .as_ref()
        .is_none_or(|v| v["slots"]["recovery"] != body.vault["slots"]["recovery"]);
    if old.is_some() && body.auth_key.is_some() && !password_changed {
        return Err(invalid("A new authKey requires a changed password slot"));
    }
    if old.is_some() && body.recovery_auth_key.is_some() && !recovery_changed {
        return Err(invalid(
            "A new recoveryAuthKey requires a changed recovery slot",
        ));
    }
    let auth_changed = body.auth_key.is_some();
    let recovery_key_changed = body.recovery_auth_key.is_some();
    if matches!(mode.as_str(), RECOVERY | RESET)
        && (!password_changed || (!oauth && body.auth_key.is_none()))
    {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Recovery requires a new password slot and authKey",
        ));
    }
    if oauth
        && (password_changed || recovery_changed || auth_changed || recovery_key_changed)
        && !factors::recent(&conn, &token)
    {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "reauthentication_required",
            "Authenticate again before changing vault slots",
        ));
    }
    if old.is_some()
        && (password_changed || recovery_changed || auth_changed || recovery_key_changed)
        && !oauth
        && !matches!(mode.as_str(), RECOVERY | RESET)
        && !verify_auth(&conn, &user.id, body.current_auth_key.as_deref())
    {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Current authKey is required",
        ));
    }
    if !oauth && password_changed && body.auth_key.is_none()
        || recovery_changed && body.recovery_auth_key.is_none()
    {
        return Err(invalid(
            "Changed slots require their new authentication keys",
        ));
    }
    if oauth && body.auth_key.is_some() {
        return Err(invalid("OAuth-only accounts have no login authKey"));
    }
    for key in [&body.auth_key, &body.recovery_auth_key]
        .into_iter()
        .flatten()
    {
        if !valid_key(key) {
            return Err(invalid("Authentication keys must be B64 of 32 bytes"));
        }
    }
    let auth_hash = body
        .auth_key
        .as_deref()
        .map(crate::hash_password)
        .transpose()
        .map_err(internal)?;
    let recovery_hash = body
        .recovery_auth_key
        .as_deref()
        .map(crate::hash_password)
        .transpose()
        .map_err(internal)?;
    let tx = conn.unchecked_transaction().map_err(internal)?;
    store_vault(&tx, &user.id, &body.vault)?;
    tx.execute(
        "INSERT OR IGNORE INTO relay_account_auth(user_id,oauth_only) VALUES (?1,?2)",
        params![user.id, oauth],
    )
    .map_err(internal)?;
    if let Some((salt, hash)) = auth_hash {
        tx.execute(
            "UPDATE relay_account_auth SET auth_salt=?2,auth_hash=?3 WHERE user_id=?1",
            params![user.id, salt, hash],
        )
        .map_err(internal)?;
    }
    if let Some((salt, hash)) = recovery_hash {
        tx.execute(
            "UPDATE relay_account_auth SET recovery_salt=?2,recovery_hash=?3 WHERE user_id=?1",
            params![user.id, salt, hash],
        )
        .map_err(internal)?;
    }
    if password_changed || recovery_changed || auth_changed || recovery_key_changed {
        tx.execute(
            "DELETE FROM relay_auth_sessions WHERE user_id=?1 AND token_hash<>?2",
            params![user.id, security::token_hash(&token)],
        )
        .map_err(internal)?;
        tx.execute(
            "DELETE FROM relay_trusted_browsers WHERE user_id=?1",
            [&user.id],
        )
        .map_err(internal)?;
        tx.execute(
            "DELETE FROM relay_factor_challenges WHERE user_id=?1",
            [&user.id],
        )
        .map_err(internal)?;
    }
    if password_changed {
        clear_reset(&tx, &user.id)?;
    }
    mark_session(&tx, &token, FULL)?;
    tx.commit().map_err(internal)?;
    Ok(Json(body.vault))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteVault {
    current_auth_key: Option<String>,
}
async fn delete_vault(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<DeleteVault>,
) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    let mode = session_mode(&conn, &token);
    let oauth = oauth_only(&conn, &user.id);
    if oauth {
        if !factors::recent(&conn, &token) {
            return Err(failure(
                StatusCode::FORBIDDEN,
                "reauthentication_required",
                "Authenticate again before starting over",
            ));
        }
    } else if mode != RESET && !verify_auth(&conn, &user.id, body.current_auth_key.as_deref()) {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Current authKey is required",
        ));
    }
    let tx = conn.unchecked_transaction().map_err(internal)?;
    for table in ["relay_account_vaults", "relay_account_browsers"] {
        tx.execute(&format!("DELETE FROM {table} WHERE user_id=?1"), [&user.id])
            .map_err(internal)?;
    }
    tx.execute("DELETE FROM relay_device_certificates WHERE device_id IN (SELECT id FROM relay_devices WHERE owner_user_id=?1)", [&user.id]).map_err(internal)?;
    tx.execute(
        "DELETE FROM relay_auth_sessions WHERE user_id=?1 AND token_hash<>?2",
        params![user.id, security::token_hash(&token)],
    )
    .map_err(internal)?;
    clear_reset(&tx, &user.id)?;
    mark_session(&tx, &token, SETUP)?;
    tx.commit().map_err(internal)?;
    Ok(Json(json!({})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Recover {
    login: String,
    recovery_auth_key: String,
}
async fn recover(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Recover>,
) -> Result<Response, Failure> {
    if body.login.len() > 320 || !valid_key(&body.recovery_auth_key) {
        return Err(failure(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid credentials",
        ));
    }
    if !state.admission.allow(
        format!("recover:{}", body.login.trim().to_ascii_lowercase()),
        10,
        300,
    ) {
        return Err(failure(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many attempts",
        ));
    }
    let conn = state.store.conn.lock().await;
    let row: Option<(String,String,String)> = conn.query_row("SELECT u.id,a.recovery_salt,a.recovery_hash FROM relay_users u JOIN relay_account_auth a ON a.user_id=u.id JOIN relay_account_vaults v ON v.user_id=u.id WHERE (u.username=?1 OR u.email=?1) AND u.enabled=1 AND a.recovery_hash IS NOT NULL", [body.login.trim().to_ascii_lowercase()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(internal)?;
    let (id, salt, hash) = row.ok_or_else(|| {
        failure(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid credentials",
        )
    })?;
    if !crate::verify_password(&body.recovery_auth_key, &salt, &hash) {
        return Err(failure(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid credentials",
        ));
    }
    let user = crate::load_user_by_id(&conn, &id).ok_or_else(|| internal("User missing"))?;
    // Recovery always requires the configured second factor, even on trusted browsers.
    if factors::has_factors(&conn, &id) {
        crate::auth_api::challenge_response_mode(&conn, &user, &headers, RECOVERY)
    } else {
        crate::auth_api::login_response_mode(&conn, &state, &user, &headers, true, false, RECOVERY)
    }
}

fn verify_signed(key: &str, input: &str, signature: &Value) -> bool {
    let Some(key) = public_key(key) else {
        return false;
    };
    let Some(sig) = signature
        .as_str()
        .and_then(|s| B64.decode(s).ok())
        .filter(|s| s.len() == 64)
    else {
        return false;
    };
    let Ok(sig) = BigNum::from_slice(&sig[..32])
        .and_then(|r| {
            BigNum::from_slice(&sig[32..]).and_then(|s| EcdsaSig::from_private_components(r, s))
        })
        .and_then(|s| s.to_der())
    else {
        return false;
    };
    Verifier::new(openssl::hash::MessageDigest::sha256(), &key)
        .and_then(|mut v| {
            v.update(input.as_bytes())?;
            v.verify(&sig)
        })
        .unwrap_or(false)
}
fn signing_input(value: &Value, prefix: &str, fields: &[&str]) -> Option<String> {
    if value["version"] != 1 {
        return None;
    }
    let mut lines = vec![prefix.to_owned()];
    for field in fields {
        let s = if let Some(s) = value[field].as_str() {
            s.to_owned()
        } else {
            value[field].as_u64()?.to_string()
        };
        if s.is_empty() || s.contains(['\r', '\n']) {
            return None;
        }
        lines.push(s);
    }
    Some(lines.join("\n"))
}
fn validate_signed(
    conn: &Connection,
    user: &str,
    value: &Value,
    kind: &str,
) -> Result<(), Failure> {
    let vault = vault(conn, user)?.ok_or_else(|| {
        failure(
            StatusCode::NOT_FOUND,
            "vault_missing",
            "Account key is missing",
        )
    })?;
    if value["accountKeyId"] != vault["accountKeyId"] {
        return Err(invalid("Account key does not match"));
    }
    let time = if kind == "browser" {
        "createdAt"
    } else {
        "revokedAt"
    };
    if value[time].as_u64().is_none_or(|n| n > i64::MAX as u64) {
        return Err(invalid("Invalid signed timestamp"));
    }
    let (prefix, fields): (&str, &[&str]) = match kind {
        "browser" => (
            "pockymoe-acct-browser-v1",
            &["accountKeyId", "browserId", "publicKey", "createdAt"],
        ),
        _ => (
            "pockymoe-acct-revoke-v1",
            &["accountKeyId", "kind", "subjectId", "revokedAt"],
        ),
    };
    if kind == "browser" {
        let key = value["publicKey"].as_str().unwrap_or_default();
        if public_key(key).is_none()
            || value["browserId"].as_str()
                != B64
                    .decode(key)
                    .ok()
                    .map(|k| B64.encode(Sha256::digest(k)))
                    .as_deref()
        {
            return Err(invalid("Invalid browser key"));
        }
    } else if !matches!(value["kind"].as_str(), Some("browser" | "device")) {
        return Err(invalid("Invalid revocation kind"));
    }
    let input =
        signing_input(value, prefix, fields).ok_or_else(|| invalid("Invalid signed document"))?;
    if !verify_signed(
        vault["accountPublicKey"].as_str().unwrap(),
        &input,
        &value["signature"],
    ) {
        return Err(invalid("Invalid account signature"));
    }
    Ok(())
}
pub(crate) fn revocations(conn: &Connection, user: &str) -> Result<Vec<Value>, Failure> {
    let mut stmt = conn.prepare("SELECT revocation_json FROM relay_account_revocations WHERE user_id=?1 ORDER BY account_key_id,kind,subject_id").map_err(internal)?;
    let rows = stmt
        .query_map([user], |r| r.get::<_, String>(0))
        .map_err(internal)?;
    rows.map(|r| serde_json::from_str(&r.map_err(internal)?).map_err(internal))
        .collect()
}
async fn browsers(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, _) = auth(&conn, &state, &headers)?;
    let mut stmt = conn.prepare("SELECT certificate_json,label,created_at,last_seen_at FROM relay_account_browsers WHERE user_id=?1 ORDER BY created_at,browser_id").map_err(internal)?;
    let rows = stmt
        .query_map([&user.id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .map_err(internal)?;
    let browsers = rows.map(|r| { let (cert,label,created,seen) = r.map_err(internal)?; Ok(json!({"certificate":serde_json::from_str::<Value>(&cert).map_err(internal)?,"label":label,"createdAt":created,"lastSeenAt":seen})) }).collect::<Result<Vec<_>,Failure>>()?;
    Ok(Json(
        json!({"browsers":browsers,"revocations":revocations(&conn,&user.id)?}),
    ))
}
#[derive(Deserialize)]
struct Browser {
    certificate: Value,
    label: String,
}
async fn register_browser(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Browser>,
) -> ApiResult {
    if body.label.chars().count() > 80 {
        return Err(invalid("Browser label must be at most 80 characters"));
    }
    let conn = state.store.conn.lock().await;
    let (user, _) = auth(&conn, &state, &headers)?;
    validate_signed(&conn, &user.id, &body.certificate, "browser")?;
    let cert = &body.certificate;
    let revoked: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM relay_account_revocations WHERE user_id=?1 AND account_key_id=?2 AND kind='browser' AND subject_id=?3)", params![user.id,cert["accountKeyId"].as_str(),cert["browserId"].as_str()],|r|r.get(0)).map_err(internal)?;
    if revoked {
        return Err(failure(
            StatusCode::UNAUTHORIZED,
            "browser_revoked",
            "This browser has been removed",
        ));
    }
    conn.execute("INSERT INTO relay_account_browsers(user_id,browser_id,certificate_json,label,created_at,last_seen_at) VALUES (?1,?2,?3,?4,?5,?6)
        ON CONFLICT(user_id,browser_id) DO UPDATE SET label=excluded.label,last_seen_at=excluded.last_seen_at",
        params![user.id,cert["browserId"].as_str(),serde_json::to_string(cert).map_err(internal)?,body.label,cert["createdAt"].as_i64(),factors::now()]).map_err(internal)?;
    Ok(Json(json!({})))
}
#[derive(Deserialize)]
struct Revoke {
    revocation: Value,
}
async fn revoke(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Revoke>,
) -> ApiResult {
    let (frame, ids) = {
        let conn = state.store.conn.lock().await;
        let (user, _) = auth(&conn, &state, &headers)?;
        validate_signed(&conn, &user.id, &body.revocation, "revocation")?;
        let v = &body.revocation;
        conn.execute("INSERT OR IGNORE INTO relay_account_revocations(user_id,account_key_id,kind,subject_id,revocation_json) VALUES (?1,?2,?3,?4,?5)", params![user.id,v["accountKeyId"].as_str(),v["kind"].as_str(),v["subjectId"].as_str(),serde_json::to_string(v).map_err(internal)?]).map_err(internal)?;
        let frame = json!({"type":"relay.revocations","revocations":revocations(&conn,&user.id)?})
            .to_string();
        let mut stmt = conn
            .prepare("SELECT id FROM relay_devices WHERE owner_user_id=?1")
            .map_err(internal)?;
        let ids = stmt
            .query_map([&user.id], |r| r.get::<_, String>(0))
            .map_err(internal)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(internal)?;
        (frame, ids)
    };
    let senders: Vec<_> = {
        let sockets = state.sockets.read().await;
        ids.iter()
            .filter_map(|id| sockets.get(id).map(|s| s.tx.clone()))
            .collect()
    };
    for tx in senders {
        let _ =
            tokio::time::timeout(std::time::Duration::from_secs(10), tx.send(frame.clone())).await;
    }
    Ok(Json(json!({})))
}
pub(crate) async fn device_revocations(state: &AppState, device: &str) -> Result<String, Failure> {
    let conn = state.store.conn.lock().await;
    let owner: Option<String> = conn
        .query_row(
            "SELECT owner_user_id FROM relay_devices WHERE id=?1",
            [device],
            |r| r.get(0),
        )
        .optional()
        .map_err(internal)?;
    Ok(json!({"type":"relay.revocations","revocations":owner.map(|o|revocations(&conn,&o)).transpose()?.unwrap_or_default()}).to_string())
}
async fn owner_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, _) = auth(&conn, &state, &headers)?;
    if !crate::auth_api::certificates::can_read(&conn, &user.id, &id)? {
        return Err(failure(
            StatusCode::NOT_FOUND,
            "not_found",
            "Device not found",
        ));
    }
    let owner: String = conn
        .query_row(
            "SELECT owner_user_id FROM relay_devices WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(internal)?;
    account_public(&conn, &owner, "not_found")
}
async fn share_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    recipient_account(state, headers, id, "relay_shares").await
}
async fn grant_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    recipient_account(state, headers, id, "relay_access_grants").await
}
async fn recipient_account(
    state: Arc<AppState>,
    headers: HeaderMap,
    id: String,
    table: &str,
) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, _) = auth(&conn, &state, &headers)?;
    let recipient: Option<String> = conn.query_row(&format!("SELECT target_user_id FROM {table} WHERE id=?1 AND owner_user_id=?2 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?3)"),params![id,user.id,crate::now_rfc3339()],|r|r.get(0)).optional().map_err(internal)?;
    account_public(
        &conn,
        &recipient.ok_or_else(|| {
            failure(
                StatusCode::NOT_FOUND,
                "not_found",
                "Shared access not found",
            )
        })?,
        "recipient_account_missing",
    )
}

/// Check every restricted request before dispatch, including compatibility and WS paths.
/// Devices enforce binding themselves; setup sessions cannot manage sharing or browsers.
pub(crate) fn request_allowed(
    _conn: &Connection,
    _user: &crate::UserRow,
    mode: &str,
    method: &axum::http::Method,
    uri: &axum::http::Uri,
) -> bool {
    use axum::http::Method;
    if mode == FULL {
        return true;
    }
    let path = uri.path();
    if path == "/relay/auth/logout" || (*method == Method::GET && path == "/relay/auth/session") {
        return true;
    }
    if path == "/relay/account/vault" {
        return matches!(*method, Method::GET | Method::PUT)
            || (mode == RESET && *method == Method::DELETE);
    }
    if matches!(mode, RECOVERY | RESET) {
        return false;
    }
    path.starts_with("/relay/auth/")
        || path == "/relay/devices"
        || path.starts_with("/relay/devices/")
        || path.starts_with("/relay/api/")
        || path == "/relay/ws"
        || path == "/relay/access"
}

#[cfg(test)]
mod vector_tests {
    use super::*;
    use aes_gcm::{
        aead::{Aead, Payload},
        Aes256Gcm, KeyInit, Nonce,
    };
    fn vectors() -> Value {
        serde_json::from_str(include_str!("../test-fixtures/account-key-v2.json")).unwrap()
    }
    fn bytes(v: &Value) -> Vec<u8> {
        B64.decode(v.as_str().unwrap()).unwrap()
    }
    fn decrypt(key: &[u8], value: &Value, aad: &str) -> Result<Vec<u8>, aes_gcm::Error> {
        Aes256Gcm::new_from_slice(key).unwrap().decrypt(
            Nonce::from_slice(&bytes(&value["iv"])),
            Payload {
                msg: &bytes(&value["ciphertext"]),
                aad: aad.as_bytes(),
            },
        )
    }
    #[test]
    fn derived_keys_vault_aad_enrollment_and_all_certificate_vectors() {
        let v = vectors();
        let d = &v["derivation"];
        for (master, info, expected) in [
            (bytes(&d["master"]), "pockymoe-relay-auth-v1", &d["authKey"]),
            (bytes(&d["master"]), "pockymoe-vault-wrap-v1", &d["wrapKey"]),
            (
                bytes(&d["recoveryCodeBytes"]),
                "pockymoe-vault-recovery-v1",
                &d["recoveryKey"],
            ),
            (
                bytes(&d["recoveryCodeBytes"]),
                "pockymoe-relay-recovery-v1",
                &d["recoveryAuthKey"],
            ),
        ] {
            let mut out = [0u8; 32];
            hkdf::Hkdf::<Sha256>::new(None, &master)
                .expand(info.as_bytes(), &mut out)
                .unwrap();
            assert_eq!(B64.encode(out), expected.as_str().unwrap());
        }
        let vault = &v["vault"]["vault"];
        validate_vault(vault, "user-123").unwrap();
        let password_aad = v["vault"]["slotAadPassword"].as_str().unwrap();
        let vdk = decrypt(
            &bytes(&d["wrapKey"]),
            &vault["slots"]["password"],
            password_aad,
        )
        .unwrap();
        assert_eq!(vdk, bytes(&v["vault"]["vdk"]));
        assert_eq!(
            decrypt(
                &bytes(&d["recoveryKey"]),
                &vault["slots"]["recovery"],
                &password_aad.replace("\npassword", "\nrecovery")
            )
            .unwrap(),
            vdk
        );
        assert_eq!(
            String::from_utf8(
                decrypt(
                    &vdk,
                    &vault["body"],
                    v["vault"]["bodyAad"].as_str().unwrap()
                )
                .unwrap()
            )
            .unwrap(),
            v["vault"]["bodyPlaintext"]
        );
        assert!(decrypt(
            &vdk,
            &vault["body"],
            v["negative"]["vaultBodyWrongRevisionAad"].as_str().unwrap()
        )
        .is_err());
        let key = v["account"]["accountPublicKey"].as_str().unwrap();
        for (name, prefix, fields) in [
            (
                "browserCertificate",
                "pockymoe-acct-browser-v1",
                vec!["accountKeyId", "browserId", "publicKey", "createdAt"],
            ),
            (
                "deviceCertificate",
                "pockymoe-acct-device-v1",
                vec!["accountKeyId", "deviceId", "identityKey", "createdAt"],
            ),
        ] {
            let doc = &v[name]["certificate"];
            let input = signing_input(doc, prefix, &fields).unwrap();
            assert_eq!(input, v[name]["signingInput"]);
            assert!(verify_signed(key, &input, &doc["signature"]));
        }
        for kind in ["browser", "device"] {
            let doc = &v["revocations"][kind];
            let input = signing_input(
                doc,
                "pockymoe-acct-revoke-v1",
                &["accountKeyId", "kind", "subjectId", "revokedAt"],
            )
            .unwrap();
            assert!(verify_signed(key, &input, &doc["signature"]));
        }
        for (name, prefix, fields) in [
            (
                "browserCertSignedByOtherAccount",
                "pockymoe-acct-browser-v1",
                vec!["accountKeyId", "browserId", "publicKey", "createdAt"],
            ),
            (
                "browserCertWrongBrowserId",
                "pockymoe-acct-browser-v1",
                vec!["accountKeyId", "browserId", "publicKey", "createdAt"],
            ),
            (
                "browserCertTamperedCreatedAt",
                "pockymoe-acct-browser-v1",
                vec!["accountKeyId", "browserId", "publicKey", "createdAt"],
            ),
            (
                "deviceCertWrongDevice",
                "pockymoe-acct-device-v1",
                vec!["accountKeyId", "deviceId", "identityKey", "createdAt"],
            ),
            (
                "deviceCertWrongIdentity",
                "pockymoe-acct-device-v1",
                vec!["accountKeyId", "deviceId", "identityKey", "createdAt"],
            ),
            (
                "revocationWrongKind",
                "pockymoe-acct-revoke-v1",
                vec!["accountKeyId", "kind", "subjectId", "revokedAt"],
            ),
        ] {
            let doc = &v["negative"][name];
            let input = signing_input(doc, prefix, &fields).unwrap();
            assert!(!verify_signed(key, &input, &doc["signature"]), "{name}");
        }
        let mut mac =
            <Hmac<Sha256> as Mac>::new_from_slice(&bytes(&v["enrollment"]["enrollSecret"]))
                .unwrap();
        mac.update(v["enrollment"]["signingInput"].as_str().unwrap().as_bytes());
        assert_eq!(
            B64.encode(mac.finalize().into_bytes()),
            v["enrollment"]["mac"]
        );
        assert_ne!(
            v["enrollment"]["mac"],
            v["negative"]["enrollmentMacWrongIdentity"]
        );
        for (name, key) in [
            (
                "requestSignature",
                v["browserCertificate"]["certificate"]["publicKey"]
                    .as_str()
                    .unwrap(),
            ),
            (
                "peerSignature",
                v["deviceCertificate"]["certificate"]["identityKey"]
                    .as_str()
                    .unwrap(),
            ),
        ] {
            let doc = &v[name];
            let prefix = if name == "requestSignature" {
                "pockymoe-request-v1\n".to_owned()
            } else {
                format!(
                    "pockymoe-peer-request-v1\n{}\n",
                    doc["callerDeviceId"].as_str().unwrap()
                )
            };
            let input = format!(
                "{prefix}{}\n{}",
                doc["aad"].as_str().unwrap(),
                B64.encode(Sha256::digest(bytes(&doc["sealed"])))
            );
            assert_eq!(input, doc["signingInput"]);
            let key = if name == "peerSignature" {
                let group = openssl::ec::EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
                let mut ctx = openssl::bn::BigNumContext::new().unwrap();
                let point =
                    openssl::ec::EcPoint::from_bytes(&group, &B64.decode(key).unwrap(), &mut ctx)
                        .unwrap();
                let ec = openssl::ec::EcKey::from_public_key(&group, &point).unwrap();
                B64.encode(PKey::from_ec_key(ec).unwrap().public_key_to_der().unwrap())
            } else {
                key.into()
            };
            assert!(verify_signed(&key, &input, &doc["signature"]));
        }
    }
}

#[cfg(test)]
mod kdf_secret_tests {
    use super::*;
    #[tokio::test]
    async fn fake_kdf_salts_are_bound_to_the_relays_persisted_secret() {
        let mut salts = Vec::new();
        for secret in ["relay-one-secret", "relay-two-secret"] {
            let (mut state, dir) = crate::tests::test_app_state("kdf-secret");
            Arc::get_mut(&mut state).unwrap().store.session_secret = secret.into();
            let response = kdf(
                State(state.clone()),
                Query(KdfQuery {
                    login: "unknown-login".into(),
                }),
            )
            .await
            .unwrap();
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let value: Value = serde_json::from_slice(&body).unwrap();
            salts.push(value["kdf"]["salt"].clone());
            drop(state);
            let _ = std::fs::remove_dir_all(dir);
        }
        assert_ne!(salts[0], salts[1]);
    }
}
