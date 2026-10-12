//! Owner-created public transcripts. Live sources are explicit device-side
//! publication capabilities, never access to private thread DTOs.
use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Scope {
    device_id: String,
    thread_id: String,
    #[serde(default)]
    snapshot: Option<Value>,
    #[serde(default)]
    publication_token: Option<String>,
}

fn owns(conn: &Connection, owner: &str, scope: &Scope) -> bool {
    Uuid::parse_str(&scope.thread_id).is_ok()
        && conn
            .query_row(
                "SELECT 1 FROM relay_devices WHERE id=?1 AND owner_user_id=?2",
                params![scope.device_id, owner],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
}

// The fragment key never reaches the relay. Store exactly the opaque envelope:
// no title, turn count, theme, images or other rendered snapshot fields. AES-GCM
// appends a 16-byte tag to ciphertext; all binary fields use unpadded base64url.
const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
const MAX_CIPHERTEXT_BYTES: usize = MAX_SNAPSHOT_BYTES + 16;

fn encrypted_envelope(input: &Value) -> Result<Value, StatusCode> {
    let object = input.as_object().ok_or(StatusCode::BAD_REQUEST)?;
    if input["version"] != 2
        || object.len() != 3
        || !object
            .keys()
            .all(|key| matches!(key.as_str(), "version" | "iv" | "ciphertext"))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let decode = |field: &str, max: usize| -> Result<Vec<u8>, StatusCode> {
        let encoded = input[field].as_str().ok_or(StatusCode::BAD_REQUEST)?;
        if encoded.len() > max.div_ceil(3) * 4 {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        if bytes.len() > max {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        Ok(bytes)
    };
    if decode("iv", 12)?.len() != 12 || decode("ciphertext", MAX_CIPHERTEXT_BYTES)?.len() < 16 {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(input.clone())
}

// Legacy sources remain readable, but an encrypted cache can never be replaced
// with plaintext (including after a device is rolled back to an older version).
fn refreshed_snapshot(input: &Value, encrypted: bool) -> Result<Value, StatusCode> {
    if encrypted {
        encrypted_envelope(input)
    } else if input["live"] == true && input.to_string().len() <= MAX_SNAPSHOT_BYTES {
        Ok(input.clone())
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}
pub(super) async fn create(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Json(scope): Json<Scope>,
) -> Response {
    let owner = {
        let conn = state.store.conn.lock().await;
        let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
        else {
            return unauthorized();
        };
        if !owns(&conn, &user.id, &scope) {
            return StatusCode::NOT_FOUND.into_response();
        }
        user.id
    };
    let created_at = now_rfc3339();
    let live_snapshot = if let Some(token) = &scope.publication_token {
        if Uuid::parse_str(token).is_err() {
            return StatusCode::BAD_REQUEST.into_response();
        }
        match internal_forward_json(
            &state,
            &scope.device_id,
            "GET",
            &format!("/api/publications/{token}"),
            None,
        )
        .await
        {
            Ok(value) if encrypted_envelope(&value).is_ok() => Some(value),
            _ => {
                return (
                    StatusCode::BAD_GATEWAY,
                    "Unable to read the device's published transcript",
                )
                    .into_response()
            }
        }
    } else {
        None
    };
    let Some(input) = live_snapshot.as_ref().or(scope.snapshot.as_ref()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let snapshot = match encrypted_envelope(input) {
        Ok(envelope) => envelope,
        Err(status) => return status.into_response(),
    };
    let serialized = snapshot.to_string();
    let id = Uuid::new_v4().simple().to_string();
    let conn = state.store.conn.lock().await;
    if !owns(&conn, &owner, &scope) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match conn.execute("INSERT INTO relay_public_links(id,owner_user_id,device_id,thread_id,snapshot_json,created_at) VALUES (?1,?2,?3,?4,?5,?6)", params![id,owner,scope.device_id,scope.thread_id,serialized,created_at]) {
        Ok(_) => {
            if let Some(token) = &scope.publication_token {
                if conn.execute("INSERT INTO relay_public_link_sources(link_id,publication_token,refreshed_at) VALUES (?1,?2,?3)", params![id,token,created_at]).is_err() {
                    let _ = conn.execute("DELETE FROM relay_public_links WHERE id=?1", [&id]);
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
            Json(json!({"id":id,"createdAt":created_at,"version":2,"live":live_snapshot.is_some()})).into_response()
        },
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub(super) async fn list(
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
    Query(scope): Query<Scope>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let Ok(mut statement) = conn.prepare("SELECT id,created_at,json_extract(snapshot_json,'$.turnCount'),EXISTS(SELECT 1 FROM relay_public_link_sources WHERE link_id=id),COALESCE(json_extract(snapshot_json,'$.version'),1),(SELECT publication_token FROM relay_public_link_sources WHERE link_id=id) FROM relay_public_links WHERE owner_user_id=?1 AND device_id=?2 AND thread_id=?3 ORDER BY created_at DESC") else { return StatusCode::INTERNAL_SERVER_ERROR.into_response() };
    let Ok(rows) = statement.query_map(params![user.id,scope.device_id,scope.thread_id], |row| Ok(json!({"id":row.get::<_,String>(0)?,"createdAt":row.get::<_,String>(1)?,"turnCount":row.get::<_,Option<i64>>(2)?,"live":row.get::<_,bool>(3)?,"version":row.get::<_,i64>(4)?,"publicationToken":row.get::<_,Option<String>>(5)?}))) else { return StatusCode::INTERNAL_SERVER_ERROR.into_response() };
    Json(rows.filter_map(Result::ok).collect::<Vec<_>>()).into_response()
}

pub(super) async fn read(Path(id): Path<String>, State(state): State<Arc<AppState>>) -> Response {
    let source = {
        let conn = state.store.conn.lock().await;
        conn.query_row("SELECT l.device_id,s.publication_token,s.refreshed_at,COALESCE(json_extract(l.snapshot_json,'$.version'),1)=2 FROM relay_public_links l JOIN relay_public_link_sources s ON s.link_id=l.id WHERE l.id=?1", [&id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,bool>(3)?))).optional().ok().flatten()
    };
    let mut stale = false;
    if let Some((device, token, refreshed, encrypted)) = source {
        let due = chrono::DateTime::parse_from_rfc3339(&refreshed)
            .map(|time| chrono::Utc::now().signed_duration_since(time).num_seconds() >= 5)
            .unwrap_or(true);
        if due {
            // Claim this refresh before awaiting the device so simultaneous readers
            // share the cached snapshot rather than fanning out identical requests.
            let claimed = state.store.conn.lock().await.execute("UPDATE relay_public_link_sources SET refreshed_at=?1 WHERE link_id=?2 AND refreshed_at=?3", params![now_rfc3339(),id,refreshed]).unwrap_or(0) == 1;
            if claimed {
                match internal_forward_json(
                    &state,
                    &device,
                    "GET",
                    &format!("/api/publications/{token}"),
                    None,
                )
                .await
                .ok()
                .and_then(|value| refreshed_snapshot(&value, encrypted).ok())
                {
                    Some(snapshot) => {
                        let _ = state.store.conn.lock().await.execute(
                            "UPDATE relay_public_links SET snapshot_json=?1 WHERE id=?2",
                            params![snapshot.to_string(), id],
                        );
                    }
                    _ => {
                        stale = true;
                        let _ = state.store.conn.lock().await.execute("UPDATE relay_public_links SET snapshot_json=json_set(snapshot_json,'$.stale',json('true')) WHERE id=?1", [&id]);
                    }
                }
            }
        }
    }
    let conn = state.store.conn.lock().await;
    let snapshot: Option<String> = conn
        .query_row(
            "SELECT snapshot_json FROM relay_public_links WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    match snapshot {
        Some(snapshot) => (
            [
                (header::CONTENT_TYPE, "application/json; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (header::REFERRER_POLICY, "no-referrer"),
            ],
            if stale {
                let mut value: Value = serde_json::from_str(&snapshot).unwrap_or(json!({}));
                value["stale"] = json!(true);
                value.to_string()
            } else {
                snapshot
            },
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub(super) async fn revoke(
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query)
    else {
        return unauthorized();
    };
    let source = conn.query_row("SELECT l.device_id,s.publication_token FROM relay_public_links l JOIN relay_public_link_sources s ON s.link_id=l.id WHERE l.id=?1 AND l.owner_user_id=?2", params![id,user.id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional().ok().flatten();
    match conn.execute(
        "DELETE FROM relay_public_links WHERE id=?1 AND owner_user_id=?2",
        params![id, user.id],
    ) {
        Ok(1) => {
            drop(conn);
            if let Some((device, token)) = source {
                tokio::spawn(async move {
                    let _ = internal_forward_json(
                        &state,
                        &device,
                        "DELETE",
                        &format!("/api/publications/{token}"),
                        None,
                    )
                    .await;
                });
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope() -> Value {
        serde_json::from_str::<Value>(include_str!("../../../tests/fixtures/public-link-v2.json"))
            .unwrap()["envelope"]
            .clone()
    }

    #[test]
    fn validates_only_bounded_ciphertext_envelopes() {
        let valid = envelope();
        assert_eq!(encrypted_envelope(&valid).unwrap(), valid);
        for invalid in [
            json!(null),
            json!({"version":1,"title":"private","turns":[]}),
            json!({"version":2,"iv":42,"ciphertext":"AAAA"}),
            json!({"version":3,"iv":valid["iv"],"ciphertext":valid["ciphertext"]}),
            json!({"version":2,"iv":"AAAA","ciphertext":valid["ciphertext"]}),
            json!({"version":2,"iv":valid["iv"],"ciphertext":"AA"}),
            json!({"version":2,"iv":valid["iv"],"ciphertext":"invalid+/="}),
        ] {
            assert_eq!(encrypted_envelope(&invalid), Err(StatusCode::BAD_REQUEST));
        }
        for field in [
            "title",
            "turns",
            "theme",
            "images",
            "encryptionKey",
            "stale",
        ] {
            let mut leaked = valid.clone();
            leaked[field] = json!("must not be stored");
            assert_eq!(encrypted_envelope(&leaked), Err(StatusCode::BAD_REQUEST));
        }
        let mut oversized = valid;
        oversized["ciphertext"] = json!("A".repeat(MAX_CIPHERTEXT_BYTES.div_ceil(3) * 4 + 1));
        assert_eq!(
            encrypted_envelope(&oversized),
            Err(StatusCode::PAYLOAD_TOO_LARGE)
        );
    }

    #[test]
    fn live_refresh_preserves_legacy_but_never_downgrades_encryption() {
        let legacy = json!({"live":true,"title":"Legacy","turns":[]});
        assert_eq!(refreshed_snapshot(&legacy, false).unwrap(), legacy);
        assert!(refreshed_snapshot(&legacy, true).is_err());
        assert_eq!(refreshed_snapshot(&envelope(), true).unwrap(), envelope());
    }

    #[tokio::test]
    async fn owner_create_list_and_revoke_store_only_the_envelope() {
        let (state, dir) = crate::tests::test_app_state("public-links-owner");
        let thread = Uuid::new_v4().to_string();
        let token = {
            let conn = state.store.conn.lock().await;
            conn.execute("INSERT INTO relay_users VALUES ('owner','owner@example.test','owner','user',1,NULL,'now','salt','hash')", []).unwrap();
            conn.execute("INSERT INTO relay_devices(id,owner_user_id,name,token_hash,token_preview,created_at) VALUES ('device','owner','Test','test-hash','test','now')", []).unwrap();
            conn.execute("INSERT INTO relay_public_links(id,owner_user_id,device_id,thread_id,snapshot_json,created_at) VALUES ('legacy','owner','device',?1,?2,'now')", params![thread,json!({"title":"Legacy","turnCount":7,"turns":[]}).to_string()]).unwrap();
            create_session(&conn, &state.store.session_secret, "owner").unwrap()
        };
        let query = || {
            Query(TokenQuery {
                token: Some(token.clone()),
                device_token: None,
                relay_session: None,
            })
        };
        let scope = |snapshot| Scope {
            device_id: "device".into(),
            thread_id: thread.clone(),
            snapshot,
            publication_token: None,
        };
        let rejected = create(
            HeaderMap::new(),
            query(),
            State(state.clone()),
            Json(scope(Some(json!({"title":"private","turns":[]})))),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        let created = create(
            HeaderMap::new(),
            query(),
            State(state.clone()),
            Json(scope(Some(envelope()))),
        )
        .await;
        assert_eq!(created.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(created.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: Value = serde_json::from_slice(&bytes).unwrap();
        let id = created["id"].as_str().unwrap();
        assert_eq!(created["version"], 2);
        assert!(created.get("turnCount").is_none());
        {
            let conn = state.store.conn.lock().await;
            let raw: String = conn
                .query_row(
                    "SELECT snapshot_json FROM relay_public_links WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(serde_json::from_str::<Value>(&raw).unwrap(), envelope());
        }
        let listed = list(
            HeaderMap::new(),
            query(),
            State(state.clone()),
            Query(scope(None)),
        )
        .await;
        let bytes = axum::body::to_bytes(listed.into_body(), usize::MAX)
            .await
            .unwrap();
        let listed: Value = serde_json::from_slice(&bytes).unwrap();
        let rows = listed.as_array().unwrap();
        assert_eq!(
            rows.len(),
            2,
            "NULL turn counts must not hide encrypted links"
        );
        assert!(rows
            .iter()
            .any(|row| row["version"] == 2 && row["turnCount"].is_null()));
        assert!(rows
            .iter()
            .any(|row| row["version"] == 1 && row["turnCount"] == 7));
        assert_eq!(
            revoke(
                Path(id.into()),
                HeaderMap::new(),
                query(),
                State(state.clone())
            )
            .await
            .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            read(Path(id.into()), State(state.clone())).await.status(),
            StatusCode::NOT_FOUND
        );
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn reads_legacy_and_encrypted_cached_links_with_stale_marker() {
        let (state, dir) = crate::tests::test_app_state("public-links-cache");
        {
            let conn = state.store.conn.lock().await;
            // Isolated fixture database; no production users/devices are touched.
            conn.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
            for (id, value) in [
                ("legacy", json!({"title":"Legacy","turns":[]})),
                ("encrypted", envelope()),
            ] {
                conn.execute("INSERT INTO relay_public_links(id,owner_user_id,device_id,thread_id,snapshot_json,created_at) VALUES (?1,'owner','offline','thread',?2,'2026-01-01T00:00:00Z')", params![id,value.to_string()]).unwrap();
            }
            conn.execute("INSERT INTO relay_public_link_sources(link_id,publication_token,refreshed_at) VALUES ('encrypted','token','2026-01-01T00:00:00Z')", []).unwrap();
        }
        let legacy = read(Path("legacy".into()), State(state.clone())).await;
        assert_eq!(legacy.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(legacy.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["title"],
            "Legacy"
        );
        for _ in 0..2 {
            let response = read(Path("encrypted".into()), State(state.clone())).await;
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["ciphertext"], envelope()["ciphertext"]);
            assert_eq!(value["stale"], true);
            assert!(value.get("title").is_none());
        }
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
