use anyhow::{bail, Result};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{SecondsFormat, TimeZone, Utc};
use hmac::{Hmac, Mac};
use pockymoe_protocol::{ApiError, AuthSessionDto, Mode};
use pockymoe_runtime::{RuntimeConfig, Supervisor};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::Arc;
use uuid::Uuid;

const AUTH_COOKIE_NAME: &str = "remote_codex_session";
// Only the authenticated tunnel can construct this extension; HTTP headers cannot.
#[derive(Clone)]
pub(crate) struct TrustedRelayForward;
#[derive(Clone)]
pub(crate) struct CliCaller(pub Option<String>);
/// Another device of the same relay owner. Only the tunnel constructs it, from the
/// `peer` object the relay itself adds to a forwarded `relay.request`.
#[derive(Clone, Debug)]
pub(crate) struct PeerCaller {
    pub device_id: String,
    pub device_name: String,
    pub user_id: String,
}
const DEFAULT_SESSION_TTL_SECONDS: i64 = 60 * 60 * 24 * 7;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionPayload {
    username: String,
    expires_at: i64,
    nonce: String,
}

pub fn validate_config(config: &RuntimeConfig) -> Result<()> {
    if !config.auth_required {
        return Ok(());
    }
    if config
        .admin_username
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        bail!(
            "{} mode requires POCKYMOE_ADMIN_USERNAME",
            mode_name(config.mode)
        );
    }
    if config
        .admin_password
        .as_deref()
        .is_none_or(|value| value.is_empty())
    {
        bail!(
            "{} mode requires POCKYMOE_ADMIN_PASSWORD",
            mode_name(config.mode)
        );
    }
    if config
        .session_secret
        .as_deref()
        .is_none_or(|value| value.len() < 16)
    {
        bail!(
            "{} mode requires POCKYMOE_SESSION_SECRET with at least 16 characters",
            mode_name(config.mode)
        );
    }
    Ok(())
}

pub async fn require_auth(
    State(state): State<Arc<Supervisor>>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let bootstrap = crate::owner_trust::bootstrap(request.method().as_str(), path);
    let owner_endpoint = path == "/api/transport/trust"
        || path == "/api/transport/device-certificate"
        || path == "/api/transport/grants"
        || path.starts_with("/api/transport/grants/");
    if bootstrap || owner_endpoint {
        if request
            .extensions()
            .get::<crate::owner_trust::EncryptedOwnerRequest>()
            .is_none()
        {
            return (
                axum::http::StatusCode::FORBIDDEN,
                Json(serde_json::json!({"code":"device_strict_mode"})),
            )
                .into_response();
        }
        if path == "/api/transport/bind"
            && request.extensions().get::<TrustedRelayForward>().is_none()
        {
            return crate::owner_trust::response(crate::owner_trust::reject(403, "owner_required"));
        }
        if owner_endpoint
            && request
                .extensions()
                .get::<crate::owner_trust::BrowserPrincipal>()
                .is_none_or(|p| !p.valid(&state))
        {
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"code":"browser_registration_required"})),
            )
                .into_response();
        }
    }
    if let Some(p) = request
        .extensions()
        .get::<crate::owner_trust::BrowserPrincipal>()
    {
        if !bootstrap && p.valid(&state) {
            if let Err(error) = crate::grants::authorize_path(
                &state,
                p,
                request.method().as_str(),
                &request.uri().to_string(),
            ) {
                return crate::owner_trust::response(Err(error));
            }
        }
    }
    if request.extensions().get::<TrustedRelayForward>().is_some()
        && crate::owner_trust::strict(&state)
    {
        let public = request.method() == axum::http::Method::GET
            && (path == "/healthz"
                || path
                    .strip_prefix("/api/publications/")
                    .is_some_and(|token| !token.is_empty() && !token.contains('/')));
        if !path.starts_with("/api/peer/")
            && !public
            && !bootstrap
            && request
                .extensions()
                .get::<crate::owner_trust::BrowserPrincipal>()
                .is_none_or(|p| !p.valid(&state))
        {
            let encrypted = request
                .extensions()
                .get::<crate::owner_trust::EncryptedOwnerRequest>()
                .is_some();
            return (if encrypted { axum::http::StatusCode::UNAUTHORIZED } else { axum::http::StatusCode::FORBIDDEN },
                Json(serde_json::json!({"code":if encrypted {"browser_registration_required"} else {"device_strict_mode"}}))).into_response();
        }
    }
    let peer_path = request.uri().path().starts_with("/api/peer/");
    let peer = request.extensions().get::<PeerCaller>().is_some();
    if peer || peer_path {
        if peer && peer_path && crate::owner_trust::strict(&state) {
            let bootstrap = path == "/api/peer/transport/certificate"
                && request.method() == axum::http::Method::POST
                && request
                    .extensions()
                    .get::<crate::owner_trust::EncryptedOwnerRequest>()
                    .is_some();
            let verified = request
                .extensions()
                .get::<crate::owner_trust::PeerPrincipal>()
                .is_some_and(|proof| {
                    proof.valid(
                        &state,
                        &request.extensions().get::<PeerCaller>().unwrap().device_id,
                    )
                });
            if !bootstrap && !verified {
                return (
                    axum::http::StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({"code":"peer_certificate_required"})),
                )
                    .into_response();
            }
        }
        if peer && peer_path && !state.peer_access_enabled() {
            return (
                axum::http::StatusCode::FORBIDDEN,
                Json(serde_json::json!({"code":"peer_access_disabled"})),
            )
                .into_response();
        }
        return if peer && peer_path && request.extensions().get::<TrustedRelayForward>().is_some() {
            next.run(request).await
        } else {
            (
                axum::http::StatusCode::FORBIDDEN,
                Json(ApiError::new(
                    "peer_forbidden",
                    "Peer credentials are required and only allow /api/peer/ requests.",
                )),
            )
                .into_response()
        };
    }
    if request.uri().path() == "/api/cli" {
        let scoped =
            bearer_token(request.headers()).and_then(|token| state.cli_token_thread(&token));
        let valid = request.extensions().get::<TrustedRelayForward>().is_none()
            && (scoped.is_some()
                || state
                    .interaction
                    .context
                    .read()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|context| {
                        bearer_token(request.headers()).is_some_and(|token| {
                            constant_time_equal(token.as_bytes(), context.token.as_bytes())
                        })
                    }));
        return if valid {
            request.extensions_mut().insert(CliCaller(scoped));
            next.run(request).await
        } else {
            (
                axum::http::StatusCode::UNAUTHORIZED,
                Json(ApiError::new(
                    "unauthorized",
                    "Local CLI credentials are required.",
                )),
            )
                .into_response()
        };
    }
    if !state.config.auth_required || is_public_path(request.uri().path()) {
        return next.run(request).await;
    }
    if state.config.mode == Mode::Relay
        && request.extensions().get::<TrustedRelayForward>().is_some()
    {
        return next.run(request).await;
    }
    if verify_request(&state.config, request.headers(), request.uri()).authenticated {
        return next.run(request).await;
    }
    (
        axum::http::StatusCode::UNAUTHORIZED,
        Json(ApiError::new("unauthorized", "Authentication is required.")),
    )
        .into_response()
}

