//! Shares and access: thread shares and grants as one list, revoking one,
//! public links and port previews. No thread titles or link contents.
use super::common::*;
use super::*;

#[derive(Clone)]
struct ShareRow {
    id: String,
    kind: &'static str,
    scope: String,
    owner_user_id: String,
    owner_username: String,
    target_user_id: String,
    target_username: String,
    device_id: String,
    device_name: String,
    workspace_label: Option<String>,
    thread_access: String,
    workspace_access: String,
    can_create_threads: bool,
    created_at: String,
    expires_at: Option<String>,
    last_access: Option<(String, String)>,
}

impl ShareRow {
    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "kind": self.kind,
            "scope": self.scope,
            "ownerUserId": self.owner_user_id,
            "ownerUsername": self.owner_username,
            "targetUserId": self.target_user_id,
            "targetUsername": self.target_username,
            "deviceId": self.device_id,
            "deviceName": self.device_name,
            "workspaceLabel": self.workspace_label,
            "threadAccess": self.thread_access,
            "workspaceAccess": self.workspace_access,
            "canCreateThreads": self.can_create_threads,
            "createdAt": self.created_at,
            "expiresAt": self.expires_at,
            "lastAccessedAt": self.last_access.as_ref().map(|(at, _)| at),
            "lastAccessKind": self.last_access.as_ref().map(|(_, kind)| kind),
        })
    }
}

/// The latest recorded visit of every shared resource.
fn last_visits(conn: &Connection) -> HashMap<(String, String), (String, String)> {
    conn.prepare(
        "SELECT a.resource_type,a.resource_id,a.accessed_at,a.kind FROM relay_share_activity a
         WHERE a.accessed_at=(SELECT MAX(b.accessed_at) FROM relay_share_activity b
                              WHERE b.resource_type=a.resource_type AND b.resource_id=a.resource_id)",
    )
    .and_then(|mut stmt| {
        stmt.query_map([], |row| {
            Ok((
                (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                (row.get::<_, String>(2)?, row.get::<_, String>(3)?),
            ))
        })?
        .collect()
    })
    .unwrap_or_default()
}

fn active_rows(conn: &Connection) -> Vec<ShareRow> {
    let visits = last_visits(conn);
    let now = now_rfc3339();
    let sql = "SELECT 'share',s.id,'thread',s.owner_user_id,COALESCE(s.owner_username,o.username),s.target_user_id,
                      COALESCE(s.target_username,t.username),s.device_id,COALESCE(d.name,s.device_name),s.workspace_label,
                      s.thread_access,s.workspace_access,0,s.created_at,s.expires_at
               FROM relay_shares s JOIN relay_users o ON o.id=s.owner_user_id JOIN relay_users t ON t.id=s.target_user_id
               LEFT JOIN relay_devices d ON d.id=s.device_id
               WHERE s.revoked_at IS NULL AND (s.expires_at IS NULL OR s.expires_at>?1)
               UNION ALL
               SELECT 'grant',g.id,g.scope,g.owner_user_id,COALESCE(g.owner_username,o.username),g.target_user_id,
                      COALESCE(g.target_username,t.username),g.device_id,COALESCE(d.name,g.device_name),g.workspace_label,
                      g.thread_access,g.workspace_access,g.can_create_threads,g.created_at,g.expires_at
               FROM relay_access_grants g JOIN relay_users o ON o.id=g.owner_user_id JOIN relay_users t ON t.id=g.target_user_id
               LEFT JOIN relay_devices d ON d.id=g.device_id
               WHERE g.revoked_at IS NULL AND (g.expires_at IS NULL OR g.expires_at>?1)
               ORDER BY 14 DESC";
    conn.prepare(sql)
        .and_then(|mut stmt| {
            stmt.query_map([&now], |row| {
                let kind: String = row.get(0)?;
                let id: String = row.get(1)?;
                Ok(ShareRow {
                    last_access: visits.get(&(kind.clone(), id.clone())).cloned(),
                    kind: if kind == "share" { "share" } else { "grant" },
                    id,
                    scope: row.get(2)?,
                    owner_user_id: row.get(3)?,
                    owner_username: row.get(4)?,
                    target_user_id: row.get(5)?,
                    target_username: row.get(6)?,
                    device_id: row.get(7)?,
                    device_name: row
                        .get::<_, Option<String>>(8)?
                        .unwrap_or_else(|| "Pockymoe device".into()),
                    workspace_label: row.get(9)?,
                    thread_access: row.get(10)?,
                    workspace_access: row.get(11)?,
                    can_create_threads: row.get::<_, i64>(12)? == 1,
                    created_at: row.get(13)?,
                    expires_at: row.get(14)?,
                })
            })?
            .collect()
        })
        .unwrap_or_default()
}

const EXPIRING_MS: i64 = 7 * DAY_MS;

fn expiring(row: &ShareRow, horizon: &str) -> bool {
    row.expires_at
        .as_deref()
        .is_some_and(|expires| expires <= horizon)
}

fn matches_filter(row: &ShareRow, filter: &str, horizon: &str) -> bool {
    match filter {
        "device" | "workspace" | "thread" => row.scope == filter,
        "expiring" => expiring(row, horizon),
        "unused" => row.last_access.is_none(),
        _ => true,
    }
}

pub(super) async fn list(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let rows = active_rows(&conn);
    let horizon = rfc3339_from_ms(now_ms() + EXPIRING_MS);
    let week = rfc3339_ago(7 * DAY_MS);
    let (created_week, revoked_week) = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM relay_shares WHERE created_at>=?1)+(SELECT COUNT(*) FROM relay_access_grants WHERE created_at>=?1),
                    (SELECT COUNT(*) FROM relay_shares WHERE revoked_at>=?1)+(SELECT COUNT(*) FROM relay_access_grants WHERE revoked_at>=?1)",
            [&week],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap_or((0, 0));
    let usage = usage_by_day(&conn, 14);
    drop(conn);
    let search = query.search();
    let matching: Vec<&ShareRow> = rows
        .iter()
        .filter(|row| matches_filter(row, query.filter(), &horizon))
        .filter(|row| {
            search.as_deref().is_none_or(|search| {
                row.owner_username.to_lowercase().contains(search)
                    || row.target_username.to_lowercase().contains(search)
                    || row.device_name.to_lowercase().contains(search)
                    || row.id.starts_with(search)
            })
        })
        .collect();
    let paging = query.paging();
    let items = matching
        .iter()
        .skip(paging.offset())
        .take(paging.size)
        .map(|row| row.json())
        .collect();
    let scope = |name: &str| rows.iter().filter(|row| row.scope == name).count();
    let summary = json!({
        "active": rows.len(),
        "createdThisWeek": created_week,
        "revokedThisWeek": revoked_week,
        "expiringSoon": rows.iter().filter(|row| expiring(row, &horizon)).count(),
        "byScope": { "device": scope("device"), "workspace": scope("workspace"), "thread": scope("thread") },
        "permissions": {
            "control": rows.iter().filter(|row| row.thread_access == "control").count(),
            "read": rows.iter().filter(|row| row.thread_access == "read").count(),
            "filesWrite": rows.iter().filter(|row| row.workspace_access == "write").count(),
            "canCreateThreads": rows.iter().filter(|row| row.can_create_threads).count(),
        },
    });
    let counts = json!({
        "all": rows.len(),
        "device": scope("device"),
        "workspace": scope("workspace"),
        "thread": scope("thread"),
        "expiring": rows.iter().filter(|row| expiring(row, &horizon)).count(),
        "unused": rows.iter().filter(|row| row.last_access.is_none()).count(),
    });
    let mut body = paging.wrap(items, matching.len());
    body.insert("counts".into(), counts);
    body.insert("summary".into(), summary);
    body.insert("usage".into(), usage);
    Json(Value::Object(body)).into_response()
}

