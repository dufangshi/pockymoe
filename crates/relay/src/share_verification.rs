//! The safety-code check before shared access starts
//! (docs/account-key-trust.md#safety-code-check).
//!
//! With the owner's `requireSafetyCode` preference on (the default), a new share
//! or grant is created pending. A pending row gives no access: the relay refuses
//! the recipient with `share_pending` and the details the Web needs to show them
//! their safety code. The owner approves it with the code the recipient sent;
//! a protected device additionally needs the owner's signed grant, which the
//! owner's browser signs only at that approval.
use super::*;

pub(super) fn ensure_schema(conn: &Connection) -> Result<()> {
    // Rows from before the check keep `pending = 0`: existing access stays active.
    for table in ["relay_shares", "relay_access_grants"] {
        for (column, definition) in [
            ("pending", "INTEGER NOT NULL DEFAULT 0"),
            ("verified_at", "TEXT"),
        ] {
            if conn
                .prepare(&format!("SELECT {column} FROM {table} LIMIT 0"))
                .is_err()
            {
                conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                    [],
                )?;
            }
        }
    }
    Ok(())
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/relay/shares/{id}/approve", post(approve_share))
        .route("/relay/grants/{id}/approve", post(approve_grant))
}

const fn table(grant: bool) -> &'static str {
    if grant {
        "relay_access_grants"
    } else {
        "relay_shares"
    }
}

/// Holds a row the owner just created when their preference asks for the check.
pub(super) fn hold_new(conn: &Connection, grant: bool, id: &str, owner: &str) -> bool {
    preferences::require_safety_code(conn, owner)
        && conn
            .execute(
                &format!("UPDATE {} SET pending=1 WHERE id=?1", table(grant)),
                [id],
            )
            .is_ok_and(|changed| changed == 1)
}

/// Adds `pending` and `verifiedAt` to portal rows of one table.
pub(super) fn enrich(conn: &Connection, grant: bool, rows: &mut [Value]) {
    let sql = format!(
        "SELECT pending,verified_at FROM {} WHERE id=?1",
        table(grant)
    );
    for row in rows {
        let Some(id) = row["id"].as_str().map(str::to_owned) else {
            continue;
        };
        let (pending, verified_at) = conn
            .query_row(&sql, [&id], |found| {
                Ok((
                    found.get::<_, i64>(0)? != 0,
                    found.get::<_, Option<String>>(1)?,
                ))
            })
            .unwrap_or((false, None));
        row["pending"] = Value::Bool(pending);
        row["verifiedAt"] = verified_at.map_or(Value::Null, Value::String);
    }
}

/// The safety code of an account key: the first 12 characters of the Crockford
/// base32 encoding of `SHA256("pockymoe-acct-safety-v1\n" + accountKeyId)`,
/// in groups of four (the Web computes the same).
pub(super) fn safety_code(account_key_id: &str) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let digest = Sha256::digest(format!("pockymoe-acct-safety-v1\n{account_key_id}").as_bytes());
    let (mut bits, mut value, mut out) = (0_u32, 0_u32, String::new());
    for byte in digest {
        value = ((value << 8) | u32::from(byte)) & 0xffff;
        bits += 8;
        while bits >= 5 && out.len() < 12 {
            bits -= 5;
            out.push(char::from(ALPHABET[((value >> bits) & 31) as usize]));
        }
        if out.len() == 12 {
            break;
        }
    }
    out.as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// What people type: case, spaces and dashes do not matter; `O` reads as `0`
/// and `I`/`L` as `1`, as for recovery codes.
pub(super) fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect()
}

/// A `share_pending` refusal for a device API path, when the user has no live
/// access to it but a share or grant there waits for the check.
pub(super) fn path_refusal(
    conn: &Connection,
    user_id: &str,
    device_id: &str,
    path: &str,
) -> Option<Response> {
    refusal(
        conn,
        user_id,
        device_id,
        resource_id_from_path(path, "threads").as_deref(),
        resource_id_from_path(path, "workspaces").as_deref(),
    )
}

