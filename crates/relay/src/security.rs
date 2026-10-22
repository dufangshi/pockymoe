//! Authentication state is server-owned. Signed tokens without a live session
//! record (including legacy sessions) intentionally require a new login.
use anyhow::{bail, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::{fs::OpenOptions, io::Write, path::Path};

pub(crate) fn random_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn token_hash(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

pub(crate) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS relay_security_events (id INTEGER PRIMARY KEY, user_id TEXT, event TEXT NOT NULL, resource_id TEXT, created_at INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS relay_auth_sessions (
        token_hash TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        strong_auth_at INTEGER,
        environment TEXT NOT NULL DEFAULT ''
    );
    CREATE TABLE IF NOT EXISTS relay_oauth_challenges (
        state_hash TEXT PRIMARY KEY, browser_hash TEXT NOT NULL, verifier TEXT NOT NULL,
        callback TEXT NOT NULL, expires_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS relay_auth_sessions_user ON relay_auth_sessions(user_id);
    CREATE INDEX IF NOT EXISTS relay_security_events_time ON relay_security_events(created_at);
    CREATE INDEX IF NOT EXISTS relay_security_events_user ON relay_security_events(user_id,created_at);
    DROP TRIGGER IF EXISTS relay_password_revoke_sessions;
    CREATE TRIGGER relay_password_revoke_sessions AFTER UPDATE OF password_hash ON relay_users
    WHEN NEW.password_hash<>''
    BEGIN DELETE FROM relay_auth_sessions WHERE user_id=NEW.id; END;
    CREATE TRIGGER IF NOT EXISTS relay_disabled_revoke_sessions AFTER UPDATE OF enabled ON relay_users
    WHEN NEW.enabled=0 BEGIN DELETE FROM relay_auth_sessions WHERE user_id=NEW.id; END;")?;
    if conn
        .prepare("SELECT trusted_browser_id FROM relay_auth_sessions LIMIT 0")
        .is_err()
    {
        conn.execute(
            "ALTER TABLE relay_auth_sessions ADD COLUMN trusted_browser_id TEXT",
            [],
        )?;
    }
    Ok(())
}

pub(crate) fn revoke_session(conn: &Connection, token: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM relay_auth_sessions WHERE token_hash=?1",
        params![token_hash(token)],
    )?;
    Ok(())
}

pub(crate) fn session_secret(data_dir: &Path) -> Result<String> {
    if let Ok(secret) = std::env::var("POCKYMOE_RELAY_SESSION_SECRET") {
        if secret.len() < 32 {
            bail!("POCKYMOE_RELAY_SESSION_SECRET must contain at least 32 characters");
        }
        return Ok(secret);
    }
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join("session-secret");
    if path.exists() {
        let secret = std::fs::read_to_string(path)?.trim().to_owned();
        if secret.len() < 32 {
            bail!("Persisted relay session-secret is invalid; restore its backup");
        }
        return Ok(secret);
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let secret = random_token();
    file.write_all(secret.as_bytes())?;
    file.sync_all()?;
    Ok(secret)
}

pub(crate) fn origin_allowed(headers: &axum::http::HeaderMap, public_base: Option<&str>) -> bool {
    let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let raw_origin = origin;
    let Ok(origin) = url::Url::parse(origin) else {
        return false;
    };
    if !matches!(origin.scheme(), "http" | "https")
        || origin.host_str().is_none()
        || raw_origin != origin.origin().ascii_serialization()
    {
        return false;
    }
    if let Some(base) = public_base {
        return url::Url::parse(base).is_ok_and(|base| base.origin() == origin.origin());
    }
    let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    // Host is the requested server authority. Never use arbitrary forwarded-host.
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    url::Url::parse(&format!("{scheme}://{host}"))
        .is_ok_and(|base| base.origin() == origin.origin())
}

// CORS wraps all API responses, including authentication and CSRF errors.
pub(crate) async fn browser_security(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<super::AppState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::{
        http::{header, Method, StatusCode},
        response::IntoResponse,
    };
    let api = request.uri().path().starts_with("/relay/");
    let allowed = api && origin_allowed(request.headers(), state.oauth.browser_origin().as_deref());
    let origin = allowed.then(|| request.headers()[header::ORIGIN].clone());
    let preflight = api
        && request.method() == Method::OPTIONS
        && request
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD);
    let requested = request
        .headers()
        .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        .cloned();
    let mut response = if preflight {
        let method_allowed = request
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_METHOD)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|m| {
                matches!(
                    m,
                    "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS"
                )
            });
        let headers_allowed = requested
            .as_ref()
            .map(|h| {
                h.to_str().ok().is_some_and(|h| {
                    h.split(',').all(|name| {
                        let name = name.trim().to_ascii_lowercase();
                        matches!(
                            name.as_str(),
                            "content-type"
                                | "accept"
                                | "range"
                                | "authorization"
                                | "if-none-match"
                                | "x-remote-codex-auth-realm"
                        ) || (name.starts_with("x-rcd-")
                            && name.parse::<header::HeaderName>().is_ok())
                    })
                })
            })
            .unwrap_or(true);
        if allowed && method_allowed && headers_allowed {
            let mut response = StatusCode::NO_CONTENT.into_response();
            let h = response.headers_mut();
            h.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                "GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS"
                    .parse()
                    .unwrap(),
            );
            if let Some(requested) = requested {
                h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, requested);
            }
            response
        } else {
            StatusCode::FORBIDDEN.into_response()
        }
    } else {
        browser_security_response(axum::extract::State(state), request, next).await
    };
    if api {
        response
            .headers_mut()
            .append(header::VARY, "Origin".parse().unwrap());
        if preflight {
            response.headers_mut().append(
                header::VARY,
                "Access-Control-Request-Method, Access-Control-Request-Headers"
                    .parse()
                    .unwrap(),
            );
        }
    }
    // Overwrite downstream device CORS: a device cannot widen the relay allowlist.
    response
        .headers_mut()
        .remove(header::ACCESS_CONTROL_ALLOW_ORIGIN);
    response
        .headers_mut()
        .remove(header::ACCESS_CONTROL_ALLOW_CREDENTIALS);
    response
        .headers_mut()
        .remove(header::ACCESS_CONTROL_EXPOSE_HEADERS);
    if let Some(origin) = origin {
        let mut expose = vec![
            "content-disposition".to_owned(),
            "content-range".to_owned(),
            "accept-ranges".to_owned(),
            "etag".to_owned(),
            "x-total-count".to_owned(),
            "link".to_owned(),
        ];
        expose.extend(
            response
                .headers()
                .keys()
                .filter(|h| h.as_str().starts_with("x-rcd-"))
                .map(|h| h.as_str().to_owned()),
        );
        let h = response.headers_mut();
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        h.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            "true".parse().unwrap(),
        );
        h.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            expose.join(", ").parse().unwrap(),
        );
    }
    response
}

