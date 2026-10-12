//! Local CLI preview provisioning. Never returns account sessions or launch tickets.
use anyhow::{bail, Context, Result};
use pockymoe_runtime::Supervisor;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::time::Duration;
use url::Url;

fn origin(value: &str) -> Result<Url> {
    let url = Url::parse(value)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("Relay advertised an invalid preview origin");
    }
    Ok(url)
}

fn path(value: Option<&str>) -> Result<&str> {
    let value = value.unwrap_or("/");
    if !value.starts_with('/')
        || value.starts_with("//")
        || value.contains(['\\', '\r', '\n'])
        || value.len() > 8192
    {
        bail!("Preview path must be a local absolute path, for example /app?tab=1");
    }
    Ok(value)
}

fn describe(
    state: &Supervisor,
    mapping: &crate::ports::Mapping,
    requested_path: &str,
) -> Result<Value> {
    let identity = crate::peer_link::relay_identity(state)
        .context("Connect this Supervisor to a Relay first")?;
    let mut url = origin(identity.port_preview_base_url.as_deref().context(
        "Port previews are unavailable. Enable POCKYMOE_PORT_PREVIEW_BASE_URL on the Relay and reconnect this Supervisor; upgrade an older Relay if needed."
    )?)?;
    url.set_host(Some(&format!(
        "p-{}.{}",
        mapping.id,
        url.host_str().unwrap()
    )))?;
    let hostname = url.host_str().unwrap().to_owned();
    let preview_origin = url.origin().ascii_serialization();
    let url = url.join(requested_path)?;
    let mut control = if let Some(base) = identity.public_base_url.as_deref() {
        origin(base)?
    } else {
        let mut base = Url::parse(
            state
                .config
                .relay_server_url
                .as_deref()
                .context("Relay URL missing")?,
        )?;
        let scheme = if matches!(base.scheme(), "https" | "wss") {
            "https"
        } else {
            "http"
        };
        base.set_scheme(scheme)
            .map_err(|_| anyhow::anyhow!("Invalid Relay URL"))?;
        base.set_path("/");
        base.set_query(None);
        base.set_fragment(None);
        origin(base.as_str())?
    };
    control.set_path(&format!(
        "/devices/{}/ports/{}",
        identity.device_id, mapping.id
    ));
    control
        .query_pairs_mut()
        .append_pair("path", requested_path);
    Ok(
        json!({"id":mapping.id,"port":mapping.port,"label":mapping.label,"createdAt":mapping.created_at,
        "url":url.as_str(),"origin":preview_origin,"hostname":hostname,"openUrl":control.as_str(),
        "relayConnected":state.relay_connected.load(Ordering::SeqCst),
        "access":"deviceOwner",
        "nextSteps":["This reserves an address; it does not start the service. Bind an HTTP service to this port on loopback.",
            "If the framework requires an allowlist, add only hostname (or origin for origin-based settings); never allow all hosts.",
            "Run preview check after starting the service. Share openUrl for browser login and access; no account token is needed in CLI output."]}),
    )
}

pub(crate) async fn run(state: &Supervisor, input: &Value) -> Result<Value> {
    let requested_path = path(input["path"].as_str())?;
    match input["operation"].as_str().unwrap_or("") {
        "previewList" => {
            let mappings = crate::ports::snapshot(state)
                .iter()
                .map(|m| describe(state, m, requested_path))
                .collect::<Result<Vec<_>>>()?;
            Ok(
                json!({"mappings":mappings,"relayConnected":state.relay_connected.load(Ordering::SeqCst),"available":crate::peer_link::relay_identity(state).and_then(|i| i.port_preview_base_url).is_some()}),
            )
        }
        "previewCreate" => {
            if !state.relay_connected.load(Ordering::SeqCst) {
                bail!("Relay is disconnected; reconnect before allocating a preview address");
            }
            let port = input["port"]
                .as_u64()
                .filter(|p| (1..=65535).contains(p))
                .context("HTTP port must be between 1 and 65535")? as u16;
            // Validate advertised configuration before persisting a mapping.
            let placeholder = crate::ports::Mapping {
                id: "0".repeat(32),
                port,
                label: String::new(),
                created_at: String::new(),
            };
            describe(state, &placeholder, requested_path)?;
            let mapping = crate::ports::create_local_mapping(
                state,
                port,
                input["label"].as_str().unwrap_or(""),
            )?;
            describe(state, &mapping, requested_path)
        }
        operation => {
            let target = input["target"]
                .as_str()
                .context("Mapping ID or port required")?;
            let mapping = crate::ports::snapshot(state)
                .into_iter()
                .find(|m| m.id == target || target.parse::<u16>().ok() == Some(m.port))
                .context("Port mapping not found; use preview create first")?;
            if operation == "previewStop" {
                crate::ports::remove_mapping(state, &mapping.id)?;
                return Ok(json!({"id":mapping.id,"port":mapping.port,"stopped":true}));
            }
            let mut result = describe(state, &mapping, requested_path)?;
            let ws_path = input["websocketPath"]
                .as_str()
                .map(|p| path(Some(p)))
                .transpose()?;
            result["check"] = check(
                mapping.port,
                result["origin"].as_str().unwrap(),
                requested_path,
                ws_path,
            )
            .await?;
            Ok(result)
        }
    }
}

