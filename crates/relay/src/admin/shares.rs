//! Shares and access: thread shares and grants as one list, revoking one,
//! public links and port previews. No thread titles or link contents.
//!
//! The list filters, sorts, counts and pages in SQL over both tables; the last
//! visit of each share comes from the activity index, never a table scan.
use super::common::*;
use super::*;

/// Every active share and grant as one relation. `?1` is now (text).
const ACTIVE: &str = "
    SELECT 'share' AS kind,s.id AS id,'thread' AS scope,s.owner_user_id AS owner_id,
           COALESCE(o.username,s.owner_username) AS owner_name,o.email AS owner_email,
           s.target_user_id AS target_id,COALESCE(t.username,s.target_username) AS target_name,t.email AS target_email,
           s.device_id AS device_id,COALESCE(dv.name,s.device_name) AS device_name,s.workspace_label AS workspace_label,
           s.thread_access AS thread_access,s.workspace_access AS workspace_access,0 AS create_threads,
           s.created_at AS created_at,s.expires_at AS expires_at,
           (SELECT MAX(a.accessed_at) FROM relay_share_activity a WHERE a.resource_type='share' AND a.resource_id=s.id) AS last_at
    FROM relay_shares s JOIN relay_users o ON o.id=s.owner_user_id JOIN relay_users t ON t.id=s.target_user_id
    LEFT JOIN relay_devices dv ON dv.id=s.device_id
    WHERE s.revoked_at IS NULL AND (s.expires_at IS NULL OR s.expires_at>?1)
    UNION ALL
    SELECT 'grant',g.id,g.scope,g.owner_user_id,COALESCE(o.username,g.owner_username),o.email,
           g.target_user_id,COALESCE(t.username,g.target_username),t.email,
           g.device_id,COALESCE(dv.name,g.device_name),g.workspace_label,
           g.thread_access,g.workspace_access,g.can_create_threads,g.created_at,g.expires_at,
           (SELECT MAX(a.accessed_at) FROM relay_share_activity a WHERE a.resource_type='grant' AND a.resource_id=g.id)
    FROM relay_access_grants g JOIN relay_users o ON o.id=g.owner_user_id JOIN relay_users t ON t.id=g.target_user_id
    LEFT JOIN relay_devices dv ON dv.id=g.device_id
    WHERE g.revoked_at IS NULL AND (g.expires_at IS NULL OR g.expires_at>?1)";

const ACCESS_RANK: &str = "((CASE WHEN thread_access='control' THEN 4 ELSE 0 END)
    + (CASE workspace_access WHEN 'write' THEN 2 WHEN 'read' THEN 1 ELSE 0 END) + create_threads)";

const EXPIRING_MS: i64 = 7 * DAY_MS;

fn share_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let kind: String = row.get(0)?;
    Ok(json!({
        "kind": if kind == "share" { "share" } else { "grant" },
        "id": row.get::<_, String>(1)?,
        "scope": row.get::<_, String>(2)?,
        "ownerUserId": row.get::<_, String>(3)?,
        "ownerUsername": row.get::<_, Option<String>>(4)?,
        "ownerEmail": row.get::<_, Option<String>>(5)?,
        "targetUserId": row.get::<_, String>(6)?,
        "targetUsername": row.get::<_, Option<String>>(7)?,
        "targetEmail": row.get::<_, Option<String>>(8)?,
        "deviceId": row.get::<_, String>(9)?,
        "deviceName": row.get::<_, Option<String>>(10)?.unwrap_or_else(|| "Pockymoe device".into()),
        "workspaceLabel": row.get::<_, Option<String>>(11)?,
        "threadAccess": row.get::<_, String>(12)?,
        "workspaceAccess": row.get::<_, String>(13)?,
        "canCreateThreads": row.get::<_, i64>(14)? == 1,
        "createdAt": row.get::<_, String>(15)?,
        "expiresAt": row.get::<_, Option<String>>(16)?,
        "lastAccessedAt": row.get::<_, Option<String>>(17)?,
        "lastAccessKind": row.get::<_, Option<String>>(18)?,
    }))
}

