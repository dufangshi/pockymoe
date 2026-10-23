//! Devices: a paged, filtered list joining the database with the tunnels the
//! relay holds in memory, one device's details, and disconnecting a tunnel.
//!
//! A relay may hold tens of thousands of devices, so the list filters, sorts,
//! counts and pages in SQL (the online set joins as a JSON list) and reads
//! per-device statistics (threads, last seen, traffic) for the page only.
use super::common::*;
use super::*;

#[derive(Clone)]
pub(super) struct DeviceRow {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) owner_user_id: String,
    pub(super) owner_username: String,
    pub(super) owner_email: String,
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
            "ownerEmail": self.owner_email,
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

const FROM: &str = "relay_devices d JOIN relay_users u ON u.id=d.owner_user_id
     LEFT JOIN relay_device_certificates c ON c.device_id=d.id
     LEFT JOIN relay_device_versions v ON v.device_id=d.id
     LEFT JOIN relay_device_lineage_counts lc ON lc.device_id=d.id";

const COLUMNS: &str = "d.id,d.name,d.owner_user_id,u.username,u.email,d.created_at,c.device_id IS NOT NULL,
     CASE WHEN json_valid(c.certificate_json) THEN json_extract(c.certificate_json,'$.createdAt') END,
     v.device_id IS NOT NULL,v.version,v.checked_at";

/// The newest tunnel of a device: when it ended, or began while open.
const LAST_SEEN: &str =
    "(SELECT COALESCE(x.ended_at,x.connected_at) FROM relay_device_connections x
     WHERE x.device_id=d.id ORDER BY x.connected_at DESC LIMIT 1)";

/// What the list needs from memory: who is online and who reconnects often.
pub(super) struct LiveSets {
    online: String,
    unstable: String,
    unstable_count: usize,
    ports: String,
}

impl LiveSets {
    pub(super) fn new(conn: &Connection, live: &Live) -> Self {
        let unstable: Vec<String> = crate::metrics::connects_since(conn, now_ms() - HOUR_MS)
            .into_iter()
            .filter(|(device, count)| {
                *count >= crate::metrics::UNSTABLE_CONNECTS && live.tunnels.contains_key(device)
            })
            .map(|(device, _)| device)
            .collect();
        Self {
            online: json_list(live.tunnels.keys()),
            unstable_count: unstable.len(),
            unstable: json_list(unstable.iter()),
            ports: serde_json::to_string(&live.previews).unwrap_or_else(|_| "{}".into()),
        }
    }
}

/// The versions older than the relay among those reported, for SQL filters.
fn outdated_versions(conn: &Connection) -> Vec<String> {
    reported_versions(conn)
        .into_iter()
        .map(|(version, _)| version)
        .filter(|version| crate::metrics::outdated(Some(version)) == Some(true))
        .collect()
}

