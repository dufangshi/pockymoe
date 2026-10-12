use super::*;
use account::FULL;

fn vectors() -> Value {
    serde_json::from_str(include_str!("../test-fixtures/account-key-v2.json")).unwrap()
}
fn test_vault(user: &str, revision: u64) -> Value {
    let mut v = vectors()["vault"]["vault"].clone();
    v["userId"] = json!(user);
    v["revision"] = json!(revision);
    v
}
fn auth_key() -> String {
    vectors()["derivation"]["authKey"].as_str().unwrap().into()
}
fn recovery_key() -> String {
    vectors()["derivation"]["recoveryAuthKey"]
        .as_str()
        .unwrap()
        .into()
}
const PASSWORD: &str = "synthetic legacy test password";

struct Fixture {
    state: Arc<AppState>,
    base: String,
    client: reqwest::Client,
    task: tokio::task::JoinHandle<()>,
    dir: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
impl Fixture {
    async fn new(name: &str) -> Self {
        let (mut state, dir) = tests::test_app_state(name);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        Arc::get_mut(&mut state).unwrap().oauth.public_base_url = Some(base.clone());
        {
            let conn = state.store.conn.lock().await;
            let (salt, hash) = hash_password(PASSWORD).unwrap();
            for (id, role) in [("legacy", "user"), ("admin", "admin")] {
                conn.execute("INSERT INTO relay_users(id,email,username,role,enabled,created_at,password_salt,password_hash) VALUES (?1,?2,?1,?3,1,'now',?4,?5)",params![id,format!("{id}@example.test"),role,salt,hash]).unwrap();
            }
            for id in ["unbound", "protected"] {
                conn.execute("INSERT INTO relay_devices(id,owner_user_id,name,token_hash,token_preview,created_at) VALUES (?1,'legacy',?1,?2,'test','now')",params![id,hash_device_token(&format!("test-{id}"))]).unwrap();
            }
            conn.execute(
                "INSERT INTO relay_device_certificates VALUES ('protected','{}')",
                [],
            )
            .unwrap();
        }
        let app = Router::new()
            .merge(auth_api::routes())
            .merge(preview::routes())
            .route("/relay/auth/register", post(register))
            .route("/relay/auth/login", post(login))
            .route("/relay/auth/logout", post(logout))
            .route("/relay/auth/session", get(session))
            .route("/relay/account/password", patch(update_account_password))
            .route("/relay/portal", get(portal))
            .route("/relay/devices", get(list_devices).post(create_device))
            .route("/relay/devices/{device_id}/api/{*rest}", any(device_api))
            .route("/relay/api/{*rest}", any(relay_api_compat))
            .route("/relay/devices/{device_id}/ws", get(client_ws))
            .route(
                "/relay/admin/users/{user_id}/reset-password",
                post(admin_reset_password),
            )
            .route(
                "/relay/admin/registrations/{request_id}/approve",
                post(approve_registration),
            )
            .route("/supervisor/tunnel", get(supervisor_tunnel))
            .fallback(spa_fallback)
            .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                security::browser_security,
            ))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            state,
            base,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            task,
            dir,
        }
    }
    fn req(&self, method: Method, path: &str, token: Option<&str>) -> reqwest::RequestBuilder {
        let req = self.client.request(method, format!("{}{path}", self.base));
        match token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }
    async fn json(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let r = self
            .req(method, path, token)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = r.status();
        let value = r.json().await.unwrap_or(Value::Null);
        (status, value)
    }
    async fn token(&self, user: &str, mode: &str) -> String {
        let conn = self.state.store.conn.lock().await;
        let token = create_session(&conn, &self.state.store.session_secret, user).unwrap();
        account::mark_session(&conn, &token, mode).unwrap();
        token
    }
    async fn register(&self) -> String {
        let (s,v) = self.json(Method::POST,"/relay/auth/register",None,json!({"username":"vector-user","email":"vector@example.test","authKey":auth_key(),"recoveryAuthKey":recovery_key(),"vault":test_vault("user-123",1)})).await;
        assert_eq!(s, StatusCode::OK);
        v["token"].as_str().unwrap().into()
    }
    async fn setup(&self) -> String {
        let (s, v) = self
            .json(
                Method::POST,
                "/relay/auth/login",
                None,
                json!({"identifier":"legacy","password":PASSWORD}),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["session"]["accountSetupRequired"], true);
        let token = v["token"].as_str().unwrap();
        let (s,_) = self.json(Method::PUT,"/relay/account/vault",Some(token),json!({"vault":test_vault("legacy",1),"expectedRevision":0,"authKey":auth_key(),"recoveryAuthKey":recovery_key()})).await;
        assert_eq!(s, StatusCode::OK);
        token.into()
    }
}

