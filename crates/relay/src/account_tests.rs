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
// Saturation tests and endpoint-rule tests share the production work pools.
static LOGIN_TESTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

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
            for (id, role) in [
                ("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", "user"),
                ("admin", "admin"),
            ] {
                conn.execute("INSERT INTO relay_users(id,email,username,role,enabled,created_at,password_salt,password_hash) VALUES (?1,?2,?1,?3,1,'now',?4,?5)",params![id,format!("{id}@example.test"),role,salt,hash]).unwrap();
            }
            for id in ["unbound", "protected"] {
                conn.execute("INSERT INTO relay_devices(id,owner_user_id,name,token_hash,token_preview,created_at) VALUES (?1,'a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4',?1,?2,'test','now')",params![id,hash_device_token(&format!("test-{id}"))]).unwrap();
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
        // Production bounds simultaneous scrypt jobs. Endpoint-rule tests share a
        // process, so serialize their login calls instead of testing overload here.
        let _login = if matches!(path, "/relay/auth/login" | "/relay/auth/recover") {
            Some(LOGIN_TESTS.acquire().await.unwrap())
        } else {
            None
        };
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
        let (s,v) = self.json(Method::POST,"/relay/auth/register",None,json!({"userId":"b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","username":"vector-user","email":"vector@example.test","authKey":auth_key(),"recoveryAuthKey":recovery_key(),"vault":test_vault("b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4",1)})).await;
        assert_eq!(s, StatusCode::OK);
        v["token"].as_str().unwrap().into()
    }
    async fn setup(&self) -> String {
        let (s, v) = self
            .json(
                Method::POST,
                "/relay/auth/login",
                None,
                json!({"identifier":"a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","password":PASSWORD,"upgrade":true}),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["session"]["account"]["setupRequired"], true);
        let token = v["token"].as_str().unwrap();
        let (s,_) = self.json(Method::PUT,"/relay/account/vault",Some(token),json!({"vault":test_vault("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4",1),"expectedRevision":0,"authKey":auth_key(),"recoveryAuthKey":recovery_key()})).await;
        assert_eq!(s, StatusCode::OK);
        token.into()
    }
}

#[tokio::test]
async fn kdf_is_deterministic_secret_bound_without_legacy_exception() {
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
    let legacy: Value = get("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(legacy["mode"], "account");
    f.register().await;
    let known: Value = get("vector-user").await.unwrap().json().await.unwrap();
    assert_eq!(known["mode"], "account");
    assert_eq!(
        known["kdf"],
        test_vault("b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", 1)["kdf"]
    );
}

#[tokio::test]
async fn legacy_sessions_are_restricted_until_setup_then_auth_key_login_only() {
    let f = Fixture::new("account-legacy").await;
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"username":"a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","password":PASSWORD,"upgrade":true}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let token = v["token"].as_str().unwrap();
    for path in [
        "/relay/portal",
        "/relay/account/security",
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
    // Setup sessions may reach device routes; devices enforce their own binding.
    assert_ne!(
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
    let mut request = json!({"vault":test_vault("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4",1),"expectedRevision":0,"authKey":auth_key(),"recoveryAuthKey":recovery_key()});
    let ordinary = f.token("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", FULL).await;
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
    assert_eq!(session["account"]["setupRequired"], false);
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
            json!({"login":"a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","password":PASSWORD,"upgrade":true})
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
            json!({"login":"a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","authKey":auth_key()})
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
            json!({"username":"plain","email":"plain@example.test","password":PASSWORD,"upgrade":true})
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
            json!({"login":"admin","password":PASSWORD,"upgrade":true}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["session"]["account"]["setupRequired"], false);
}

#[tokio::test]
async fn vault_cas_immutable_identity_limits_and_slot_authentication() {
    let f = Fixture::new("account-vault-rules").await;
    let token = f.register().await;
    let mut v = test_vault("b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", 2);
    let body = |v: Value| json!({"vault":v,"expectedRevision":1,"currentAuthKey":auth_key()});
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
    assert_eq!(v["session"]["account"]["recovery"], true);
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
    let mut vault = test_vault("b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", 2);
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
        conn.execute("INSERT INTO relay_totp(user_id,secret,enabled,expires_at) VALUES ('a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4','unused',1,0)",[]).unwrap();
        auth_factors::recovery_codes(&conn, "a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4").unwrap()
    };
    // Existing second-factor backup codes are intentionally separate from the vault recovery key.
    let response = f
        .req(Method::POST, "/relay/auth/recover", None)
        .json(&json!({"login":"a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","recoveryAuthKey":recovery_key()}))
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
    assert_eq!(value["session"]["account"]["recovery"], true);
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
    let token = f.token(ACCOUNT_USER, FULL).await;
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
            "UPDATE relay_devices SET owner_user_id='b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4' WHERE id='unbound'",
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
        auth_factors::mark_strong(&conn, &token).unwrap();
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
            json!({"login":"oauth-only","password":PASSWORD,"upgrade":true})
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
    let other = f.token("b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", FULL).await;
    let mut v = test_vault("b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", 2);
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
    let other = f.token("b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", FULL).await;
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
    assert_eq!(f.json(Method::PUT,"/relay/account/vault",Some(&token),json!({"vault":test_vault("b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4",1),"expectedRevision":0,"authKey":auth_key(),"recoveryAuthKey":recovery_key()})).await.0,StatusCode::OK);
    assert!(load_user_by_id(
        &*f.state.store.conn.lock().await,
        "b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4"
    )
    .is_some());
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
    let (status,request) = f.json(Method::POST,"/relay/auth/register",None,json!({"userId":"b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","username":"vector-user","email":"vector@example.test","authKey":auth_key(),"recoveryAuthKey":recovery_key(),"vault":test_vault("b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4",1)})).await;
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
    assert_eq!(
        v["session"]["user"]["id"],
        "b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4"
    );
    let token = v["token"].as_str().unwrap();
    let v: Value = f
        .req(Method::GET, "/relay/account/vault", Some(token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["userId"], "b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4");
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
            "SELECT password_hash FROM relay_users WHERE id='a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // Simulate an OAuth account created before the new metadata table existed.
    conn.execute("INSERT INTO relay_user_identities(id,user_id,provider,provider_subject,provider_email,created_at) VALUES ('old-oauth','a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4','google','old-subject','legacy@example.test','now')",[]).unwrap();
    account::ensure_schema(&conn).unwrap();
    account::ensure_schema(&conn).unwrap();
    assert!(!account::oauth_only(
        &conn,
        "a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4"
    ));
    assert!(!account::oauth_only(&conn, "admin"));
    let after: String = conn
        .query_row(
            "SELECT password_hash FROM relay_users WHERE id='a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4'",
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
        conn.execute("INSERT INTO relay_totp(user_id,secret,enabled,expires_at) VALUES ('a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4','unused',1,0)",[]).unwrap();
        auth_factors::recovery_codes(&conn, "a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4").unwrap()
    };
    let response = f
        .req(Method::POST, "/relay/auth/login", None)
        .json(&json!({"identifier":"a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","password":PASSWORD,"upgrade":true}))
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
    assert_eq!(body["session"]["account"]["setupRequired"], true);
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

const ACCOUNT_USER: &str = "b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4";
fn registration(user: &str) -> Value {
    json!({"userId":user,"username":"vector-user","email":"vector@example.test","authKey":auth_key(),"recoveryAuthKey":recovery_key(),"vault":test_vault(user,1)})
}
#[tokio::test]
async fn registration_requires_uuid_v4_matching_vault_and_reports_id_collisions() {
    let f = Fixture::new("account-registration-id").await;
    for user in ["not-a-uuid", "b45aed3e-2f46-1b9c-93c1-e6f9d80c51e4"] {
        assert_eq!(
            f.json(
                Method::POST,
                "/relay/auth/register",
                None,
                registration(user)
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut mismatch = registration(ACCOUNT_USER);
    mismatch["vault"]["userId"] = json!("another-user");
    assert_eq!(
        f.json(Method::POST, "/relay/auth/register", None, mismatch)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    f.register().await;
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/register",
            None,
            registration(ACCOUNT_USER),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["code"], "user_id_taken");
    let f = Fixture::new("account-pending-id").await;
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
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/register",
            None,
            registration(ACCOUNT_USER),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(v["request"]["id"], ACCOUNT_USER);
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/register",
            None,
            registration(ACCOUNT_USER),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["code"], "user_id_taken");
}

#[tokio::test]
async fn account_state_is_present_in_session_login_register_recover_and_oauth_answers() {
    let f = Fixture::new("account-state-contract").await;
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","password":PASSWORD,"upgrade":true}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v["account"],
        json!({"mode":"legacy","setupRequired":true,"recovery":false,"hasVault":false,"accountKeyId":null})
    );
    assert_eq!(v["account"], v["session"]["account"]);
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/register",
            None,
            registration(ACCOUNT_USER),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v["account"],
        json!({"mode":"account","setupRequired":false,"recovery":false,"hasVault":true,"accountKeyId":vectors()["account"]["accountKeyId"]})
    );
    assert_eq!(v["account"], v["session"]["account"]);
    let session: Value = f
        .req(
            Method::GET,
            "/relay/auth/session",
            Some(v["token"].as_str().unwrap()),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["account"], v["account"]);
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"vector-user","authKey":auth_key()}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["account"], session["account"]);
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/recover",
            None,
            json!({"login":"vector-user","recoveryAuthKey":recovery_key()}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["account"]["recovery"], true);
    assert_eq!(v["account"], v["session"]["account"]);
    let conn = f.state.store.conn.lock().await;
    conn.execute(
        "INSERT INTO relay_account_auth(user_id,oauth_only) VALUES ('a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4',1)",
        [],
    )
    .unwrap();
    let user = load_user_by_id(&conn, "a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4").unwrap();
    let response =
        auth_api::login_response(&conn, &f.state, &user, &HeaderMap::new(), true, false).unwrap();
    let v: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16 * 1024).await.unwrap()).unwrap();
    assert_eq!(
        v["account"],
        json!({"mode":"oauth","setupRequired":true,"recovery":false,"hasVault":false,"accountKeyId":null})
    );
}