/// Conditions of the list's filters over `ACTIVE` (`?2` is the expiry horizon).
fn conditions(query: &ListQuery, binds: &mut Binds) -> Vec<String> {
    let mut list = Vec::new();
    let expiring = "expires_at IS NOT NULL AND expires_at<=?2";
    match query.filter() {
        scope @ ("device" | "workspace" | "thread") => list.push(format!("scope='{scope}'")),
        "expiring" => list.push(expiring.into()),
        "unused" => list.push("last_at IS NULL".into()),
        _ => {}
    }
    if let Some(scope @ ("device" | "workspace" | "thread")) = ListQuery::pick(&query.scope) {
        list.push(format!("scope='{scope}'"));
    }
    if let Some(owner) = ListQuery::pick(&query.owner) {
        list.push(format!("owner_id={}", binds.add(owner.to_string())));
    }
    if let Some(recipient) = ListQuery::pick(&query.recipient) {
        list.push(format!("target_id={}", binds.add(recipient.to_string())));
    }
    match ListQuery::pick(&query.access) {
        Some("control") => list.push("thread_access='control'".into()),
        Some("read") => list.push("thread_access='read'".into()),
        Some("filesWrite") => list.push("workspace_access='write'".into()),
        Some("filesRead") => list.push("workspace_access='read'".into()),
        Some("createThreads") => list.push("create_threads=1".into()),
        _ => {}
    }
    match ListQuery::pick(&query.expiry) {
        Some("expiring") => list.push(expiring.into()),
        Some("set") => list.push("expires_at IS NOT NULL".into()),
        Some("none") => list.push("expires_at IS NULL".into()),
        _ => {}
    }
    // Times are RFC 3339 text: compare as text in the same form.
    match ListQuery::pick(&query.seen) {
        Some("never") => list.push("last_at IS NULL".into()),
        _ => {
            let (from, to) = query.seen_window().text();
            if !from.is_empty() {
                list.push(format!("last_at>={}", binds.add(from)));
            }
            if !to.is_empty() {
                list.push(format!("last_at<{}", binds.add(to)));
            }
        }
    }
    let (from, to) = query.created_window().text();
    if !from.is_empty() {
        list.push(format!("created_at>={}", binds.add(from)));
    }
    if !to.is_empty() {
        list.push(format!("created_at<{}", binds.add(to)));
    }
    if let Some(search) = query.search() {
        let pattern = binds.add(like_pattern(&search));
        let prefix = binds.add(format!("{}%", search.replace(['%', '_', '\\'], "")));
        list.push(format!(
            "(lower(owner_name) LIKE {pattern} ESCAPE '\\' OR lower(target_name) LIKE {pattern} ESCAPE '\\'
              OR lower(device_name) LIKE {pattern} ESCAPE '\\' OR id LIKE {prefix})"
        ));
    }
    list
}

fn order(query: &ListQuery) -> String {
    let newest = "created_at DESC, kind, id";
    let Some((key, dir)) = query.sort_by(Dir::Asc) else {
        return newest.into();
    };
    let d = dir.sql();
    match key {
        "scope" => format!("scope {d}, {newest}"),
        "owner" => format!("owner_name COLLATE NOCASE {d}, {newest}"),
        "recipient" => format!("target_name COLLATE NOCASE {d}, {newest}"),
        "device" => format!("device_name COLLATE NOCASE {d}, {newest}"),
        "access" => format!("{ACCESS_RANK} {d}, {newest}"),
        // Never used and no expiry sort last either way.
        "lastUsed" => format!("last_at IS NULL, last_at {d}, {newest}"),
        "expires" => format!("expires_at IS NULL, expires_at {d}, {newest}"),
        "created" => format!("created_at {d}, kind, id"),
        _ => newest.into(),
    }
}