async fn browser_security_response(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<super::AppState>>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::{
        http::{header, Method, StatusCode},
        response::IntoResponse,
    };
    let path = request.uri().path();
    // This document route either consumes an existing owner lease + cookie or
    // falls back to the Web sign-in flow. It never serves application content.
    let preview_entry = matches!(*request.method(), Method::GET | Method::HEAD)
        && super::preview::browser_entry_path(path);
    if state.oauth.api_only(request.headers())
        && !(path == "/relay"
            || path.starts_with("/relay/")
            || path == "/healthz"
            || path == "/supervisor/tunnel"
            || path.starts_with("/supervisor/port-stream/")
            || preview_entry)
    {
        return super::spa_fallback(axum::extract::State(state), request).await;
    }
    // This header is internal; callers cannot supply a resolved credential.
    request.headers_mut().remove("x-pockymoe-resolved-session");
    let headers = request.headers();
    if request.uri().path().starts_with("/relay/auth/") && request.method() != Method::GET {
        let peer = request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|p| p.0.ip().to_string())
            .unwrap_or_else(|| "unknown".into());
        if !state.admission.allow(format!("auth-ip:{peer}"), 120, 60) {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
    }
    let websocket = headers
        .get("upgrade")
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));
    let mutation = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    let has_origin = headers.contains_key("origin");
    let authenticated_native = super::bearer_token(headers).is_some();
    // Older supervisors authenticate this native-only endpoint with a query token.
    // Its handler still validates the device-token hash; browser Origin checks apply.
    let native_device_tunnel = request.uri().path() == "/supervisor/tunnel";
    let cross_site = headers
        .get("sec-fetch-site")
        .is_some_and(|v| v == "cross-site");
    if (mutation || websocket)
        && ((has_origin && !origin_allowed(headers, state.oauth.browser_origin().as_deref()))
            || cross_site
            || (websocket && !has_origin && !authenticated_native && !native_device_tunnel))
    {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(
                serde_json::json!({"code":"forbidden","message":"Request origin is not allowed"}),
            ),
        )
            .into_response();
    }
    // A cookie-authenticated mutation must prove same-origin, even for requests
    // which avoid CORS preflight. Explicit native bearer clients have no cookies.
    if mutation
        && (super::relay_cookie(headers).is_some()
            || cookie(headers, "remote_codex_relay_admin_session").is_some())
        && !authenticated_native
        && !has_origin
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let query: super::TokenQuery = super::TokenQuery {
        token: request.uri().query().and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(k, _)| k == "token")
                .map(|(_, v)| v.into_owned())
        }),
        relay_session: request.uri().query().and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(k, _)| k == "relaySession")
                .map(|(_, v)| v.into_owned())
        }),
        ..Default::default()
    };
    let resolved_token = if request.uri().path().starts_with("/relay/admin/") {
        super::bearer_token(headers).or_else(|| cookie(headers, "remote_codex_relay_admin_session"))
    } else {
        super::extract_session_token(headers, &query)
    };
    if let Some(token) = &resolved_token {
        let conn = state.store.conn.lock().await;
        if let Some(user) = super::load_user_by_session(&conn, &state.store.session_secret, &token)
        {
            let mode = super::account::session_mode(&conn, &token);
            if !preview_entry
                && !super::account::request_allowed(
                    &conn,
                    &user,
                    &mode,
                    request.method(),
                    request.uri(),
                )
            {
                return (StatusCode::FORBIDDEN, axum::Json(serde_json::json!({
                    "code":"forbidden",
                    "message":if mode == super::account::RECOVERY { "Complete account recovery first" } else { "Secure your account first" }
                }))).into_response();
            }
        }
    }
    let Ok(resolved_header) = resolved_token.as_deref().unwrap_or("").parse() else {
        return super::unauthorized();
    };
    request
        .headers_mut()
        .insert("x-pockymoe-resolved-session", resolved_header);
    let transport_worker = request
        .uri()
        .path()
        .starts_with("/assets/relayServiceWorker-");
    let api_only = state.oauth.api_only(request.headers());
    let retired_worker = api_only
        && matches!(*request.method(), Method::GET | Method::HEAD)
        && super::retired_worker_asset_path(request.uri().path());
    let mut response = next.run(request).await;
    let retirement_response = retired_worker && response.status() == StatusCode::OK;
    let h = response.headers_mut();
    if api_only && !retirement_response {
        h.remove("service-worker-allowed");
    }
    if transport_worker && !api_only {
        h.insert("service-worker-allowed", "/".parse().unwrap());
    }
    h.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    h.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    h.insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
    let bootstrap = super::RELAY_BOOTSTRAP
        .trim_start_matches("<script>")
        .trim_end_matches("</script>");
    let bootstrap_hash =
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(bootstrap.as_bytes()));
    h.entry(header::CONTENT_SECURITY_POLICY).or_insert(format!("default-src 'self'; script-src 'self' 'wasm-unsafe-eval' 'sha256-{bootstrap_hash}'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; worker-src 'self'; frame-src 'self' blob:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'").parse().unwrap());
    if h.contains_key(header::SET_COOKIE) {
        h.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    }
    if state
        .oauth
        .public_base_url
        .as_deref()
        .is_some_and(|u| u.starts_with("https://"))
    {
        h.insert(
            header::STRICT_TRANSPORT_SECURITY,
            "max-age=31536000".parse().unwrap(),
        );
    }
    response
}