/// Every reported version with its device count, newest first.
fn reported_versions(conn: &Connection) -> Vec<(String, i64)> {
    let mut versions: Vec<(String, i64)> = conn
        .prepare(
            "SELECT version,COUNT(*) FROM relay_device_versions WHERE version IS NOT NULL GROUP BY version",
        )
        .and_then(|mut stmt| {
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .unwrap_or_default();
    versions.sort_by(|a, b| compare_versions(&b.0, &a.0));
    versions
}

/// SQL conditions of the list's filters (values from fixed lists or bound).
fn conditions(
    query: &ListQuery,
    sets: &LiveSets,
    outdated: &[String],
    binds: &mut Binds,
    now: i64,
) -> Vec<String> {
    let online = format!(
        "d.id IN (SELECT value FROM json_each({}))",
        binds.add(sets.online.clone())
    );
    let unstable = format!(
        "d.id IN (SELECT value FROM json_each({}))",
        binds.add(sets.unstable.clone())
    );
    let outdated = format!(
        "v.version IN (SELECT value FROM json_each({}))",
        binds.add(json_list(outdated.iter()))
    );
    let mut list = Vec::new();
    match query.filter() {
        "online" => list.push(online.clone()),
        "offline" => list.push(format!("NOT {online}")),
        "unprotected" => list.push("c.device_id IS NULL".into()),
        "protected" => list.push("c.device_id IS NOT NULL".into()),
        "unstable" => list.push(unstable.clone()),
        "outdated" => list.push(outdated.clone()),
        _ => {}
    }
    if let Some(owner) = ListQuery::pick(&query.owner) {
        list.push(format!("d.owner_user_id={}", binds.add(owner.to_string())));
    }
    match ListQuery::pick(&query.state) {
        Some("online") => list.push(format!("{online} AND NOT {unstable}")),
        Some("connected") => list.push(online.clone()),
        Some("offline") => list.push(format!("NOT {online}")),
        Some("unstable") => list.push(unstable.clone()),
        _ => {}
    }
    match ListQuery::pick(&query.protection) {
        Some("protected") => list.push("c.device_id IS NOT NULL".into()),
        Some("unprotected") => list.push("c.device_id IS NULL".into()),
        _ => {}
    }
    match ListQuery::pick(&query.version) {
        Some("outdated") => list.push(outdated.clone()),
        Some("current") => list.push(format!("v.version IS NOT NULL AND NOT {outdated}")),
        Some("unknown") => list.push("v.version IS NULL".into()),
        Some(exact) => {
            if let Some(exact) = exact.strip_prefix("v:") {
                list.push(format!("v.version={}", binds.add(exact.to_string())));
            }
        }
        None => {}
    }
    // Last seen: now while connected, else when the newest tunnel ended.
    match ListQuery::pick(&query.seen) {
        Some("never") => list.push(format!("NOT {online} AND {LAST_SEEN} IS NULL")),
        _ => {
            let window = query.seen_window();
            if !window.is_open() {
                let from = binds.add(window.from.unwrap_or(i64::MIN));
                let to = binds.add(window.to.unwrap_or(i64::MAX));
                let at = binds.add(now);
                list.push(format!(
                    "(CASE WHEN {online} THEN {at} ELSE {LAST_SEEN} END) >= {from}
                     AND (CASE WHEN {online} THEN {at} ELSE {LAST_SEEN} END) < {to}"
                ));
            }
        }
    }
    let created = query.created_window();
    if !created.is_open() {
        let (from, to) = created.text();
        if !from.is_empty() {
            list.push(format!("d.created_at>={}", binds.add(from)));
        }
        if !to.is_empty() {
            list.push(format!("d.created_at<{}", binds.add(to)));
        }
    }
    if let Some(search) = query.search() {
        let pattern = binds.add(like_pattern(&search));
        let prefix = binds.add(format!("{}%", search.replace(['%', '_', '\\'], "")));
        list.push(format!(
            "(lower(d.name) LIKE {pattern} ESCAPE '\\' OR lower(u.username) LIKE {pattern} ESCAPE '\\'
              OR lower(u.email) LIKE {pattern} ESCAPE '\\' OR d.id LIKE {prefix})"
        ));
    }
    list
}

/// The ORDER BY of a sort; ties keep name order and unknown values sort last
/// in either direction.
fn order(
    query: &ListQuery,
    sets: &LiveSets,
    conn: &Connection,
    binds: &mut Binds,
    now: i64,
) -> String {
    let by_name = "d.name COLLATE NOCASE ASC, d.id ASC";
    let online = |binds: &mut Binds| {
        format!(
            "d.id IN (SELECT value FROM json_each({}))",
            binds.add(sets.online.clone())
        )
    };
    let Some((key, dir)) = query.sort_by(Dir::Asc) else {
        // Online first, each group by name.
        return format!("NOT {}, {by_name}", online(binds));
    };
    let d = dir.sql();
    match key {
        "name" => format!("d.name COLLATE NOCASE {d}, d.id {d}"),
        "owner" => format!("u.username COLLATE NOCASE {d}, {by_name}"),
        "state" => {
            let online = online(binds);
            let unstable = format!(
                "d.id IN (SELECT value FROM json_each({}))",
                binds.add(sets.unstable.clone())
            );
            format!("(CASE WHEN NOT {online} THEN 0 WHEN {unstable} THEN 1 ELSE 2 END) {d}, {by_name}")
        }
        "protection" => format!("(c.device_id IS NOT NULL) {d}, {by_name}"),
        "version" => {
            // Dotted versions compare as numbers: rank the reported ones.
            let mut versions: Vec<String> = reported_versions(conn)
                .into_iter()
                .map(|(version, _)| version)
                .collect();
            versions.sort_by(|a, b| compare_versions(a, b));
            let cases: String = versions
                .iter()
                .take(500)
                .enumerate()
                .map(|(rank, version)| format!(" WHEN {} THEN {rank}", binds.add(version.clone())))
                .collect();
            if cases.is_empty() {
                format!("v.version IS NULL, v.version {d}, {by_name}")
            } else {
                format!("v.version IS NULL, (CASE v.version{cases} END) {d}, {by_name}")
            }
        }
        // Counts written with each lineage snapshot; a device without a row
        // (lineage written another way) is counted on the spot.
        "threads" => format!(
            "COALESCE(lc.threads,(SELECT COUNT(*) FROM relay_thread_lineage l WHERE l.device_id=d.id)) {d}, {by_name}"
        ),
        "ports" => format!(
            "COALESCE((SELECT p.value FROM json_each({}) p WHERE p.key=d.id),0) {d}, {by_name}",
            binds.add(sets.ports.clone())
        ),
        "created" => format!("d.created_at {d}, {by_name}"),
        "lastSeen" => {
            let online = online(binds);
            let at = binds.add(now);
            let seen = format!("(CASE WHEN {online} THEN {at} ELSE {LAST_SEEN} END)");
            format!("{seen} IS NULL, {seen} {d}, {by_name}")
        }
        "reconnects" => format!(
            "(SELECT COUNT(*) FROM relay_device_connections r WHERE r.device_id=d.id AND r.connected_at>={}) {d}, {by_name}",
            binds.add(now - HOUR_MS)
        ),
        "traffic" => {
            let first = now - now.rem_euclid(DAY_MS) - 6 * DAY_MS;
            format!(
                "COALESCE((SELECT SUM(t.requests) FROM relay_device_traffic_daily t WHERE t.device_id=d.id AND t.day>={}),0) {d}, {by_name}",
                binds.add(first)
            )
        }
        _ => format!("NOT {}, {by_name}", online(binds)),
    }
}

/// Builds full rows for devices read in SQL: tunnels and previews from
/// memory, statistics for these devices only.
fn enrich(conn: &Connection, live: &Live, rows: Vec<BaseRow>) -> Vec<DeviceRow> {
    let ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
    let started = crate::metrics::history_start(conn);
    let recorded = started.is_some();
    let now = now_ms();
    let today = now - now.rem_euclid(DAY_MS);
    let last_seen = crate::metrics::last_seen_of(conn, &ids);
    let traffic = if recorded {
        crate::metrics::daily_requests_of(conn, &ids, 7)
    } else {
        HashMap::new()
    };
    let mut lineage = conn.prepare_cached(
        "SELECT COUNT(*),COUNT(parent_thread_id),COUNT(DISTINCT workspace_id) FROM relay_thread_lineage WHERE device_id=?1",
    );
    let mut connects = conn.prepare_cached(
        "SELECT COUNT(*) FROM relay_device_connections WHERE device_id=?1 AND connected_at>=?2",
    );
    rows.into_iter()
        .map(|row| {
            let threads = lineage
                .as_mut()
                .ok()
                .and_then(|stmt| {
                    stmt.query_row([&row.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                        .ok()
                })
                .unwrap_or_default();
            let connects_hour = recorded.then(|| {
                connects
                    .as_mut()
                    .ok()
                    .and_then(|stmt| {
                        stmt.query_row(params![row.id, now - HOUR_MS], |r| r.get::<_, i64>(0))
                            .ok()
                    })
                    .unwrap_or(0)
            });
            // Days before recording started are unknown, not zero.
            let traffic7d = recorded.then(|| {
                traffic
                    .get(&row.id)
                    .cloned()
                    .unwrap_or_else(|| vec![0; 7])
                    .into_iter()
                    .enumerate()
                    .map(|(index, requests)| {
                        let day = today - (6 - index as i64) * DAY_MS;
                        started
                            .is_some_and(|begun| day + DAY_MS > begun)
                            .then_some(requests)
                    })
                    .collect()
            });
            DeviceRow {
                tunnel: live.tunnels.get(&row.id).cloned(),
                port_mappings: live.previews.get(&row.id).copied().unwrap_or(0),
                last_seen: last_seen.get(&row.id).copied(),
                threads,
                connects_hour,
                traffic7d,
                id: row.id,
                name: row.name,
                owner_user_id: row.owner_user_id,
                owner_username: row.owner_username,
                owner_email: row.owner_email,
                created_at: row.created_at,
                protected: row.protected,
                protected_at: row.protected_at,
                version: row.version,
            }
        })
        .collect()
}

/// A device as SQL reads it, before the per-device statistics.
pub(super) struct BaseRow {
    id: String,
    name: String,
    owner_user_id: String,
    owner_username: String,
    owner_email: String,
    created_at: String,
    protected: bool,
    protected_at: Option<String>,
    version: Option<(Option<String>, i64)>,
}

fn base_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BaseRow> {
    let has_version: bool = row.get(8)?;
    Ok(BaseRow {
        id: row.get(0)?,
        name: row.get(1)?,
        owner_user_id: row.get(2)?,
        owner_username: row.get(3)?,
        owner_email: row.get(4)?,
        created_at: row.get(5)?,
        protected: row.get(6)?,
        // The certificate's signed creation time, when it is text.
        protected_at: row.get::<_, Option<String>>(7).ok().flatten(),
        version: if has_version {
            Some((row.get(9)?, row.get(10)?))
        } else {
            None
        },
    })
}

/// One owner's devices (online first, by name), at most `limit`, and how many
/// they have in all.
pub(super) fn owner_rows(
    conn: &Connection,
    live: &Live,
    owner: &str,
    limit: usize,
) -> (Vec<DeviceRow>, i64) {
    let query = ListQuery {
        owner: Some(owner.to_string()),
        ..Default::default()
    };
    page(conn, live, &query, 0, limit)
}

/// One device, with everything the list shows.
pub(super) fn one(conn: &Connection, live: &Live, id: &str) -> Option<DeviceRow> {
    let rows: Vec<BaseRow> = conn
        .prepare(&format!("SELECT {COLUMNS} FROM {FROM} WHERE d.id=?1"))
        .and_then(|mut stmt| stmt.query_map([id], base_row)?.collect())
        .unwrap_or_default();
    enrich(conn, live, rows).pop()
}

/// A page of the list and the number of matching devices.
fn page(
    conn: &Connection,
    live: &Live,
    query: &ListQuery,
    offset: usize,
    limit: usize,
) -> (Vec<DeviceRow>, i64) {
    let sets = LiveSets::new(conn, live);
    let outdated = outdated_versions(conn);
    page_with(conn, live, query, offset, limit, &sets, &outdated)
}

fn page_with(
    conn: &Connection,
    live: &Live,
    query: &ListQuery,
    offset: usize,
    limit: usize,
    sets: &LiveSets,
    outdated: &[String],
) -> (Vec<DeviceRow>, i64) {
    let now = now_ms();
    let mut binds = Binds::default();
    let conditions = conditions(query, sets, outdated, &mut binds, now);
    let condition = if conditions.is_empty() {
        "1".to_string()
    } else {
        conditions.join(" AND ")
    };
    let total = count(
        conn,
        &format!(
            "SELECT COUNT(*) FROM {FROM} WHERE {condition} AND {}",
            binds.keep_all()
        ),
        binds.params(),
    );
    let order = order(query, sets, conn, &mut binds, now);
    let limit_at = binds.add(limit as i64);
    let offset_at = binds.add(offset as i64);
    let rows: Vec<BaseRow> = conn
        .prepare(&format!(
            "SELECT {COLUMNS} FROM {FROM} WHERE {condition} ORDER BY {order} LIMIT {limit_at} OFFSET {offset_at}"
        ))
        .and_then(|mut stmt| stmt.query_map(binds.params(), base_row)?.collect())
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "admin device list failed");
            Vec::new()
        });
    (enrich(conn, live, rows), total)
}

