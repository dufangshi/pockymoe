use super::*;

#[tokio::test]
async fn hub_app_page_is_read_only_and_never_exposes_thread_data() {
    let f = fixture().await;
    let h = hello(&f, "ag-private");
    let (_socket, bound) = attach_client(&f, &h).await;
    let thread = bound["threadId"].as_str().unwrap();
    let client = reqwest::Client::new();
    for (method, path, status, html) in [
        ("GET", "/".to_owned(), 200, true),
        ("GET", "/index.html".to_owned(), 200, true),
        ("GET", "/api/status".to_owned(), 200, false),
        ("GET", "/v1/status".to_owned(), 200, false),
        ("GET", "/healthz".to_owned(), 200, false),
        ("GET", "/api/threads".to_owned(), 403, false),
        ("GET", format!("/api/threads/{thread}"), 403, false),
        ("GET", format!("/threads/{thread}"), 403, false),
        ("GET", "/api/runtime/config".to_owned(), 403, false),
        ("GET", "/api/management/supervisor".to_owned(), 403, false),
        ("POST", "/api/threads".to_owned(), 403, false),
        ("POST", "/".to_owned(), 403, false),
        ("GET", TREER_ATTACH_PATH.to_owned(), 403, false),
        ("GET", "/api/events".to_owned(), 403, false),
    ] {
        let response = client
            .request(
                method.parse().unwrap(),
                format!("http://127.0.0.1:{}{path}", f.port),
            )
            .header("x-treer-identity-token", "synthetic-identity-token")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status, "{method} {path}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response.headers()["content-type"],
            if html {
                "text/html; charset=utf-8"
            } else {
                "application/json"
            }
        );
        let body = response.text().await.unwrap();
        assert!(body.contains("Use a Pockymoe Agent to open conversations"));
        assert!(body.contains("Stopping this App takes every Pockymoe Agent on this machine offline; they reconnect when it starts again"));
        for private in [
            thread,
            "Treer test",
            "ag-private",
            "synthetic-identity-token",
            &f.hub.token,
        ] {
            assert!(!body.contains(private), "{path} leaked private data");
        }
        if !html {
            let status: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(status["boundAgents"], 1);
            assert_eq!(status["runningVersion"], "test");
            assert_eq!(status["appName"], "pockymoe");
            assert!(status["machine"].is_string());
        }
    }
}

#[tokio::test]
async fn hub_app_page_rejects_mixed_markers_duplicate_tokens_and_remote_transport() {
    use tower::ServiceExt;
    let f = fixture().await;
    let app = crate::http::router_with_treer(f.hub.state.clone(), Some(f.hub.clone()));
    for (peer, extra) in [
        (None, None),
        (Some("192.0.2.1:1234"), None),
        (
            Some("127.0.0.1:1234"),
            Some(("x-treer-interface-source", "proxy-ui")),
        ),
        (
            Some("127.0.0.1:1234"),
            Some(("x-treer-agent-id", "fake-agent")),
        ),
        (
            Some("127.0.0.1:1234"),
            Some(("x_treer_interface_source", "proxy-ui")),
        ),
        (
            Some("127.0.0.1:1234"),
            Some(("x-treer-identity-token", "duplicate")),
        ),
    ] {
        let mut req = axum::http::Request::builder()
            .uri("/")
            .header("x-treer-identity-token", "synthetic-token");
        if let Some((key, value)) = extra {
            req = req.header(key, value);
        }
        let mut req = req.body(axum::body::Body::empty()).unwrap();
        if let Some(peer) = peer {
            req.extensions_mut()
                .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        }
        assert_eq!(app.clone().oneshot(req).await.unwrap().status(), 403);
    }
}

#[tokio::test]
async fn hub_app_html_escapes_machine() {
    let f = fixture().await;
    f.hub
        .state
        .db
        .with(|c| {
            c.execute(
                "UPDATE hosts SET hostname=?1",
                ["<script>alert('machine')</script>"],
            )?;
            Ok(())
        })
        .unwrap();
    let body = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/", f.port))
        .header("x-treer-identity-token", "synthetic-token")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("&lt;script&gt;alert(&#39;machine&#39;)&lt;/script&gt;"));
    assert!(!body.contains("<script>"));
}
