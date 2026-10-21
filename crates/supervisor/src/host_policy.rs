//! Central HTTP and interaction gates. Authentication for enabled features is unchanged.
use pockymoe_runtime::{host_mode::require_feature, Supervisor};
use serde_json::Value;

pub(crate) fn route_feature(state: &Supervisor, path: &str) -> Option<(bool, &'static str)> {
    let caps = &state.host_capabilities;
    if path == "/api/transport" || path.starts_with("/api/transport/") {
        Some((caps.account_binding, "accountBinding"))
    } else if path == "/api/peer"
        || path.starts_with("/api/peer/")
        || path == "/api/config/peer-access"
    {
        Some((caps.device_peer, "devicePeer"))
    } else if path == "/api/port-mappings" || path.starts_with("/api/port-mappings/") {
        Some((caps.previews, "previews"))
    } else {
        None
    }
}

pub(crate) fn require_command(state: &Supervisor, input: &Value) -> anyhow::Result<()> {
    let caps = &state.host_capabilities;
    let operation = input["operation"].as_str().unwrap_or("");
    if matches!(
        operation,
        "devices" | "peerAccess" | "peerTrust" | "fsList" | "fsGet" | "outbox"
    ) || input["deviceId"].as_str().is_some()
    {
        require_feature(caps.device_peer, "devicePeer")?;
    }
    if matches!(
        operation,
        "previewCreate" | "previewList" | "previewCheck" | "previewStop"
    ) {
        require_feature(caps.previews, "previews")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use pockymoe_runtime::host_mode::{HostMode, FEATURE_DISABLED_CODE};
    use serde_json::json;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn setup(mode: HostMode) -> (tempfile::TempDir, Arc<Supervisor>) {
        let (dir, original) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
        let mut config = original.config.clone();
        drop(original);
        config.host_mode = mode;
        // The dropped fixture's background tasks may still own its database.
        config.database_url = dir.path().join("host-policy.sqlite");
        let db = pockymoe_runtime::Database::open(&config.database_url).unwrap();
        let state = Arc::new(Supervisor::new(config, db, vec![]));
        state.configure_cli("http://127.0.0.1:0".into());
        (dir, state)
    }

    async fn request(
        state: &Arc<Supervisor>,
        method: &str,
        path: &str,
        input: Value,
        authenticated: bool,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if authenticated {
            let token = if path == "/api/cli" {
                state
                    .interaction
                    .context
                    .read()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .token
                    .clone()
            } else {
                crate::auth::login(&state.config, "admin", "secret123")
                    .unwrap()
                    .0
            };
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = crate::router(state.clone())
            .oneshot(request.body(Body::from(input.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[test]
    fn host_mode_parsing_and_capabilities() {
        assert_eq!(HostMode::default(), HostMode::Standalone);
        for (name, mode, enabled) in [
            ("standalone", HostMode::Standalone, true),
            ("hosted", HostMode::Hosted, false),
        ] {
            assert_eq!(name.parse::<HostMode>().unwrap(), mode);
            assert_eq!(
                serde_json::to_value(mode.capabilities()).unwrap(),
                json!({"relay":enabled,"accountBinding":enabled,"devicePeer":enabled,"previews":enabled})
            );
        }
        for name in ["", "relay", "local", "Hosted", "invalid"] {
            assert!(name.parse::<HostMode>().is_err());
        }
    }

    #[tokio::test]
    async fn hosted_routes_and_cli_commands_return_stable_error() {
        let (_dir, state) = setup(HostMode::Hosted);
        for (method, path) in [
            ("POST", "/api/transport/bind"),
            ("POST", "/api/transport/browser"),
            ("GET", "/api/transport/enrollment"),
            ("PUT", "/api/transport/device-certificate"),
            ("PUT", "/api/transport/revocations"),
            ("GET", "/api/transport/trust"),
            ("GET", "/api/transport/grants"),
            ("PUT", "/api/transport/grants/id"),
            ("POST", "/api/peer/cli"),
            ("POST", "/api/peer/files/uploads"),
            ("POST", "/api/peer/transport/certificate"),
            ("GET", "/api/peer/files/read"),
            ("GET", "/api/config/peer-access"),
            ("PATCH", "/api/config/peer-access"),
            ("GET", "/api/port-mappings"),
            ("POST", "/api/port-mappings"),
            ("DELETE", "/api/port-mappings/id"),
            ("POST", "/api/port-mappings/id/lease"),
        ] {
            let (status, body) = request(&state, method, path, json!({}), true).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
            assert_eq!(body["code"], FEATURE_DISABLED_CODE, "{path}");
        }
        for operation in [
            "devices",
            "peerAccess",
            "peerTrust",
            "fsList",
            "fsGet",
            "outbox",
            "previewCreate",
            "previewList",
            "previewCheck",
            "previewStop",
        ] {
            let (status, body) = request(
                &state,
                "POST",
                "/api/cli",
                json!({"operation":operation}),
                true,
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{operation}: {body}");
            assert_eq!(body["code"], FEATURE_DISABLED_CODE);
        }
        for operation in ["list", "create", "show", "status", "send", "transcript"] {
            let (status, body) = request(
                &state,
                "POST",
                "/api/cli",
                json!({"operation":operation,"deviceId":"other"}),
                true,
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(body["code"], FEATURE_DISABLED_CODE);
        }
    }

    #[tokio::test]
    async fn hosted_preserves_local_cli_and_http_authentication() {
        let (_dir, state) = setup(HostMode::Hosted);
        for (method, path) in [
            ("GET", "/api/threads"),
            ("POST", "/api/cli"),
            ("GET", "/api/config/runtime"),
        ] {
            let (status, _) =
                request(&state, method, path, json!({"operation":"list"}), false).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        }
        let (status, body) = request(
            &state,
            "POST",
            "/api/cli",
            json!({"operation":"list"}),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["threads"], json!([]));
        let (status, body) = request(&state, "GET", "/api/config/runtime", json!({}), true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["hostMode"], "hosted");
        assert_eq!(
            body["hostCapabilities"],
            json!({"relay":false,"accountBinding":false,"devicePeer":false,"previews":false})
        );
        assert!(body["capabilities"]["terminal"].is_boolean());
        let (_, body) = request(&state, "GET", "/healthz", json!({}), false).await;
        assert_eq!(body["hostMode"], "hosted");
        assert_eq!(body["relayConnected"], false);
    }

    #[tokio::test]
    async fn standalone_keeps_preview_and_local_commands_enabled() {
        let (_dir, state) = setup(HostMode::Standalone);
        let (status, body) = request(
            &state,
            "POST",
            "/api/cli",
            json!({"operation":"previewList"}),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = request(
            &state,
            "POST",
            "/api/cli",
            json!({"operation":"peerAccess"}),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (_, body) = request(&state, "GET", "/api/config/runtime", json!({}), true).await;
        assert_eq!(body["hostMode"], "standalone");
        assert_eq!(
            body["hostCapabilities"],
            json!({"relay":true,"accountBinding":true,"devicePeer":true,"previews":true})
        );
    }

    #[tokio::test]
    async fn hosted_tunnel_never_attempts_connection_and_previews_cannot_start() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (_dir, mut state) = setup(HostMode::Hosted);
        Arc::get_mut(&mut state).unwrap().config.relay_server_url =
            Some(format!("ws://{}", listener.local_addr().unwrap()));
        let error = crate::run_relay_tunnel(state.clone()).await.unwrap_err();
        assert!(error.is::<pockymoe_runtime::host_mode::FeatureDisabled>());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
        assert!(
            crate::ports::create_local_mapping(&state, 4013, "test", None)
                .err()
                .unwrap()
                .is::<pockymoe_runtime::host_mode::FeatureDisabled>()
        );
        assert!(crate::ports::snapshot(&state).is_empty());
    }
}