pub(super) fn refusal(
    conn: &Connection,
    user_id: &str,
    device_id: &str,
    thread_id: Option<&str>,
    workspace_id: Option<&str>,
) -> Option<Response> {
    let matched = crate::pending_access(conn, user_id, device_id, thread_id, workspace_id)
        .and_then(|access| match (access.share_id, access.grant_id) {
            (Some(id), _) => Some((false, id)),
            (_, Some(id)) => Some((true, id)),
            _ => None,
        });
    // A path that names no thread or workspace (registration, the device's
    // own lists) points at the newest pending row on the device, if the user
    // has nothing live there.
    let (grant, id) = match matched {
        Some(found) => found,
        None if thread_id.is_none() && workspace_id.is_none() => {
            newest_pending(conn, user_id, device_id)?
        }
        None => return None,
    };
    let share = pending_summary(conn, grant, &id)?;
    let owner = share["ownerUsername"]
        .as_str()
        .unwrap_or("The owner")
        .to_owned();
    Some(
        (
            StatusCode::FORBIDDEN,
            Json(json!({
                "code": "share_pending",
                "message": format!("{owner} has not confirmed your safety code yet, so this shared access has not started."),
                "share": share,
            })),
        )
            .into_response(),
    )
}

fn newest_pending(conn: &Connection, user_id: &str, device_id: &str) -> Option<(bool, String)> {
    conn.query_row(
        "SELECT kind,id FROM (
           SELECT 0 AS kind,id,created_at FROM relay_shares
           WHERE target_user_id=?1 AND device_id=?2 AND pending=1 AND revoked_at IS NULL
             AND (expires_at IS NULL OR expires_at>?3)
           UNION ALL
           SELECT 1,id,created_at FROM relay_access_grants
           WHERE target_user_id=?1 AND device_id=?2 AND pending=1 AND revoked_at IS NULL
             AND (expires_at IS NULL OR expires_at>?3)
         ) ORDER BY created_at DESC LIMIT 1",
        params![user_id, device_id, now_rfc3339()],
        |row| Ok((row.get::<_, i64>(0)? == 1, row.get::<_, String>(1)?)),
    )
    .optional()
    .ok()
    .flatten()
}

/// What the recipient's waiting page names: who shared what, on which device.
fn pending_summary(conn: &Connection, grant: bool, id: &str) -> Option<Value> {
    let scope = if grant { "scope" } else { "'thread'" };
    conn.query_row(
        &format!(
            "SELECT id,{scope},owner_username,device_id,device_name,thread_id,thread_title,
                    workspace_id,workspace_label,label,thread_access,workspace_access
             FROM {} WHERE id=?1",
            table(grant)
        ),
        [id],
        |row| {
            Ok(json!({
                "kind": if grant { "grant" } else { "share" },
                "id": row.get::<_, String>(0)?,
                "scope": row.get::<_, String>(1)?,
                "ownerUsername": row.get::<_, Option<String>>(2)?,
                "deviceId": row.get::<_, String>(3)?,
                "deviceName": row.get::<_, Option<String>>(4)?,
                "threadId": row.get::<_, Option<String>>(5)?,
                "threadTitle": row.get::<_, Option<String>>(6)?,
                "workspaceId": row.get::<_, Option<String>>(7)?,
                "workspaceLabel": row.get::<_, Option<String>>(8)?,
                "label": row.get::<_, Option<String>>(9)?,
                "threadAccess": row.get::<_, String>(10)?,
                "workspaceAccess": row.get::<_, String>(11)?,
            }))
        },
    )
    .optional()
    .ok()
    .flatten()
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApproveInput {
    safety_code: Option<String>,
}

async fn approve_share(
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Json(input): Json<ApproveInput>,
) -> Response {
    approve(&state, &headers, &query, false, &id, input).await
}

async fn approve_grant(
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Json(input): Json<ApproveInput>,
) -> Response {
    approve(&state, &headers, &query, true, &id, input).await
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(ApiError::new(code, message))).into_response()
}

