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
    pub(super) version: Option<(Option<String>, i64)>,
    pub(super) last_seen: Option<i64>,
    pub(super) connects_hour: Option<i64>,
    pub(super) traffic7d: Option<Vec<Option<i64>>>,
}

impl DeviceRow {
    pub(super) fn connected(&self) -> bool {
        self.tunnel.is_some()
    }
    pub(super) fn unstable(&self) -> bool {
        self.connected()
            && self
                .connects_hour
                .is_some_and(|count| count >= crate::metrics::UNSTABLE_CONNECTS)
    }
    pub(super) fn outdated(&self) -> bool {
        crate::metrics::outdated(self.version_text()).unwrap_or(false)
    }
    fn version_text(&self) -> Option<&str> {
        self.version
            .as_ref()
            .and_then(|(version, _)| version.as_deref())
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
            "version": self.version_text(),
            "versionCheckedAt": self.version.as_ref().map(|(_, at)| rfc3339_from_ms(*at)),
            "outdated": crate::metrics::outdated(self.version_text()),
            "lastSeenAt": if self.connected() { None } else { self.last_seen.map(rfc3339_from_ms) },
            "reconnects1h": self.connects_hour,
            "unstable": self.connects_hour.map(|_| self.unstable()),
            "traffic7d": self.traffic7d,
        })
    }
}

