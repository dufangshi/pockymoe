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
    conn.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS relay_mobile_oauth_code ON relay_mobile_oauth(code_hash);")?;
    Ok(())
}
pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/relay/auth/mobile/prepare", post(prepare))
        .route("/relay/auth/mobile/exchange", post(exchange))
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
    Json(input): Json<Prepare>,
) -> Response {
    // Limit every attempt, including malformed/incorrect invites, before DB work.
    // Never trust forwarded IP headers supplied by unauthenticated browsers.
    let peer = peer
        .map(|p| p.0 .0.ip().to_string())
        .unwrap_or_else(|| "unknown".into());
    if !state
        .admission
        .allow(format!("mobile-prepare-ip:{peer}"), 5, 600)
        || !state
            .admission
            .allow("mobile-prepare-global".into(), 200, 600)
    {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    if OAuthProvider::parse(&input.provider).is_none()
        || input.proof_hash.len() != 43
        || !input
            .proof_hash
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let conn = state.store.conn.lock().await;
    let now = chrono::Utc::now().timestamp_millis();
    let id = Uuid::new_v4().to_string();
    let result = (|| -> Result<()> {
        conn.execute("DELETE FROM relay_mobile_oauth WHERE expires_at<=?1", [now])?;
        let count: i64 =
            conn.query_row("SELECT count(*) FROM relay_mobile_oauth", [], |r| r.get(0))?;
        anyhow::ensure!(count < 10000, "Too many pending mobile logins");
        let invite = input.invite_code.as_deref().map(invite_hash);
        if let (Some(supplied), Some(expected)) = (
            invite.as_deref(),
            relay_setting(&conn, "registrationPassword"),
        ) {
            anyhow::ensure!(
                invite_code_matches(Some(supplied), &expected),
                "Invalid invite code"
            );
        }
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
    async fn prepare_is_bounded_per_peer_and_globally_before_invite_validation() {
        let (state, dir) = crate::tests::test_app_state("mobile-rate");
        for index in 0..6 {
            let source_peer = peer(format!("127.0.0.1:{}", 40000 + index).parse().unwrap());
            let result = prepare(State(state.clone()), source_peer, Json(input())).await;
            assert_eq!(
                result.status(),
                if index < 5 {
                    StatusCode::OK
                } else {
                    StatusCode::TOO_MANY_REQUESTS
                }
            );
        }
        // Rejected attempts never create rows and a second peer is independent.
        let source_peer = peer("127.0.0.2:40000".parse().unwrap());
        assert_eq!(
            prepare(State(state.clone()), source_peer, Json(input()))
                .await
                .status(),
            StatusCode::OK
        );
        for index in 0..194 {
            let source_peer = peer(std::net::SocketAddr::new(
                std::net::Ipv4Addr::new(10, 0, (index / 250) as u8, (index % 250 + 1) as u8).into(),
                40000,
            ));
            assert_eq!(
                prepare(State(state.clone()), source_peer, Json(input()))
                    .await
                    .status(),
                StatusCode::OK
            );
        }
        let source_peer = peer("192.0.2.1:40000".parse().unwrap());
        assert_eq!(
            prepare(State(state.clone()), source_peer, Json(input()))
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        let count: i64 = state
            .store
            .conn
            .lock()
            .await
            .query_row("SELECT count(*) FROM relay_mobile_oauth", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 200);
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
            let response = prepare(State(state.clone()), None, Json(value)).await;
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
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