fn classify(status: u16, body: &str) -> &'static str {
    let body = body.to_ascii_lowercase();
    if matches!(status, 400 | 403 | 421)
        && [
            "blocked request",
            "host is not allowed",
            "invalid host header",
            "disallowedhost",
            "allowedhosts",
            "alloweddevorigins",
            "origin is not allowed",
        ]
        .iter()
        .any(|s| body.contains(s))
    {
        "hostRejected"
    } else if status >= 400 {
        "httpError"
    } else {
        "httpReady"
    }
}

async fn check(
    port: u16,
    preview_origin: &str,
    requested_path: &str,
    websocket_path: Option<&str>,
) -> Result<Value> {
    let public = origin(preview_origin)?;
    let authority = &public[url::Position::BeforeHost..url::Position::AfterPort];
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut last_error = String::new();
    let mut reply = None;
    // Match the gateway's IPv4/IPv6 fallback and forwarded Host semantics.
    for host in ["127.0.0.1", "[::1]"] {
        let local = format!("http://{host}:{port}{requested_path}");
        match client
            .get(local)
            .header("host", format!("127.0.0.1:{port}"))
            .header("x-forwarded-host", authority)
            .header("x-forwarded-proto", public.scheme())
            .send()
            .await
        {
            Ok(response) => {
                reply = Some((host, response));
                break;
            }
            Err(error) => {
                last_error = if error.is_connect() {
                    "serviceNotRunning"
                } else if error.is_timeout() {
                    "serviceTimeout"
                } else {
                    "httpProbeFailed"
                }
                .into()
            }
        }
    }
    let Some((host, mut response)) = reply else {
        return Ok(
            json!({"status":last_error,"scope":"localUpstream","websocket":"notChecked",
            "advice":"Confirm the process is running on the exact mapped port and serves HTTP on loopback. This probe does not test public DNS, TLS or browser login."}),
        );
    };
    let code = response.status().as_u16();
    let mut body = Vec::new();
    // Bound both body size and total read time, including streaming responses.
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while body.len() < 65536 {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    body.extend_from_slice(&chunk[..chunk.len().min(65536 - body.len())])
                }
                _ => break,
            }
        }
    })
    .await;
    let status = classify(code, &String::from_utf8_lossy(&body));
    let mut ws = "notChecked";
    if let Some(ws_path) = websocket_path {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = format!("ws://{host}:{port}{ws_path}").into_client_request()?;
        request
            .headers_mut()
            .insert("host", format!("127.0.0.1:{port}").parse()?);
        request
            .headers_mut()
            .insert("x-forwarded-host", authority.parse()?);
        request
            .headers_mut()
            .insert("x-forwarded-proto", public.scheme().parse()?);
        request
            .headers_mut()
            .insert("origin", format!("http://127.0.0.1:{port}").parse()?);
        ws = match tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async(request),
        )
        .await
        {
            Ok(Ok((mut socket, _))) => {
                let _ = tokio::time::timeout(Duration::from_secs(1), socket.close(None)).await;
                "handshakeAccepted"
            }
            _ => "handshakeFailed",
        };
    }
    let advice = match status {
        "hostRejected" => "Inspect the rejection and framework version. Add only the assigned hostname to Vite server.allowedHosts, webpack devServer.allowedHosts or Django ALLOWED_HOSTS; Next.js origin restrictions use allowedDevOrigins. Preserve existing entries and restart the dev server. Do not disable host checks.",
        "httpError" => "The service responded with an HTTP error. Inspect its response/logs; an arbitrary 403 is not proof of an allowlist problem.",
        _ if ws == "handshakeFailed" => "HTTP responds but this WebSocket handshake failed. Check the framework's actual HMR path, token/subprotocol requirements and proxy WebSocket support. For a confirmed Vite public-address problem, use server.ws (older Vite: server.hmr) with the assigned hostname, wss and public clientPort; do not broadly allow origins.",
        _ => "Local HTTP responds. Verify openUrl in the user's browser for public DNS/TLS/authentication and actual hot reload. A local probe cannot certify the entire Relay/browser path.",
    };
    Ok(
        json!({"status":status,"httpStatus":code,"scope":"localUpstream","websocket":ws,"advice":advice}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn strict_local_machine_and_thread_cli_previews_survive_session_expiry_and_owner_removal()
    {
        use axum::{
            body::Body,
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;
        let (_dir, state) =
            crate::tunnel::tests::state_with_relay_url("https://remote.example.test");
        let (connection, _outbound) = crate::peer_link::tests::connected(&state, "device");
        connection.connected(Some(crate::peer_link::RelayIdentity {
            device_id: "device".into(),
            device_name: "Device".into(),
            port_preview_base_url: Some("https://example.test".into()),
            public_base_url: Some("https://remote.example.test".into()),
        }));
        state.relay_connected.store(true, Ordering::SeqCst);
        crate::owner_trust::tests::live_owner(&state);
        let machine_token = state.configure_cli("http://127.0.0.1:0".into()).token;
        let thread_token = state.cli_thread_token("local-thread");
        let router = crate::http::router(state.clone());
        for (port, token) in [(4014, machine_token), (4015, thread_token)] {
            let input = json!({"operation":"previewCreate","port":port,"label":"Local CLI"});
            let request = Request::builder()
                .method("POST")
                .uri("/api/cli")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(input.to_string()))
                .unwrap();
            assert_eq!(
                router.clone().oneshot(request).await.unwrap().status(),
                StatusCode::OK
            );
        }
        assert_eq!(crate::ports::snapshot(&state).len(), 2);
        state
            .db
            .with(|c| {
                c.execute("UPDATE owner_sessions SET expires_at=0", [])?;
                Ok(())
            })
            .unwrap();
        assert_eq!(crate::ports::snapshot(&state).len(), 2);
        state
            .db
            .with(|c| {
                c.execute("DELETE FROM owner_credentials", [])?;
                Ok(())
            })
            .unwrap();
        assert!(crate::owner_trust::strict(&state));
        assert_eq!(crate::ports::snapshot(&state).len(), 2);
    }

    #[test]
    fn validates_paths_and_distinguishes_host_rejection_from_application_errors() {
        for invalid in [
            "//evil.test",
            "https://evil.test",
            "/\\evil",
            "/\r\nHost:evil",
        ] {
            assert!(path(Some(invalid)).is_err());
        }
        assert_eq!(
            path(Some("/app?tab=1#section")).unwrap(),
            "/app?tab=1#section"
        );
        assert_eq!(
            classify(403, "Blocked request. This host is not allowed."),
            "hostRejected"
        );
        assert_eq!(classify(403, "Forbidden: login required"), "httpError");
        assert_eq!(classify(200, "Documentation for allowedHosts"), "httpReady");
    }
    #[tokio::test]
    async fn reserves_stable_address_before_starting_service_without_account_secrets() {
        let (_dir, state) =
            crate::tunnel::tests::state_with_relay_url("https://remote.example.test");
        let (connection, _outbound) = crate::peer_link::tests::connected(&state, "device");
        connection.connected(Some(crate::peer_link::RelayIdentity {
            device_id: "device".into(),
            device_name: "Device".into(),
            port_preview_base_url: Some("https://example.test".into()),
            public_base_url: Some("https://remote.example.test".into()),
        }));
        state.relay_connected.store(true, Ordering::SeqCst);
        let create =
            json!({"operation":"previewCreate","port":4013,"label":"Demo","path":"/app?q=1"});
        let first = run(&state, &create).await.unwrap();
        let second = run(&state, &create).await.unwrap();
        assert_eq!(first["id"], second["id"]);
        assert_eq!(
            first["hostname"],
            format!("p-{}.example.test", first["id"].as_str().unwrap())
        );
        assert!(first["openUrl"]
            .as_str()
            .unwrap()
            .starts_with("https://remote.example.test/devices/device/ports/"));
        assert_eq!(crate::ports::snapshot(&state).len(), 1);
        assert!(!first.to_string().contains("__rc_launch"));
        assert!(run(&state, &json!({"operation":"previewCreate","port":0}))
            .await
            .is_err());
        assert!(run(
            &state,
            &json!({"operation":"previewCreate","port":state.config.port})
        )
        .await
        .is_err());
        run(&state, &json!({"operation":"previewStop","target":"4013"}))
            .await
            .unwrap();
        assert!(crate::ports::snapshot(&state).is_empty());
        state.relay_connected.store(false, Ordering::SeqCst);
        assert!(run(&state, &create).await.is_err());
    }
}