/// Every device (or one owner's), with what the database and memory know.
pub(super) fn rows(conn: &Connection, live: &Live, owner: Option<&str>) -> Vec<DeviceRow> {
    let lineage = lineage_counts(conn);
    let stats = super::stats::device_stats(conn);
    // Days before recording started are unknown, not zero.
    let started = crate::metrics::history_start(conn);
    let today = now_ms() - now_ms().rem_euclid(DAY_MS);
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
        let traffic7d = stats.recorded.then(|| {
            let days = stats
                .traffic
                .get(&id)
                .cloned()
                .unwrap_or_else(|| vec![0; 7]);
            days.into_iter()
                .enumerate()
                .map(|(index, requests)| {
                    let day = today - (6 - index as i64) * DAY_MS;
                    started
                        .is_some_and(|begun| day + DAY_MS > begun)
                        .then_some(requests)
                })
                .collect()
        });
        Ok(DeviceRow {
            version: stats.versions.get(&id).cloned(),
            last_seen: stats.last_seen.get(&id).copied(),
            connects_hour: stats
                .recorded
                .then(|| stats.connects_hour.get(&id).copied().unwrap_or(0)),
            traffic7d,
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
        "protected" => device.protected,
        "unstable" => device.unstable(),
        "outdated" => device.outdated(),
        _ => true,
    }
}

/// When the device was last seen: now while connected, else when its last
/// tunnel ended (None when the relay has no record).
fn seen_at(device: &DeviceRow, now: i64) -> Option<i64> {
    if device.connected() {
        Some(now)
    } else {
        device.last_seen
    }
}

/// The column filters of the list (owner, state, protection, version, last
/// seen, created), on top of the legacy one-of `filter`.
fn matches_columns(device: &DeviceRow, query: &ListQuery, now: i64) -> bool {
    if !matches_filter(device, query.filter()) {
        return false;
    }
    if let Some(owner) = ListQuery::pick(&query.owner) {
        if device.owner_user_id != owner {
            return false;
        }
    }
    if let Some(state) = ListQuery::pick(&query.state) {
        let ok = match state {
            "online" => device.connected() && !device.unstable(),
            "connected" => device.connected(),
            "offline" => !device.connected(),
            "unstable" => device.unstable(),
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    if let Some(protection) = ListQuery::pick(&query.protection) {
        if (protection == "protected") != device.protected {
            return false;
        }
    }
    if let Some(version) = ListQuery::pick(&query.version) {
        let text = device.version_text();
        let ok = match version {
            "outdated" => device.outdated(),
            "current" => text.is_some() && !device.outdated(),
            "unknown" => text.is_none(),
            exact => exact
                .strip_prefix("v:")
                .is_some_and(|exact| text == Some(exact)),
        };
        if !ok {
            return false;
        }
    }
    match ListQuery::pick(&query.seen) {
        Some("never") => {
            if seen_at(device, now).is_some() {
                return false;
            }
        }
        _ => {
            let window = query.seen_window();
            if !window.is_open() && !window.contains(seen_at(device, now)) {
                return false;
            }
        }
    }
    let created = query.created_window();
    if !created.is_open() && !created.contains(ms_from_rfc3339(&device.created_at)) {
        return false;
    }
    true
}

fn traffic_total(device: &DeviceRow) -> i64 {
    device
        .traffic7d
        .as_ref()
        .map_or(0, |days| days.iter().flatten().sum())
}

/// Sorts by a column; ties keep name order. Unknown values sort last in
/// either direction.
fn sort_devices(devices: &mut [DeviceRow], key: &str, dir: Dir, now: i64) {
    use std::cmp::Ordering;
    let by_name = |a: &DeviceRow, b: &DeviceRow| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    };
    let state_rank = |d: &DeviceRow| -> i64 {
        if !d.connected() {
            0
        } else if d.unstable() {
            1
        } else {
            2
        }
    };
    let optional = |a: Option<i64>, b: Option<i64>| match (a, b) {
        (Some(a), Some(b)) => dir.apply(a.cmp(&b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    devices.sort_by(|a, b| {
        let ordering = match key {
            "name" => dir.apply(by_name(a, b)),
            "owner" => dir.apply(
                a.owner_username
                    .to_lowercase()
                    .cmp(&b.owner_username.to_lowercase()),
            ),
            "state" => dir.apply(state_rank(a).cmp(&state_rank(b))),
            "protection" => dir.apply(a.protected.cmp(&b.protected)),
            "version" => match (a.version_text(), b.version_text()) {
                (Some(x), Some(y)) => dir.apply(compare_versions(x, y)),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
            "threads" => dir.apply(a.threads.0.cmp(&b.threads.0)),
            "ports" => dir.apply(a.port_mappings.cmp(&b.port_mappings)),
            "created" => dir.apply(a.created_at.cmp(&b.created_at)),
            "lastSeen" => optional(seen_at(a, now), seen_at(b, now)),
            "reconnects" => optional(a.connects_hour, b.connects_hour),
            "traffic" => dir.apply(traffic_total(a).cmp(&traffic_total(b))),
            _ => Ordering::Equal,
        };
        ordering.then_with(|| by_name(a, b))
    });
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
    super_flush(&state, &conn);
    let all = rows(&conn, &live, None);
    let owner_label = ListQuery::pick(&query.owner)
        .and_then(|owner| usernames(&conn, &[owner.to_string()]).remove(owner));
    drop(conn);
    let now = now_ms();
    let search = query.search();
    let mut matching: Vec<DeviceRow> = all
        .iter()
        .filter(|device| matches_columns(device, &query, now))
        .filter(|device| {
            search.as_deref().is_none_or(|search| {
                device.name.to_lowercase().contains(search)
                    || device.owner_username.to_lowercase().contains(search)
                    || device.id.starts_with(search)
            })
        })
        .cloned()
        .collect();
    match query.sort_by(Dir::Asc) {
        Some((key, dir)) => sort_devices(&mut matching, key, dir, now),
        // Online first, each group by name.
        None => matching.sort_by_key(|device| !device.connected()),
    }
    // Every reported version with its device count, for the version filter.
    let mut versions: Vec<(String, usize)> = Vec::new();
    for device in &all {
        if let Some(version) = device.version_text() {
            match versions.iter_mut().find(|(v, _)| v == version) {
                Some(entry) => entry.1 += 1,
                None => versions.push((version.to_string(), 1)),
            }
        }
    }
    versions.sort_by(|a, b| compare_versions(&b.0, &a.0));
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
        "unstable": all.iter().filter(|d| d.unstable()).count(),
        "outdated": all.iter().filter(|d| d.outdated()).count(),
        "unknownVersion": all.iter().filter(|d| d.version_text().is_none()).count(),
    });
    let mut body = paging.wrap(items, matching.len());
    body.insert("counts".into(), counts);
    body.insert("relayVersion".into(), json!(RELAY_VERSION));
    body.insert(
        "versions".into(),
        json!(versions
            .iter()
            .map(|(version, devices)| json!({ "version": version, "devices": devices }))
            .collect::<Vec<_>>()),
    );
    body.insert("labels".into(), json!({ "owner": owner_label }));
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
    super_flush(&state, &conn);
    let (connections, traffic) = super::stats::device_detail(&conn, &device_id);
    Json(json!({
        "device": device.json(),
        "upstreamProtocols": protocols,
        "shares": shares,
        "previews": { "mappings": device.port_mappings, "sessions": sessions, "streams": streams },
        "connections": connections,
        "traffic": traffic,
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
    let (admin, name) = {
        let (conn, admin) = match admin_conn(&state, &headers).await {
            Ok(value) => value,
            Err(response) => return response,
        };
        let name: Option<String> = conn
            .query_row(
                "SELECT name FROM relay_devices WHERE id=?1",
                [&device_id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        let Some(name) = name else {
            return not_found("Device not found");
        };
        (admin, name)
    };
    let was_connected = drop_tunnel(&state, &device_id).await;
    super::log_action(
        &*state.store.conn.lock().await,
        &admin,
        "device.disconnected",
        Some(("device", &device_id, &name)),
        json!({ "wasConnected": was_connected }),
    );
    Json(json!({ "id": device_id, "wasConnected": was_connected })).into_response()
}

/// Removes the device's current tunnel and the browser sockets attached to it.
pub(crate) async fn drop_tunnel(state: &AppState, device_id: &str) -> bool {
    let removed = state.sockets.write().await.remove(device_id);
    if let Some(socket) = &removed {
        state
            .metrics
            .disconnected(socket.connection_id, crate::metrics::EndReason::Admin);
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

/// Writes queued tunnel connection events first, so a list read right after a
/// reconnect already counts it.
pub(super) fn super_flush(state: &AppState, conn: &Connection) {
    crate::metrics::flush_connections(state, conn);
}