fn is_public_path(path: &str) -> bool {
    matches!(
        path,
        "/healthz" | "/readyz" | "/api/auth/login" | "/api/auth/logout" | "/api/auth/session"
    ) || (!path.starts_with("/api/") && path != "/ws")
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Local => "local",
        Mode::Server => "server",
        Mode::Relay => "relay",
    }
}

pub fn local_session(config: &RuntimeConfig) -> AuthSessionDto {
    AuthSessionDto {
        authenticated: true,
        username: None,
        expires_at: None,
        mode: config.mode,
        auth_required: false,
    }
}

pub fn unauthenticated_session(config: &RuntimeConfig) -> AuthSessionDto {
    AuthSessionDto {
        authenticated: false,
        username: None,
        expires_at: None,
        mode: config.mode,
        auth_required: config.auth_required,
    }
}

pub fn verify_request(config: &RuntimeConfig, headers: &HeaderMap, uri: &Uri) -> AuthSessionDto {
    if !config.auth_required {
        return local_session(config);
    }
    let token = bearer_token(headers)
        .or_else(|| query_token(uri))
        .or_else(|| cookie_token(headers));
    token
        .and_then(|token| verify_token(config, &token))
        .unwrap_or_else(|| unauthenticated_session(config))
}

pub fn login(
    config: &RuntimeConfig,
    username: &str,
    password: &str,
) -> Option<(String, AuthSessionDto)> {
    if !config.auth_required {
        return Some((String::new(), local_session(config)));
    }
    let expected_username = config.admin_username.as_deref()?;
    let expected_password = config.admin_password.as_deref()?;
    if !constant_time_equal(username.as_bytes(), expected_username.as_bytes())
        || !constant_time_equal(password.as_bytes(), expected_password.as_bytes())
    {
        return None;
    }
    let ttl_seconds = session_ttl_seconds();
    let expires_at = Utc::now().timestamp_millis() + ttl_seconds * 1000;
    let payload = SessionPayload {
        username: expected_username.to_string(),
        expires_at,
        nonce: Uuid::new_v4().simple().to_string(),
    };
    let payload_text = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).ok()?);
    let signature = sign(config.session_secret.as_deref()?, &payload_text)?;
    let token = format!("{payload_text}.{signature}");
    Some((
        token,
        AuthSessionDto {
            authenticated: true,
            username: Some(expected_username.to_string()),
            expires_at: timestamp_string(expires_at),
            mode: config.mode,
            auth_required: true,
        },
    ))
}

