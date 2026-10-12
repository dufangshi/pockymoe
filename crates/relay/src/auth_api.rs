pub(crate) mod certificates;
mod passkeys;
use super::{auth_factors as factors, security, AppState, TokenQuery, UserRow};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) type Failure = (StatusCode, Json<Value>);
pub(crate) type ApiResult = Result<Json<Value>, Failure>;
pub(crate) fn failure(status: StatusCode, code: &str, message: &str) -> Failure {
    (status, Json(json!({"code":code,"message":message})))
}
pub(crate) fn invalid(message: &str) -> Failure {
    failure(StatusCode::BAD_REQUEST, "bad_request", message)
}
pub(crate) fn internal(_: impl std::fmt::Display) -> Failure {
    failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "Security operation failed",
    )
}
pub(crate) fn auth(
    conn: &Connection,
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(UserRow, String), Failure> {
    let token = super::extract_session_token(headers, &TokenQuery::default()).ok_or_else(|| {
        failure(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Sign in to continue",
        )
    })?;
    let user = super::load_user_by_session(conn, &state.store.session_secret, &token).ok_or_else(
        || {
            failure(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Sign in to continue",
            )
        },
    )?;
    Ok((user, token))
}
fn recent(conn: &Connection, token: &str) -> Result<(), Failure> {
    if factors::recent(conn, token) {
        Ok(())
    } else {
        Err(failure(
            StatusCode::FORBIDDEN,
            "reauthentication_required",
            "Verify your identity to change security settings",
        ))
    }
}
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/relay/devices/{id}/setup-token", post(device_setup_token))
        .route("/relay/devices/{id}/bootstrap", post(device_bootstrap))
        .route("/relay/setup/redeem", post(redeem_bootstrap))
        .route("/setup.sh", get(setup_script))
        .route("/setup.ps1", get(setup_powershell_script))
        .merge(passkeys::routes())
        .merge(certificates::routes())
        .merge(crate::account::routes())
        .route("/relay/devices/{id}/token", post(rotate_device_token))
        .route("/relay/account/security", get(summary))
        .route("/relay/account/security/reauth", post(reauth))
        .route("/relay/account/security/authenticator/enroll", post(enroll))
        .route(
            "/relay/account/security/authenticator/confirm",
            post(confirm_enrollment),
        )
        .route(
            "/relay/account/security/authenticator",
            delete(disable_totp),
        )
        .route(
            "/relay/account/security/recovery-codes",
            post(regenerate_codes),
        )
        .route(
            "/relay/account/security/sessions/{id}",
            delete(revoke_session),
        )
        .route(
            "/relay/account/security/browsers/{id}",
            delete(revoke_browser),
        )
        .route(
            "/relay/auth/challenge",
            get(challenge_status)
                .post(complete_challenge)
                .delete(cancel_challenge),
        )
}

pub(crate) fn login_response(
    conn: &Connection,
    state: &AppState,
    user: &UserRow,
    headers: &HeaderMap,
    strong: bool,
    remember: bool,
) -> Result<Response, Failure> {
    login_response_mode(
        conn,
        state,
        user,
        headers,
        strong,
        remember,
        crate::account::login_mode(conn, user),
    )
}