pub(super) async fn list(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let live = live(&state).await;
    let (conn, _) = match admin_read(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    blocking(|| {
        let paging = query.paging();
        let sets = LiveSets::new(&conn, &live);
        let outdated = outdated_versions(&conn);
        let (rows, total) = page_with(
            &conn,
            &live,
            &query,
            paging.offset(),
            paging.size,
            &sets,
            &outdated,
        );
        // Counts for the filter menus, over every device in one pass.
        let mut binds = Binds::default();
        let online = binds.add(sets.online.clone());
        let outdated_list = binds.add(json_list(outdated.iter()));
        let counts = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*),
                            COALESCE(SUM(d.id IN (SELECT value FROM json_each({online}))),0),
                            COALESCE(SUM(c.device_id IS NOT NULL),0),
                            COALESCE(SUM(v.version IN (SELECT value FROM json_each({outdated_list}))),0),
                            COALESCE(SUM(v.version IS NULL),0)
                     FROM relay_devices d LEFT JOIN relay_device_certificates c ON c.device_id=d.id
                     LEFT JOIN relay_device_versions v ON v.device_id=d.id"
                ),
                binds.params(),
                |row| {
                    let all: i64 = row.get(0)?;
                    let online: i64 = row.get(1)?;
                    let protected: i64 = row.get(2)?;
                    Ok(json!({
                        "all": all,
                        "online": online,
                        "offline": all - online,
                        "protected": protected,
                        "unprotected": all - protected,
                        "unstable": sets.unstable_count,
                        "outdated": row.get::<_, i64>(3)?,
                        "unknownVersion": row.get::<_, i64>(4)?,
                    }))
                },
            )
            .unwrap_or(Value::Null);
        // Every reported version with its device count, for the version filter.
        let versions: Vec<Value> = reported_versions(&conn)
            .into_iter()
            .take(100)
            .map(|(version, devices)| json!({ "version": version, "devices": devices }))
            .collect();
        let owner_label = ListQuery::pick(&query.owner)
            .and_then(|owner| usernames(&conn, &[owner.to_string()]).remove(owner));
        drop(conn);
        let items = rows.iter().map(DeviceRow::json).collect();
        let mut body = paging.wrap(items, total as usize);
        body.insert("counts".into(), counts);
        body.insert("relayVersion".into(), json!(RELAY_VERSION));
        body.insert("versions".into(), json!(versions));
        body.insert("labels".into(), json!({ "owner": owner_label }));
        Json(Value::Object(body)).into_response()
    })
}

