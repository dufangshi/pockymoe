//! Google/GitHub sign-up honours the invite code: the code given at the OAuth
//! start stays on the relay (hashed) and the callback checks it before any
//! account or pending registration is created. A mock Google provider issues
//! one identity per authorization code (`code` = subject).
use super::*;

const INVITE: &str = "invite-code-1";

struct OAuthFixture {
    state: Arc<AppState>,
    base: String,
    provider: String,
    client: reqwest::Client,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    dir: PathBuf,
}
impl Drop for OAuthFixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn mock_token(body: String) -> Json<Value> {
    let code = url::form_urlencoded::parse(body.as_bytes())
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default();
    Json(json!({ "access_token": code, "token_type": "Bearer" }))
}

async fn mock_userinfo(headers: HeaderMap) -> Json<Value> {
    let subject = bearer_token(&headers).unwrap_or_default();
    Json(json!({
        "sub": subject,
        "email": format!("{subject}@example.test"),
        "email_verified": true,
    }))
}

impl OAuthFixture {
    async fn new(name: &str, settings: &[(&str, &str)]) -> Self {
        let provider_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let provider = format!("http://{}", provider_listener.local_addr().unwrap());
        let provider_app = Router::new()
            .route("/token", post(mock_token))
            .route("/userinfo", get(mock_userinfo));
        let provider_task = tokio::spawn(async move {
            axum::serve(provider_listener, provider_app).await.unwrap();
        });
        let (mut state, dir) = tests::test_app_state(name);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        {
            let mutable = Arc::get_mut(&mut state).unwrap();
            mutable.oauth.public_base_url = Some(base.clone());
            mutable.oauth.enable_test_google(&provider);
        }
        {
            let conn = state.store.conn.lock().await;
            conn.execute("INSERT INTO relay_users(id,email,username,role,enabled,created_at,password_salt,password_hash) VALUES ('admin','admin@example.test','admin','admin',1,'now','salt','hash')", []).unwrap();
            set_relay_setting(&conn, "googleAuthEnabled", "true").unwrap();
            for (key, value) in settings {
                set_relay_setting(&conn, key, value).unwrap();
            }
        }
        let app = Router::new()
            .route(
                "/relay/auth/oauth/{provider}/start",
                get(oauth_start).post(oauth_start_with_invite),
            )
            .route("/relay/auth/oauth/{provider}/callback", get(oauth_callback))
            .route(
                "/relay/admin/registrations/{request_id}/approve",
                post(approve_registration),
            )
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
            provider,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            tasks: vec![provider_task, task],
            dir,
        }
    }

    async fn setting(&self, key: &str, value: &str) {
        let conn = self.state.store.conn.lock().await;
        set_relay_setting(&conn, key, value).unwrap();
    }

    /// `POST .../start`; returns the status, the JSON body and the browser cookie.
    async fn post_start(&self, body: Value) -> (StatusCode, Value, Option<String>) {
        let response = self
            .client
            .post(format!("{}/relay/auth/oauth/google/start", self.base))
            .header("origin", &self.base)
            .json(&body)
            .send()
            .await
            .unwrap();
        let cookie = oauth_cookie(&response);
        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null), cookie)
    }

    /// `GET .../start` as the sign-in buttons use it; returns the provider URL.
    async fn get_start(&self) -> (String, String) {
        let response = self
            .client
            .get(format!("{}/relay/auth/oauth/google/start", self.base))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        let cookie = oauth_cookie(&response).expect("OAuth browser cookie");
        let location = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        (location, cookie)
    }

    /// Returns from the provider as `subject`; returns the relay's redirect target.
    async fn callback(&self, provider_url: &str, cookie: &str, subject: &str) -> String {
        let url = url::Url::parse(provider_url).unwrap();
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .unwrap();
        let response = self
            .client
            .get(format!("{}/relay/auth/oauth/google/callback", self.base))
            .query(&[("code", subject), ("state", &state)])
            .header(header::COOKIE, format!("remote_codex_oauth={cookie}"))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_redirection(), "{}", response.status());
        response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned()
    }

    /// Starts with an invite code (or GET when `None`) and returns as `subject`.
    async fn sign_up(&self, invite: Option<&str>, subject: &str) -> String {
        let (url, cookie) = match invite {
            Some(code) => {
                let (status, body, cookie) = self.post_start(json!({ "inviteCode": code })).await;
                assert_eq!(status, StatusCode::OK, "{body}");
                (body["url"].as_str().unwrap().to_owned(), cookie.unwrap())
            }
            None => self.get_start().await,
        };
        self.callback(&url, &cookie, subject).await
    }

    async fn count(&self, sql: &str, subject: &str) -> i64 {
        let conn = self.state.store.conn.lock().await;
        conn.query_row(sql, [subject], |row| row.get(0)).unwrap()
    }

    async fn challenges(&self) -> i64 {
        let conn = self.state.store.conn.lock().await;
        conn.query_row("SELECT count(*) FROM relay_oauth_challenges", [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    /// (users, linked identities, pending registrations) for `subject`.
    async fn records(&self, subject: &str) -> (i64, i64, i64) {
        (
            self.count(
                "SELECT count(*) FROM relay_users WHERE email=?1||'@example.test'",
                subject,
            )
            .await,
            self.count(
                "SELECT count(*) FROM relay_user_identities WHERE provider='google' AND provider_subject=?1",
                subject,
            )
            .await,
            self.count(
                "SELECT count(*) FROM relay_pending_registrations
                 WHERE provider_subject=?1 AND status='pending'",
                subject,
            )
            .await,
        )
    }
}

fn oauth_cookie(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            let value = value.strip_prefix("remote_codex_oauth=")?;
            assert!(value.contains("Path=/relay/auth/oauth"), "{value}");
            value.split(';').next().map(str::to_owned)
        })
}