#[tokio::test]
async fn kdf_is_deterministic_secret_bound_and_preserves_legacy_exception() {
    let f = Fixture::new("account-kdf").await;
    let get = |login: &str| {
        f.req(Method::GET, &format!("/relay/auth/kdf?login={login}"), None)
            .send()
    };
    let a = get("missing").await.unwrap();
    assert_eq!(a.headers()[header::CACHE_CONTROL], "no-store");
    let a: Value = a.json().await.unwrap();
    let b: Value = get("MISSING").await.unwrap().json().await.unwrap();
    let c: Value = get("other").await.unwrap().json().await.unwrap();
    assert_eq!(a, b);
    assert_ne!(a["kdf"]["salt"], c["kdf"]["salt"]);
    assert_eq!(a["mode"], "account");
    assert_eq!(a["kdf"]["memoryKiB"], 65536);
    assert_eq!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(a["kdf"]["salt"].as_str().unwrap())
            .unwrap()
            .len(),
        16
    );
    let legacy: Value = get("legacy").await.unwrap().json().await.unwrap();
    assert_eq!(legacy["mode"], "legacy");
    f.register().await;
    let known: Value = get("vector-user").await.unwrap().json().await.unwrap();
    assert_eq!(known["mode"], "account");
    assert_eq!(known["kdf"], test_vault("user-123", 1)["kdf"]);
}