pub(super) async fn list(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match admin_read(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    blocking(|| {
        let now = now_ms();
        let window = match chart_window(&query, Range::Week, now) {
            Ok(window) => window,
            Err(message) => return bad_request(message),
        };
        let paging = query.paging();
        let mut binds = Binds::default();
        binds.add(now_rfc3339());
        binds.add(rfc3339_from_ms(now + EXPIRING_MS));
        let conditions = conditions(&query, &mut binds);
        let condition = if conditions.is_empty() {
            "1".to_string()
        } else {
            conditions.join(" AND ")
        };
        // Without filters the totals below count every active share already.
        let filtered = !conditions.is_empty();
        let total = if filtered {
            count(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM ({ACTIVE}) WHERE {condition} AND {}",
                    binds.keep_all()
                ),
                binds.params(),
            )
        } else {
            0
        };
        let limit = binds.add(paging.size as i64);
        let offset = binds.add(paging.offset() as i64);
        let items: Vec<Value> = conn
            .prepare(&format!(
                "SELECT kind,id,scope,owner_id,owner_name,owner_email,target_id,target_name,target_email,
                        device_id,device_name,workspace_label,thread_access,workspace_access,create_threads,
                        created_at,expires_at,last_at,
                        (SELECT a.kind FROM relay_share_activity a WHERE a.resource_type=x.kind AND a.resource_id=x.id
                         ORDER BY a.accessed_at DESC LIMIT 1)
                 FROM ({ACTIVE}) x WHERE {condition} ORDER BY {} LIMIT {limit} OFFSET {offset}",
                order(&query)
            ))
            .and_then(|mut stmt| stmt.query_map(binds.params(), share_json)?.collect())
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "admin share list failed");
                Vec::new()
            });
        // Totals over every active share, in one pass.
        let (all, device, workspace, thread, expiring, unused, control, read, files_write, create) = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*),COALESCE(SUM(scope='device'),0),COALESCE(SUM(scope='workspace'),0),
                            COALESCE(SUM(scope='thread'),0),
                            COALESCE(SUM(expires_at IS NOT NULL AND expires_at<=?2),0),COALESCE(SUM(last_at IS NULL),0),
                            COALESCE(SUM(thread_access='control'),0),COALESCE(SUM(thread_access='read'),0),
                            COALESCE(SUM(workspace_access='write'),0),COALESCE(SUM(create_threads=1),0)
                     FROM ({ACTIVE})"
                ),
                params![now_rfc3339(), rfc3339_from_ms(now + EXPIRING_MS)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, i64>(9)?,
                    ))
                },
            )
            .unwrap_or_default();
        let changed = |since: &str| -> (i64, i64) {
            conn.query_row(
                "SELECT (SELECT COUNT(*) FROM relay_shares WHERE created_at>=?1)+(SELECT COUNT(*) FROM relay_access_grants WHERE created_at>=?1),
                        (SELECT COUNT(*) FROM relay_shares WHERE revoked_at>=?1)+(SELECT COUNT(*) FROM relay_access_grants WHERE revoked_at>=?1)",
                [since],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((0, 0))
        };
        let (created_week, revoked_week) = changed(&rfc3339_ago(7 * DAY_MS));
        let (created_range, revoked_range) = changed(&rfc3339_from_ms(window.start));
        let usage = usage_over(&conn, &window);
        let names = usernames(
            &conn,
            &[
                ListQuery::pick(&query.owner),
                ListQuery::pick(&query.recipient),
            ]
            .into_iter()
            .flatten()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        );
        drop(conn);
        let summary = json!({
            "active": all,
            "createdThisWeek": created_week,
            "revokedThisWeek": revoked_week,
            "createdInRange": created_range,
            "revokedInRange": revoked_range,
            "expiringSoon": expiring,
            "byScope": { "device": device, "workspace": workspace, "thread": thread },
            "permissions": {
                "control": control,
                "read": read,
                "filesWrite": files_write,
                "canCreateThreads": create,
            },
        });
        let counts = json!({
            "all": all,
            "device": device,
            "workspace": workspace,
            "thread": thread,
            "expiring": expiring,
            "unused": unused,
        });
        let total = if filtered { total } else { all };
        let mut body = paging.wrap(items, total as usize);
        body.insert("counts".into(), counts);
        body.insert("summary".into(), summary);
        body.insert("usage".into(), usage);
        body.insert("window".into(), window.json());
        let label = |id: Option<&str>| id.and_then(|id| names.get(id).cloned());
        body.insert(
            "labels".into(),
            json!({
                "owner": label(ListQuery::pick(&query.owner)),
                "recipient": label(ListQuery::pick(&query.recipient)),
            }),
        );
        Json(Value::Object(body)).into_response()
    })
}

