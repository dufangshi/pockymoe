use super::*;

pub(super) struct Fixture {
    pub state: Arc<AppState>,
    pub base: String,
    pub client: reqwest::Client,
    tokens: HashMap<String, String>,
    data_dir: PathBuf,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

impl Fixture {
    pub async fn new(name: &str) -> Self {
        let (mut state, data_dir) = crate::tests::test_app_state(name);
        Arc::get_mut(&mut state).unwrap().oauth.public_base_url =
            Some("https://pockymoe.test:8443/base/".into());
        let mut tokens = HashMap::new();
        {
            let conn = state.store.conn.lock().await;
            for user in [
                "owner",
                "stranger",
                "device-guest",
                "thread-guest",
                "workspace-guest",
                "share-guest",
            ] {
                conn.execute(
                    "INSERT INTO relay_users (id,email,username,role,enabled,created_at,password_salt,password_hash)
                     VALUES (?1,?2,?1,'user',1,'now','salt','hash')",
                    params![user, format!("{user}@example.test")],
                ).unwrap();
                tokens.insert(
                    user.into(),
                    create_session(&conn, &state.store.session_secret, user).unwrap(),
                );
            }
            conn.execute("INSERT INTO relay_devices (id,owner_user_id,name,token_hash,token_preview,created_at)
                          VALUES ('device','owner','Test device','hash','test','now')", []).unwrap();
            for (user, scope, thread, workspace, workspace_access) in [
                ("device-guest", "device", None, None, "none"),
                ("thread-guest", "thread", Some("thread"), None, "none"),
                (
                    "workspace-guest",
                    "workspace",
                    None,
                    Some("workspace"),
                    "read",
                ),
            ] {
                conn.execute("INSERT INTO relay_access_grants
                    (id,owner_user_id,target_user_id,device_id,scope,thread_id,workspace_id,thread_access,workspace_access,created_at)
                    VALUES (?1,'owner',?1,'device',?2,?3,?4,'read',?5,'now')",
                    params![user, scope, thread, workspace, workspace_access]).unwrap();
            }
            conn.execute(
                "INSERT INTO relay_shares
                (id,owner_user_id,target_user_id,device_id,thread_id,thread_access,created_at)
                VALUES ('share','owner','share-guest','device','shared-thread','read','now')",
                [],
            )
            .unwrap();
        }
        let app = Router::new()
            .merge(auth_api::routes())
            .route(
                "/relay/shares/{share_id}",
                axum::routing::patch(update_share),
            )
            .route(
                "/relay/grants/{grant_id}",
                axum::routing::patch(update_grant),
            )
            .route("/relay/devices/{device_id}", delete(delete_device))
            .route("/relay/devices/{device_id}/api/{*rest}", any(device_api))
            .route("/relay/api/{*rest}", any(relay_api_compat))
            .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                security::browser_security,
            ))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            state,
            base,
            client: reqwest::Client::new(),
            tokens,
            data_dir,
            server,
        }
    }

    pub fn request(
        &self,
        method: Method,
        path: &str,
        user: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let request = self.client.request(method, format!("{}{path}", self.base));
        match user {
            Some(user) => request.bearer_auth(&self.tokens[user]),
            None => request,
        }
    }
}