pub(crate) fn login_response_mode(
    conn: &Connection,
    state: &AppState,
    user: &UserRow,
    headers: &HeaderMap,
    strong: bool,
    remember: bool,
    mode: &str,
) -> Result<Response, Failure> {
    let tx = conn.unchecked_transaction().map_err(internal)?;
    let conn = &tx;
    security::audit(
        conn,
        Some(&user.id),
        if strong {
            "login.mfa"
        } else {
            "login.password_or_oauth"
        },
        None,
    );
    if mode == crate::account::RESET {
        crate::account::consume_reset(conn, &user.id)?;
    }
    let token =
        super::create_session(conn, &state.store.session_secret, &user.id).map_err(internal)?;
    crate::account::mark_session(conn, &token, mode)?;
    // OAuth vault mutations require a fresh provider login or actual second
    // factor; a recovery key / temporary password alone is not that proof.
    let oauth_recovery = crate::account::oauth_only(conn, &user.id)
        && matches!(mode, crate::account::RECOVERY | crate::account::RESET);
    if strong && (!oauth_recovery || factors::has_factors(conn, &user.id)) {
        factors::mark_strong(conn, &token).map_err(internal)?;
    }
    let trust = if remember {
        Some(factors::trust(conn, &user.id, headers).map_err(internal)?)
    } else {
        None
    };
    let browser_token = trust
        .clone()
        .or_else(|| security::cookie(headers, "remote_codex_trusted_browser"));
    let browser_id: Option<String> = browser_token.and_then(|t| conn.query_row("SELECT id FROM relay_trusted_browsers WHERE token_hash=?1 AND user_id=?2 AND expires_at>?3",params![security::token_hash(&t),user.id,factors::now()],|r|r.get(0)).optional().ok().flatten());
    conn.execute(
        "UPDATE relay_auth_sessions SET environment=?2,trusted_browser_id=?3 WHERE token_hash=?1",
        params![
            security::token_hash(&token),
            factors::browser_name(headers),
            browser_id
        ],
    )
    .map_err(internal)?;
    let session = crate::account::session_json(conn, user, &state.oauth, &token);
    let mut response = super::with_session_cookie(
        Json(json!({"token":token,"account":session["account"],"session":session})).into_response(),
        &token,
    );
    if user.role == "admin" {
        response.headers_mut().insert(header::SET_COOKIE,format!("remote_codex_relay_admin_session={token}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=1209600").parse().unwrap());
    }
    if let Some(trust) = trust {
        response.headers_mut().append(header::SET_COOKIE,format!("remote_codex_trusted_browser={trust}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=2592000").parse().unwrap());
    }
    response.headers_mut().append(header::SET_COOKIE,"remote_codex_factor_challenge=; HttpOnly; Secure; SameSite=Lax; Path=/relay/auth; Max-Age=0".parse().unwrap());
    tx.commit().map_err(internal)?;
    Ok(response)
}

