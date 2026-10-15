//! The optional local-mode passcode, end to end through the router.
use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use pockymoe_protocol::Mode;
use pockymoe_runtime::Supervisor;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

fn local_state(dir: &tempfile::TempDir) -> Arc<Supervisor> {
    let (_relay_dir, original) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let mut config = original.config.clone();
    drop(original);
    config.mode = Mode::Local;
    config.auth_required = false;
    config.admin_username = None;
    config.admin_password = None;
    config.session_secret = None;
    config.relay_server_url = None;
    config.relay_agent_token = None;
    config.database_url = dir.path().join("supervisor.sqlite");
    config.workspace_root = dir.path().join("workspaces");
    let db = pockymoe_runtime::Database::open(&config.database_url).unwrap();
    let state = Arc::new(Supervisor::new(config, db, vec![]));
    state.configure_cli("http://127.0.0.1:0".into());
    state
}

async fn call(
    state: &Arc<Supervisor>,
    method: &str,
    path: &str,
    input: Value,
    token: Option<&str>,
) -> (StatusCode, Value, Option<String>) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = crate::router(state.clone())
        .oneshot(request.body(Body::from(input.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .map(|value| value.to_str().unwrap().to_owned());
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        cookie,
    )
}

fn owner_token(state: &Supervisor) -> String {
    state
        .interaction
        .context
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .token
        .clone()
}

#[tokio::test]
async fn local_mode_opens_until_a_passcode_is_set_and_then_signs_browsers_in() {
    let dir = tempfile::tempdir().unwrap();
    let state = local_state(&dir);
    // No passcode: the first visitor walks straight in.
    let (status, _, _) = call(&state, "GET", "/api/config/runtime", json!(null), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, session, _) = call(&state, "GET", "/api/auth/session", json!(null), None).await;
    assert_eq!(session["authenticated"], true);
    assert_eq!(session["passcode"], false);

    let (status, _, _) = call(
        &state,
        "PUT",
        "/api/auth/passcode",
        json!({"passcode":"ab"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body, cookie) = call(
        &state,
        "PUT",
        "/api/auth/passcode",
        json!({"passcode":"open sesame"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // The browser that set it stays signed in.
    assert!(cookie.unwrap().starts_with("remote_codex_session="));
    let first = body["token"].as_str().unwrap().to_owned();

    let (status, _, _) = call(&state, "GET", "/api/config/runtime", json!(null), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (_, session, _) = call(&state, "GET", "/api/auth/session", json!(null), None).await;
    assert_eq!(
        (
            session["authenticated"].clone(),
            session["authRequired"].clone(),
            session["passcode"].clone()
        ),
        (json!(false), json!(true), json!(true))
    );
    let (status, _, _) = call(
        &state,
        "GET",
        "/api/config/runtime",
        json!(null),
        Some(&first),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, _) = call(
        &state,
        "POST",
        "/api/auth/login",
        json!({"password":"wrong"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body, _) = call(
        &state,
        "POST",
        "/api/auth/login",
        json!({"password":"open sesame"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["session"]["passcode"], true);

    // A new passcode signs out every other browser.
    let (_, body, _) = call(
        &state,
        "PUT",
        "/api/auth/passcode",
        json!({"passcode":"new secret"}),
        Some(&first),
    )
    .await;
    let second = body["token"].as_str().unwrap().to_owned();
    let (status, _, _) = call(
        &state,
        "GET",
        "/api/config/runtime",
        json!(null),
        Some(&first),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = call(
        &state,
        "GET",
        "/api/config/runtime",
        json!(null),
        Some(&second),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // It survives a restart.
    drop(state);
    let state = local_state(&dir);
    let (status, _, _) = call(&state, "GET", "/api/config/runtime", json!(null), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A forgotten passcode: the computer's owner turns it off with the local CLI.
    let owner = owner_token(&state);
    let (_, body, _) = call(
        &state,
        "POST",
        "/api/cli",
        json!({"operation":"passcodeStatus"}),
        Some(&owner),
    )
    .await;
    assert_eq!(body["passcode"], true);
    let (status, body, _) = call(
        &state,
        "POST",
        "/api/cli",
        json!({"operation":"passcodeOff"}),
        Some(&owner),
    )
    .await;
    assert_eq!(
        (status, body["passcode"].clone()),
        (StatusCode::OK, json!(false))
    );
    let (status, _, _) = call(&state, "GET", "/api/config/runtime", json!(null), None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn relay_and_server_modes_cannot_set_a_local_passcode() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let admin = crate::auth::login(&state.config, "admin", "secret123")
        .unwrap()
        .0;
    let (status, _, _) = call(
        &state,
        "PUT",
        "/api/auth/passcode",
        json!({"passcode":"open sesame"}),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(crate::auth::local_passcode(&state).is_none());
}