#[tokio::test]
async fn browser_device_and_compat_routes_forward_owner_session_headers() {
    let fixture = Fixture::new("owner-header-routing").await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    fixture.state.sockets.write().await.insert(
        "device".into(),
        DeviceSocket {
            tx,
            connection_id: Uuid::new_v4(),
            connected_at: now_rfc3339(),
            last_heartbeat_at: now_rfc3339(),
        },
    );
    for path in [
        "/relay/devices/device/api/transport/session",
        "/relay/api/transport/session",
    ] {
        let request = fixture
            .request(Method::POST, path, Some("owner"))
            .header("x-rcd-key", "key")
            .header("x-rcd-request", "request")
            .header("x-rcd-session", "session-value")
            .header("x-rcd-signature", "signature-value")
            .header("authorization-untrusted", "filtered")
            .body("ciphertext");
        let result = tokio::spawn(async move { request.send().await.unwrap() });
        let raw = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let frame: Value = serde_json::from_str(&raw).unwrap();
        let headers = &frame["payload"]["headers"];
        assert_eq!(headers["x-rcd-session"], "session-value");
        assert_eq!(headers["x-rcd-signature"], "signature-value");
        assert_eq!(headers["x-rcd-key"], "key");
        assert!(headers.get("authorization").is_none());
        assert!(headers.get("authorization-untrusted").is_none());
        assert!(frame.get("peer").is_none());
        fixture
            .state
            .pending
            .lock()
            .unwrap()
            .remove(frame["requestId"].as_str().unwrap())
            .unwrap()
            .tx
            .send(json!({"statusCode":200,"body":"ok"}))
            .unwrap();
        assert_eq!(result.await.unwrap().status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn certificates_preserve_json_and_enforce_access_and_deletion() {
    let fixture = Fixture::new("certificate-access").await;
    let path = "/relay/devices/device/certificate";
    for user in [None, Some("owner"), Some("stranger")] {
        let status = fixture
            .request(Method::GET, path, user)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(
            status,
            if user.is_none() {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::NOT_FOUND
            }
        );
    }
    // Unknown fields, whitespace and unverified assertions are preserved by the relay.
    let raw = "{\n \"version\": 1, \"deviceId\": \"device\", \"assertion\": {\"signature\": \"unverified\"}, \"future\": [1,2] }\n";
    let response = fixture
        .request(Method::PUT, path, Some("owner"))
        .body(raw)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text().await.unwrap(), raw);
    for user in [
        "owner",
        "device-guest",
        "thread-guest",
        "workspace-guest",
        "share-guest",
    ] {
        let response = fixture
            .request(Method::GET, path, Some(user))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{user}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.text().await.unwrap(), raw);
        if user != "owner" {
            assert_eq!(
                fixture
                    .request(Method::PUT, path, Some(user))
                    .body(raw)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
    }
    assert_eq!(
        fixture
            .request(Method::GET, path, Some("stranger"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request(Method::PUT, path, None)
            .body(raw)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    {
        let conn = fixture.state.store.conn.lock().await;
        conn.execute(
            "UPDATE relay_access_grants SET revoked_at='now' WHERE id='device-guest'",
            [],
        )
        .unwrap();
        conn.execute("UPDATE relay_access_grants SET expires_at='2000-01-01T00:00:00Z' WHERE id='thread-guest'", []).unwrap();
        conn.execute(
            "UPDATE relay_access_grants SET workspace_access='none' WHERE id='workspace-guest'",
            [],
        )
        .unwrap();
        conn.execute("UPDATE relay_shares SET revoked_at='now'", [])
            .unwrap();
    }
    for user in [
        "device-guest",
        "thread-guest",
        "workspace-guest",
        "share-guest",
    ] {
        assert_eq!(
            fixture
                .request(Method::GET, path, Some(user))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND,
            "{user}"
        );
    }
    let updated = "{\"version\":1,\"deviceId\":\"device\",\"replacement\":true}";
    assert_eq!(
        fixture
            .request(Method::PUT, path, Some("owner"))
            .body(updated)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .request(Method::GET, path, Some("owner"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        updated
    );
    assert_eq!(
        fixture
            .request(Method::DELETE, "/relay/devices/device", Some("owner"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let count: i64 = fixture
        .state
        .store
        .conn
        .lock()
        .await
        .query_row(
            "SELECT count(*) FROM relay_device_certificates",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        fixture
            .request(Method::GET, path, Some("owner"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn certificate_validation_size_limit_and_browser_origin_protection() {
    let fixture = Fixture::new("certificate-validation").await;
    let path = "/relay/devices/device/certificate";
    for raw in [
        "not json",
        "[]",
        "null",
        "1",
        "{}",
        "{\"version\":2,\"deviceId\":\"device\"}",
        "{\"version\":1,\"deviceId\":\"wrong\"}",
        "{\"version\":\"1\",\"deviceId\":\"device\"}",
    ] {
        assert_eq!(
            fixture
                .request(Method::PUT, path, Some("owner"))
                .body(raw)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let prefix = "{\"version\":1,\"deviceId\":\"device\"}";
    let exact = format!("{prefix}{}", " ".repeat(16 * 1024 - prefix.len()));
    assert_eq!(
        fixture
            .request(Method::PUT, path, Some("owner"))
            .body(exact.clone())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .request(Method::PUT, path, Some("owner"))
            .body(format!("{exact} "))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        fixture
            .request(Method::GET, path, Some("owner"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        exact
    );
    // Cookie mutations need a same-origin Origin; bearer mutations reject foreign origins too.
    for origin in [
        None,
        Some("https://evil.test"),
        Some("https://pockymoe.test:8443"),
    ] {
        let request = fixture
            .request(Method::PUT, path, None)
            .header(
                header::COOKIE,
                format!("remote_codex_relay_session={}", fixture.tokens["owner"]),
            )
            .body(prefix);
        let request = match origin {
            Some(origin) => request.header(header::ORIGIN, origin),
            None => request,
        };
        assert_eq!(
            request.send().await.unwrap().status(),
            if origin == Some("https://pockymoe.test:8443") {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
    }
    assert_eq!(
        fixture
            .request(Method::PUT, path, Some("owner"))
            .header(header::ORIGIN, "https://evil.test")
            .body(prefix)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request(Method::PUT, path, Some("owner"))
            .header("sec-fetch-site", "cross-site")
            .body(prefix)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn certificates_store_interop_vectors_without_verifying_assertions() {
    let fixture = Fixture::new("certificate-interop-storage").await;
    let vectors: Value =
        serde_json::from_str(include_str!("../test-fixtures/owner-trust-v1.json")).unwrap();
    let certificate = &vectors["deviceCertificate"]["certificate"];
    let id = certificate["deviceId"].as_str().unwrap();
    fixture
        .state
        .store
        .conn
        .lock()
        .await
        .execute(
            "INSERT INTO relay_devices (id,owner_user_id,name,token_hash,token_preview,created_at)
         VALUES (?1,'owner','Vector device','vector-hash','test','now')",
            params![id],
        )
        .unwrap();
    let path = format!("/relay/devices/{id}/certificate");
    let mut certificates = vec![certificate.clone()];
    for negative in vectors["negative"].as_object().unwrap().values() {
        let mut certificate = certificate.clone();
        certificate["assertion"] = negative["assertion"].clone();
        certificates.push(certificate);
    }
    for certificate in certificates {
        let raw = serde_json::to_string_pretty(&certificate).unwrap();
        assert_eq!(
            fixture
                .request(Method::PUT, &path, Some("owner"))
                .body(raw.clone())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            fixture
                .request(Method::GET, &path, Some("owner"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            raw
        );
        let reopened = RelayStore::open(
            fixture.data_dir.join("relay-store.sqlite"),
            "test-secret".into(),
        )
        .unwrap();
        let stored: String = reopened
            .conn
            .lock()
            .await
            .query_row(
                "SELECT certificate_json FROM relay_device_certificates WHERE device_id=?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, raw);
    }
}

#[tokio::test]
async fn grantee_registration_routes_any_live_scope_without_resource_ids() {
    let fixture = Fixture::new("grantee-registration-routing").await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    fixture.state.sockets.write().await.insert(
        "device".into(),
        DeviceSocket {
            tx,
            connection_id: Uuid::new_v4(),
            connected_at: now_rfc3339(),
            last_heartbeat_at: now_rfc3339(),
        },
    );
    for prefix in ["/relay/devices/device/api", "/relay/api"] {
        for user in [
            "thread-guest",
            "workspace-guest",
            "device-guest",
            "share-guest",
        ] {
            let request = fixture
                .request(
                    Method::POST,
                    &format!("{prefix}/transport/owner-session"),
                    Some(user),
                )
                .header("x-rcd-key", "scoped-key")
                .header("x-rcd-request", "request")
                .header("x-rcd-enc", "encapsulation")
                .header("x-rcd-sealed", "ciphertext");
            let result = tokio::spawn(async move { request.send().await.unwrap() });
            let raw = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            let frame: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(frame["payload"]["path"], "/api/transport/owner-session");
            assert_eq!(frame["payload"]["method"], "POST");
            assert_eq!(frame["payload"]["headers"]["x-rcd-key"], "scoped-key");
            assert_eq!(frame["payload"]["headers"]["x-rcd-enc"], "encapsulation");
            assert_eq!(frame["payload"]["headers"]["x-rcd-sealed"], "ciphertext");
            fixture
                .state
                .pending
                .lock()
                .unwrap()
                .remove(frame["requestId"].as_str().unwrap())
                .unwrap()
                .tx
                .send(json!({"statusCode":200,"body":"{\"role\":\"grantee\"}"}))
                .unwrap();
            assert_eq!(
                result.await.unwrap().status(),
                StatusCode::OK,
                "{prefix} {user}"
            );
        }
        // The existing thread/workspace key handshake remains available.
        for (user, resource) in [
            ("thread-guest", "threads/thread"),
            ("share-guest", "threads/shared-thread"),
            ("workspace-guest", "workspaces/workspace"),
        ] {
            let request = fixture.request(
                Method::GET,
                &format!("{prefix}/{resource}/transport/key?challenge=fresh"),
                Some(user),
            );
            let result = tokio::spawn(async move { request.send().await.unwrap() });
            let raw = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            let frame: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(
                frame["payload"]["path"],
                format!("/api/{resource}/transport/key?challenge=fresh")
            );
            fixture
                .state
                .pending
                .lock()
                .unwrap()
                .remove(frame["requestId"].as_str().unwrap())
                .unwrap()
                .tx
                .send(json!({"statusCode":200,"body":"{}"}))
                .unwrap();
            assert_eq!(
                result.await.unwrap().status(),
                StatusCode::OK,
                "{prefix} {user}"
            );
        }
    }
}

#[tokio::test]
async fn registration_exception_does_not_open_other_transport_routes_or_plaintext() {
    let fixture = Fixture::new("grantee-registration-boundaries").await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    fixture.state.sockets.write().await.insert(
        "device".into(),
        DeviceSocket {
            tx,
            connection_id: Uuid::new_v4(),
            connected_at: now_rfc3339(),
            last_heartbeat_at: now_rfc3339(),
        },
    );
    for prefix in ["/relay/devices/device/api", "/relay/api"] {
        for user in [
            "thread-guest",
            "workspace-guest",
            "device-guest",
            "share-guest",
        ] {
            for (method, path) in [
                (Method::GET, "/transport/owner-session"),
                (Method::PUT, "/transport/owner-session"),
                (Method::DELETE, "/transport/owner-session"),
                (Method::POST, "/transport/owner-session/extra"),
                (Method::POST, "/transport/session"),
                (Method::POST, "/transport/pair"),
                (Method::GET, "/transport/trust"),
                (Method::GET, "/transport/grants"),
                (Method::POST, "/transport/credentials"),
                (Method::DELETE, "/transport/sessions/id"),
            ] {
                let response = fixture
                    .request(method, &format!("{prefix}{path}"), Some(user))
                    .header("x-rcd-key", "key")
                    .header("x-rcd-request", "request")
                    .header("x-rcd-enc", "enc")
                    .header("x-rcd-sealed", "ciphertext")
                    .send()
                    .await
                    .unwrap();
                assert!(!response.status().is_success(), "{prefix} {user} {path}");
                assert!(
                    rx.try_recv().is_err(),
                    "unauthorized request forwarded: {path}"
                );
            }
            // Reject plaintext and partial encryption envelopes at the relay.
            for supplied in [
                vec![],
                vec!["x-rcd-key"],
                vec!["x-rcd-key", "x-rcd-request"],
                vec!["x-rcd-request", "x-rcd-enc"],
                vec!["x-rcd-key", "x-rcd-enc"],
            ] {
                let mut request = fixture.request(
                    Method::POST,
                    &format!("{prefix}/transport/owner-session"),
                    Some(user),
                );
                for name in supplied {
                    request = request.header(name, "value");
                }
                assert!(!request
                    .body("plaintext")
                    .send()
                    .await
                    .unwrap()
                    .status()
                    .is_success());
                assert!(rx.try_recv().is_err());
            }
        }
        for user in [None, Some("stranger")] {
            let response = fixture
                .request(
                    Method::POST,
                    &format!("{prefix}/transport/owner-session"),
                    user,
                )
                .header("x-rcd-key", "key")
                .header("x-rcd-request", "request")
                .header("x-rcd-enc", "enc")
                .header("x-rcd-sealed", "ciphertext")
                .send()
                .await
                .unwrap();
            assert!(!response.status().is_success());
            assert!(rx.try_recv().is_err());
        }
    }
}

#[tokio::test]
async fn revoked_expired_disabled_and_other_device_rows_cannot_register() {
    let fixture = Fixture::new("grantee-registration-live-rows").await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    fixture.state.sockets.write().await.insert(
        "device".into(),
        DeviceSocket {
            tx,
            connection_id: Uuid::new_v4(),
            connected_at: now_rfc3339(),
            last_heartbeat_at: now_rfc3339(),
        },
    );
    for mutation in ["revoked", "expired", "other-device", "disabled"] {
        {
            let conn = fixture.state.store.conn.lock().await;
            conn.execute(
                "UPDATE relay_shares SET revoked_at=NULL,expires_at=NULL,device_id='device'",
                [],
            )
            .unwrap();
            conn.execute(
                "UPDATE relay_access_grants SET revoked_at=NULL,expires_at=NULL,device_id='device'",
                [],
            )
            .unwrap();
            conn.execute("UPDATE relay_users SET enabled=1", [])
                .unwrap();
            match mutation {
                "revoked" => {
                    conn.execute("UPDATE relay_shares SET revoked_at='now'", [])
                        .unwrap();
                    conn.execute("UPDATE relay_access_grants SET revoked_at='now'", [])
                        .unwrap();
                }
                "expired" => {
                    conn.execute(
                        "UPDATE relay_shares SET expires_at='2000-01-01T00:00:00Z'",
                        [],
                    )
                    .unwrap();
                    conn.execute(
                        "UPDATE relay_access_grants SET expires_at='2000-01-01T00:00:00Z'",
                        [],
                    )
                    .unwrap();
                }
                "other-device" => {
                    conn.execute("INSERT OR IGNORE INTO relay_devices(id,owner_user_id,name,token_hash,token_preview,created_at) VALUES('other-device','owner','Other device','other-hash','test','now')",[]).unwrap();
                    conn.execute("UPDATE relay_shares SET device_id='other-device'", [])
                        .unwrap();
                    conn.execute(
                        "UPDATE relay_access_grants SET device_id='other-device'",
                        [],
                    )
                    .unwrap();
                }
                _ => {
                    conn.execute("UPDATE relay_users SET enabled=0 WHERE id<>'owner'", [])
                        .unwrap();
                }
            }
        }
        // Explicit target route cannot borrow another device's live rows.
        for user in [
            "thread-guest",
            "workspace-guest",
            "device-guest",
            "share-guest",
        ] {
            let response = fixture
                .request(
                    Method::POST,
                    "/relay/devices/device/api/transport/owner-session",
                    Some(user),
                )
                .header("x-rcd-key", "key")
                .header("x-rcd-request", "request")
                .header("x-rcd-enc", "enc")
                .header("x-rcd-sealed", "ciphertext")
                .send()
                .await
                .unwrap();
            assert!(!response.status().is_success(), "{mutation} {user}");
            assert!(rx.try_recv().is_err());
        }
    }
}

#[tokio::test]
async fn sharing_patch_can_restore_null_values_after_failed_device_edits() {
    let fixture = Fixture::new("sharing-null-rollback").await;
    for (table, path, id) in [
        ("relay_shares", "/relay/shares/share", "share"),
        (
            "relay_access_grants",
            "/relay/grants/device-guest",
            "device-guest",
        ),
    ] {
        {
            let conn = fixture.state.store.conn.lock().await;
            conn.execute(&format!("UPDATE {table} SET label='New label',expires_at='2030-01-01T00:00:00Z' WHERE id=?1"), params![id]).unwrap();
        }
        assert_eq!(
            fixture
                .request(Method::PATCH, path, Some("owner"))
                .json(&json!({"threadAccess":"control"}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        {
            let conn = fixture.state.store.conn.lock().await;
            let values: (Option<String>, Option<String>) = conn
                .query_row(
                    &format!("SELECT label,expires_at FROM {table} WHERE id=?1"),
                    params![id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(
                values,
                (
                    Some("New label".into()),
                    Some("2030-01-01T00:00:00Z".into())
                )
            );
        }
        assert_eq!(
            fixture
                .request(Method::PATCH, path, Some("owner"))
                .json(&json!({"label":null,"expiresAt":null,"threadAccess":"read"}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let conn = fixture.state.store.conn.lock().await;
        let values: (Option<String>, Option<String>, String) = conn
            .query_row(
                &format!("SELECT label,expires_at,thread_access FROM {table} WHERE id=?1"),
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(values, (None, None, "read".into()));
    }
    {
        let conn = fixture.state.store.conn.lock().await;
        conn.execute("UPDATE relay_shares SET workspace_id='new',workspace_label='New',thread_title='New' WHERE id='share'", []).unwrap();
    }
    assert_eq!(
        fixture
            .request(Method::PATCH, "/relay/shares/share", Some("owner"))
            .json(&json!({"workspaceId":null,"workspaceLabel":null,"threadTitle":null}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let conn = fixture.state.store.conn.lock().await;
    let values: (Option<String>, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT workspace_id,workspace_label,thread_title FROM relay_shares WHERE id='share'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(values, (None, None, None));
}
