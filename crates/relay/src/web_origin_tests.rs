use super::*;

struct WebFixture {
    base: String,
    origin: String,
    cookie: String,
    client: reqwest::Client,
    task: tokio::task::JoinHandle<()>,
    dir: PathBuf,
}
impl Drop for WebFixture {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
impl WebFixture {
    async fn new(separate: bool) -> Self {
        let (mut state, dir) = tests::test_app_state("web-origin");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let origin = if separate {
            "http://localhost:4188".to_owned()
        } else {
            base.clone()
        };
        let mutable = Arc::get_mut(&mut state).unwrap();
        mutable.oauth.public_base_url = Some(base.clone());
        mutable.oauth.web_origin = separate.then(|| origin.clone());
        let dist = dir.join("web");
        std::fs::create_dir_all(dist.join("assets")).unwrap();
        std::fs::write(
            dist.join("index.html"),
            "<html><head></head><body>Web</body></html>",
        )
        .unwrap();
        std::fs::write(dist.join("assets/relayServiceWorker-test.js"), "// worker").unwrap();
        mutable.web_dist = Some(dist);
        let cookie = {
            let conn = state.store.conn.lock().await;
            conn.execute("INSERT INTO relay_users (id,email,username,role,enabled,created_at,password_salt,password_hash) VALUES ('owner','owner@example.test','owner','user',1,'now','salt','hash')", []).unwrap();
            account::store_vault(
                &conn,
                "owner",
                &json!({"revision":1,"accountKeyId":"test-key","accountPublicKey":"test-public"}),
            )
            .unwrap();
            let token = create_session(&conn, &state.store.session_secret, "owner").unwrap();
            account::mark_session(&conn, &token, account::FULL).unwrap();
            format!("remote_codex_relay_session={token}")
        };
        async fn echo(headers: HeaderMap) -> Response {
            let mut response =
                Json(json!({"cookie": relay_cookie(&headers).is_some()})).into_response();
            response
                .headers_mut()
                .insert("x-rcd-encrypted", "1".parse().unwrap());
            response
                .headers_mut()
                .insert("content-range", "bytes 0-1/2".parse().unwrap());
            // Simulate a device trying to widen CORS.
            response
                .headers_mut()
                .insert("access-control-allow-origin", "*".parse().unwrap());
            response
                .headers_mut()
                .insert("service-worker-allowed", "/".parse().unwrap());
            response
        }
        let app = Router::new()
            .merge(auth_api::routes())
            .route("/relay/auth/session", get(session))
            .route("/relay/probe", get(echo).post(echo))
            .route("/relay/ws-probe", get(echo))
            .route("/relay/assets/relayServiceWorker-probe.js", get(echo))
            .route("/relay/auth/oauth/{provider}/callback", get(oauth_callback))
            .fallback(spa_fallback)
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                security::browser_security,
            ))
            .with_state(state);
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            base,
            origin,
            cookie,
            task,
            dir,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
        }
    }
    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .header("origin", &self.origin)
    }
}

#[tokio::test]
async fn cors_and_cookie_auth_work_in_both_origin_modes() {
    for separate in [false, true] {
        let f = WebFixture::new(separate).await;
        let response = f.request(Method::OPTIONS, "/relay/probe")
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "content-type, accept, range, x-rcd-key, x-rcd-request, x-rcd-session, x-rcd-signature, x-remote-codex-auth-realm")
            .send().await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()["access-control-allow-origin"], f.origin);
        assert_eq!(
            response.headers()["access-control-allow-credentials"],
            "true"
        );
        let vary = response
            .headers()
            .get_all("vary")
            .iter()
            .map(|h| h.to_str().unwrap())
            .collect::<Vec<_>>()
            .join(",");
        assert!(vary.contains("Origin") && vary.contains("Access-Control-Request-Headers"));
        for method in [Method::GET, Method::POST] {
            let response = f
                .request(method, "/relay/probe")
                .header("cookie", &f.cookie)
                .header(
                    "sec-fetch-site",
                    if separate { "same-site" } else { "same-origin" },
                )
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["access-control-allow-origin"], f.origin);
            let expose = response.headers()["access-control-expose-headers"]
                .to_str()
                .unwrap();
            assert!(
                expose.contains("x-rcd-encrypted")
                    && expose.contains("content-disposition")
                    && expose.contains("content-range")
            );
            assert_eq!(response.json::<Value>().await.unwrap()["cookie"], true);
        }
        let session = f
            .request(Method::GET, "/relay/auth/session")
            .header("cookie", &f.cookie)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(session["user"]["id"], "owner");
    }
}