pub fn session_cookie(token: &str) -> String {
    format!(
        "{AUTH_COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}",
        session_ttl_seconds()
    )
}

pub fn clear_session_cookie() -> &'static str {
    "remote_codex_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"
}

fn verify_token(config: &RuntimeConfig, token: &str) -> Option<AuthSessionDto> {
    let mut parts = token.split('.');
    let payload_text = parts.next()?;
    let signature = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let expected = sign(config.session_secret.as_deref()?, payload_text)?;
    if !constant_time_equal(signature.as_bytes(), expected.as_bytes()) {
        return None;
    }
    let payload: SessionPayload =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload_text).ok()?).ok()?;
    if payload.username.is_empty()
        || payload.nonce.is_empty()
        || payload.expires_at <= Utc::now().timestamp_millis()
    {
        return None;
    }
    Some(AuthSessionDto {
        authenticated: true,
        username: Some(payload.username),
        expires_at: timestamp_string(payload.expires_at),
        mode: config.mode,
        auth_required: true,
    })
}

fn sign(secret: &str, payload_text: &str) -> Option<String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(payload_text.as_bytes());
    Some(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let Ok(mut mac) = HmacSha256::new_from_slice(b"pockymoe-constant-time-compare") else {
        return false;
    };
    mac.update(left);
    let expected = mac.finalize().into_bytes();
    let Ok(mut verifier) = HmacSha256::new_from_slice(b"pockymoe-constant-time-compare") else {
        return false;
    };
    verifier.update(right);
    verifier.verify_slice(&expected).is_ok()
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty())
        .then(|| token.trim().to_string())
}

fn query_token(uri: &Uri) -> Option<String> {
    url::form_urlencoded::parse(uri.query()?.as_bytes()).find_map(|(key, value)| {
        (key == "token" && !value.trim().is_empty()).then(|| value.into_owned())
    })
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|entry| {
            let (name, value) = entry.trim().split_once('=')?;
            (name == AUTH_COOKIE_NAME && !value.is_empty()).then(|| value.to_string())
        })
}

fn session_ttl_seconds() -> i64 {
    std::env::var("POCKYMOE_SESSION_TTL_SECONDS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_SESSION_TTL_SECONDS)
}

fn timestamp_string(timestamp_millis: i64) -> Option<String> {
    Utc.timestamp_millis_opt(timestamp_millis)
        .single()
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Millis, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::tests::state_with_relay_url;
    use axum::{body::Body, http::StatusCode};
    use tower::ServiceExt;

    fn peer() -> PeerCaller {
        PeerCaller {
            device_id: "remote-device".into(),
            device_name: "Remote device".into(),
            user_id: "owner".into(),
        }
    }
    #[tokio::test]
    async fn peer_caller_and_peer_routes_require_each_other_even_without_auth() {
        for auth_required in [true, false] {
            let (_dir, mut state) = state_with_relay_url("http://127.0.0.1:1");
            Arc::get_mut(&mut state).unwrap().config.auth_required = auth_required;
            state.set_peer_access(true).unwrap();
            let router = crate::http::router(state.clone());
            for (trusted, caller) in [(false, false), (true, false), (false, true)] {
                let mut request = axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/peer/cli")
                    .header("content-type", "application/json")
                    .header("x-pockymoe-relay-forwarded", "1")
                    .header("x-pockymoe-peer-device", "remote-device");
                if trusted {
                    request = request.extension(TrustedRelayForward);
                }
                if caller {
                    request = request.extension(peer());
                }
                let response = router
                    .clone()
                    .oneshot(request.body(Body::from(r#"{"operation":"info"}"#)).unwrap())
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::FORBIDDEN);
            }
            let token = login(&state.config, "admin", "secret123").unwrap().0;
            let response = router
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/api/peer/cli")
                        .header("authorization", format!("Bearer {token}"))
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"operation":"info"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            for path in [
                "/api/version",
                "/api/config/peer-access",
                "/api/cli",
                "/healthz",
                "/api/peers/cli",
            ] {
                let response = router
                    .clone()
                    .oneshot(
                        axum::http::Request::builder()
                            .uri(path)
                            .extension(TrustedRelayForward)
                            .extension(peer())
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
            }
            let response = router
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/api/peer/cli")
                        .header("content-type", "application/json")
                        .extension(TrustedRelayForward)
                        .extension(peer())
                        .body(Body::from(r#"{"operation":"info"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_ne!(value["code"], "peer_forbidden");
        }
    }
}