/// Recipients' recorded visits per day and kind. The activity table merges
/// repeats within five minutes and keeps 100 rows per share, so this is
/// approximate.
fn usage_by_day(conn: &Connection, days: i64) -> Value {
    let start = day_of(now_ms() - (days - 1) * DAY_MS);
    let rows: Vec<(String, String, i64)> = conn
        .prepare(
            "SELECT substr(accessed_at,1,10),kind,COUNT(*) FROM relay_share_activity
             WHERE accessed_at>=?1 GROUP BY 1,2",
        )
        .and_then(|mut stmt| {
            stmt.query_map([&start], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect()
        })
        .unwrap_or_default();
    let series: Vec<Value> = (0..days)
        .map(|offset| {
            let day = day_of(now_ms() - (days - 1 - offset) * DAY_MS);
            let mut kinds = serde_json::Map::new();
            for (row_day, kind, count) in &rows {
                if row_day == &day {
                    kinds.insert(kind.clone(), json!(count));
                }
            }
            json!({ "day": day, "kinds": kinds })
        })
        .collect();
    json!({ "days": series, "approximate": true })
}

/// Revoking as admin only stops the relay from forwarding. A protected device
/// also enforces its own signed grant, which only the owner can delete.
pub(super) async fn revoke(
    Path((kind, id)): Path<(String, String)>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, admin) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let table = match kind.as_str() {
        "share" => "relay_shares",
        "grant" => "relay_access_grants",
        _ => return not_found("Share not found"),
    };
    let revoked_at = now_rfc3339();
    let label: Option<(String, String, String)> = conn
        .query_row(
            &format!(
                "SELECT COALESCE(s.owner_username,''),COALESCE(s.target_username,''),{}
                 FROM {table} s WHERE s.id=?1",
                if kind == "share" {
                    "'thread'"
                } else {
                    "s.scope"
                }
            ),
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .ok()
        .flatten();
    match conn.execute(
        &format!("UPDATE {table} SET revoked_at=?1 WHERE id=?2 AND revoked_at IS NULL"),
        params![revoked_at, id],
    ) {
        Ok(1) => {
            let (owner, target, scope) = label.unwrap_or_default();
            super::log_action(
                &conn,
                &admin,
                "share.revoked",
                Some((&kind, &id, &format!("{owner} → {target}"))),
                json!({ "scope": scope }),
            );
            Json(json!({ "id": id, "kind": kind, "revokedAt": revoked_at })).into_response()
        }
        Ok(_) => not_found("Share not found"),
        Err(_) => internal_error(),
    }
}

/// Public links: owner, device, age, size and whether the stored snapshot is
/// encrypted. Never the content or its title.
pub(super) async fn public_links(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let paging = query.paging();
    let condition = match query.filter() {
        "legacy" => "COALESCE(json_extract(l.snapshot_json,'$.version'),1)<>2",
        "encrypted" => "COALESCE(json_extract(l.snapshot_json,'$.version'),1)=2",
        "live" => "s.link_id IS NOT NULL",
        _ => "1",
    };
    let from = "FROM relay_public_links l JOIN relay_users u ON u.id=l.owner_user_id
                JOIN relay_devices d ON d.id=l.device_id
                LEFT JOIN relay_public_link_sources s ON s.link_id=l.id";
    let total = count(
        &conn,
        &format!("SELECT COUNT(*) {from} WHERE {condition}"),
        [],
    );
    let items: Vec<Value> = conn
        .prepare(&format!(
            "SELECT l.id,u.username,d.name,l.created_at,s.refreshed_at,s.link_id IS NOT NULL,
                    COALESCE(json_extract(l.snapshot_json,'$.version'),1)=2,length(l.snapshot_json)
             {from} WHERE {condition} ORDER BY l.created_at DESC LIMIT ?1 OFFSET ?2"
        ))
        .and_then(|mut stmt| {
            stmt.query_map(params![paging.size as i64, paging.offset() as i64], |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "ownerUsername": row.get::<_, String>(1)?,
                    "deviceName": row.get::<_, String>(2)?,
                    "createdAt": row.get::<_, String>(3)?,
                    "refreshedAt": row.get::<_, Option<String>>(4)?,
                    "live": row.get::<_, bool>(5)?,
                    "encrypted": row.get::<_, bool>(6)?,
                    "bytes": row.get::<_, i64>(7)?,
                }))
            })?
            .collect()
        })
        .unwrap_or_default();
    let summary = public_link_summary(&conn);
    let mut body = paging.wrap(items, total as usize);
    body.insert("summary".into(), summary);
    Json(Value::Object(body)).into_response()
}