pub(super) async fn detail(
    Path(device_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let live = live(&state).await;
    let (previews, _, _) = state.preview.usage();
    let (conn, _) = match admin_read(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    blocking(|| {
        let Some(device) = one(&conn, &live, &device_id) else {
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
                "SELECT s.id,'share','thread',COALESCE(t.username,s.target_username),s.created_at,s.target_user_id FROM relay_shares s
                   LEFT JOIN relay_users t ON t.id=s.target_user_id
                  WHERE s.device_id=?1 AND s.revoked_at IS NULL AND (s.expires_at IS NULL OR s.expires_at>?2)
                 UNION ALL
                 SELECT g.id,'grant',g.scope,COALESCE(t.username,g.target_username),g.created_at,g.target_user_id FROM relay_access_grants g
                   LEFT JOIN relay_users t ON t.id=g.target_user_id
                  WHERE g.device_id=?1 AND g.revoked_at IS NULL AND (g.expires_at IS NULL OR g.expires_at>?2)
                 ORDER BY 5 DESC LIMIT 100",
            )
            .and_then(|mut stmt| {
                stmt.query_map(params![device_id, now], |row| {
                    Ok(json!({
                        "id": row.get::<_, String>(0)?,
                        "kind": row.get::<_, String>(1)?,
                        "scope": row.get::<_, String>(2)?,
                        "targetUsername": row.get::<_, Option<String>>(3)?,
                        "targetUserId": row.get::<_, Option<String>>(5)?,
                    }))
                })?
                .collect()
            })
            .unwrap_or_default();
        let (sessions, streams) = previews.get(&device_id).copied().unwrap_or_default();
        // The drawer's range: 24 hours, 7 or 30 days (tunnels are kept 30 days).
        let range = match query.range(Range::Day) {
            Range::Quarter => Range::Month,
            range => range,
        };
        let (connections, traffic) = super::stats::device_detail(&conn, &device_id, range);
        Json(json!({
            "device": device.json(),
            "upstreamProtocols": protocols,
            "shares": shares,
            "previews": { "mappings": device.port_mappings, "sessions": sessions, "streams": streams },
            "range": range.key(),
            "connections": connections,
            "traffic": traffic,
        }))
        .into_response()
    })
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
