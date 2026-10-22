//! Devices: a paged, filtered list joining the database with the tunnels the
//! relay holds in memory, one device's details, and disconnecting a tunnel.
use super::common::*;
use super::*;

#[derive(Clone)]
pub(super) struct DeviceRow {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) owner_user_id: String,
    pub(super) owner_username: String,
    pub(super) created_at: String,
    pub(super) tunnel: Option<(String, String)>,
    pub(super) protected: bool,
    pub(super) protected_at: Option<String>,
    pub(super) threads: (i64, i64, i64),
    pub(super) port_mappings: usize,
}

impl DeviceRow {
    pub(super) fn connected(&self) -> bool {
        self.tunnel.is_some()
    }
    pub(super) fn json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "ownerUserId": self.owner_user_id,
            "ownerUsername": self.owner_username,
            "createdAt": self.created_at,
            "connected": self.connected(),
            "connectedAt": self.tunnel.as_ref().map(|(connected, _)| connected),
            "lastHeartbeatAt": self.tunnel.as_ref().map(|(_, heartbeat)| heartbeat),
            "protected": self.protected,
            "protectedAt": self.protected_at,
            "threadCount": self.threads.0,
            "agentThreadCount": self.threads.1,
            "workspaceCount": self.threads.2,
            "portMappings": self.port_mappings,
        })
    }
}

/// Every device (or one owner's), with what the database and memory know.
pub(super) fn rows(conn: &Connection, live: &Live, owner: Option<&str>) -> Vec<DeviceRow> {
    let lineage = lineage_counts(conn);
    let sql = format!(
        "SELECT d.id,d.name,d.owner_user_id,u.username,d.created_at,c.device_id IS NOT NULL,
                CASE WHEN json_valid(c.certificate_json) THEN json_extract(c.certificate_json,'$.createdAt') END
         FROM relay_devices d JOIN relay_users u ON u.id=d.owner_user_id
         LEFT JOIN relay_device_certificates c ON c.device_id=d.id
         {} ORDER BY d.name COLLATE NOCASE, d.id",
        if owner.is_some() {
            "WHERE d.owner_user_id=?1"
        } else {
            ""
        }
    );
    let map = |row: &rusqlite::Row<'_>| -> rusqlite::Result<DeviceRow> {
        let id: String = row.get(0)?;
        Ok(DeviceRow {
            tunnel: live.tunnels.get(&id).cloned(),
            threads: lineage.get(&id).copied().unwrap_or_default(),
            port_mappings: live.previews.get(&id).copied().unwrap_or(0),
            name: row.get(1)?,
            owner_user_id: row.get(2)?,
            owner_username: row.get(3)?,
            created_at: row.get(4)?,
            protected: row.get(5)?,
            // The certificate's signed creation time, when it is text.
            protected_at: row.get::<_, Option<String>>(6).ok().flatten(),
            id,
        })
    };
    conn.prepare(&sql)
        .and_then(|mut stmt| match owner {
            Some(owner) => stmt.query_map([owner], map)?.collect(),
            None => stmt.query_map([], map)?.collect(),
        })
        .unwrap_or_default()
}

fn matches_filter(device: &DeviceRow, filter: &str) -> bool {
    match filter {
        "online" => device.connected(),
        "offline" => !device.connected(),
        "unprotected" => !device.protected,
        _ => true,
    }
}