pub(super) fn public_link_summary(conn: &Connection) -> Value {
    conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(COALESCE(json_extract(l.snapshot_json,'$.version'),1)=2),0),
                COALESCE(SUM(EXISTS(SELECT 1 FROM relay_public_link_sources s WHERE s.link_id=l.id)),0),
                COALESCE(SUM(length(l.snapshot_json)),0)
         FROM relay_public_links l",
        [],
        |row| {
            let total: i64 = row.get(0)?;
            let encrypted: i64 = row.get(1)?;
            Ok(json!({
                "total": total,
                "encrypted": encrypted,
                "legacy": total - encrypted,
                "live": row.get::<_, i64>(2)?,
                "bytes": row.get::<_, i64>(3)?,
            }))
        },
    )
    .unwrap_or(Value::Null)
}

/// Port previews held in memory: mappings, sessions and streams per device.
pub(super) async fn previews(headers: HeaderMap, State(state): State<Arc<AppState>>) -> Response {
    let mappings = state.preview.mapping_counts();
    let (usage, sessions, streams) = state.preview.usage();
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut ids: Vec<String> = mappings.keys().chain(usage.keys()).cloned().collect();
    ids.sort();
    ids.dedup();
    let names = device_names(&conn, &ids);
    let owners: HashMap<String, String> = ids
        .iter()
        .filter_map(|id| {
            conn.query_row(
                "SELECT u.username FROM relay_devices d JOIN relay_users u ON u.id=d.owner_user_id WHERE d.id=?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .map(|name| (id.clone(), name))
        })
        .collect();
    let devices: Vec<Value> = ids
        .iter()
        .filter(|id| names.contains_key(*id))
        .map(|id| {
            let (device_sessions, device_streams) = usage.get(id).copied().unwrap_or_default();
            json!({
                "deviceId": id,
                "deviceName": names.get(id),
                "ownerUsername": owners.get(id),
                "mappings": mappings.get(id).copied().unwrap_or(0),
                "sessions": device_sessions,
                "streams": device_streams,
            })
        })
        .collect();
    Json(json!({
        "configured": state.preview.base.is_some(),
        "mappings": mappings.values().sum::<usize>(),
        "sessions": sessions,
        "streams": streams,
        "devices": devices,
    }))
    .into_response()
}