pub(crate) fn cookie(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get("cookie")?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key == name && !value.is_empty()).then(|| value.to_string())
        })
}

pub(crate) fn start_oauth(
    conn: &Connection,
    state: &str,
    callback: &str,
) -> Result<(String, String)> {
    let browser = random_token();
    let verifier = random_token();
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "DELETE FROM relay_oauth_challenges WHERE expires_at<=?1",
        params![now],
    )?;
    let count: i64 = conn.query_row("SELECT count(*) FROM relay_oauth_challenges", [], |r| {
        r.get(0)
    })?;
    if count >= 4096 {
        bail!("Too many pending OAuth logins");
    }
    conn.execute("INSERT INTO relay_oauth_challenges(state_hash,browser_hash,verifier,callback,expires_at) VALUES (?1,?2,?3,?4,?5)",
        params![token_hash(state), token_hash(&browser), verifier, callback, now + 600_000])?;
    Ok((browser, verifier))
}

pub(crate) fn consume_oauth(
    conn: &Connection,
    state: &str,
    browser: &str,
) -> Option<(String, String)> {
    conn.query_row("DELETE FROM relay_oauth_challenges WHERE state_hash=?1 AND browser_hash=?2 AND expires_at>?3 RETURNING verifier,callback",
        params![token_hash(state), token_hash(browser), chrono::Utc::now().timestamp_millis()],
        |r| Ok((r.get(0)?, r.get(1)?))).ok()
}