fn oauth_error(location: &str) -> Option<String> {
    let query = location.split_once('?')?.1;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == "oauthError")
        .map(|(_, value)| value.into_owned())
}

const SIGNED_IN: &str = "/relay-portal";

#[tokio::test]
async fn oauth_sign_up_requires_the_invite_code_and_linked_accounts_sign_in_without_it() {
    let f = OAuthFixture::new("oauth-invite", &[("registrationPassword", INVITE)]).await;

    // (a) No code: from a sign-in button (GET) or an empty field.
    let location = f.sign_up(None, "nocode").await;
    assert_eq!(
        oauth_error(&location).as_deref(),
        Some(OAUTH_INVITE_REQUIRED)
    );
    let location = f.sign_up(Some("   "), "nocode").await;
    assert_eq!(
        oauth_error(&location).as_deref(),
        Some(OAUTH_INVITE_REQUIRED)
    );
    assert_eq!(f.records("nocode").await, (0, 0, 0));

    // (b) A wrong code is refused at the start, before the provider.
    let challenges = f.challenges().await;
    let (status, body, cookie) = f.post_start(json!({ "inviteCode": "wrong-code" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "invalid_invite_code");
    assert_eq!(body["message"], "The invite code is not valid.");
    assert!(cookie.is_none());
    assert_eq!(f.challenges().await, challenges);

    // (c) The right code: the provider URL comes back as JSON, and only a hash
    // of the code is kept with the pending login.
    let (status, body, cookie) = f
        .post_start(json!({ "inviteCode": format!(" {INVITE} ") }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let url = body["url"].as_str().unwrap();
    assert!(
        url.starts_with(&format!("{}/authorize?", f.provider)),
        "{url}"
    );
    assert!(!url.contains(INVITE));
    let stored: Vec<Option<String>> = {
        let conn = f.state.store.conn.lock().await;
        let mut statement = conn
            .prepare("SELECT invite_hash FROM relay_oauth_challenges")
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let expected_hash = security::token_hash(INVITE);
    assert!(stored.contains(&Some(expected_hash.clone())), "{stored:?}");
    assert!(stored.iter().flatten().all(|hash| !hash.contains(INVITE)));
    let location = f.callback(url, &cookie.unwrap(), "invited").await;
    assert_eq!(location, SIGNED_IN);
    assert_eq!(f.records("invited").await, (1, 1, 0));

    // (d) The linked identity signs in with no code at all.
    let location = f.sign_up(None, "invited").await;
    assert_eq!(location, SIGNED_IN);
    assert_eq!(f.records("invited").await, (1, 1, 0));

    // A code changed between the start and the callback no longer matches.
    let (status, body, cookie) = f.post_start(json!({ "inviteCode": INVITE })).await;
    assert_eq!(status, StatusCode::OK);
    f.setting("registrationPassword", "rotated-code-2").await;
    let location = f
        .callback(body["url"].as_str().unwrap(), &cookie.unwrap(), "late")
        .await;
    assert_eq!(
        oauth_error(&location).as_deref(),
        Some("The invite code is not valid.")
    );
    assert_eq!(f.records("late").await, (0, 0, 0));
}

#[tokio::test]
async fn approval_with_an_invite_code_queues_only_invited_oauth_sign_ups() {
    let f = OAuthFixture::new(
        "oauth-invite-approval",
        &[
            ("registrationPassword", INVITE),
            ("registrationApprovalRequired", "true"),
        ],
    )
    .await;
    let location = f.sign_up(None, "uninvited").await;
    assert_eq!(
        oauth_error(&location).as_deref(),
        Some(OAUTH_INVITE_REQUIRED)
    );
    assert_eq!(f.records("uninvited").await, (0, 0, 0));

    let location = f.sign_up(Some(INVITE), "queued").await;
    assert_eq!(location, "/relay-portal?oauthPending=1");
    assert_eq!(f.records("queued").await, (0, 0, 1));
    let (request_id, provider, status): (String, String, String) = {
        let conn = f.state.store.conn.lock().await;
        conn.query_row(
            "SELECT id,provider,status FROM relay_pending_registrations WHERE provider_subject='queued'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
    };
    assert_eq!((provider.as_str(), status.as_str()), ("google", "pending"));

    let admin = {
        let conn = f.state.store.conn.lock().await;
        let token = create_session(&conn, &f.state.store.session_secret, "admin").unwrap();
        account::mark_session(&conn, &token, account::FULL).unwrap();
        token
    };
    let response = f
        .client
        .post(format!(
            "{}/relay/admin/registrations/{request_id}/approve",
            f.base
        ))
        .bearer_auth(&admin)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(f.records("queued").await, (1, 1, 0));
    // Approved and linked: signs in from a sign-in button, no code.
    assert_eq!(f.sign_up(None, "queued").await, SIGNED_IN);
}

#[tokio::test]
async fn closed_registration_refuses_new_oauth_identities_even_with_the_invite_code() {
    let f = OAuthFixture::new("oauth-invite-closed", &[("registrationPassword", INVITE)]).await;
    assert_eq!(f.sign_up(Some(INVITE), "early").await, SIGNED_IN);
    f.setting("registrationEnabled", "false").await;

    // The start still works (a linked account may be signing in); the callback decides.
    let location = f.sign_up(Some(INVITE), "late").await;
    assert_eq!(
        oauth_error(&location).as_deref(),
        Some("Registration is currently disabled.")
    );
    assert_eq!(f.records("late").await, (0, 0, 0));
    assert_eq!(f.sign_up(None, "early").await, SIGNED_IN);
}

#[tokio::test]
async fn oauth_start_get_redirects_and_post_returns_the_provider_url() {
    let f = OAuthFixture::new("oauth-invite-start", &[]).await;
    // GET is unchanged: a redirect, the browser cookie, no invite kept.
    let (location, _) = f.get_start().await;
    assert!(location.starts_with(&format!("{}/authorize?", f.provider)));
    let stored: Option<String> = {
        let conn = f.state.store.conn.lock().await;
        conn.query_row(
            "SELECT invite_hash FROM relay_oauth_challenges",
            [],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(stored, None);

    // Without a configured code any code (or none) is accepted.
    let (status, body, cookie) = f.post_start(json!({ "inviteCode": "anything" })).await;
    assert_eq!(status, StatusCode::OK);
    assert!(cookie.is_some());
    assert_eq!(body.as_object().unwrap().len(), 1, "{body}");
    let (status, _, cookie) = f.post_start(json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(cookie.is_some());
    assert_eq!(f.sign_up(Some(""), "open").await, SIGNED_IN);
    assert_eq!(f.records("open").await, (1, 1, 0));

    // The provider checks of the GET start apply to the POST start too.
    let response = f
        .client
        .post(format!("{}/relay/auth/oauth/github/start", f.base))
        .header("origin", &f.base)
        .json(&json!({ "inviteCode": INVITE }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    f.setting("googleAuthEnabled", "false").await;
    let (status, _, cookie) = f.post_start(json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(cookie.is_none());
    let response = f
        .client
        .post(format!("{}/relay/auth/oauth/google/start", f.base))
        .header("origin", &f.base)
        .header(header::CONTENT_TYPE, "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