pub(super) async fn list(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let live = live(&state).await;
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let all = rows(&conn, &live, None);
    drop(conn);
    let search = query.search();
    let mut matching: Vec<DeviceRow> = all
        .iter()
        .filter(|device| matches_filter(device, query.filter()))
        .filter(|device| {
            search.as_deref().is_none_or(|search| {
                device.name.to_lowercase().contains(search)
                    || device.owner_username.to_lowercase().contains(search)
                    || device.id.starts_with(search)
            })
        })
        .cloned()
        .collect();
    match query.sort.as_deref() {
        Some("name") => {}
        Some("owner") => matching.sort_by(|a, b| a.owner_username.cmp(&b.owner_username)),
        Some("created") => matching.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
        Some("threads") => matching.sort_by(|a, b| b.threads.0.cmp(&a.threads.0)),
        // Online first, each group by name.
        _ => matching.sort_by_key(|device| !device.connected()),
    }
    let paging = query.paging();
    let items = paging
        .slice(&matching)
        .iter()
        .map(DeviceRow::json)
        .collect();
    let counts = json!({
        "all": all.len(),
        "online": all.iter().filter(|d| d.connected()).count(),
        "offline": all.iter().filter(|d| !d.connected()).count(),
        "protected": all.iter().filter(|d| d.protected).count(),
        "unprotected": all.iter().filter(|d| !d.protected).count(),
    });
    let mut body = paging.wrap(items, matching.len());
    body.insert("counts".into(), counts);
    body.insert("relayVersion".into(), json!(RELAY_VERSION));
    Json(Value::Object(body)).into_response()
}

pub(super) async fn detail(
    Path(device_id): Path<String>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let live = live(&state).await;
    let (previews, _, _) = state.preview.usage();
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(owner) = conn
        .query_row(
            "SELECT owner_user_id FROM relay_devices WHERE id=?1",
            [&device_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
    else {
        return not_found("Device not found");
    };
    let Some(device) = rows(&conn, &live, Some(&owner))
        .into_iter()
        .find(|device| device.id == device_id)
    else {
        return not_found("Device not found");
    };
    // Only the protocols: connection names are the owner's own labels.
    let protocols: Vec<String> = conn
        .prepare(
            "SELECT DISTINCT protocol FROM relay_upstream_catalog WHERE device_id=?1 ORDER BY protocol",
        )
        .and_then(|mut stmt| stmt.query_map([&device_id], |row| row.get(0))?.collect())
        .unwrap_or_default();
    let now = now_rfc3339();
    let shares: Vec<Value> = conn
        .prepare(
            "SELECT id,'share','thread',target_username,created_at FROM relay_shares
              WHERE device_id=?1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?2)
             UNION ALL
             SELECT id,'grant',scope,target_username,created_at FROM relay_access_grants
              WHERE device_id=?1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?2)
             ORDER BY 5 DESC",
        )
        .and_then(|mut stmt| {
            stmt.query_map(params![device_id, now], |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "kind": row.get::<_, String>(1)?,
                    "scope": row.get::<_, String>(2)?,
                    "targetUsername": row.get::<_, Option<String>>(3)?,
                }))
            })?
            .collect()
        })
        .unwrap_or_default();
    let (sessions, streams) = previews.get(&device_id).copied().unwrap_or_default();
    Json(json!({
        "device": device.json(),
        "upstreamProtocols": protocols,
        "shares": shares,
        "previews": { "mappings": device.port_mappings, "sessions": sessions, "streams": streams },
    }))
    .into_response()
}

/// Drops the device's tunnel. The device reconnects by itself within seconds;
/// this is for troubleshooting a stuck connection and changes no access.
pub(super) async fn disconnect(
    Path(device_id): Path<String>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    {
        let (conn, _admin) = match admin_conn(&state, &headers).await {
            Ok(value) => value,
            Err(response) => return response,
        };
        let exists = conn
            .query_row(
                "SELECT 1 FROM relay_devices WHERE id=?1",
                [&device_id],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some();
        if !exists {
            return not_found("Device not found");
        }
    }
    let was_connected = drop_tunnel(&state, &device_id).await;
    Json(json!({ "id": device_id, "wasConnected": was_connected })).into_response()
}

/// Removes the device's current tunnel and the browser sockets attached to it.
pub(crate) async fn drop_tunnel(state: &AppState, device_id: &str) -> bool {
    let removed = state.sockets.write().await.remove(device_id);
    if let Some(socket) = &removed {
        state
            .clients
            .write()
            .await
            .retain(|_, client| client.device_id != device_id);
        state
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|_, request| {
                request.device_id != device_id || request.connection_id != socket.connection_id
            });
    }
    removed.is_some()
}