/// Recipients' recorded visits per day (per hour for two days or less) and
/// kind over the window. The activity table merges repeats within five
/// minutes and keeps 100 rows per share, so this is approximate.
fn usage_over(conn: &Connection, window: &ChartWindow) -> Value {
    let hourly = window.span() <= 2 * DAY_MS;
    let (step, width) = if hourly { (HOUR_MS, 13) } else { (DAY_MS, 10) };
    let first = if hourly {
        window.start - window.start.rem_euclid(step)
    } else {
        window.first_day()
    };
    let rows: Vec<(String, String, i64)> = conn
        .prepare(&format!(
            "SELECT substr(accessed_at,1,{width}),kind,COUNT(*) FROM relay_share_activity
             WHERE accessed_at>=?1 AND accessed_at<?2 GROUP BY 1,2"
        ))
        .and_then(|mut stmt| {
            stmt.query_map(
                params![rfc3339_from_ms(first), rfc3339_from_ms(window.end)],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?
            .collect()
        })
        .unwrap_or_default();
    let buckets = if hourly {
        ((window.end - first + step - 1) / step).clamp(1, 400)
    } else {
        window.days().min(400)
    };
    let series: Vec<Value> = (0..buckets)
        .map(|index| {
            let at = first + index * step;
            let key = rfc3339_from_ms(at)[..width].to_string();
            let mut kinds = serde_json::Map::new();
            for (row_key, kind, count) in &rows {
                if *row_key == key {
                    kinds.insert(kind.clone(), json!(count));
                }
            }
            if hourly {
                json!({ "day": day_of(at), "at": rfc3339_from_ms(at), "kinds": kinds })
            } else {
                json!({ "day": day_of(at), "kinds": kinds })
            }
        })
        .collect();
    json!({ "days": series, "approximate": true, "resolutionSeconds": step / 1000 })
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
/// encrypted (from `relay_public_link_stats`). Never the content or its title.
pub(super) async fn public_links(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match admin_read(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    blocking(|| {
        let paging = query.paging();
        let condition = match query.filter() {
            "legacy" => "COALESCE(st.encrypted,0)=0",
            "encrypted" => "COALESCE(st.encrypted,0)=1",
            "live" => "s.link_id IS NOT NULL",
            _ => "1",
        };
        let from = "FROM relay_public_links l JOIN relay_users u ON u.id=l.owner_user_id
                    JOIN relay_devices d ON d.id=l.device_id
                    LEFT JOIN relay_public_link_sources s ON s.link_id=l.id
                    LEFT JOIN relay_public_link_stats st ON st.link_id=l.id";
        let total = count(
            &conn,
            &format!("SELECT COUNT(*) {from} WHERE {condition}"),
            [],
        );
        let order = match query.sort_by(Dir::Desc) {
            Some((key, dir)) => {
                let d = dir.sql();
                match key {
                    "owner" => format!("u.username {d}, l.created_at DESC"),
                    "device" => format!("d.name {d}, l.created_at DESC"),
                    "storage" => format!("COALESCE(st.encrypted,0) {d}, l.created_at DESC"),
                    "live" => format!("s.link_id IS NOT NULL {d}, l.created_at DESC"),
                    "refreshed" => format!("s.refreshed_at IS NULL, s.refreshed_at {d}"),
                    "size" => format!("COALESCE(st.bytes,0) {d}"),
                    _ => format!("l.created_at {d}"),
                }
            }
            None => "l.created_at DESC".to_string(),
        };
        let items: Vec<Value> = conn
            .prepare(&format!(
                "SELECT l.id,u.username,d.name,l.created_at,s.refreshed_at,s.link_id IS NOT NULL,
                        COALESCE(st.encrypted,0),COALESCE(st.bytes,0),l.owner_user_id,u.email,l.device_id
                 {from} WHERE {condition} ORDER BY {order} LIMIT ?1 OFFSET ?2"
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
                        "ownerUserId": row.get::<_, String>(8)?,
                        "ownerEmail": row.get::<_, String>(9)?,
                        "deviceId": row.get::<_, String>(10)?,
                    }))
                })?
                .collect()
            })
            .unwrap_or_default();
        let summary = public_link_summary(&conn);
        let mut body = paging.wrap(items, total as usize);
        body.insert("summary".into(), summary);
        Json(Value::Object(body)).into_response()
    })
}

pub(super) fn public_link_summary(conn: &Connection) -> Value {
    conn.query_row(
        "SELECT COUNT(*),COALESCE(SUM(st.encrypted),0),
                COALESCE(SUM(EXISTS(SELECT 1 FROM relay_public_link_sources s WHERE s.link_id=l.id)),0),
                COALESCE(SUM(st.bytes),0)
         FROM relay_public_links l LEFT JOIN relay_public_link_stats st ON st.link_id=l.id",
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

/// Port previews held in memory: mappings, sessions and streams per device,
/// busiest first, at most `PREVIEW_ROWS` (with the total).
const PREVIEW_ROWS: usize = 200;

pub(super) async fn previews(headers: HeaderMap, State(state): State<Arc<AppState>>) -> Response {
    let mappings = state.preview.mapping_counts();
    let (usage, sessions, streams) = state.preview.usage();
    let (conn, _) = match admin_read(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    blocking(|| {
        let mut ids: Vec<String> = mappings.keys().chain(usage.keys()).cloned().collect();
        ids.sort();
        ids.dedup();
        let weight = |id: &String| {
            let (s, st) = usage.get(id).copied().unwrap_or_default();
            (mappings.get(id).copied().unwrap_or(0), s, st)
        };
        ids.sort_by(|a, b| weight(b).cmp(&weight(a)).then_with(|| a.cmp(b)));
        let total = ids.len();
        ids.truncate(PREVIEW_ROWS);
        let refs = device_refs(&conn, &ids);
        drop(conn);
        let devices: Vec<Value> = ids
            .iter()
            .filter_map(|id| {
                let device = refs.get(id)?;
                let (device_sessions, device_streams) = usage.get(id).copied().unwrap_or_default();
                let mut row = device.fields();
                row.insert("deviceId".into(), json!(id));
                row.insert(
                    "mappings".into(),
                    json!(mappings.get(id).copied().unwrap_or(0)),
                );
                row.insert("sessions".into(), json!(device_sessions));
                row.insert("streams".into(), json!(device_streams));
                Some(Value::Object(row))
            })
            .collect();
        Json(json!({
            "configured": state.preview.base.is_some(),
            "mappings": mappings.values().sum::<usize>(),
            "sessions": sessions,
            "streams": streams,
            "devices": devices,
            "devicesTotal": total,
        }))
        .into_response()
    })
}
