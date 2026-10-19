//! Standalone owner endpoints; Relay accounts must use the Relay account API.
use crate::http::AppState;
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use pockymoe_protocol::{Mode, NotificationSettings};
use serde::Deserialize;
use serde_json::json;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/account/notifications/preferences",
            get(load).put(save),
        )
        .route(
            "/api/account/notifications/turns/{thread}/{turn}",
            get(subscription).put(subscribe),
        )
        .route("/api/account/notifications/events", get(events))
}
fn available(state: &AppState) -> bool {
    state.config.mode != Mode::Relay && state.config.relay_server_url.is_none()
}
fn response(result: anyhow::Result<serde_json::Value>) -> Response {
    match result {
        Ok(value) => ([(header::CACHE_CONTROL,"no-store")],Json(value)).into_response(),
        Err(_) => (StatusCode::CONFLICT,Json(json!({"code":"notification_update_failed","message":"无法更新通知：轮次可能已经结束，请刷新后重试。"}))).into_response(),
    }
}
async fn load(State(state): State<AppState>) -> Response {
    if !available(&state) {
        return StatusCode::FORBIDDEN.into_response();
    }
    response(
        state
            .local_notification_settings()
            .and_then(|v| Ok(serde_json::to_value(v)?)),
    )
}
async fn save(State(state): State<AppState>, Json(value): Json<NotificationSettings>) -> Response {
    if !available(&state) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !value.valid() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    response(
        state
            .save_local_notification_settings(&value)
            .map(|_| json!(value)),
    )
}
async fn subscription(
    State(state): State<AppState>,
    Path((thread, turn)): Path<(String, String)>,
) -> Response {
    if !available(&state) {
        return StatusCode::FORBIDDEN.into_response();
    }
    response(
        state
            .local_turn_subscription(&thread, &turn)
            .map(|subscribed| json!({"subscribed":subscribed})),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Subscribe {
    subscribed: bool,
}
async fn subscribe(
    State(state): State<AppState>,
    Path((thread, turn)): Path<(String, String)>,
    Json(body): Json<Subscribe>,
) -> Response {
    if !available(&state) {
        return StatusCode::FORBIDDEN.into_response();
    }
    response(
        state
            .set_local_turn_subscription(&thread, &turn, body.subscribed)
            .map(|_| json!({"subscribed":body.subscribed})),
    )
}
async fn events(State(state): State<AppState>) -> Response {
    if !available(&state) {
        return StatusCode::FORBIDDEN.into_response();
    }
    response(state.local_notifications().map(|events| json!(events)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use pockymoe_runtime::{Database, Supervisor};
    use std::sync::Arc;
    use tower::ServiceExt;

    #[tokio::test]
    async fn local_owner_settings_persist_across_browsers_and_relay_devices_cannot_override_them() {
        let dir = tempfile::tempdir().unwrap();
        let (_other, original) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
        let mut config = original.config.clone();
        drop(original);
        config.mode = Mode::Local;
        config.auth_required = false;
        config.relay_server_url = None;
        config.database_url = dir.path().join("local.sqlite");
        config.workspace_root = dir.path().join("work");
        let state = Arc::new(Supervisor::new(
            config.clone(),
            Database::open(&config.database_url).unwrap(),
            vec![],
        ));
        let app = routes().with_state(state.clone());
        let request = |method: &str, body: &str| {
            Request::builder()
                .method(method)
                .uri("/api/account/notifications/preferences")
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        let response = app.clone().oneshot(request("GET", "")).await.unwrap();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let bytes = to_bytes(response.into_body(), 10000).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["scope"],
            "subscribed"
        );
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "PUT",
                    r#"{"scope":"all","devices":[],"workspaces":[]}"#
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        drop(app);
        drop(state);
        let reopened = Arc::new(Supervisor::new(
            config.clone(),
            Database::open(&config.database_url).unwrap(),
            vec![],
        ));
        assert_eq!(
            reopened.local_notification_settings().unwrap().scope,
            pockymoe_protocol::NotificationScope::All
        );
        config.mode = Mode::Relay;
        let relay = Arc::new(Supervisor::new(
            config,
            Database::open(&dir.path().join("relay.sqlite")).unwrap(),
            vec![],
        ));
        assert_eq!(
            routes()
                .with_state(relay)
                .oneshot(request("PUT", r#"{"scope":"all"}"#))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert!(crate::grants::owner_only(
            "/api/account/notifications/preferences"
        ));
    }
}
