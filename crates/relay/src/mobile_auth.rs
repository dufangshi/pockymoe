//! System-browser OAuth handoff. Only the originating WebView holds the proof;
//! the deep link contains an expiring one-use identifier, never a login session.
use super::*;

pub(super) const CALLBACK: &str = "remotecodex://auth/callback";
pub(super) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS relay_mobile_oauth (
      id TEXT PRIMARY KEY, proof_hash TEXT NOT NULL, provider TEXT NOT NULL,
      invite_hash TEXT, oauth_state TEXT UNIQUE, user_id TEXT REFERENCES relay_users(id) ON DELETE CASCADE,
      return_path TEXT, expires_at INTEGER NOT NULL);")?;
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
async fn prepare(State(state): State<Arc<AppState>>, Json(input): Json<Prepare>) -> Response {
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
pub(super) fn complete(conn: &Connection, signed: &str, user: &str) -> Result<Option<Response>> {
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM relay_mobile_oauth WHERE oauth_state=?1 AND expires_at>?2",
            params![signed, chrono::Utc::now().timestamp_millis()],
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = id else {
        return Ok(None);
    };
    conn.execute(
        "UPDATE relay_mobile_oauth SET user_id=?2,return_path='/relay-portal' WHERE id=?1",
        params![id, user],
    )?;
    Ok(Some(
        Redirect::temporary(&format!("{CALLBACK}?id={id}")).into_response(),
    ))
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
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM relay_mobile_oauth WHERE oauth_state=?1 AND expires_at>?2",
            params![signed, chrono::Utc::now().timestamp_millis()],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let Some(id) = id else {
        return response;
    };
    if conn
        .execute(
            "UPDATE relay_mobile_oauth SET return_path=?2 WHERE id=?1",
            params![id, path],
        )
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    Redirect::temporary(&format!("{CALLBACK}?id={id}")).into_response()
}
#[derive(Deserialize)]
struct Exchange {
    id: String,
    proof: String,
}
async fn exchange(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<Exchange>,
) -> Response {
    if input.proof.len() != 64 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let conn = state.store.conn.lock().await;
    let hash = security::token_hash(&input.proof);
    let row: Option<(Option<String>,String)> = conn.query_row("SELECT user_id,return_path FROM relay_mobile_oauth WHERE id=?1 AND proof_hash=?2 AND return_path IS NOT NULL AND expires_at>?3",params![input.id,hash,chrono::Utc::now().timestamp_millis()],|r|Ok((r.get(0)?,r.get(1)?))).optional().ok().flatten();
    let Some((user, path)) = row else {
        return unauthorized();
    };
    if conn
        .execute("DELETE FROM relay_mobile_oauth WHERE id=?1", [&input.id])
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
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
