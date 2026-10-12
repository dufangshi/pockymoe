use super::*;
use axum::{body::Bytes, extract::DefaultBodyLimit};

const MAX_CERTIFICATE_BYTES: usize = 16 * 1024;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/relay/devices/{id}/certificate",
        get(read)
            .put(write)
            .layer(DefaultBodyLimit::max(MAX_CERTIFICATE_BYTES)),
    )
}

// A certificate is needed even for a browser with only a thread/workspace grant.
// Resolve those contexts through the same access rules as the device API.
fn can_read(conn: &Connection, user: &str, device: &str) -> Result<bool, Failure> {
    use crate::effective_access;
    if effective_access(conn, user, device, None, None).is_some() {
        return Ok(true);
    }
    let mut stmt = conn
        .prepare(
            "SELECT thread_id,workspace_id FROM relay_shares WHERE target_user_id=?1 AND device_id=?2
             UNION SELECT thread_id,workspace_id FROM relay_access_grants WHERE target_user_id=?1 AND device_id=?2",
        )
        .map_err(internal)?;
    let contexts = stmt
        .query_map(params![user, device], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })
        .map_err(internal)?;
    for context in contexts {
        let (thread, workspace) = context.map_err(internal)?;
        if thread.as_deref().is_some_and(|thread| {
            effective_access(conn, user, device, Some(thread), None).is_some()
        }) || workspace.as_deref().is_some_and(|workspace| {
            effective_access(conn, user, device, None, Some(workspace)).is_some()
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn not_found() -> Failure {
    failure(
        StatusCode::NOT_FOUND,
        "not_found",
        "Device certificate not found",
    )
}

fn response(raw: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        raw,
    )
        .into_response()
}

async fn read(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, Failure> {
    let conn = state.store.conn.lock().await;
    let (user, _) = auth(&conn, &state, &headers)?;
    if !can_read(&conn, &user.id, &id)? {
        return Err(not_found());
    }
    let raw = conn
        .query_row(
            "SELECT certificate_json FROM relay_device_certificates WHERE device_id=?1",
            params![id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(internal)?
        .ok_or_else(not_found)?;
    Ok(response(raw))
}

async fn write(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Failure> {
    let conn = state.store.conn.lock().await;
    let (user, _) = auth(&conn, &state, &headers)?;
    let owned: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM relay_devices WHERE id=?1 AND owner_user_id=?2)",
            params![id, user.id],
            |row| row.get(0),
        )
        .map_err(internal)?;
    if !owned {
        return Err(not_found());
    }
    if body.len() > MAX_CERTIFICATE_BYTES {
        return Err(failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "Device certificate is too large",
        ));
    }
    let certificate: Value = serde_json::from_slice(&body)
        .map_err(|_| invalid("Device certificate must be a JSON object"))?;
    if !certificate.is_object()
        || certificate["version"].as_u64() != Some(1)
        || certificate["deviceId"].as_str() != Some(&id)
    {
        return Err(invalid(
            "Device certificate must have version 1 and a matching deviceId",
        ));
    }
    let raw =
        String::from_utf8(body.to_vec()).map_err(|_| invalid("Invalid certificate encoding"))?;
    conn.execute(
        "INSERT INTO relay_device_certificates(device_id,certificate_json) VALUES (?1,?2)
         ON CONFLICT(device_id) DO UPDATE SET certificate_json=excluded.certificate_json",
        params![id, raw],
    )
    .map_err(internal)?;
    Ok(response(raw))
}
