//! System-browser OAuth handoff. Only the originating WebView holds the proof;
//! the callback contains a fresh secret code, never the public preparation ID.
//! Both that code and the WebView proof are required; neither alone grants login.
use super::*;

pub(super) const CALLBACK: &str = "remotecodex://auth/callback";
pub(super) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS relay_mobile_oauth (
      id TEXT PRIMARY KEY, proof_hash TEXT NOT NULL, provider TEXT NOT NULL,
      invite_hash TEXT, oauth_state TEXT UNIQUE, user_id TEXT REFERENCES relay_users(id) ON DELETE CASCADE,
      return_path TEXT, expires_at INTEGER NOT NULL);")?;
    let has_code: bool = conn
        .prepare("PRAGMA table_info(relay_mobile_oauth)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "code_hash");
    if !has_code {
        conn.execute_batch("ALTER TABLE relay_mobile_oauth ADD COLUMN code_hash TEXT;")?;
    }
    if conn
        .prepare("SELECT consent_hash FROM relay_mobile_oauth LIMIT 0")
        .is_err()
    {
        conn.execute_batch("ALTER TABLE relay_mobile_oauth ADD COLUMN consent_hash TEXT;")?;
    }
    conn.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS relay_mobile_oauth_code ON relay_mobile_oauth(code_hash);")?;
    Ok(())
}
pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/relay/auth/mobile/prepare", post(prepare))
        .route("/relay/auth/mobile/exchange", post(exchange))
        .route("/relay/auth/mobile/consent", post(consent))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Prepare {
    provider: String,
    proof_hash: String,
    invite_code: Option<String>,
}
async fn prepare(
    State(state): State<Arc<AppState>>,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    Json(input): Json<Prepare>,
) -> Response {
    // Both auth middleware and prepare use the same trusted-proxy policy.
    let peer = state
        .trusted_proxies
        .client_ip(peer.map(|p| p.0 .0.ip()), &headers);
    if !state
        .admission
        .allow(format!("mobile-prepare-ip:{peer}"), 5, 600)
    {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let provider = OAuthProvider::parse(&input.provider);
    let valid_proof = input.proof_hash.len() == 43
        && input
            .proof_hash
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    let conn = state.store.conn.lock().await;
    let now = chrono::Utc::now().timestamp_millis();
    let id = Uuid::new_v4().to_string();
    let invite = input.invite_code.as_deref().map(invite_hash);
    let configured_invite = relay_setting(&conn, "registrationPassword");
    let invalid_invite = match (invite.as_deref(), configured_invite.as_deref()) {
        (Some(supplied), Some(expected)) => !invite_code_matches(Some(supplied), expected),
        _ => false,
    };
    let protected = configured_invite
        .as_deref()
        .is_some_and(|expected| invite_code_matches(invite.as_deref(), expected))
        || authenticated_user(
            &conn,
            &state.store.session_secret,
            &headers,
            &TokenQuery::default(),
        )
        .is_some();
    // Anonymous preparation cannot consume the capacity reserved for people
    // with an invite or a valid account session. Providers have independent
    // generous budgets; this is not a global five-request proxy bucket.
    if !protected
        && !state.admission.allow(
            format!(
                "mobile-prepare-anonymous:{}",
                provider.map_or("invalid", OAuthProvider::as_str)
            ),
            2_000,
            600,
        )
    {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    if !protected
        && !state
            .admission
            .allow("mobile-prepare-anonymous-global".into(), 4_000, 600)
    {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    // Invalid proofs/invites also consume anonymous budgets. Header spoofing at
    // an exposed origin must not turn IP-bucket rotation into an unbounded oracle.
    if provider.is_none() || !valid_proof || invalid_invite {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let result = (|| -> Result<()> {
        conn.execute("DELETE FROM relay_mobile_oauth WHERE expires_at<=?1", [now])?;
        let count: i64 =
            conn.query_row("SELECT count(*) FROM relay_mobile_oauth", [], |r| r.get(0))?;
        anyhow::ensure!(count < 10000, "Too many pending mobile logins");
        conn.execute("INSERT INTO relay_mobile_oauth(id,proof_hash,provider,invite_hash,expires_at) VALUES (?1,?2,?3,?4,?5)",params![id,input.proof_hash,input.provider,invite,now+600000])?;
        Ok(())
    })();
    if result.is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = format!("/relay/auth/oauth/{}/start?mobileId={id}", input.provider);
    // The page resolves this against its configured Relay, including self-hosts.
    Json(json!({"path":path,"callbackUrl":CALLBACK})).into_response()
}

fn consent_html(
    message: &str,
    form: &str,
    status: StatusCode,
    provider_origin: Option<&str>,
) -> Response {
    let html = include_str!("mobile_consent.html")
        .replace("{{MESSAGE}}", message)
        .replace("{{FORM}}", form);
    let mut response = (status, axum::response::Html(html)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    // Chromium applies form-action to redirects as well as the POST target.
    // Allow only this configured provider and the fixed app callback scheme.
    let policy=format!("default-src 'none'; style-src 'unsafe-inline'; form-action 'self' {} remotecodex:; base-uri 'none'; frame-ancestors 'none'",provider_origin.unwrap_or(""));
    response
        .headers_mut()
        .insert(header::CONTENT_SECURITY_POLICY, policy.parse().unwrap());
    // A no-referrer form navigation can send Origin: null. Keep the Origin
    // proof for this same-origin POST; no referrer goes to the external provider.
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "same-origin".parse().unwrap());
    response
}
fn consent_expired() -> Response {
    consent_html(
        "登录请求已过期或不属于此浏览器，请返回 App 重试。",
        "",
        StatusCode::BAD_REQUEST,
        None,
    )
}

pub(super) async fn consent_page(state: &AppState, id: &str, provider: &str) -> Response {
    let Some(kind) = OAuthProvider::parse(provider) else {
        return consent_expired();
    };
    let Ok(provider_origin) = state.oauth.authorization_origin(kind) else {
        return consent_expired();
    };
    let conn = state.store.conn.lock().await;
    if invite(&conn, id, provider).is_err() {
        return consent_expired();
    }
    let nonce = security::random_token();
    let result=conn.execute("UPDATE relay_mobile_oauth SET consent_hash=?2 WHERE id=?1 AND oauth_state IS NULL AND code_hash IS NULL AND expires_at>?3",params![id,security::token_hash(&nonce),chrono::Utc::now().timestamp_millis()]);
    if !matches!(result, Ok(1)) {
        return consent_expired();
    }
    // Both substitutions are server-generated UUID/base64url values, not markup.
    let form = format!(
        r#"<form method="post" action="/relay/auth/mobile/consent"><input type="hidden" name="id" value="{id}"><input type="hidden" name="csrf" value="{nonce}"><div class="actions"><button type="submit" name="decision" value="continue">继续</button><button type="submit" name="decision" value="cancel">取消</button></div></form>"#
    );
    let mut response = consent_html(
        "如果不是你刚刚发起的，请关闭此页",
        &form,
        StatusCode::OK,
        Some(&provider_origin),
    );
    response.headers_mut().insert(header::SET_COOKIE,format!("remote_codex_mobile_consent={nonce}; HttpOnly; Secure; SameSite=Lax; Path=/relay/auth/mobile; Max-Age=600").parse().unwrap());
    response
}
#[derive(Deserialize)]
struct Consent {
    id: String,
    csrf: String,
    decision: String,
}
async fn consent(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Form(input): axum::Form<Consent>,
) -> Response {
    if !security::origin_allowed(&headers, state.oauth.public_base_url.as_deref()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(cookie) = security::cookie(&headers, "remote_codex_mobile_consent") else {
        return consent_expired();
    };
    if input.csrf.len() != 43
        || !bool::from(cookie.as_bytes().ct_eq(input.csrf.as_bytes()))
        || !["continue", "cancel"].contains(&input.decision.as_str())
    {
        return consent_expired();
    }
    let pending = {
        let conn = state.store.conn.lock().await;
        conn.query_row("UPDATE relay_mobile_oauth SET consent_hash=NULL WHERE id=?1 AND consent_hash=?2 AND oauth_state IS NULL AND code_hash IS NULL AND expires_at>?3 RETURNING provider,invite_hash",params![input.id,security::token_hash(&input.csrf),chrono::Utc::now().timestamp_millis()],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?))).optional().ok().flatten()
    };
    let Some((provider, invite)) = pending else {
        return consent_expired();
    };
    if input.decision == "cancel" {
        let conn = state.store.conn.lock().await;
        let code = security::random_token();
        let path = format!("/relay-portal?oauthCode=oauth_cancelled&oauthProvider={provider}");
        if conn.execute("UPDATE relay_mobile_oauth SET code_hash=?2,return_path=?3 WHERE id=?1 AND code_hash IS NULL",params![input.id,security::token_hash(&code),path]).is_err() {return StatusCode::INTERNAL_SERVER_ERROR.into_response()}
        return Redirect::to(&format!("{CALLBACK}?code={code}")).into_response();
    }
    match begin_oauth(&state, &headers, &provider, invite.as_deref()).await {
        Ok((target, cookie)) => {
            if bind(&*state.store.conn.lock().await, &input.id, &target).is_err() {
                return consent_expired();
            }
            let mut response = Redirect::to(&target).into_response();
            response.headers_mut().insert(header::SET_COOKIE, cookie);
            response
        }
        Err(response) => response,
    }
}

pub(super) fn invite(conn: &Connection, id: &str, provider: &str) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT invite_hash FROM relay_mobile_oauth WHERE id=?1 AND provider=?2 AND oauth_state IS NULL AND expires_at>?3",params![id,provider,chrono::Utc::now().timestamp_millis()],|r|r.get(0))?)
}
pub(super) fn bind(conn: &Connection, id: &str, target: &str) -> Result<()> {
    let target = url::Url::parse(target)?;
    let signed = target
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .ok_or_else(|| anyhow::anyhow!("Missing state"))?;
    anyhow::ensure!(conn.execute("UPDATE relay_mobile_oauth SET oauth_state=?2 WHERE id=?1 AND oauth_state IS NULL AND expires_at>?3",params![id,signed,chrono::Utc::now().timestamp_millis()])? == 1, "Expired mobile login");
    Ok(())
}
fn finish(
    conn: &Connection,
    signed: &str,
    user: Option<&str>,
    path: &str,
) -> Result<Option<Response>> {
    // The preparer knows id and proof. Only the system-browser callback gets this
    // fresh 256-bit code; storing its hash prevents the public ID being redeemed.
    let code = security::random_token();
    let changed = conn.execute("UPDATE relay_mobile_oauth SET user_id=?2,return_path=?3,code_hash=?4 WHERE oauth_state=?1 AND code_hash IS NULL AND expires_at>?5",
        params![signed,user,path,security::token_hash(&code),chrono::Utc::now().timestamp_millis()])?;
    Ok((changed == 1)
        .then(|| Redirect::temporary(&format!("{CALLBACK}?code={code}")).into_response()))
}
pub(super) fn complete(conn: &Connection, signed: &str, user: &str) -> Result<Option<Response>> {
    finish(conn, signed, Some(user), "/relay-portal")
}
// Preserve the existing localized refusal/pending-sign-up route across the
// browser boundary, but never accept a client-selected redirect destination.
pub(super) fn redirect(conn: &Connection, signed: &str, response: Response) -> Response {
    let Some(location) = response
        .headers()
        .get(header::LOCATION)
        .and_then(|h| h.to_str().ok())
    else {
        return response;
    };
    let Ok(url) = url::Url::parse("https://mobile.invalid").and_then(|base| base.join(location))
    else {
        return response;
    };
    if url.path() != "/relay-portal" {
        return response;
    }
    let path = format!(
        "/relay-portal{}",
        url.query().map(|q| format!("?{q}")).unwrap_or_default()
    );
    match finish(conn, signed, None, &path) {
        Ok(Some(response)) => response,
        Ok(None) => response,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
struct Exchange {
    #[serde(default)]
    code: String,
    #[serde(default)]
    proof: String,
}
async fn exchange(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<Exchange>,
) -> Response {
    // The same response for a missing/invalid code, pending, expired, unknown,
    // wrong proof or consumed login: polling a known preparation ID is useless.
    if input.proof.len() != 64 || input.code.len() != 43 {
        return unauthorized();
    }
    let conn = state.store.conn.lock().await;
    let hash = security::token_hash(&input.proof);
    let code_hash = security::token_hash(&input.code);
    let row: Option<(Option<String>,String)> = conn.query_row("DELETE FROM relay_mobile_oauth WHERE code_hash=?1 AND proof_hash=?2 AND return_path IS NOT NULL AND expires_at>?3 RETURNING user_id,return_path",params![code_hash,hash,chrono::Utc::now().timestamp_millis()],|r|Ok((r.get(0)?,r.get(1)?))).optional().ok().flatten();
    let Some((user, path)) = row else {
        return unauthorized();
    };
    let Some(user) = user else {
        return Json(json!({"path":path})).into_response();
    };
    let Some(user) = load_user_by_id(&conn, &user).filter(|u| u.enabled != 0) else {
        return unauthorized();
    };
    // Issue cookies in the WebView, with the same MFA and browser trust policy.
    let result = if auth_factors::has_factors(&conn, &user.id)
        && !auth_factors::trusted(&conn, &user.id, &headers)
    {
        auth_api::challenge_response(&conn, &user, &headers)
    } else {
        auth_api::login_response(&conn, &state, &user, &headers, true, false)
    };
    result.unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn peer(
        address: std::net::SocketAddr,
    ) -> Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>> {
        Some(axum::Extension(axum::extract::ConnectInfo(address)))
    }
    fn input() -> Prepare {
        Prepare {
            provider: "google".into(),
            proof_hash: security::token_hash(&"ab".repeat(32)),
            invite_code: None,
        }
    }
    #[tokio::test]
    async fn prepare_proxied_clients_have_independent_buckets_and_untrusted_peers_cannot_spoof() {
        let (mut state, dir) = crate::tests::test_app_state("mobile-proxy-rate");
        Arc::get_mut(&mut state).unwrap().trusted_proxies =
            security::TrustedProxies::parse("127.0.0.1")
                .unwrap()
                .with_client_ip_header("CF-Connecting-IP")
                .unwrap();
        for index in 0..6 {
            let mut headers = HeaderMap::new();
            headers.insert("cf-connecting-ip", "198.51.100.10".parse().unwrap());
            assert_eq!(
                prepare(
                    State(state.clone()),
                    peer("127.0.0.1:40000".parse().unwrap()),
                    headers,
                    Json(input())
                )
                .await
                .status(),
                if index < 5 {
                    StatusCode::OK
                } else {
                    StatusCode::TOO_MANY_REQUESTS
                }
            );
        }
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", "198.51.100.11".parse().unwrap());
        assert_eq!(
            prepare(
                State(state.clone()),
                peer("127.0.0.1:40000".parse().unwrap()),
                headers,
                Json(input())
            )
            .await
            .status(),
            StatusCode::OK
        );
        for index in 0..6 {
            let mut headers = HeaderMap::new();
            headers.insert(
                "cf-connecting-ip",
                format!("198.51.100.{}", 50 + index).parse().unwrap(),
            );
            assert_eq!(
                prepare(
                    State(state.clone()),
                    peer("192.0.2.1:40000".parse().unwrap()),
                    headers,
                    Json(input())
                )
                .await
                .status(),
                if index < 5 {
                    StatusCode::OK
                } else {
                    StatusCode::TOO_MANY_REQUESTS
                }
            );
        }
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn anonymous_provider_exhaustion_preserves_other_provider_invites_and_accounts() {
        let (state, dir) = crate::tests::test_app_state("mobile-provider-rate");
        for _ in 0..2000 {
            assert!(state
                .admission
                .allow("mobile-prepare-anonymous:google".into(), 2000, 600));
        }
        assert_eq!(
            prepare(
                State(state.clone()),
                peer("192.0.2.1:40000".parse().unwrap()),
                HeaderMap::new(),
                Json(input())
            )
            .await
            .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        let mut github = input();
        github.provider = "github".into();
        assert_eq!(
            prepare(
                State(state.clone()),
                peer("192.0.2.2:40000".parse().unwrap()),
                HeaderMap::new(),
                Json(github)
            )
            .await
            .status(),
            StatusCode::OK
        );
        for _ in 0..3999 {
            assert!(state
                .admission
                .allow("mobile-prepare-anonymous-global".into(), 4000, 600));
        }
        let mut other_provider = input();
        other_provider.provider = "github".into();
        assert_eq!(
            prepare(
                State(state.clone()),
                peer("192.0.2.5:40000".parse().unwrap()),
                HeaderMap::new(),
                Json(other_provider)
            )
            .await
            .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        let token = {
            let conn = state.store.conn.lock().await;
            set_relay_setting(&conn, "registrationPassword", "synthetic-invite").unwrap();
            conn.execute("INSERT INTO relay_users VALUES ('owner','owner','owner','user',1,NULL,'now','salt','hash')",[]).unwrap();
            create_session(&conn, &state.store.session_secret, "owner").unwrap()
        };
        let mut invited = input();
        invited.invite_code = Some("synthetic-invite".into());
        assert_eq!(
            prepare(
                State(state.clone()),
                peer("192.0.2.3:40000".parse().unwrap()),
                HeaderMap::new(),
                Json(invited)
            )
            .await
            .status(),
            StatusCode::OK
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        assert_eq!(
            prepare(
                State(state.clone()),
                peer("192.0.2.4:40000".parse().unwrap()),
                headers,
                Json(input())
            )
            .await
            .status(),
            StatusCode::OK
        );
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn incorrect_invites_and_malformed_proofs_consume_the_same_prepare_budget() {
        let (state, dir) = crate::tests::test_app_state("mobile-invite-rate");
        set_relay_setting(
            &*state.store.conn.lock().await,
            "registrationPassword",
            "synthetic-invite",
        )
        .unwrap();
        for index in 0..6 {
            let mut value = input();
            value.invite_code = Some("incorrect".into());
            if index % 2 == 0 {
                value.proof_hash = "invalid".into();
            }
            let response = prepare(State(state.clone()), None, HeaderMap::new(), Json(value)).await;
            assert_eq!(
                response.status(),
                if index < 5 {
                    StatusCode::BAD_REQUEST
                } else {
                    StatusCode::TOO_MANY_REQUESTS
                }
            );
        }
        assert_eq!(
            state
                .store
                .conn
                .lock()
                .await
                .query_row("SELECT count(*) FROM relay_mobile_oauth", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        // The five malformed/incorrect attempts above count globally as well.
        for _ in 0..3995 {
            assert!(state
                .admission
                .allow("mobile-prepare-anonymous-global".into(), 4000, 600));
        }
        assert!(!state
            .admission
            .allow("mobile-prepare-anonymous-global".into(), 4000, 600));
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