/// The owner confirms the code the recipient sent. A recipient with an account
/// key must be confirmed by its code; one without (no safety code exists yet)
/// can be activated without a code, which records no verification. An active
/// row accepts the same call to record that its code was checked.
async fn approve(
    state: &AppState,
    headers: &HeaderMap,
    query: &TokenQuery,
    grant: bool,
    id: &str,
    input: ApproveInput,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(owner) = authenticated_user(&conn, &state.store.session_secret, headers, query) else {
        return unauthorized();
    };
    let table = table(grant);
    let recipient: Option<String> = conn
        .query_row(
            &format!(
                "SELECT target_user_id FROM {table} WHERE id=?1 AND owner_user_id=?2
                   AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?3)"
            ),
            params![id, owner.id, now_rfc3339()],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let Some(recipient) = recipient else {
        return failure(
            StatusCode::NOT_FOUND,
            "not_found",
            "Shared access not found",
        );
    };
    let key = match crate::account::vault(&conn, &recipient) {
        Ok(vault) => vault.and_then(|vault| vault["accountKeyId"].as_str().map(str::to_owned)),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let verified = match (&key, input.safety_code.as_deref()) {
        (Some(key), Some(code)) => {
            if normalize_code(code) != normalize_code(&safety_code(key)) {
                return failure(
                    StatusCode::CONFLICT,
                    "safety_code_mismatch",
                    "This code does not match the recipient's account key.",
                );
            }
            true
        }
        (Some(_), None) => {
            return failure(
                StatusCode::BAD_REQUEST,
                "safety_code_required",
                "Enter the safety code the recipient sent you.",
            )
        }
        (None, Some(_)) => {
            return failure(
                StatusCode::CONFLICT,
                "recipient_account_missing",
                "The recipient has no account key, so they have no safety code yet.",
            )
        }
        (None, None) => false,
    };
    let now = now_rfc3339();
    if conn
        .execute(
            &format!(
                "UPDATE {table} SET pending=0,
                   verified_at=CASE WHEN ?2 THEN ?3 ELSE verified_at END
                 WHERE id=?1"
            ),
            params![id, verified, now],
        )
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    security::audit(
        &conn,
        Some(&owner.id),
        if grant {
            "grant.approved"
        } else {
            "share.approved"
        },
        Some(id),
    );
    let verified_at: Option<String> = conn
        .query_row(
            &format!("SELECT verified_at FROM {table} WHERE id=?1"),
            [id],
            |row| row.get(0),
        )
        .ok()
        .flatten();
    Json(json!({ "id": id, "pending": false, "verifiedAt": verified_at })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_codes_match_the_shared_vector_and_read_like_people_type_them() {
        let vectors: Value =
            serde_json::from_str(include_str!("../test-fixtures/account-key-v2.json")).unwrap();
        let vector = &vectors["safetyCode"];
        assert_eq!(
            safety_code(vector["accountKeyId"].as_str().unwrap()),
            vector["display"].as_str().unwrap()
        );
        assert_eq!(
            normalize_code("vdnz-db8z 3hkc"),
            vector["code"].as_str().unwrap()
        );
        assert_eq!(normalize_code("O1 il"), "0111");
    }

    struct Fixture {
        base: String,
        client: reqwest::Client,
        state: Arc<AppState>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    impl Fixture {
        async fn new(name: &str) -> (Self, [String; 3]) {
            let (state, _dir) = crate::tests::test_app_state(name);
            let tokens = {
                let conn = state.store.conn.lock().await;
                conn.execute_batch(
                    "INSERT INTO relay_users VALUES
                       ('owner','o@test','owner','user',1,NULL,'now','salt','hash'),
                       ('alice','a@test','alice','user',1,NULL,'now','salt','hash'),
                       ('bob','b@test','bob','user',1,NULL,'now','salt','hash');
                     INSERT INTO relay_devices(id,owner_user_id,name,token_hash,token_preview,created_at)
                       VALUES ('dev','owner','Laptop','hash','test','now');
                     INSERT INTO relay_account_vaults VALUES
                       ('alice','{\"accountKeyId\":\"bbNZDxMlJqPBsWsFLvOi-pnS-vAKzGgs9HevaG1vOgY\"}',1,
                        'bbNZDxMlJqPBsWsFLvOi-pnS-vAKzGgs9HevaG1vOgY','key');",
                )
                .unwrap();
                ["owner", "alice", "bob"]
                    .map(|user| create_session(&conn, &state.store.session_secret, user).unwrap())
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let app = Router::new()
                .merge(routes())
                .merge(preferences::routes())
                .route("/relay/portal", get(crate::portal))
                .route("/relay/access", get(crate::relay_access))
                .route("/relay/shares", post(crate::create_share))
                .route("/relay/grants", post(crate::create_grant))
                .route(
                    "/relay/devices/{device_id}/api/{*rest}",
                    any(crate::device_api),
                )
                .route(
                    "/relay/devices/{device_id}/presence",
                    get(crate::device_presence),
                )
                .with_state(state.clone());
            let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let client = reqwest::Client::new();
            (
                Self {
                    base,
                    client,
                    state,
                    task,
                },
                tokens,
            )
        }
        async fn call(
            &self,
            method: Method,
            path: &str,
            token: &str,
            body: Option<Value>,
        ) -> (StatusCode, Value) {
            let mut request = self
                .client
                .request(method, format!("{}{path}", self.base))
                .bearer_auth(token);
            if let Some(body) = body {
                request = request.json(&body);
            }
            let response = request.send().await.unwrap();
            let status = response.status();
            (status, response.json().await.unwrap_or(Value::Null))
        }
    }

    #[tokio::test]
    async fn a_new_share_waits_for_the_recipients_safety_code_before_it_gives_access() {
        let (f, [owner, alice, bob]) = Fixture::new("share-verification-flow").await;
        let (status, share) = f
            .call(
                Method::POST,
                "/relay/shares",
                &owner,
                Some(json!({
                    "targetIdentifier": "alice", "deviceId": "dev", "threadId": "t1",
                    "threadTitle": "Fix login", "workspaceId": "web", "workspaceLabel": "Web app",
                    "threadAccess": "control"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(share["pending"], true);
        let id = share["id"].as_str().unwrap().to_owned();
        // Both sides see it as pending.
        let (_, portal) = f.call(Method::GET, "/relay/portal", &alice, None).await;
        assert_eq!(portal["sharedWithMe"][0]["pending"], true);
        assert_eq!(portal["sharedThreadsWithMe"][0]["pending"], true);
        let (_, portal) = f.call(Method::GET, "/relay/portal", &owner, None).await;
        assert_eq!(portal["sharedByMe"][0]["pending"], true);
        assert_eq!(portal["sharedByMe"][0]["verifiedAt"], Value::Null);
        // The recipient is refused with what the waiting page names.
        for path in [
            "/relay/devices/dev/api/threads/t1",
            "/relay/devices/dev/api/threads/t1/transport/key?challenge=abcdefgh",
            "/relay/devices/dev/api/transport/browser",
            "/relay/access?deviceId=dev&threadId=t1",
        ] {
            let (status, body) = f.call(Method::GET, path, &alice, None).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
            assert_eq!(body["code"], "share_pending", "{path}");
            assert_eq!(body["share"]["id"], id.as_str());
            assert_eq!(body["share"]["kind"], "share");
            assert_eq!(body["share"]["scope"], "thread");
            assert_eq!(body["share"]["ownerUsername"], "owner");
            assert_eq!(body["share"]["threadTitle"], "Fix login");
            assert_eq!(body["share"]["deviceName"], "Laptop");
        }
        // Another thread on the device is not this share's business.
        let (status, _) = f
            .call(
                Method::GET,
                "/relay/devices/dev/api/threads/other",
                &alice,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // Someone else learns nothing about it.
        let (status, body) = f
            .call(Method::GET, "/relay/devices/dev/api/threads/t1", &bob, None)
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_ne!(body["code"], "share_pending");
        // Presence still answers, so the recipient's list shows the device.
        let (status, _) = f
            .call(
                Method::GET,
                "/relay/devices/dev/presence?threadId=t1",
                &alice,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);

        // Only the owner approves, and only with the matching code.
        let approve = format!("/relay/shares/{id}/approve");
        let (status, _) = f
            .call(
                Method::POST,
                &approve,
                &alice,
                Some(json!({"safetyCode": "VDNZ DB8Z 3HKC"})),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body) = f
            .call(Method::POST, &approve, &owner, Some(json!({})))
            .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some("safety_code_required"))
        );
        let (status, body) = f
            .call(
                Method::POST,
                &approve,
                &owner,
                Some(json!({"safetyCode": "VDNZ DB8Z 3HKD"})),
            )
            .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::CONFLICT, Some("safety_code_mismatch"))
        );
        let (_, body) = f
            .call(
                Method::GET,
                "/relay/access?deviceId=dev&threadId=t1",
                &alice,
                None,
            )
            .await;
        assert_eq!(body["code"], "share_pending");
        let (status, body) = f
            .call(
                Method::POST,
                &approve,
                &owner,
                Some(json!({"safetyCode": "vdnz-db8z-3hkc"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["pending"], false);
        assert!(body["verifiedAt"].is_string());
        let (status, body) = f
            .call(
                Method::GET,
                "/relay/access?deviceId=dev&threadId=t1",
                &alice,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["shareId"], id.as_str());
        // The device is offline here: the relay now forwards instead of refusing.
        let (status, _) = f
            .call(
                Method::GET,
                "/relay/devices/dev/api/threads/t1",
                &alice,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let (_, portal) = f.call(Method::GET, "/relay/portal", &owner, None).await;
        assert_eq!(portal["sharedByMe"][0]["pending"], false);
        assert!(portal["sharedByMe"][0]["verifiedAt"].is_string());
    }

    #[tokio::test]
    async fn the_check_follows_the_owners_preference_and_recipients_without_a_key() {
        let (f, [owner, alice, bob]) = Fixture::new("share-verification-preference").await;
        let (_, preferences) = f
            .call(Method::GET, "/relay/account/preferences", &owner, None)
            .await;
        assert_eq!(preferences["requireSafetyCode"], true);
        // A recipient without an account key has no code: approval needs none
        // and records no verification.
        let (_, grant) = f
            .call(
                Method::POST,
                "/relay/grants",
                &owner,
                Some(json!({
                    "targetIdentifier": "bob", "deviceId": "dev", "scope": "workspace",
                    "workspaceId": "web", "workspaceLabel": "Web app", "threadAccess": "read"
                })),
            )
            .await;
        assert_eq!(grant["pending"], true);
        let id = grant["id"].as_str().unwrap();
        let (_, body) = f
            .call(
                Method::GET,
                "/relay/devices/dev/api/workspaces/web/threads",
                &bob,
                None,
            )
            .await;
        assert_eq!(
            (body["code"].as_str(), body["share"]["scope"].as_str()),
            (Some("share_pending"), Some("workspace"))
        );
        assert_eq!(body["share"]["kind"], "grant");
        let approve = format!("/relay/grants/{id}/approve");
        let (status, body) = f
            .call(
                Method::POST,
                &approve,
                &owner,
                Some(json!({"safetyCode": "VDNZ DB8Z 3HKC"})),
            )
            .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::CONFLICT, Some("recipient_account_missing"))
        );
        let (status, body) = f
            .call(Method::POST, &approve, &owner, Some(json!({})))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["verifiedAt"], Value::Null);
        let (status, _) = f
            .call(
                Method::GET,
                "/relay/access?deviceId=dev&workspaceId=web",
                &bob,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);

        // Turned off, new shares start at once; the other preference stays.
        let (status, preferences) = f
            .call(
                Method::PATCH,
                "/relay/account/preferences",
                &owner,
                Some(json!({"requireSafetyCode": false})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            preferences,
            json!({"sendShortcut":"ctrlEnter","requireSafetyCode":false})
        );
        let (_, share) = f
            .call(Method::POST, "/relay/shares", &owner, Some(json!({
                "targetIdentifier": "alice", "deviceId": "dev", "threadId": "t2", "threadAccess": "read"
            })))
            .await;
        assert_eq!(share["pending"], false);
        let (status, _) = f
            .call(
                Method::GET,
                "/relay/access?deviceId=dev&threadId=t2",
                &alice,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        // Rows from before the check (pending defaults to 0) stay active.
        f.state.store.conn.lock().await.execute(
            "INSERT INTO relay_shares(id,owner_user_id,target_user_id,device_id,thread_id,thread_access,created_at)
             VALUES ('old','owner','alice','dev','t3','read','now')",
            [],
        ).unwrap();
        let (status, _) = f
            .call(
                Method::GET,
                "/relay/access?deviceId=dev&threadId=t3",
                &alice,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        // An active row records a later check without changing access.
        let (status, body) = f
            .call(
                Method::POST,
                "/relay/shares/old/approve",
                &owner,
                Some(json!({"safetyCode": "VDNZDB8Z3HKC"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["verifiedAt"].is_string());
    }
}