pub(crate) fn challenge_response(
    conn: &Connection,
    user: &UserRow,
    headers: &HeaderMap,
) -> Result<Response, Failure> {
    challenge_response_mode(conn, user, headers, crate::account::login_mode(conn, user))
}
pub(crate) fn challenge_response_mode(
    conn: &Connection,
    user: &UserRow,
    headers: &HeaderMap,
    mode: &str,
) -> Result<Response, Failure> {
    security::audit(conn, Some(&user.id), "login.challenge", None);
    let id = factors::challenge(
        conn,
        &user.id,
        "login",
        headers,
        None,
        json!({"accountMode":mode}),
    )
    .map_err(internal)?;
    let mut response = Json(json!({"challengeRequired":true,"authenticator":factors::has_totp(conn,&user.id),"passkey":has_passkey(conn,&user.id),"session":{"authenticated":false,"user":null}})).into_response();
    response.headers_mut().insert(header::SET_COOKIE,format!("remote_codex_factor_challenge={id}; HttpOnly; Secure; SameSite=Lax; Path=/relay/auth; Max-Age=600").parse().unwrap());
    Ok(response)
}
fn has_passkey(conn: &Connection, user: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM relay_passkeys WHERE user_id=?1)",
        params![user],
        |r| r.get(0),
    )
    .unwrap_or(false)
}
fn pending(
    conn: &Connection,
    headers: &HeaderMap,
    attempt: bool,
) -> Result<(String, String), Failure> {
    let id = security::cookie(headers, "remote_codex_factor_challenge")
        .ok_or_else(|| invalid("Login verification expired. Sign in again."))?;
    let hash = security::token_hash(&id);
    let user: Option<String> = conn.query_row("SELECT c.user_id FROM relay_factor_challenges c JOIN relay_users u ON u.id=c.user_id WHERE c.id_hash=?1 AND c.purpose='login' AND c.environment=?2 AND c.expires_at>?3 AND c.attempts<5 AND u.enabled=1",params![hash,factors::environment(headers),factors::now()],|r|r.get(0)).optional().map_err(internal)?;
    let user = user.ok_or_else(|| invalid("Login verification expired. Sign in again."))?;
    if attempt {
        conn.execute(
            "UPDATE relay_factor_challenges SET attempts=attempts+1 WHERE id_hash=?1",
            params![hash],
        )
        .map_err(internal)?;
    }
    Ok((hash, user))
}
fn challenge_mode(conn: &Connection, challenge: &str) -> Result<String, Failure> {
    let data: String = conn
        .query_row(
            "SELECT data FROM relay_factor_challenges WHERE id_hash=?1",
            [challenge],
            |r| r.get(0),
        )
        .map_err(internal)?;
    let data: Value = serde_json::from_str(&data).map_err(internal)?;
    Ok(data["accountMode"]
        .as_str()
        .unwrap_or(crate::account::FULL)
        .to_owned())
}
async fn challenge_status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (_, user) = pending(&conn, &headers, false)?;
    Ok(Json(
        json!({"challengeRequired":true,"authenticator":factors::has_totp(&conn,&user),"passkey":has_passkey(&conn,&user)}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeInput {
    code: String,
    #[serde(default)]
    remember_browser: bool,
}
async fn complete_challenge(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CodeInput>,
) -> Result<Response, Failure> {
    let conn = state.store.conn.lock().await;
    let (challenge, user_id) = pending(&conn, &headers, true)?;
    if !state.admission.allow(format!("factor:{user_id}"), 10, 300) {
        return Err(failure(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many attempts. Try again later.",
        ));
    }
    let mode = challenge_mode(&conn, &challenge)?;
    let recovery = factors::verify_code(
        &conn,
        &state.store.session_secret,
        &user_id,
        &body.code,
        false,
    )
    .map_err(|_| invalid("Code is invalid, expired, or already used"))?;
    conn.execute(
        "DELETE FROM relay_factor_challenges WHERE id_hash=?1",
        params![challenge],
    )
    .map_err(internal)?;
    if recovery {
        conn.execute(
            "DELETE FROM relay_trusted_browsers WHERE user_id=?1",
            params![user_id],
        )
        .map_err(internal)?;
        conn.execute(
            "DELETE FROM relay_auth_sessions WHERE user_id=?1",
            params![user_id],
        )
        .map_err(internal)?;
    }
    let user =
        super::load_user_by_id(&conn, &user_id).ok_or_else(|| invalid("Account unavailable"))?;
    login_response_mode(
        &conn,
        &state,
        &user,
        &headers,
        true,
        body.remember_browser && !recovery,
        &mode,
    )
}

async fn summary(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    let session_hash = security::token_hash(&token);
    let mut stmt = conn.prepare("SELECT token_hash,environment,created_at,expires_at FROM relay_auth_sessions WHERE user_id=?1 AND expires_at>?2 ORDER BY created_at DESC").map_err(internal)?;
    let sessions = stmt.query_map(params![user.id,factors::now()],|r| { let id:String=r.get(0)?; Ok(json!({"current":id==session_hash,"id":id,"name":r.get::<_,String>(1)?,"createdAt":r.get::<_,i64>(2)?,"expiresAt":r.get::<_,i64>(3)?})) }).map_err(internal)?.collect::<Result<Vec<_>,_>>().map_err(internal)?;
    let mut stmt = conn.prepare("SELECT id,name,created_at,expires_at,last_used_at FROM relay_trusted_browsers WHERE user_id=?1 AND expires_at>?2 ORDER BY last_used_at DESC").map_err(internal)?;
    let browsers = stmt.query_map(params![user.id,factors::now()],|r| Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"createdAt":r.get::<_,i64>(2)?,"expiresAt":r.get::<_,i64>(3)?,"lastUsedAt":r.get::<_,i64>(4)?}))).map_err(internal)?.collect::<Result<Vec<_>,_>>().map_err(internal)?;
    let mut stmt = conn.prepare("SELECT id,name,created_at,last_used_at FROM relay_passkeys WHERE user_id=?1 ORDER BY created_at DESC").map_err(internal)?;
    let passkeys = stmt.query_map(params![user.id],|r| Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"createdAt":r.get::<_,i64>(2)?,"lastUsedAt":r.get::<_,Option<i64>>(3)?}))).map_err(internal)?.collect::<Result<Vec<_>,_>>().map_err(internal)?;
    let codes: i64 = conn
        .query_row(
            "SELECT count(*) FROM relay_recovery_codes WHERE user_id=?1",
            params![user.id],
            |r| r.get(0),
        )
        .map_err(internal)?;
    Ok(Json(
        json!({"authenticatorEnabled":factors::has_totp(&conn,&user.id),"recoveryCodesRemaining":codes,"passkeys":passkeys,"sessions":sessions,"trustedBrowsers":browsers,"passkeyAvailable":state.oauth.public_base_url.is_some(),"recentlyVerified":factors::recent(&conn,&token)}),
    ))
}
#[derive(Deserialize)]
struct ReauthInput {
    password: Option<String>,
    #[serde(rename = "authKey")]
    auth_key: Option<String>,
    code: Option<String>,
}
async fn reauth(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ReauthInput>,
) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    if !state.admission.allow(format!("reauth:{}", user.id), 5, 300) {
        return Err(failure(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many attempts. Try again later.",
        ));
    }
    if factors::has_factors(&conn, &user.id) {
        let recovery = factors::verify_code(
            &conn,
            &state.store.session_secret,
            &user.id,
            body.code.as_deref().unwrap_or_default(),
            false,
        )
        .map_err(|_| invalid("Code is invalid, expired, or already used"))?;
        if recovery {
            conn.execute(
                "DELETE FROM relay_trusted_browsers WHERE user_id=?1",
                params![user.id],
            )
            .map_err(internal)?;
            conn.execute(
                "DELETE FROM relay_auth_sessions WHERE user_id=?1 AND token_hash<>?2",
                params![user.id, security::token_hash(&token)],
            )
            .map_err(internal)?;
        }
    } else {
        if crate::account::oauth_only(&conn, &user.id) {
            return Err(failure(
                StatusCode::FORBIDDEN,
                "reauthentication_required",
                "Sign in with OAuth again or use a second factor",
            ));
        }
        let has_vault = user.role != "admin" && crate::account::has_vault(&conn, &user.id);
        let (salt, hash): (String, String) = conn
            .query_row(
                if has_vault {
                    "SELECT auth_salt,auth_hash FROM relay_account_auth WHERE user_id=?1"
                } else {
                    "SELECT password_salt,password_hash FROM relay_users WHERE id=?1"
                },
                params![user.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(internal)?;
        let password = if has_vault {
            body.auth_key.as_deref()
        } else {
            body.password.as_deref()
        }
        .unwrap_or_default();
        if password.len() > 1024
            || (has_vault && !crate::account::valid_key(password))
            || !super::verify_password(password, &salt, &hash)
        {
            return Err(invalid("Password is incorrect"));
        }
    }
    factors::mark_strong(&conn, &token).map_err(internal)?;
    if crate::account::has_vault(&conn, &user.id) {
        crate::account::mark_session(&conn, &token, crate::account::FULL)?;
    }
    let proof = factors::password_grant(&conn, &user.id, &token, &headers).map_err(internal)?;
    security::audit(&conn, Some(&user.id), "security.reauthenticated", None);
    Ok(Json(json!({"ok":true,"verificationToken":proof})))
}
async fn enroll(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    recent(&conn, &token)?;
    Ok(Json(
        factors::enroll(&conn, &state.store.session_secret, &user.id, &user.email)
            .map_err(|_| invalid("Authenticator enrollment could not start"))?,
    ))
}
async fn confirm_enrollment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CodeInput>,
) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    recent(&conn, &token)?;
    if !state.admission.allow(format!("enroll:{}", user.id), 5, 300) {
        return Err(failure(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many attempts. Try again later.",
        ));
    }
    factors::verify_code(
        &conn,
        &state.store.session_secret,
        &user.id,
        &body.code,
        true,
    )
    .map_err(|_| invalid("Code is invalid, expired, or already used"))?;
    let codes = factors::recovery_codes(&conn, &user.id).map_err(internal)?;
    security::audit(&conn, Some(&user.id), "authenticator.enabled", None);
    conn.execute(
        "DELETE FROM relay_auth_sessions WHERE user_id=?1 AND token_hash<>?2",
        params![user.id, security::token_hash(&token)],
    )
    .map_err(internal)?;
    factors::mark_strong(&conn, &token).map_err(internal)?;
    Ok(Json(json!({"recoveryCodes":codes})))
}
async fn disable_totp(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    recent(&conn, &token)?;
    conn.execute("DELETE FROM relay_totp WHERE user_id=?1", params![user.id])
        .map_err(internal)?;
    security::audit(&conn, Some(&user.id), "authenticator.disabled", None);
    conn.execute(
        "DELETE FROM relay_trusted_browsers WHERE user_id=?1",
        params![user.id],
    )
    .map_err(internal)?;
    conn.execute(
        "DELETE FROM relay_factor_challenges WHERE user_id=?1",
        params![user.id],
    )
    .map_err(internal)?;
    conn.execute(
        "DELETE FROM relay_auth_sessions WHERE user_id=?1 AND token_hash<>?2",
        params![user.id, security::token_hash(&token)],
    )
    .map_err(internal)?;
    if !has_passkey(&conn, &user.id) {
        conn.execute(
            "DELETE FROM relay_recovery_codes WHERE user_id=?1",
            params![user.id],
        )
        .map_err(internal)?;
    }
    Ok(Json(json!({"ok":true})))
}
async fn regenerate_codes(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    recent(&conn, &token)?;
    if !factors::has_factors(&conn, &user.id) {
        return Err(invalid("Enable an authenticator or passkey first"));
    }
    Ok(Json(
        json!({"recoveryCodes":factors::recovery_codes(&conn,&user.id).map_err(internal)?}),
    ))
}
async fn revoke_session(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    recent(&conn, &token)?;
    conn.execute(
        "DELETE FROM relay_auth_sessions WHERE user_id=?1 AND token_hash=?2",
        params![user.id, id],
    )
    .map_err(internal)?;
    security::audit(&conn, Some(&user.id), "session.revoked", None);
    Ok(Json(json!({"ok":true})))
}
async fn revoke_browser(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    recent(&conn, &token)?;
    conn.execute(
        "DELETE FROM relay_auth_sessions WHERE user_id=?1 AND trusted_browser_id=?2",
        params![user.id, id],
    )
    .map_err(internal)?;
    conn.execute(
        "DELETE FROM relay_trusted_browsers WHERE user_id=?1 AND id=?2",
        params![user.id, id],
    )
    .map_err(internal)?;
    security::audit(&conn, Some(&user.id), "browser.revoked", Some(&id));
    Ok(Json(json!({"ok":true})))
}