#[tokio::test]
async fn legacy_sessions_are_restricted_until_setup_then_auth_key_login_only() {
    let f = Fixture::new("account-legacy").await;
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"username":"legacy","password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let token = v["token"].as_str().unwrap();
    for path in [
        "/relay/portal",
        "/relay/account/security",
        "/relay/devices/protected/api/workspaces",
        "/relay/shares/x/recipient-account",
    ] {
        assert_eq!(
            f.req(Method::GET, path, Some(token))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    // Both bearer and query-token compatibility paths obey the restriction.
    assert_eq!(
        f.req(
            Method::GET,
            &format!("/relay/api/workspaces?deviceId=protected&relaySession={token}"),
            None
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_ne!(
        f.req(
            Method::GET,
            "/relay/devices/unbound/api/workspaces",
            Some(token)
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.req(Method::GET, "/relay/account/vault", Some(token))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let mut request = json!({"vault":test_vault("legacy",1),"expectedRevision":0,"authKey":auth_key(),"recoveryAuthKey":recovery_key()});
    let ordinary = f.token("legacy", FULL).await;
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&ordinary),
            request.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    request["authKey"] = Value::Null;
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(token),
            request.clone()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    request["authKey"] = json!(auth_key());
    assert_eq!(
        f.json(Method::PUT, "/relay/account/vault", Some(token), request)
            .await
            .0,
        StatusCode::OK
    );
    let session: Value = f
        .req(Method::GET, "/relay/auth/session", Some(token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["accountSetupRequired"], false);
    assert_eq!(
        f.req(Method::GET, "/relay/portal", Some(token))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"legacy","password":PASSWORD})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"legacy","authKey":auth_key()})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.json(
            Method::PATCH,
            "/relay/account/password",
            Some(token),
            json!({"currentPassword":PASSWORD,"newPassword":"unwanted password"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn registration_requires_vault_and_derived_keys_and_admin_login_stays_plaintext() {
    let f = Fixture::new("account-registration").await;
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/register",
            None,
            json!({"username":"plain","email":"plain@example.test","password":PASSWORD})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let token = f.register().await;
    assert_eq!(
        f.req(Method::GET, "/relay/account/vault", Some(&token))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"vector-user","password":auth_key()})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"vector-user","authKey":auth_key()})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"admin","password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["session"]["accountSetupRequired"], false);
}

#[tokio::test]
async fn vault_cas_immutable_identity_limits_and_slot_authentication() {
    let f = Fixture::new("account-vault-rules").await;
    let token = f.register().await;
    let mut v = test_vault("user-123", 2);
    let body = |v: Value| json!({"vault":v,"expectedRevision":1});
    let (s, _) = f
        .json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            body(v.clone()),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            body(v.clone())
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    v["revision"] = json!(3);
    for (field, value) in [
        ("userId", json!("other")),
        ("accountKeyId", json!("other")),
        (
            "accountPublicKey",
            vectors()["otherAccount"]["accountPublicKey"].clone(),
        ),
    ] {
        let mut bad = v.clone();
        bad[field] = value;
        assert_eq!(
            f.json(
                Method::PUT,
                "/relay/account/vault",
                Some(&token),
                json!({"vault":bad,"expectedRevision":2})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut weak = v.clone();
    weak["kdf"]["memoryKiB"] = json!(1024);
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            json!({"vault":weak,"expectedRevision":2})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut huge = v.clone();
    huge["body"]["ciphertext"] = json!("A".repeat(65536));
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            json!({"vault":huge,"expectedRevision":2})
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    v["slots"]["password"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    let mut req = json!({"vault":v,"expectedRevision":2,"authKey":recovery_key()});
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            req.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    req["currentAuthKey"] = json!(auth_key());
    assert_eq!(
        f.json(Method::PUT, "/relay/account/vault", Some(&token), req)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"vector-user","authKey":auth_key()})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"vector-user","authKey":recovery_key()})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn recovery_sessions_only_read_vault_and_replace_password_and_key() {
    let f = Fixture::new("account-recovery").await;
    f.register().await;
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/recover",
            None,
            json!({"login":"vector-user","recoveryAuthKey":auth_key()})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/recover",
            None,
            json!({"login":"vector-user","recoveryAuthKey":recovery_key()}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["session"]["recoverySession"], true);
    let token = v["token"].as_str().unwrap();
    assert_eq!(
        f.req(Method::GET, "/relay/account/vault", Some(token))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    for path in [
        "/relay/portal",
        "/relay/devices",
        "/relay/account/browsers",
        "/relay/devices/unbound/api/workspaces",
    ] {
        assert_eq!(
            f.req(Method::GET, path, Some(token))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let mut vault = test_vault("user-123", 2);
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(token),
            json!({"vault":vault,"expectedRevision":1})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    vault["slots"]["password"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(token),
            json!({"vault":vault,"expectedRevision":1,"authKey":recovery_key()})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.req(Method::GET, "/relay/portal", Some(token))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn setup_and_recovery_mode_survive_second_factor_challenges() {
    let f = Fixture::new("account-second-factor").await;
    f.setup().await;
    let codes = {
        let conn = f.state.store.conn.lock().await;
        conn.execute("INSERT INTO relay_totp(user_id,secret,enabled,expires_at) VALUES ('legacy','unused',1,0)",[]).unwrap();
        auth_factors::recovery_codes(&conn, "legacy").unwrap()
    };
    // Existing second-factor backup codes are intentionally separate from the vault recovery key.
    let response = f
        .req(Method::POST, "/relay/auth/recover", None)
        .json(&json!({"login":"legacy","recoveryAuthKey":recovery_key()}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|h| {
            h.to_str()
                .ok()
                .filter(|s| s.starts_with("remote_codex_factor_challenge="))
                .map(|s| s.split(';').next().unwrap().to_owned())
        })
        .unwrap();
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["challengeRequired"], true);
    let response = f
        .req(Method::POST, "/relay/auth/challenge", None)
        .header(header::COOKIE, &cookie)
        .header(header::ORIGIN, &f.base)
        .json(&json!({"code":codes[0]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["session"]["recoverySession"], true);
    let token = value["token"].as_str().unwrap();
    assert_eq!(
        f.req(Method::GET, "/relay/account/browsers", Some(token))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn browser_registry_checks_vectors_labels_and_permanent_revocations() {
    let f = Fixture::new("account-browsers").await;
    let token = f.register().await;
    let vectors = vectors();
    let cert = &vectors["browserCertificate"]["certificate"];
    for name in [
        "browserCertSignedByOtherAccount",
        "browserCertWrongBrowserId",
        "browserCertTamperedCreatedAt",
    ] {
        assert_eq!(
            f.json(
                Method::POST,
                "/relay/account/browsers",
                Some(&token),
                json!({"certificate":vectors["negative"][name],"label":"Test browser"})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "{name}"
        );
    }
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/browsers",
            Some(&token),
            json!({"certificate":cert,"label":"x".repeat(81)})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    for label in ["Browser one", "Renamed browser"] {
        assert_eq!(
            f.json(
                Method::POST,
                "/relay/account/browsers",
                Some(&token),
                json!({"certificate":cert,"label":label})
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let value: Value = f
        .req(Method::GET, "/relay/account/browsers", Some(&token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(value["browsers"].as_array().unwrap().len(), 1);
    assert_eq!(value["browsers"][0]["label"], "Renamed browser");
    assert_eq!(value["browsers"][0]["certificate"], *cert);
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/revocations",
            Some(&token),
            json!({"revocation":vectors["negative"]["revocationWrongKind"]})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    for _ in 0..2 {
        assert_eq!(
            f.json(
                Method::POST,
                "/relay/account/revocations",
                Some(&token),
                json!({"revocation":vectors["revocations"]["browser"]})
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/browsers",
            Some(&token),
            json!({"certificate":cert,"label":"Re-register"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let value: Value = f
        .req(Method::GET, "/relay/account/browsers", Some(&token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(value["revocations"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn revocations_broadcast_only_to_owners_devices_and_on_every_connect() {
    use tokio_tungstenite::connect_async;
    let f = Fixture::new("account-revocations").await;
    let token = f.register().await;
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "UPDATE relay_devices SET owner_user_id='user-123' WHERE id='unbound'",
            [],
        )
        .unwrap();
    }
    let connect = || {
        connect_async(
            f.base.replacen("http", "ws", 1) + "/supervisor/tunnel?deviceToken=test-unbound",
        )
    };
    let (mut socket, _) = connect().await.unwrap();
    let read = |m: tokio_tungstenite::tungstenite::Message| {
        serde_json::from_str::<Value>(m.to_text().unwrap()).unwrap()
    };
    assert_eq!(
        read(socket.next().await.unwrap().unwrap())["type"],
        "relay.connected"
    );
    let first = read(socket.next().await.unwrap().unwrap());
    assert_eq!(first, json!({"type":"relay.revocations","revocations":[]}));
    let (tx, mut other_rx) = tokio::sync::mpsc::channel(4);
    f.state.sockets.write().await.insert(
        "protected".into(),
        DeviceSocket {
            tx,
            connection_id: Uuid::new_v4(),
            connected_at: now_rfc3339(),
            last_heartbeat_at: now_rfc3339(),
        },
    );
    let revocation = vectors()["revocations"]["browser"].clone();
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/revocations",
            Some(&token),
            json!({"revocation":revocation})
        )
        .await
        .0,
        StatusCode::OK
    );
    let pushed = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(read(pushed)["revocations"], json!([revocation]));
    assert!(other_rx.try_recv().is_err());
    socket.close(None).await.unwrap();
    let (mut again, _) = connect().await.unwrap();
    again.next().await.unwrap().unwrap();
    assert_eq!(
        read(again.next().await.unwrap().unwrap())["revocations"],
        json!([revocation])
    );
    again.close(None).await.unwrap();
}

#[tokio::test]
async fn owner_and_recipient_account_visibility_and_retired_endpoints() {
    let f = trust_tests::Fixture::new("account-directory").await;
    {
        let conn = f.state.store.conn.lock().await;
        for user in ["owner", "share-guest", "device-guest"] {
            account::store_vault(&conn, user, &test_vault(user, 1)).unwrap();
        }
    }
    for user in [
        "owner",
        "share-guest",
        "device-guest",
        "thread-guest",
        "workspace-guest",
    ] {
        assert_eq!(
            f.request(
                Method::GET,
                "/relay/devices/device/owner-account",
                Some(user)
            )
            .send()
            .await
            .unwrap()
            .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        f.request(
            Method::GET,
            "/relay/devices/device/owner-account",
            Some("stranger")
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::NOT_FOUND
    );
    for (path, target) in [
        ("/relay/shares/share/recipient-account", "share-guest"),
        (
            "/relay/grants/device-guest/recipient-account",
            "device-guest",
        ),
    ] {
        let r = f
            .request(Method::GET, path, Some("owner"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v: Value = r.json().await.unwrap();
        assert_eq!(v.as_object().unwrap().len(), 2);
        assert_eq!(
            f.request(Method::GET, path, Some(target))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    let r = f
        .request(
            Method::GET,
            "/relay/grants/thread-guest/recipient-account",
            Some("owner"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        r.json::<Value>().await.unwrap()["code"],
        "recipient_account_missing"
    );
    for path in [
        "/relay/account/passkeys/public",
        "/relay/shares/share/recipient-passkeys",
        "/relay/grants/device-guest/recipient-passkeys",
    ] {
        assert_eq!(
            f.request(Method::GET, path, Some("owner"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    f.state
        .store
        .conn
        .lock()
        .await
        .execute("UPDATE relay_shares SET revoked_at='now'", [])
        .unwrap();
    assert_eq!(
        f.request(
            Method::GET,
            "/relay/devices/device/owner-account",
            Some("share-guest")
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn oauth_only_accounts_have_no_password_and_can_create_and_update_vault_without_auth_key() {
    let f = Fixture::new("account-oauth").await;
    let identity = ExternalIdentity {
        provider: OAuthProvider::Google,
        subject: "test-subject".into(),
        email: "oauth@example.test".into(),
        username: "oauth-only".into(),
    };
    let (user, token) = {
        let conn = f.state.store.conn.lock().await;
        let OAuthOutcome::Login(token) =
            authenticate_external_identity(&conn, &f.state.store.session_secret, &identity)
                .unwrap()
        else {
            panic!("Unexpected approval");
        };
        let user = load_user_by_session(&conn, &f.state.store.session_secret, &token).unwrap();
        let password: (String, String) = conn
            .query_row(
                "SELECT password_salt,password_hash FROM relay_users WHERE id=?1",
                [&user.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(password, (String::new(), String::new()));
        (user, token)
    };
    let mut vault = test_vault(&user.id, 1);
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            json!({"vault":vault,"expectedRevision":0,"recoveryAuthKey":recovery_key()})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"oauth-only","password":PASSWORD})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"oauth-only","authKey":auth_key()})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    vault["revision"] = json!(2);
    vault["slots"]["password"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            json!({"vault":vault,"expectedRevision":1})
        )
        .await
        .0,
        StatusCode::OK
    );
    vault["revision"] = json!(3);
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            json!({"vault":vault,"expectedRevision":2,"authKey":auth_key()})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn recovery_slot_regeneration_requires_new_hash_and_revokes_other_sessions() {
    let f = Fixture::new("account-recovery-slot").await;
    let token = f.register().await;
    let other = f.token("user-123", FULL).await;
    let mut v = test_vault("user-123", 2);
    v["slots"]["recovery"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    let mut req = json!({"vault":v,"expectedRevision":1,"currentAuthKey":auth_key()});
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            req.clone()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    req["recoveryAuthKey"] = json!(auth_key());
    assert_eq!(
        f.json(Method::PUT, "/relay/account/vault", Some(&token), req)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.req(Method::GET, "/relay/account/vault", Some(&other))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/recover",
            None,
            json!({"login":"vector-user","recoveryAuthKey":recovery_key()})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/recover",
            None,
            json!({"login":"vector-user","recoveryAuthKey":auth_key()})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn delete_vault_requires_current_key_and_can_start_over_without_rewriting_users() {
    let f = Fixture::new("account-delete").await;
    let token = f.register().await;
    let other = f.token("user-123", FULL).await;
    assert_eq!(
        f.json(
            Method::DELETE,
            "/relay/account/vault",
            Some(&token),
            json!({"currentAuthKey":recovery_key()})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.json(
            Method::DELETE,
            "/relay/account/vault",
            Some(&token),
            json!({"currentAuthKey":auth_key()})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.req(Method::GET, "/relay/account/vault", Some(&token))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.req(Method::GET, "/relay/account/vault", Some(&other))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(f.json(Method::PUT,"/relay/account/vault",Some(&token),json!({"vault":test_vault("user-123",1),"expectedRevision":0,"authKey":auth_key(),"recoveryAuthKey":recovery_key()})).await.0,StatusCode::OK);
    assert!(load_user_by_id(&*f.state.store.conn.lock().await, "user-123").is_some());
}

#[tokio::test]
async fn pending_registration_keeps_vault_user_id_and_keys_through_admin_approval() {
    let f = Fixture::new("account-approval").await;
    f.state
        .store
        .conn
        .lock()
        .await
        .execute(
            "INSERT INTO relay_settings VALUES ('registrationApprovalRequired','true')",
            [],
        )
        .unwrap();
    let (status,request) = f.json(Method::POST,"/relay/auth/register",None,json!({"username":"vector-user","email":"vector@example.test","authKey":auth_key(),"recoveryAuthKey":recovery_key(),"vault":test_vault("user-123",1)})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let admin = f.token("admin", FULL).await;
    let path = format!(
        "/relay/admin/registrations/{}/approve",
        request["request"]["id"].as_str().unwrap()
    );
    assert_eq!(
        f.json(Method::POST, &path, Some(&admin), json!({})).await.0,
        StatusCode::OK
    );
    let (status, v) = f
        .json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"vector-user","authKey":auth_key()}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["session"]["user"]["id"], "user-123");
    let token = v["token"].as_str().unwrap();
    let v: Value = f
        .req(Method::GET, "/relay/account/vault", Some(token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["userId"], "user-123");
}

#[tokio::test]
async fn api_only_preview_data_is_routed_instead_of_redirected() {
    let (mut state, dir) = tests::test_app_state("account-preview-origin");
    let mutable = Arc::get_mut(&mut state).unwrap();
    mutable.oauth.public_base_url = Some("https://relay.example.test".into());
    mutable.oauth.web_origin = Some("https://web.example.test".into());
    let app = Router::new()
        .merge(preview::routes())
        .fallback(spa_fallback)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security::browser_security,
        ))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let response = client
        .get(format!("{base}/supervisor/port-stream/no-ticket"))
        .send()
        .await
        .unwrap();
    // Missing ticket is rejected by the data-path handler, never redirected to HTML.
    assert!(!response.status().is_redirection());
    assert!(response.headers().get(header::LOCATION).is_none());
    assert!(response.status().is_client_error());
    server.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn schema_upgrade_preserves_existing_users_and_oauth_password_rows() {
    let f = Fixture::new("account-schema").await;
    let conn = f.state.store.conn.lock().await;
    let before: String = conn
        .query_row(
            "SELECT password_hash FROM relay_users WHERE id='legacy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // Simulate an OAuth account created before the new metadata table existed.
    conn.execute("INSERT INTO relay_user_identities(id,user_id,provider,provider_subject,provider_email,created_at) VALUES ('old-oauth','legacy','google','old-subject','legacy@example.test','now')",[]).unwrap();
    account::ensure_schema(&conn).unwrap();
    account::ensure_schema(&conn).unwrap();
    assert!(account::oauth_only(&conn, "legacy"));
    assert!(!account::oauth_only(&conn, "admin"));
    let after: String = conn
        .query_row(
            "SELECT password_hash FROM relay_users WHERE id='legacy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        conn.query_row("SELECT count(*) FROM relay_devices", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn legacy_setup_second_factor_preserves_setup_restriction() {
    let f = Fixture::new("account-legacy-factor").await;
    let codes = {
        let conn = f.state.store.conn.lock().await;
        conn.execute("INSERT INTO relay_totp(user_id,secret,enabled,expires_at) VALUES ('legacy','unused',1,0)",[]).unwrap();
        auth_factors::recovery_codes(&conn, "legacy").unwrap()
    };
    let response = f
        .req(Method::POST, "/relay/auth/login", None)
        .json(&json!({"identifier":"legacy","password":PASSWORD}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|h| {
            h.to_str()
                .ok()
                .filter(|s| s.starts_with("remote_codex_factor_challenge="))
                .map(|s| s.split(';').next().unwrap().to_owned())
        })
        .unwrap();
    let response = f
        .req(Method::POST, "/relay/auth/challenge", None)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, &f.base)
        .json(&json!({"code":codes[0]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["session"]["accountSetupRequired"], true);
    assert_eq!(
        f.req(
            Method::GET,
            "/relay/portal",
            Some(body["token"].as_str().unwrap())
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
}