#[tokio::test]
async fn setup_sessions_allow_device_and_auth_routes_but_refuse_sharing_and_registry() {
    let f = Fixture::new("account-setup-routes").await;
    let token = f
        .token("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", account::SETUP)
        .await;
    let conn = f.state.store.conn.lock().await;
    let user = load_user_by_id(&conn, "a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4").unwrap();
    for (method, path) in [
        (Method::GET, "/relay/auth/session"),
        (Method::POST, "/relay/auth/logout"),
        (Method::POST, "/relay/auth/passkey/start"),
        (Method::GET, "/relay/account/vault"),
        (Method::PUT, "/relay/account/vault"),
        (Method::GET, "/relay/devices"),
        (Method::POST, "/relay/devices"),
        (Method::DELETE, "/relay/devices/protected"),
        (Method::GET, "/relay/devices/protected/api/workspaces"),
        (Method::GET, "/relay/devices/protected/ws"),
        (Method::GET, "/relay/api/workspaces?deviceId=protected"),
        (Method::GET, "/relay/ws?deviceId=protected"),
    ] {
        assert!(
            account::request_allowed(
                &conn,
                &user,
                account::SETUP,
                &method,
                &path.parse().unwrap()
            ),
            "{path}"
        );
    }
    for path in [
        "/relay/shares",
        "/relay/grants",
        "/relay/account/browsers",
        "/relay/account/revocations",
        "/relay/account/security",
    ] {
        assert!(
            !account::request_allowed(
                &conn,
                &user,
                account::SETUP,
                &Method::POST,
                &path.parse().unwrap()
            ),
            "{path}"
        );
    }
    drop(conn);
    assert_eq!(
        f.req(Method::GET, "/relay/devices", Some(&token))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_ne!(
        f.req(
            Method::GET,
            "/relay/devices/protected/api/workspaces",
            Some(&token)
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/revocations",
            Some(&token),
            json!({"revocation":vectors()["revocations"]["browser"]})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

async fn admin_reset(f: &Fixture) {
    let admin = f.token("admin", FULL).await;
    assert_eq!(
        f.json(
            Method::POST,
            &format!("/relay/admin/users/{ACCOUNT_USER}/reset-password"),
            Some(&admin),
            json!({"password":PASSWORD})
        )
        .await
        .0,
        StatusCode::OK
    );
}
#[tokio::test]
async fn admin_reset_preserves_vault_replaces_login_once_and_limits_session_until_slot_repair() {
    let f = Fixture::new("account-admin-reset").await;
    let original = f.register().await;
    admin_reset(&f).await;
    assert_eq!(
        f.req(Method::GET, "/relay/account/vault", Some(&original))
            .send()
            .await
            .unwrap()
            .status(),
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
        StatusCode::UNAUTHORIZED
    );
    let kdf: Value = f
        .req(Method::GET, "/relay/auth/kdf?login=vector-user", None)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(kdf["mode"], "account");
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"vector-user","password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["account"]["recovery"], true);
    let token = v["token"].as_str().unwrap();
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"vector-user","password":PASSWORD})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let fetched: Value = f
        .req(Method::GET, "/relay/account/vault", Some(token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(fetched, test_vault(ACCOUNT_USER, 1));
    for path in ["/relay/devices", "/relay/portal", "/relay/account/browsers"] {
        assert_eq!(
            f.req(Method::GET, path, Some(token))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let mut vault = test_vault(ACCOUNT_USER, 2);
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
    let conn = f.state.store.conn.lock().await;
    assert_eq!(
        conn.query_row(
            "SELECT reset_pending FROM relay_account_auth WHERE user_id=?1",
            [ACCOUNT_USER],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    drop(conn);
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
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"vector-user","password":PASSWORD})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn reset_session_can_delete_vault_without_current_key_and_temporary_password_stays_consumed()
{
    let f = Fixture::new("account-reset-start-over").await;
    f.register().await;
    admin_reset(&f).await;
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"vector-user","password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let token = v["token"].as_str().unwrap();
    assert_eq!(
        f.json(
            Method::DELETE,
            "/relay/account/vault",
            Some(token),
            json!({})
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
            json!({"login":"vector-user","password":PASSWORD})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let state: Value = f
        .req(Method::GET, "/relay/auth/session", Some(token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        state["account"],
        json!({"mode":"legacy","setupRequired":true,"recovery":false,"hasVault":false,"accountKeyId":null})
    );
    assert_eq!(f.json(Method::PUT,"/relay/account/vault",Some(token),json!({"vault":test_vault(ACCOUNT_USER,1),"expectedRevision":0,"authKey":auth_key(),"recoveryAuthKey":recovery_key()})).await.0,StatusCode::OK);
}

#[tokio::test]
async fn temporary_reset_password_requires_second_factor_and_preserves_recovery_mode() {
    let f = Fixture::new("account-reset-factor").await;
    f.register().await;
    admin_reset(&f).await;
    let (codes, trusted) = {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "INSERT INTO relay_totp(user_id,secret,enabled,expires_at) VALUES (?1,'unused',1,0)",
            [ACCOUNT_USER],
        )
        .unwrap();
        (
            auth_factors::recovery_codes(&conn, ACCOUNT_USER).unwrap(),
            auth_factors::trust(&conn, ACCOUNT_USER, &HeaderMap::new()).unwrap(),
        )
    };
    let response = {
        let _gate = LOGIN_TESTS.acquire().await.unwrap();
        f.req(Method::POST, "/relay/auth/login", None)
            .header(
                header::COOKIE,
                format!("remote_codex_trusted_browser={trusted}"),
            )
            .header(header::ORIGIN, &f.base)
            .json(&json!({"login":"vector-user","password":PASSWORD}))
            .send()
            .await
            .unwrap()
    };
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
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, &f.base)
        .json(&json!({"code":codes[0]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["account"]["recovery"], true);
    assert_eq!(value["account"], value["session"]["account"]);
    let token = value["token"].as_str().unwrap();
    assert_eq!(
        f.req(Method::GET, "/relay/account/browsers", Some(token))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.json(
            Method::DELETE,
            "/relay/account/vault",
            Some(token),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn oauth_slot_updates_and_deletion_require_authentication_within_ten_minutes() {
    let f = Fixture::new("account-oauth-recent").await;
    let token = f
        .token("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", account::SETUP)
        .await;
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "INSERT INTO relay_account_auth(user_id,oauth_only) VALUES ('a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4',1)",
            [],
        )
        .unwrap();
    }
    let mut vault = test_vault("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", 1);
    let create = json!({"vault":vault,"expectedRevision":0,"recoveryAuthKey":recovery_key()});
    let (s, v) = f
        .json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            create.clone(),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["code"], "reauthentication_required");
    {
        let conn = f.state.store.conn.lock().await;
        auth_factors::mark_strong(&conn, &token).unwrap();
    }
    assert_eq!(
        f.json(Method::PUT, "/relay/account/vault", Some(&token), create)
            .await
            .0,
        StatusCode::OK
    );
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "UPDATE relay_auth_sessions SET strong_auth_at=?1 WHERE token_hash=?2",
            params![auth_factors::now() - 600_001, security::token_hash(&token)],
        )
        .unwrap();
    }
    // Every PUT needs recent OAuth proof, including body-only edits.
    vault["revision"] = json!(2);
    vault["body"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            json!({"vault":vault,"expectedRevision":1})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    {
        let conn = f.state.store.conn.lock().await;
        auth_factors::mark_strong(&conn, &token).unwrap();
    }
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
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "UPDATE relay_auth_sessions SET strong_auth_at=?1",
            [auth_factors::now() - 600_001],
        )
        .unwrap();
    }
    vault["revision"] = json!(3);
    vault["slots"]["password"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    let slot_request = json!({"vault":vault,"expectedRevision":2});
    let (s, v) = f
        .json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            slot_request.clone(),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["code"], "reauthentication_required");
    let (s, v) = f
        .json(
            Method::DELETE,
            "/relay/account/vault",
            Some(&token),
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["code"], "reauthentication_required");
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "UPDATE relay_auth_sessions SET strong_auth_at=?1 WHERE token_hash=?2",
            params![auth_factors::now() - 540_000, security::token_hash(&token)],
        )
        .unwrap();
    }
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            slot_request
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.json(
            Method::DELETE,
            "/relay/account/vault",
            Some(&token),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn vault_reauth_uses_auth_key_and_admin_legacy_passwords_remain_supported() {
    let f = Fixture::new("account-reauth-key").await;
    let token = f.register().await;
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/security/reauth",
            Some(&token),
            json!({"password":auth_key()})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/security/reauth",
            Some(&token),
            json!({"authKey":recovery_key()})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/account/security/reauth",
            Some(&token),
            json!({"authKey":auth_key()}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ok"], true);
    {
        let conn = f.state.store.conn.lock().await;
        assert!(auth_factors::recent(&conn, &token));
    }
    for user in ["a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", "admin"] {
        let token = f.token(user, FULL).await;
        assert_eq!(
            f.json(
                Method::POST,
                "/relay/account/security/reauth",
                Some(&token),
                json!({"password":PASSWORD})
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let oauth = f.token("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", FULL).await;
    f.state
        .store
        .conn
        .lock()
        .await
        .execute(
            "INSERT INTO relay_account_auth(user_id,oauth_only) VALUES ('a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4',1)",
            [],
        )
        .unwrap();
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/account/security/reauth",
            Some(&oauth),
            json!({"password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["code"], "reauthentication_required");
}

#[tokio::test]
async fn oauth_recovery_session_can_rewrap_password_without_login_auth_key() {
    let f = Fixture::new("account-oauth-recovery").await;
    let token = f
        .token("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", account::SETUP)
        .await;
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "INSERT INTO relay_account_auth(user_id,oauth_only) VALUES ('a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4',1)",
            [],
        )
        .unwrap();
        auth_factors::mark_strong(&conn, &token).unwrap();
    }
    assert_eq!(f.json(Method::PUT,"/relay/account/vault",Some(&token),json!({"vault":test_vault("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4",1),"expectedRevision":0,"recoveryAuthKey":recovery_key()})).await.0,StatusCode::OK);
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/auth/recover",
            None,
            json!({"login":"a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","recoveryAuthKey":recovery_key()}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let no_factor = v["token"].as_str().unwrap();
    let mut candidate = test_vault("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", 2);
    candidate["slots"]["password"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(no_factor),
            json!({"vault":candidate,"expectedRevision":1})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let codes = {
        let conn = f.state.store.conn.lock().await;
        conn.execute("INSERT INTO relay_totp(user_id,secret,enabled,expires_at) VALUES ('a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4','unused',1,0)",[]).unwrap();
        auth_factors::recovery_codes(&conn, "a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4").unwrap()
    };
    let response = f
        .req(Method::POST, "/relay/auth/recover", None)
        .json(&json!({"login":"a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","recoveryAuthKey":recovery_key()}))
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
    let v: Value = response.json().await.unwrap();
    assert_eq!(v["account"]["mode"], "oauth");
    assert_eq!(v["account"]["recovery"], true);
    let recovery = v["token"].as_str().unwrap();
    let mut vault = test_vault("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", 2);
    vault["slots"]["password"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(recovery),
            json!({"vault":vault,"expectedRevision":1})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn body_only_vault_updates_do_not_complete_pending_admin_reset() {
    let f = Fixture::new("account-reset-body-update").await;
    f.register().await;
    admin_reset(&f).await;
    // A subsequently authenticated provider session may edit only the encrypted
    // body. Completing reset still requires password-slot repair or deletion.
    let token = f.token(ACCOUNT_USER, FULL).await;
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            json!({"vault":test_vault(ACCOUNT_USER,2),"expectedRevision":1,"recoveryAuthKey":recovery_key()})
        )
        .await
        .0,
        StatusCode::OK
    );
    let conn = f.state.store.conn.lock().await;
    assert!(account::reset_available(&conn, ACCOUNT_USER));
}

#[tokio::test]
async fn security_revision_kdf_never_discloses_account_class_or_reset_state() {
    let f = Fixture::new("security-kdf").await;
    for login in ["a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", "admin", "missing"] {
        let v: Value = f
            .req(Method::GET, &format!("/relay/auth/kdf?login={login}"), None)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(v["mode"], "account");
    }
    f.register().await;
    admin_reset(&f).await;
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"vector-user","password":PASSWORD,"upgrade":true})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let v: Value = f
        .req(Method::GET, "/relay/auth/kdf?login=vector-user", None)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["mode"], "account");
    assert_eq!(
        v["kdf"],
        test_vault("b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", 1)["kdf"]
    );
}

#[tokio::test]
async fn security_revision_plaintext_legacy_login_requires_explicit_upgrade() {
    let f = Fixture::new("security-upgrade").await;
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","password":PASSWORD})
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
            json!({"login":"a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","password":PASSWORD,"upgrade":true})
        )
        .await
        .0,
        StatusCode::OK
    );
    f.setup().await;
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":"a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4","password":PASSWORD,"upgrade":true})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn security_revision_first_vault_erases_legacy_password_hash() {
    let f = Fixture::new("security-password-clear").await;
    f.setup().await;
    let conn = f.state.store.conn.lock().await;
    let (salt, hash): (String, String) = conn
        .query_row(
            "SELECT password_salt,password_hash FROM relay_users WHERE id='a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((salt, hash), (String::new(), String::new()));
}

#[tokio::test]
async fn security_revision_anonymous_registration_cannot_attach_to_oauth_pending_request() {
    let f = Fixture::new("security-oauth-pending").await;
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute("INSERT OR REPLACE INTO relay_settings(key,value) VALUES ('registrationApprovalRequired','true')",[]).unwrap();
        insert_pending_registration(
            &conn,
            "victim@example.test",
            "victim",
            "",
            "",
            "google",
            Some("synthetic-subject"),
            None,
        )
        .unwrap();
    }
    let user = Uuid::new_v4().to_string();
    let (status,value)=f.json(Method::POST,"/relay/auth/register",None,json!({"userId":user,"email":"victim@example.test","username":"attacker","authKey":auth_key(),"recoveryAuthKey":recovery_key(),"vault":test_vault(&user,1)})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(value["request"].is_null());
    let conn = f.state.store.conn.lock().await;
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM relay_pending_account_vaults",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn security_revision_body_only_vault_write_requires_current_authentication() {
    let f = Fixture::new("security-body-write").await;
    let token = f.register().await;
    let v = test_vault("b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", 2);
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            json!({"vault":v,"expectedRevision":1})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn security_revision_query_token_cannot_bypass_cookie_session_restrictions() {
    let f = Fixture::new("security-session-resolution").await;
    let token = f
        .token("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", account::SETUP)
        .await;
    let response = f
        .req(Method::GET, "/relay/account/security?token=junk", None)
        .header(
            header::COOKIE,
            format!("remote_codex_relay_session={token}"),
        )
        .send()
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::OK);
    let response = f
        .req(Method::GET, "/relay/account/security", None)
        .header("x-pockymoe-resolved-session", "junk")
        .header(
            header::COOKIE,
            format!("remote_codex_relay_session={token}"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn security_revision_browser_revocation_ends_bound_relay_sessions() {
    let f = Fixture::new("security-browser-sessions").await;
    let token = f.register().await;
    let user = "b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4";
    let other = f.token(user, FULL).await;
    assert_eq!(f.json(Method::POST,"/relay/account/browsers",Some(&token),json!({"certificate":vectors()["browserCertificate"]["certificate"],"label":"synthetic"})).await.0,StatusCode::OK);
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/revocations",
            Some(&other),
            json!({"revocation":vectors()["revocations"]["browser"]})
        )
        .await
        .0,
        StatusCode::OK
    );
    let value: Value = f
        .req(Method::GET, "/relay/auth/session", Some(&token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(value["authenticated"], false);
    let value: Value = f
        .req(Method::GET, "/relay/auth/session", Some(&other))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(value["authenticated"], true);
}

#[tokio::test]
async fn security_revision_revocations_are_filtered_to_current_account_key() {
    let f = Fixture::new("security-revocation-filter").await;
    let token = f.register().await;
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute("INSERT INTO relay_account_revocations VALUES (?1,'old-key','browser','old-browser','{}')",["b45aed3e-2f46-4b9c-93c1-e6f9d80c51e4"]).unwrap();
    }
    let value: Value = f
        .req(Method::GET, "/relay/account/browsers", Some(&token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(value["revocations"], json!([]));
}

#[tokio::test]
async fn security_revision_device_credentials_require_recent_authentication() {
    let f = Fixture::new("security-device-tokens").await;
    let token = f
        .token("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", account::SETUP)
        .await;
    for path in [
        "/relay/devices/unbound/setup-token",
        "/relay/devices/unbound/bootstrap",
    ] {
        let (status, value) = f.json(Method::POST, path, Some(&token), json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(value["code"], "reauthentication_required");
    }
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/devices",
            Some(&token),
            json!({"name":"synthetic"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    {
        let conn = f.state.store.conn.lock().await;
        auth_factors::mark_strong(&conn, &token).unwrap();
    }
    for path in [
        "/relay/devices/unbound/setup-token",
        "/relay/devices/unbound/bootstrap",
    ] {
        assert_eq!(
            f.json(Method::POST, path, Some(&token), json!({})).await.0,
            StatusCode::OK
        );
    }
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/devices",
            Some(&token),
            json!({"name":"synthetic"})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn security_revision_registration_rejects_noncanonical_uuid() {
    let f = Fixture::new("security-canonical-uuid").await;
    let user = "B45AED3E-2F46-4B9C-93C1-E6F9D80C51E4";
    assert_eq!(f.json(Method::POST,"/relay/auth/register",None,json!({"userId":user,"email":"upper@example.test","username":"upper","authKey":auth_key(),"recoveryAuthKey":recovery_key(),"vault":test_vault(user,1)})).await.0,StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn security_revision_recovery_proof_can_update_vault_and_recover_interrupted_start_over() {
    let f = Fixture::new("security-start-over").await;
    let token = f.register().await;
    let mut v = test_vault(ACCOUNT_USER, 2);
    v["slots"]["password"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    let next_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([6; 32]);
    assert_eq!(f.json(Method::PUT,"/relay/account/vault",Some(&token),json!({"vault":v,"expectedRevision":1,"authKey":next_key,"currentRecoveryAuthKey":recovery_key()})).await.0,StatusCode::OK);
    let (s, value) = f
        .json(
            Method::POST,
            "/relay/auth/recover",
            None,
            json!({"login":"vector-user","recoveryAuthKey":recovery_key()}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let recovery = value["token"].as_str().unwrap();
    assert_eq!(
        f.json(
            Method::DELETE,
            "/relay/account/vault",
            Some(recovery),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (s, value) = f
        .json(
            Method::POST,
            "/relay/auth/recover",
            None,
            json!({"login":"vector-user","recoveryAuthKey":recovery_key()}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(value["account"]["setupRequired"], true);
    let setup = value["token"].as_str().unwrap();
    assert_eq!(f.json(Method::PUT,"/relay/account/vault",Some(setup),json!({"vault":test_vault(ACCOUNT_USER,1),"expectedRevision":0,"authKey":auth_key(),"recoveryAuthKey":recovery_key()})).await.0,StatusCode::OK);
}

#[tokio::test]
async fn security_revision_browser_cap_keeps_idempotent_registration_working() {
    let f = Fixture::new("security-browser-cap").await;
    let token = f.register().await;
    {
        let conn = f.state.store.conn.lock().await;
        for n in 0..32 {
            conn.execute(
                "INSERT INTO relay_account_browsers VALUES (?1,?2,?3,'synthetic',0,0)",
                params![
                    ACCOUNT_USER,
                    format!("synthetic-{n}"),
                    vectors()["browserCertificate"]["certificate"].to_string()
                ],
            )
            .unwrap();
        }
    }
    let body =
        json!({"certificate":vectors()["browserCertificate"]["certificate"],"label":"synthetic"});
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/account/browsers",
            Some(&token),
            body.clone(),
        )
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(v["code"], "browser_limit");
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "DELETE FROM relay_account_browsers WHERE browser_id='synthetic-0'",
            [],
        )
        .unwrap();
    }
    for _ in 0..2 {
        assert_eq!(
            f.json(
                Method::POST,
                "/relay/account/browsers",
                Some(&token),
                body.clone()
            )
            .await
            .0,
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn security_revision_revocation_cap_retains_permanent_tombstones_and_idempotence() {
    let f = Fixture::new("security-revocation-cap").await;
    let token = f.register().await;
    {
        let conn = f.state.store.conn.lock().await;
        let tx = conn.unchecked_transaction().unwrap();
        for n in 0..4096 {
            tx.execute(
                "INSERT INTO relay_account_revocations VALUES (?1,?2,'browser',?3,'{}')",
                params![
                    ACCOUNT_USER,
                    vectors()["account"]["accountKeyId"].as_str(),
                    format!("synthetic-{n}")
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let body = json!({"revocation":vectors()["revocations"]["browser"]});
    let (s, v) = f
        .json(
            Method::POST,
            "/relay/account/revocations",
            Some(&token),
            body.clone(),
        )
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(v["code"], "revocation_limit");
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "DELETE FROM relay_account_revocations WHERE subject_id='synthetic-0'",
            [],
        )
        .unwrap();
    }
    for _ in 0..2 {
        assert_eq!(
            f.json(
                Method::POST,
                "/relay/account/revocations",
                Some(&token),
                body.clone()
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let conn = f.state.store.conn.lock().await;
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM relay_account_revocations", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        4096
    );
}

#[tokio::test]
async fn security_revision_old_sha256_login_is_upgraded_and_old_sessions_are_restricted() {
    let f = Fixture::new("security-old-sessions").await;
    let user = "a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4";
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "UPDATE relay_users SET password_salt=?1,password_hash=?2 WHERE id=?3",
            params![LEGACY_SHA256_SALT, legacy_sha256(PASSWORD), user],
        )
        .unwrap();
    }
    let old = f.token(user, FULL).await;
    assert_eq!(
        f.req(Method::GET, "/relay/account/security", Some(&old))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":user,"password":PASSWORD,"upgrade":true})
        )
        .await
        .0,
        StatusCode::OK
    );
    let conn = f.state.store.conn.lock().await;
    let (salt, hash): (String, String) = conn
        .query_row(
            "SELECT password_salt,password_hash FROM relay_users WHERE id=?1",
            [user],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_ne!(salt, LEGACY_SHA256_SALT);
    assert!(verify_password(PASSWORD, &salt, &hash));
}

#[tokio::test]
async fn security_revision_combined_oauth_identity_keeps_password_login() {
    let f = Fixture::new("security-combined-oauth").await;
    let user = "a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4";
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute("INSERT INTO relay_user_identities VALUES ('combined',?1,'google','synthetic','legacy@example.test','now')",[user]).unwrap();
    }
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":user,"password":PASSWORD,"upgrade":true})
        )
        .await
        .0,
        StatusCode::OK
    );
    f.setup().await;
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":user,"authKey":auth_key()})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn security_revision_first_legacy_vault_requires_recent_upgrade_authentication() {
    let f = Fixture::new("security-setup-recent").await;
    let user = "a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4";
    let token = f.token(user, account::SETUP).await;
    let body = json!({"vault":test_vault(user,1),"expectedRevision":0,"authKey":auth_key(),"recoveryAuthKey":recovery_key()});
    let (s, v) = f
        .json(Method::PUT, "/relay/account/vault", Some(&token), body)
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["code"], "reauthentication_required");
}

#[tokio::test]
async fn security_revision_oauth_body_only_put_requires_recent_authentication() {
    let f = Fixture::new("security-oauth-body").await;
    let token = f.register().await;
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute("UPDATE relay_account_auth SET oauth_only=1,auth_hash=NULL,auth_salt=NULL WHERE user_id=?1",[ACCOUNT_USER]).unwrap();
        conn.execute("UPDATE relay_auth_sessions SET strong_auth_at=NULL", [])
            .unwrap();
    }
    let (s, v) = f
        .json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            json!({"vault":test_vault(ACCOUNT_USER,2),"expectedRevision":1}),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["code"], "reauthentication_required");
}

#[tokio::test]
async fn security_revision_anonymous_scrypt_runs_without_holding_database_lock() {
    let f = Fixture::new("security-password-lock").await;
    let auth = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([77; 32]);
    let recovery = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([78; 32]);
    let probe = |secret: String| {
        let (result, rx) = tokio::sync::oneshot::channel();
        PASSWORD_LOCK_PROBES
            .lock()
            .unwrap()
            .push(PasswordLockProbe {
                secret,
                connection: Arc::downgrade(&f.state.store.conn),
                result,
                resume: None,
            });
        rx
    };
    let registration_probe = probe(auth.clone());
    let mut body = json!({"userId":ACCOUNT_USER,"username":"vector-user","email":"vector@example.test","vault":test_vault(ACCOUNT_USER,1)});
    body["authKey"] = json!(auth);
    body["recoveryAuthKey"] = json!(recovery);
    assert_eq!(
        f.json(Method::POST, "/relay/auth/register", None, body)
            .await
            .0,
        StatusCode::OK
    );
    assert!(
        registration_probe.await.unwrap(),
        "registration scrypt held the database lock"
    );
    let recovery_probe = probe(recovery.clone());
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/recover",
            None,
            json!({"login":"vector-user","recoveryAuthKey":recovery})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        recovery_probe.await.unwrap(),
        "recovery scrypt held the database lock"
    );
}

#[tokio::test]
async fn security_revision_migration_does_not_trust_preupgrade_sessions() {
    let f = Fixture::new("security-session-migration").await;
    let token = f.register().await;
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "ALTER TABLE relay_auth_sessions DROP COLUMN account_mode",
            [],
        )
        .unwrap();
        account::ensure_schema(&conn).unwrap();
        assert_eq!(account::session_mode(&conn, &token), account::SETUP);
    }
    assert_eq!(
        f.req(Method::GET, "/relay/account/browsers", Some(&token))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/security/reauth",
            Some(&token),
            json!({"authKey":auth_key()})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.req(Method::GET, "/relay/account/browsers", Some(&token))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn security_revision_bound_session_cannot_change_browser_to_evade_revocation() {
    let f = Fixture::new("security-browser-rebinding").await;
    let token = f.register().await;
    let cert = vectors()["browserCertificate"]["certificate"].clone();
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/browsers",
            Some(&token),
            json!({"certificate":cert,"label":"synthetic"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let mut second = cert.clone();
    second["publicKey"] = vectors()["otherAccount"]["accountPublicKey"].clone();
    let der = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(second["publicKey"].as_str().unwrap())
        .unwrap();
    second["browserId"] =
        json!(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(der)));
    let account_key = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(
            vectors()["account"]["accountPrivateKeyPkcs8"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
    let key = openssl::pkey::PKey::private_key_from_pkcs8(&account_key).unwrap();
    let mut signer =
        openssl::sign::Signer::new(openssl::hash::MessageDigest::sha256(), &key).unwrap();
    let input = format!(
        "pockymoe-acct-browser-v1\n{}\n{}\n{}\n{}",
        second["accountKeyId"].as_str().unwrap(),
        second["browserId"].as_str().unwrap(),
        second["publicKey"].as_str().unwrap(),
        second["createdAt"].as_u64().unwrap()
    );
    signer.update(input.as_bytes()).unwrap();
    let sig = openssl::ecdsa::EcdsaSig::from_der(&signer.sign_to_vec().unwrap()).unwrap();
    let mut raw = sig.r().to_vec_padded(32).unwrap();
    raw.extend(sig.s().to_vec_padded(32).unwrap());
    second["signature"] = json!(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw));
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/browsers",
            Some(&token),
            json!({"certificate":second,"label":"other"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn review_two_browser_caps_ignore_old_keys_and_revoked_rows() {
    let f = Fixture::new("review-two-browser-count").await;
    let token = f.register().await;
    let cert = vectors()["browserCertificate"]["certificate"].clone();
    {
        let conn = f.state.store.conn.lock().await;
        for n in 0..32 {
            let mut old = cert.clone();
            old["accountKeyId"] = json!("old-key");
            conn.execute(
                "INSERT INTO relay_account_browsers VALUES (?1,?2,?3,'old',0,0)",
                params![ACCOUNT_USER, format!("old-{n}"), old.to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO relay_account_browsers VALUES (?1,?2,?3,'revoked',0,0)",
                params![ACCOUNT_USER, format!("revoked-{n}"), cert.to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO relay_account_revocations VALUES (?1,?2,'browser',?3,'{}')",
                params![
                    ACCOUNT_USER,
                    cert["accountKeyId"].as_str(),
                    format!("revoked-{n}")
                ],
            )
            .unwrap();
        }
    }
    let (status, value) = f
        .json(
            Method::POST,
            "/relay/account/browsers",
            Some(&token),
            json!({"certificate":cert,"label":"live"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let response: Value = f
        .req(Method::GET, "/relay/account/browsers", Some(&token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["browsers"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn review_two_browser_revocation_removes_registry_row() {
    let f = Fixture::new("review-two-browser-revoke").await;
    let token = f.register().await;
    let other = f.token(ACCOUNT_USER, FULL).await;
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/browsers",
            Some(&token),
            json!({"certificate":vectors()["browserCertificate"]["certificate"],"label":"live"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/revocations",
            Some(&other),
            json!({"revocation":vectors()["revocations"]["browser"]})
        )
        .await
        .0,
        StatusCode::OK
    );
    let conn = f.state.store.conn.lock().await;
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM relay_account_browsers WHERE user_id=?1",
            [ACCOUNT_USER],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM relay_account_revocations WHERE user_id=?1",
            [ACCOUNT_USER],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn review_two_delete_signed_out_browser_keeps_other_sessions_and_current_key_scope() {
    let f = Fixture::new("review-two-browser-delete").await;
    let token = f.register().await;
    let other = f.token(ACCOUNT_USER, FULL).await;
    let cert = vectors()["browserCertificate"]["certificate"].clone();
    let browser = cert["browserId"].as_str().unwrap();
    let path = format!("/relay/account/browsers/{browser}");
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/browsers",
            Some(&other),
            json!({"certificate":cert,"label":"live"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.json(Method::DELETE, &path, Some(&token), json!({}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "UPDATE relay_auth_sessions SET expires_at=0 WHERE token_hash=?1",
            [security::token_hash(&other)],
        )
        .unwrap();
        let mut old = cert.clone();
        old["accountKeyId"] = json!("old-key");
        conn.execute(
            "INSERT INTO relay_account_browsers VALUES (?1,'old',?2,'old',0,0)",
            params![ACCOUNT_USER, old.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO relay_account_browsers VALUES (?1,'foreign',?2,'foreign',0,0)",
            params!["a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", cert.to_string()],
        )
        .unwrap();
    }
    assert_eq!(
        f.json(Method::DELETE, &path, None, json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.json(Method::DELETE, &path, Some(&token), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    for id in [browser, "old", "foreign"] {
        assert_eq!(
            f.json(
                Method::DELETE,
                &format!("/relay/account/browsers/{id}"),
                Some(&token),
                json!({})
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    let session: Value = f
        .req(Method::GET, "/relay/auth/session", Some(&token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["authenticated"], true);
    let conn = f.state.store.conn.lock().await;
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM relay_auth_sessions WHERE user_id=?1",
            [ACCOUNT_USER],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2,
        "delete must not delete sessions, even expired ones"
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM relay_account_revocations", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        0,
        "unsigned row deletion must not manufacture a revocation"
    );
}

#[tokio::test]
async fn review_two_previous_vault_is_removed_and_old_ciphertext_scrubbed() {
    let f = Fixture::new("review-two-no-previous").await;
    let token = f.register().await;
    // Simulate the prior schema; upgrade must remove only obsolete ciphertext.
    {
        let conn = f.state.store.conn.lock().await;
        if conn
            .prepare("SELECT previous_vault_json FROM relay_account_vaults LIMIT 0")
            .is_err()
        {
            conn.execute(
                "ALTER TABLE relay_account_vaults ADD COLUMN previous_vault_json TEXT",
                [],
            )
            .unwrap();
        }
        conn.execute(
            "UPDATE relay_account_vaults SET previous_vault_json=?1 WHERE user_id=?2",
            params![test_vault(ACCOUNT_USER, 1).to_string(), ACCOUNT_USER],
        )
        .unwrap();
        account::ensure_schema(&conn).unwrap();
        assert!(conn
            .prepare("SELECT previous_vault_json FROM relay_account_vaults LIMIT 0")
            .is_err());
        assert_eq!(
            account::vault(&conn, ACCOUNT_USER).unwrap().unwrap(),
            test_vault(ACCOUNT_USER, 1)
        );
    }
    assert_eq!(f.json(Method::PUT,"/relay/account/vault",Some(&token),json!({"vault":test_vault(ACCOUNT_USER,2),"expectedRevision":1,"currentAuthKey":auth_key()})).await.0,StatusCode::OK);
    for method in [Method::GET, Method::POST] {
        let response = f
            .req(method, "/relay/account/vault/previous", Some(&token))
            .json(&json!({"currentAuthKey":auth_key()}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

fn password_probe(f: &Fixture, secret: String) -> tokio::sync::oneshot::Receiver<bool> {
    let (result, rx) = tokio::sync::oneshot::channel();
    PASSWORD_LOCK_PROBES
        .lock()
        .unwrap()
        .push(PasswordLockProbe {
            secret,
            connection: Arc::downgrade(&f.state.store.conn),
            result,
            resume: None,
        });
    rx
}

#[tokio::test]
async fn review_two_vault_proofs_and_new_hashes_run_outside_database_lock() {
    let f = Fixture::new("review-two-vault-lock").await;
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([91; 32]);
    let recovery = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([92; 32]);
    let next_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([93; 32]);
    let next_recovery = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([94; 32]);
    let (s,body)=f.json(Method::POST,"/relay/auth/register",None,json!({"userId":ACCOUNT_USER,"username":"vector-user","email":"vector@example.test","authKey":key,"recoveryAuthKey":recovery,"vault":test_vault(ACCOUNT_USER,1)})).await;
    assert_eq!(s, StatusCode::OK);
    let token = body["token"].as_str().unwrap();
    let auth_probe = password_probe(&f, key.clone());
    let auth_hash_probe = password_probe(&f, next_key.clone());
    let recovery_hash_probe = password_probe(&f, next_recovery.clone());
    let mut next = test_vault(ACCOUNT_USER, 2);
    next["slots"]["password"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    next["slots"]["recovery"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    assert_eq!(f.json(Method::PUT,"/relay/account/vault",Some(token),json!({"vault":next,"expectedRevision":1,"currentAuthKey":key,"authKey":next_key,"recoveryAuthKey":next_recovery})).await.0,StatusCode::OK);
    assert!(auth_probe.await.unwrap(), "current auth proof held DB lock");
    assert!(auth_hash_probe.await.unwrap(), "new auth hash held DB lock");
    assert!(
        recovery_hash_probe.await.unwrap(),
        "new recovery hash held DB lock"
    );
    // An unchanged recovery slot can be used as the write proof.
    let new_recovery_probe = password_probe(&f, next_recovery.clone());
    next["revision"] = json!(3);
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(token),
            json!({"vault":next,"expectedRevision":2,"recoveryAuthKey":next_recovery})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        new_recovery_probe.await.unwrap(),
        "recovery proof held DB lock"
    );
    let delete_probe = password_probe(&f, next_key.clone());
    assert_eq!(
        f.json(
            Method::DELETE,
            "/relay/account/vault",
            Some(token),
            json!({"currentAuthKey":next_key})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(delete_probe.await.unwrap(), "delete proof held DB lock");
}

#[tokio::test]
async fn review_two_vault_mutations_have_a_shared_per_user_rate_limit() {
    let f = Fixture::new("review-two-vault-rate").await;
    let token = f.register().await;
    let other = f.token(ACCOUNT_USER, FULL).await;
    for n in 0..10 {
        let method = if n % 2 == 0 {
            Method::PUT
        } else {
            Method::DELETE
        };
        let request = if method == Method::PUT {
            json!({"vault":test_vault(ACCOUNT_USER,2),"expectedRevision":1})
        } else {
            json!({})
        };
        assert_eq!(
            f.json(
                method,
                "/relay/account/vault",
                Some(if n % 2 == 0 { &token } else { &other }),
                request
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    let (status, value) = f
        .json(
            Method::DELETE,
            "/relay/account/vault",
            Some(&token),
            json!({"currentAuthKey":auth_key()}),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(value["code"], "rate_limited");
    let unrelated = f
        .token("a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4", account::RESET)
        .await;
    assert_eq!(
        f.json(
            Method::DELETE,
            "/relay/account/vault",
            Some(&unrelated),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn review_two_unknown_recovery_does_not_take_real_account_work_slots() {
    let f = Fixture::new("review-two-dummy-work").await;
    let _gate = LOGIN_TESTS.acquire().await.unwrap();
    let slots = ANONYMOUS_PASSWORD_WORK.acquire_many(4).await.unwrap();
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([111; 32]);
    let started = password_probe(&f, key.clone());
    let request = f
        .req(Method::POST, "/relay/auth/recover", None)
        .json(&json!({"login":"unknown","recoveryAuthKey":key}));
    let task = tokio::spawn(async move { request.send().await.unwrap() });
    // Observe work admission before scrypt itself; hash duration is irrelevant.
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), started).await;
    drop(slots);
    let response = task.await.unwrap();
    assert!(result
        .expect("dummy work queued behind real-account slots")
        .unwrap());
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn review_two_revocation_credential_changes_and_cas_races_invalidate_vault_proofs() {
    for (case, method) in [
        (0, Method::PUT),
        (1, Method::DELETE),
        (2, Method::PUT),
        (3, Method::PUT),
    ] {
        let f = Fixture::new(&format!("review-two-proof-race-{case}")).await;
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([100 + case; 32]);
        let (s,body)=f.json(Method::POST,"/relay/auth/register",None,json!({"userId":ACCOUNT_USER,"username":"vector-user","email":"vector@example.test","authKey":key,"recoveryAuthKey":recovery_key(),"vault":test_vault(ACCOUNT_USER,1)})).await;
        assert_eq!(s, StatusCode::OK);
        let token = body["token"].as_str().unwrap().to_owned();
        if case == 3 {
            let conn = f.state.store.conn.lock().await;
            conn.execute(
                "UPDATE relay_account_auth SET oauth_only=1 WHERE user_id=?1",
                [ACCOUNT_USER],
            )
            .unwrap();
        }
        let (result, started) = tokio::sync::oneshot::channel();
        let (resume, wait) = std::sync::mpsc::channel();
        PASSWORD_LOCK_PROBES
            .lock()
            .unwrap()
            .push(PasswordLockProbe {
                secret: key.clone(),
                connection: Arc::downgrade(&f.state.store.conn),
                result,
                resume: Some(wait),
            });
        let request_body = if method == Method::PUT {
            json!({"vault":test_vault(ACCOUNT_USER,2),"expectedRevision":1,"currentAuthKey":key})
        } else {
            json!({"currentAuthKey":key})
        };
        let req = f
            .req(method, "/relay/account/vault", Some(&token))
            .json(&request_body);
        let task = tokio::spawn(async move { req.send().await.unwrap() });
        let unlocked = started.await.unwrap();
        if !unlocked {
            let _ = resume.send(());
        }
        assert!(
            unlocked,
            "proof race test requires verification outside DB lock"
        );
        {
            let conn = f.state.store.conn.lock().await;
            match case {
                0 => {
                    conn.execute(
                        "DELETE FROM relay_auth_sessions WHERE token_hash=?1",
                        [security::token_hash(&token)],
                    )
                    .unwrap();
                }
                1 => {
                    conn.execute(
                        "UPDATE relay_account_auth SET auth_hash='changed' WHERE user_id=?1",
                        [ACCOUNT_USER],
                    )
                    .unwrap();
                }
                2 => {
                    account::store_vault(&conn, ACCOUNT_USER, &test_vault(ACCOUNT_USER, 2))
                        .unwrap();
                }
                _ => {
                    conn.execute(
                        "UPDATE relay_auth_sessions SET strong_auth_at=0 WHERE token_hash=?1",
                        [security::token_hash(&token)],
                    )
                    .unwrap();
                }
            }
        }
        resume.send(()).unwrap();
        let response = task.await.unwrap();
        assert_eq!(
            response.status(),
            if case == 0 {
                StatusCode::UNAUTHORIZED
            } else if case == 3 {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::CONFLICT
            }
        );
        if case == 3 {
            let value: Value = response.json().await.unwrap();
            assert_eq!(value["code"], "reauthentication_required");
        }
        let conn = f.state.store.conn.lock().await;
        assert_eq!(
            account::vault(&conn, ACCOUNT_USER).unwrap().unwrap()["revision"],
            if case == 2 { 2 } else { 1 }
        );
    }
}

#[tokio::test]
async fn review_two_revocation_cap_counts_only_current_key() {
    let f = Fixture::new("review-two-current-revocation-cap").await;
    let token = f.register().await;
    {
        let conn = f.state.store.conn.lock().await;
        let tx = conn.unchecked_transaction().unwrap();
        for n in 0..4096 {
            tx.execute(
                "INSERT INTO relay_account_revocations VALUES (?1,'old-key','browser',?2,'{}')",
                params![ACCOUNT_USER, format!("old-{n}")],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/account/revocations",
            Some(&token),
            json!({"revocation":vectors()["revocations"]["browser"]})
        )
        .await
        .0,
        StatusCode::OK
    );
    let conn = f.state.store.conn.lock().await;
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM relay_account_revocations", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        4097,
        "old tombstones remain permanent"
    );
}

#[tokio::test]
async fn review_two_known_login_can_run_while_dummy_capacity_is_full() {
    let f = Fixture::new("review-two-real-login-capacity").await;
    f.register().await;
    // Hold the same test gate as ordinary auth calls before saturating a pool,
    // so parallel rule tests aren't accidentally turned into overload tests.
    let _gate = LOGIN_TESTS.acquire().await.unwrap();
    let dummy = DUMMY_PASSWORD_WORK.acquire_many(2).await.unwrap();
    let response = f
        .req(Method::POST, "/relay/auth/login", None)
        .json(&json!({"login":"vector-user","authKey":auth_key()}))
        .send()
        .await
        .unwrap();
    drop(dummy);
    assert_eq!(response.status(), StatusCode::OK);
    let real = LOGIN_PASSWORD_WORK.acquire_many(4).await.unwrap();
    let response = f
        .req(Method::POST, "/relay/auth/login", None)
        .json(&json!({"login":"unknown-login","authKey":auth_key()}))
        .send()
        .await
        .unwrap();
    drop(real);
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "dummy login must not queue behind real login capacity"
    );
}

#[tokio::test]
async fn documentation_admin_reset_without_vault_logs_in_once_to_setup_without_upgrade() {
    let f = Fixture::new("doc-legacy-reset").await;
    let user = "a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4";
    let temporary = "temporary reset test password";
    let admin = f.token("admin", FULL).await;
    assert_eq!(
        f.json(
            Method::POST,
            &format!("/relay/admin/users/{user}/reset-password"),
            Some(&admin),
            json!({"password":temporary})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, body) = f
        .json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":user,"password":temporary}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["account"],
        json!({"mode":"legacy","setupRequired":true,"recovery":false,"hasVault":false,"accountKeyId":null})
    );
    let token = body["token"].as_str().unwrap();
    for upgrade in [false, true] {
        assert_eq!(
            f.json(
                Method::POST,
                "/relay/auth/login",
                None,
                json!({"login":user,"password":temporary,"upgrade":upgrade})
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        f.req(Method::GET, "/relay/portal", Some(token))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.json(Method::PUT,"/relay/account/vault",Some(token),json!({"vault":test_vault(user,1),"expectedRevision":0,"authKey":auth_key(),"recoveryAuthKey":recovery_key()})).await.0,StatusCode::OK);
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":user,"authKey":auth_key()})
        )
        .await
        .0,
        StatusCode::OK
    );
    let conn = f.state.store.conn.lock().await;
    assert_eq!(
        conn.query_row(
            "SELECT reset_pending,reset_consumed FROM relay_account_auth WHERE user_id=?1",
            [user],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        )
        .unwrap(),
        (0, 1)
    );
}

#[tokio::test]
async fn documentation_admin_reset_without_vault_keeps_mfa_and_one_time_consumption() {
    let f = Fixture::new("doc-legacy-reset-factor").await;
    let user = "a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4";
    let admin = f.token("admin", FULL).await;
    assert_eq!(
        f.json(
            Method::POST,
            &format!("/relay/admin/users/{user}/reset-password"),
            Some(&admin),
            json!({"password":PASSWORD})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (codes, trusted) = {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "INSERT INTO relay_totp(user_id,secret,enabled,expires_at) VALUES (?1,'unused',1,0)",
            [user],
        )
        .unwrap();
        (
            auth_factors::recovery_codes(&conn, user).unwrap(),
            auth_factors::trust(&conn, user, &HeaderMap::new()).unwrap(),
        )
    };
    let response = {
        let _gate = LOGIN_TESTS.acquire().await.unwrap();
        f.req(Method::POST, "/relay/auth/login", None)
            .header(header::ORIGIN, &f.base)
            .header(
                header::COOKIE,
                format!("remote_codex_trusted_browser={trusted}"),
            )
            .json(&json!({"login":user,"password":PASSWORD}))
            .send()
            .await
            .unwrap()
    };
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
        .header(header::ORIGIN, &f.base)
        .header(header::COOKIE, cookie)
        .json(&json!({"code":codes[0]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["account"]["setupRequired"], true);
    assert_eq!(value["account"]["recovery"], false);
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":user,"password":PASSWORD,"upgrade":true})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn documentation_recovery_proof_is_separate_from_new_recovery_key() {
    let f = Fixture::new("doc-current-recovery").await;
    let token = f.register().await;
    let next_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([227; 32]);
    let mut vault = test_vault(ACCOUNT_USER, 2);
    vault["slots"]["recovery"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    assert_eq!(f.json(Method::PUT,"/relay/account/vault",Some(&token),json!({"vault":vault,"expectedRevision":1,"currentRecoveryAuthKey":recovery_key(),"recoveryAuthKey":next_key})).await.0,StatusCode::OK);
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
            json!({"login":"vector-user","recoveryAuthKey":next_key})
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
            json!({"vault":vault,"expectedRevision":2,"currentRecoveryAuthKey":next_key})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn documentation_recovery_login_can_regenerate_password_and_recovery_slots() {
    let f = Fixture::new("doc-recovery-login-regeneration").await;
    let original = f.register().await;
    let (status, login) = f
        .json(
            Method::POST,
            "/relay/auth/recover",
            None,
            json!({"login":"vector-user","recoveryAuthKey":recovery_key()}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(login["account"]["recovery"], true);
    let token = login["token"].as_str().unwrap();
    let next_auth = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([230; 32]);
    let next_recovery = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([231; 32]);
    let mut vault = test_vault(ACCOUNT_USER, 2);
    vault["slots"]["recovery"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    let mut write = json!({
        "vault":vault,"expectedRevision":1,
        "currentRecoveryAuthKey":recovery_key(),"recoveryAuthKey":next_recovery
    });
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(token),
            write.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "a limited recovery session must replace the password slot before promotion"
    );
    write["vault"]["slots"]["password"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    write["authKey"] = json!(next_auth);
    assert_eq!(
        f.json(Method::PUT, "/relay/account/vault", Some(token), write)
            .await
            .0,
        StatusCode::OK
    );
    let response = f
        .req(Method::GET, "/relay/auth/session", Some(token))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let session: Value = response.json().await.unwrap();
    assert_eq!(session["account"]["recovery"], false);
    assert_eq!(session["account"]["setupRequired"], false);
    assert_eq!(
        f.req(Method::GET, "/relay/account/vault", Some(&original))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for (path, field, old, new) in [
        ("/relay/auth/login", "authKey", auth_key(), next_auth),
        (
            "/relay/auth/recover",
            "recoveryAuthKey",
            recovery_key(),
            next_recovery,
        ),
    ] {
        for (key, expected) in [(old, StatusCode::UNAUTHORIZED), (new, StatusCode::OK)] {
            let mut body = json!({"login":"vector-user"});
            body[field] = json!(key);
            assert_eq!(f.json(Method::POST, path, None, body).await.0, expected);
        }
    }
}

#[tokio::test]
async fn documentation_recovery_new_key_is_not_a_proof_but_body_only_compatibility_remains() {
    let f = Fixture::new("doc-recovery-field-rules").await;
    let token = f.register().await;
    let mut vault = test_vault(ACCOUNT_USER, 2);
    vault["slots"]["recovery"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    for proof in [
        Value::Null,
        json!(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([228; 32])),
    ] {
        assert_eq!(f.json(Method::PUT,"/relay/account/vault",Some(&token),json!({"vault":vault,"expectedRevision":1,"currentRecoveryAuthKey":proof,"recoveryAuthKey":recovery_key()})).await.0,StatusCode::FORBIDDEN);
    }
    vault = test_vault(ACCOUNT_USER, 2);
    vault["slots"]["password"]["iv"] = json!("AAAAAAAAAAAAAAAA");
    assert_eq!(f.json(Method::PUT,"/relay/account/vault",Some(&token),json!({"vault":vault,"expectedRevision":1,"authKey":auth_key(),"recoveryAuthKey":recovery_key()})).await.0,StatusCode::FORBIDDEN,"compatibility recoveryAuthKey proofs only apply to body-only writes");
    vault = test_vault(ACCOUNT_USER, 2);
    assert_eq!(
        f.json(
            Method::PUT,
            "/relay/account/vault",
            Some(&token),
            json!({"vault":vault,"expectedRevision":1,"recoveryAuthKey":recovery_key()})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn documentation_admin_reset_keeps_oauth_only_accounts_oauth_only_before_first_vault() {
    let f = Fixture::new("doc-oauth-reset-no-vault").await;
    let user = "a45aed3e-2f46-4b9c-93c1-e6f9d80c51e4";
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "UPDATE relay_users SET password_salt='',password_hash='' WHERE id=?1",
            [user],
        )
        .unwrap();
        conn.execute("INSERT INTO relay_user_identities VALUES ('doc-oauth',?1,'google','synthetic','legacy@example.test','now')",[user]).unwrap();
        assert!(account::oauth_only(&conn, user));
    }
    let admin = f.token("admin", FULL).await;
    assert_eq!(
        f.json(
            Method::POST,
            &format!("/relay/admin/users/{user}/reset-password"),
            Some(&admin),
            json!({"password":PASSWORD})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        account::oauth_only(&*f.state.store.conn.lock().await, user),
        "temporary reset must not convert an OAuth-only account into a password account"
    );
    let (s, value) = f
        .json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":user,"password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(value["account"]["mode"], "oauth");
    assert_eq!(value["account"]["setupRequired"], true);
    assert_eq!(
        f.json(
            Method::POST,
            "/relay/auth/login",
            None,
            json!({"login":user,"password":PASSWORD})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}