/// Process-local admission bounds supplement the durable per-challenge limits.
/// Peer addresses come from the listener, never untrusted forwarding headers.
#[derive(Default)]
pub(crate) struct Admission {
    buckets: std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, u32)>>,
}
impl Admission {
    pub fn allow(&self, key: String, limit: u32, seconds: u64) -> bool {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        buckets.retain(|_, (start, _)| now.duration_since(*start).as_secs() < 600);
        if buckets.len() >= 8192 && !buckets.contains_key(&key) {
            return false;
        }
        let (start, count) = buckets.entry(key).or_insert((now, 0));
        if now.duration_since(*start).as_secs() >= seconds {
            *start = now;
            *count = 0;
        }
        if *count >= limit {
            return false;
        }
        *count += 1;
        true
    }
}

// Deliberately no passwords, tokens, OTPs, request bodies, or raw user agents.
pub(crate) fn audit(conn: &Connection, user: Option<&str>, event: &str, resource: Option<&str>) {
    let now = chrono::Utc::now().timestamp_millis();
    if conn.execute("INSERT INTO relay_security_events(user_id,event,resource_id,created_at) VALUES (?1,?2,?3,?4)",params![user,event,resource,now]).is_err() {tracing::warn!(event,"Security audit write failed");}
    let _=conn.execute("DELETE FROM relay_security_events WHERE id IN (SELECT id FROM relay_security_events ORDER BY id DESC LIMIT -1 OFFSET 10000)",[]);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oauth_requires_original_browser_and_is_single_use() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE relay_oauth_challenges(state_hash TEXT PRIMARY KEY,browser_hash TEXT,verifier TEXT,callback TEXT,expires_at INTEGER)").unwrap();
        let (browser, verifier) =
            start_oauth(&conn, "state", "https://relay.example/callback").unwrap();
        assert!(consume_oauth(&conn, "state", "another browser").is_none());
        let result = consume_oauth(&conn, "state", &browser).unwrap();
        assert_eq!(result, (verifier, "https://relay.example/callback".into()));
        assert!(consume_oauth(&conn, "state", &browser).is_none());
    }
    #[test]
    fn configured_origin_is_not_overridden_by_proxy_headers() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("host", "attacker.example".parse().unwrap());
        headers.insert("x-forwarded-host", "relay.example".parse().unwrap());
        headers.insert("origin", "https://attacker.example".parse().unwrap());
        assert!(!origin_allowed(&headers, Some("https://relay.example")));
        headers.insert("origin", "https://relay.example".parse().unwrap());
        assert!(origin_allowed(&headers, Some("https://relay.example")));
        headers.insert("origin", "null".parse().unwrap());
        assert!(!origin_allowed(&headers, Some("https://relay.example")));
    }
}