#[tokio::test]
async fn cors_and_websocket_origin_allowlist_cannot_be_widened() {
    for separate in [false, true] {
        let f = WebFixture::new(separate).await;
        let denied = f
            .client
            .post(format!("{}/relay/probe", f.base))
            .header("origin", "https://evil.example.test")
            .header("cookie", &f.cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert!(!denied.headers().contains_key("access-control-allow-origin"));
        for headers in [
            ("https://evil.example.test", "POST", "content-type"),
            (f.origin.as_str(), "TRACE", "content-type"),
            (f.origin.as_str(), "POST", "x-unapproved"),
        ] {
            let response = f
                .client
                .request(Method::OPTIONS, format!("{}/relay/probe", f.base))
                .header("origin", headers.0)
                .header("access-control-request-method", headers.1)
                .header("access-control-request-headers", headers.2)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        for origin in [&f.origin, &"https://evil.example.test".to_owned()] {
            let response = f
                .client
                .get(format!("{}/relay/ws-probe", f.base))
                .header("upgrade", "websocket")
                .header("origin", origin)
                .send()
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if origin == &f.origin {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                }
            );
        }
        if separate {
            let relay_origin = f
                .client
                .post(format!("{}/relay/probe", f.base))
                .header("origin", &f.base)
                .send()
                .await
                .unwrap();
            assert_eq!(relay_origin.status(), StatusCode::FORBIDDEN);
            assert!(!relay_origin
                .headers()
                .contains_key("access-control-allow-origin"));
        }
        let no_origin = f
            .client
            .post(format!("{}/relay/probe", f.base))
            .header("cookie", &f.cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(no_origin.status(), StatusCode::FORBIDDEN);
        let get = f
            .client
            .get(format!("{}/relay/probe", f.base))
            .header("origin", "https://evil.example.test")
            .send()
            .await
            .unwrap();
        assert!(!get.headers().contains_key("access-control-allow-origin"));
    }
}

#[tokio::test]
async fn api_only_redirects_all_web_assets_and_oauth_returns_to_web() {
    for separate in [false, true] {
        let f = WebFixture::new(separate).await;
        for path in [
            "/",
            "/devices/d/threads/t?view=all",
            "/setup.sh",
            "/setup.ps1",
            "/api/config/runtime",
        ] {
            let response = f.request(Method::GET, path).send().await.unwrap();
            if separate {
                assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
                assert_eq!(
                    response.headers()["location"],
                    format!("{}{path}", f.origin)
                );
                assert!(!response.headers().contains_key("service-worker-allowed"));
                assert!(response.text().await.unwrap().is_empty());
            } else {
                assert_eq!(response.status(), StatusCode::OK);
                if path.starts_with("/assets/") {
                    assert_eq!(response.headers()["service-worker-allowed"], "/");
                }
                if path == "/" {
                    assert!(response.text().await.unwrap().contains(RELAY_BOOTSTRAP));
                }
            }
        }
        assert_eq!(
            f.request(Method::GET, "/relay/missing")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        if separate {
            assert_eq!(
                f.request(Method::GET, "/relay")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
        let response = f
            .request(
                Method::GET,
                "/relay/auth/oauth/invalid/callback?code=x&state=x",
            )
            .send()
            .await
            .unwrap();
        let location = response.headers()["location"].to_str().unwrap();
        assert!(location.starts_with(&if separate {
            format!("{}/relay-portal?oauthCode=", f.origin)
        } else {
            "/relay-portal?oauthCode=".to_owned()
        }));
        assert!(location.contains("&oauthError="), "{location}");
    }
}

#[test]
fn web_origin_validation_and_callback_addressing_are_separate() {
    for origin in [
        "https://web.example.test",
        "http://localhost:4188",
        "http://127.0.0.1:4188",
        "http://[::1]:4188",
    ] {
        assert!(oauth::validate_web_origin(origin).is_ok());
    }
    for origin in [
        "http://web.example.test",
        "https://web.example.test/path",
        "https://user@web.example.test",
        "https://web.example.test?query=1",
        "https://web.example.test#fragment",
        "null",
    ] {
        assert!(oauth::validate_web_origin(origin).is_err());
    }
    let mut config = OAuthConfig::default();
    config.public_base_url = Some("https://relay.example.test".into());
    config.web_origin = Some("https://web.example.test".into());
    assert_eq!(
        config.browser_origin().as_deref(),
        Some("https://web.example.test")
    );
    assert_eq!(
        config.callback_url(&HeaderMap::new(), OAuthProvider::Google),
        "https://relay.example.test/relay/auth/oauth/google/callback"
    );
}

#[tokio::test]
async fn api_only_retires_old_workers_and_preserves_their_original_scope() {
    let f = WebFixture::new(true).await;
    for path in [
        "/assets/relayServiceWorker-oldHash.js",
        "/service-worker.js",
        "/relayServiceWorker.js",
    ] {
        for method in [Method::GET, Method::HEAD] {
            let response = f
                .client
                .request(method.clone(), format!("{}{path}", f.base))
                .header("service-worker", "script")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "application/javascript; charset=utf-8"
            );
            if path.starts_with("/assets/") {
                assert_eq!(response.headers()["service-worker-allowed"], "/");
            } else {
                assert!(!response.headers().contains_key("service-worker-allowed"));
            }
            assert!(!response.headers().contains_key(header::LOCATION));
            let body = response.text().await.unwrap();
            if method == Method::HEAD {
                assert!(body.is_empty());
            } else {
                assert_eq!(body, include_str!("retired_service_worker.js"));
                assert!(body.contains("self.skipWaiting()"));
                assert!(body.contains("self.registration.unregister()"));
                assert!(body.contains("client.navigate(client.url)"));
                assert!(!body.contains("'fetch'"));
            }
        }
    }
    let same = WebFixture::new(false).await;
    let response = same
        .request(Method::GET, "/assets/relayServiceWorker-test.js")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["service-worker-allowed"], "/");
    assert_eq!(response.text().await.unwrap(), "// worker");
    for path in [
        "/assets/relayServiceWorker-oldHash.js/map.js",
        "/assets/index-old.js",
    ] {
        assert_eq!(
            f.request(Method::GET, path).send().await.unwrap().status(),
            StatusCode::TEMPORARY_REDIRECT
        );
    }
}

#[tokio::test]
async fn api_only_other_responses_cannot_grant_service_worker_scope() {
    let f = WebFixture::new(true).await;
    for path in [
        "/relay/probe",
        "/relay/assets/relayServiceWorker-probe.js",
        "/relay/missing",
        "/",
        "/assets/index-old.js",
        "/assets/relayServiceWorker-oldHash.js/map.js",
        "/service-worker.js",
        "/relayServiceWorker.js",
    ] {
        for method in [Method::GET, Method::HEAD, Method::POST] {
            let response = f.request(method, path).send().await.unwrap();
            assert!(
                !response.headers().contains_key("service-worker-allowed"),
                "scope leaked on {path}"
            );
        }
    }
}

#[tokio::test]
async fn security_revision_same_origin_csp_allows_only_wasm_evaluation() {
    let f = WebFixture::new(false).await;
    let response = f.request(Method::GET, "/").send().await.unwrap();
    let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(csp.contains("script-src 'self' 'wasm-unsafe-eval' 'sha256-"));
    assert!(!csp.contains("'unsafe-eval'"));
    assert!(csp.contains("connect-src 'self';"));
}