async fn cancel_challenge(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(id) = security::cookie(&headers, "remote_codex_factor_challenge") {
        let conn = state.store.conn.lock().await;
        let _ = conn.execute(
            "DELETE FROM relay_factor_challenges WHERE id_hash=?1",
            params![security::token_hash(&id)],
        );
    }
    ([ (header::SET_COOKIE,"remote_codex_factor_challenge=; HttpOnly; Secure; SameSite=Lax; Path=/relay/auth; Max-Age=0") ],Json(json!({"ok":true}))).into_response()
}
async fn setup_script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        include_str!("../../../scripts/setup.sh")
            .replace("__POCKYMOE_VERSION__", env!("CARGO_PKG_VERSION")),
    )
}
async fn setup_powershell_script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        include_str!("../../../scripts/setup.ps1"),
    )
}
async fn device_bootstrap(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, Failure> {
    let conn = state.store.conn.lock().await;
    let (user, session) = auth(&conn, &state, &headers)?;
    recent(&conn, &session)?;
    let owned: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM relay_devices WHERE id=?1 AND owner_user_id=?2)",
            params![id, user.id],
            |r| r.get(0),
        )
        .map_err(internal)?;
    if !owned {
        return Err(failure(
            StatusCode::NOT_FOUND,
            "not_found",
            "Device not found",
        ));
    }
    let code = security::random_token();
    let expires = factors::now() + 60 * 60 * 1000;
    conn.execute(
        "DELETE FROM relay_bootstrap_codes WHERE expires_at<=?1",
        [factors::now()],
    )
    .map_err(internal)?;
    conn.execute(
        "INSERT INTO relay_bootstrap_codes(code_hash,device_id,expires_at) VALUES (?1,?2,?3)",
        params![security::token_hash(&code), id, expires],
    )
    .map_err(internal)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"code":code,"expiresAt":expires})),
    )
        .into_response())
}
#[derive(Deserialize)]
struct BootstrapRedemption {
    code: String,
}
#[cfg(test)]
mod bootstrap_tests {
    use super::*;
    #[tokio::test]
    async fn enrollment_is_single_use_and_rejects_expired_or_disabled_devices() {
        let (state, _) = crate::tests::test_app_state("bootstrap");
        {
            let conn = state.store.conn.lock().await;
            conn.execute("INSERT INTO relay_users VALUES ('owner','o@example.test','owner','user',1,NULL,'now','salt','hash')", []).unwrap();
            conn.execute("INSERT INTO relay_devices VALUES ('device','owner','test',NULL,'hash','preview','now')", []).unwrap();
            for (code, expires) in [
                ("valid", factors::now() + 3_600_000),
                ("expired", factors::now() - 1),
                ("disabled", factors::now() + 3_600_000),
            ] {
                conn.execute(
                    "INSERT INTO relay_bootstrap_codes VALUES (?1,'device',?2)",
                    params![security::token_hash(code), expires],
                )
                .unwrap();
            }
        }
        let redeem = |code: &str| {
            redeem_bootstrap(
                State(state.clone()),
                Json(BootstrapRedemption { code: code.into() }),
            )
        };
        assert_eq!(
            redeem("expired").await.unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );
        let response = redeem("valid").await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            redeem("valid").await.unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );
        state
            .store
            .conn
            .lock()
            .await
            .execute("UPDATE relay_users SET enabled=0", [])
            .unwrap();
        assert_eq!(
            redeem("disabled").await.unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );
    }
}
async fn redeem_bootstrap(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BootstrapRedemption>,
) -> Result<Response, Failure> {
    let conn = state.store.conn.lock().await;
    let tx = conn.unchecked_transaction().map_err(internal)?;
    let id:Option<String>=tx.query_row("SELECT b.device_id FROM relay_bootstrap_codes b JOIN relay_devices d ON d.id=b.device_id JOIN relay_users u ON u.id=d.owner_user_id WHERE b.code_hash=?1 AND b.expires_at>?2 AND u.enabled=1",params![security::token_hash(&body.code),factors::now()],|r|r.get(0)).optional().map_err(internal)?;
    let id = id.ok_or_else(|| {
        failure(
            StatusCode::UNAUTHORIZED,
            "invalid_setup_code",
            "Setup code expired or already used. Copy a new setup command.",
        )
    })?;
    let token = super::device_tokens::get_or_create(&tx, &state.store.session_secret, &id)
        .map_err(internal)?;
    tx.execute(
        "DELETE FROM relay_bootstrap_codes WHERE code_hash=?1",
        [security::token_hash(&body.code)],
    )
    .map_err(internal)?;
    tx.commit().map_err(internal)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"deviceId":id,"token":token})),
    )
        .into_response())
}
async fn device_setup_token(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, Failure> {
    let conn = state.store.conn.lock().await;
    let (user, session) = auth(&conn, &state, &headers)?;
    recent(&conn, &session)?;
    let owned: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM relay_devices WHERE id=?1 AND owner_user_id=?2)",
            params![id, user.id],
            |r| r.get(0),
        )
        .map_err(internal)?;
    if !owned {
        return Err(failure(
            StatusCode::NOT_FOUND,
            "not_found",
            "Device not found",
        ));
    }
    let token = super::device_tokens::get_or_create(&conn, &state.store.session_secret, &id)
        .map_err(internal)?;
    security::audit(&conn, Some(&user.id), "device.setup_copied", Some(&id));
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"token":token})),
    )
        .into_response())
}
async fn rotate_device_token(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, session) = auth(&conn, &state, &headers)?;
    recent(&conn, &session)?;
    let device: Option<(String, String)> = conn
        .query_row(
            "SELECT name,created_at FROM relay_devices WHERE id=?1 AND owner_user_id=?2",
            params![id, user.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(internal)?;
    let Some((name, created)) = device else {
        return Err(failure(
            StatusCode::NOT_FOUND,
            "not_found",
            "Device not found",
        ));
    };
    let token = format!("rcd_{}", security::random_token());
    let preview = super::preview_token(&token);
    {
        let tx = conn.unchecked_transaction().map_err(internal)?;
        tx.execute(
            "UPDATE relay_devices SET token=NULL,token_hash=?2,token_preview=?3 WHERE id=?1",
            params![id, super::hash_device_token(&token), preview],
        )
        .map_err(internal)?;
        super::device_tokens::save(&tx, &state.store.session_secret, &id, &token)
            .map_err(internal)?;
        tx.execute(
            "DELETE FROM relay_bootstrap_codes WHERE device_id=?1",
            [&id],
        )
        .map_err(internal)?;
        tx.commit().map_err(internal)?;
    }
    security::audit(&conn, Some(&user.id), "device.token_rotated", Some(&id));
    drop(conn);
    state.sockets.write().await.remove(&id);
    state
        .clients
        .write()
        .await
        .retain(|_, client| client.device_id != id);
    Ok(Json(
        json!({"token":token,"device":super::device_json(super::DeviceJsonInput {id:&id,owner_user_id:&user.id,name:&name,created_at:&created,connected:false,connected_at:None,last_heartbeat_at:None,token:None,token_preview:Some(&preview)})}),
    ))
}
