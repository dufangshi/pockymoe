//! Sessions and security: the relay-wide security audit (account ID, event,
//! resource ID and time; nothing else is recorded) and live sessions.
use super::common::*;
use super::*;

/// The category the console groups an event into.
pub(crate) fn event_category(event: &str) -> &'static str {
    match event {
        "login.mfa" | "login.password_or_oauth" => "login",
        "login.challenge" => "challenge",
        "security.reauthenticated" => "reauth",
        "login.failed" => "failure",
        "registration.submitted"
        | "device.created"
        | "device.deleted"
        | "share.created"
        | "share.revoked"
        | "grant.created"
        | "grant.revoked" => "activity",
        _ => "credential",
    }
}

fn event_json(
    row: (i64, Option<String>, String, Option<String>, i64),
    users: &HashMap<String, String>,
    devices: &HashMap<String, String>,
) -> Value {
    let (id, user, event, resource, at) = row;
    // Device events name the device; other resources stay IDs.
    let resource_label = resource
        .as_ref()
        .filter(|_| event.starts_with("device."))
        .and_then(|resource| devices.get(resource))
        .cloned();
    json!({
        "id": id,
        "at": rfc3339_from_ms(at),
        "event": event,
        "category": event_category(&event),
        "userId": user,
        "username": user.as_ref().and_then(|user| users.get(user)),
        "resourceId": resource,
        "resourceLabel": resource_label,
    })
}

type EventRow = (i64, Option<String>, String, Option<String>, i64);

fn read_events(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> Vec<Value> {
    let rows: Vec<EventRow> = conn
        .prepare(sql)
        .and_then(|mut stmt| {
            stmt.query_map(params, |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect()
        })
        .unwrap_or_default();
    let user_ids: Vec<String> = rows.iter().filter_map(|row| row.1.clone()).collect();
    let device_ids: Vec<String> = rows
        .iter()
        .filter(|row| row.2.starts_with("device."))
        .filter_map(|row| row.3.clone())
        .collect();
    let users = usernames(conn, &user_ids);
    let devices = device_names(conn, &device_ids);
    rows.into_iter()
        .map(|row| event_json(row, &users, &devices))
        .collect()
}

pub(super) fn recent(conn: &Connection, limit: i64) -> Vec<Value> {
    read_events(
        conn,
        "SELECT id,user_id,event,resource_id,created_at FROM relay_security_events ORDER BY created_at DESC,id DESC LIMIT ?1",
        [limit],
    )
}

pub(super) fn recent_for_user(conn: &Connection, user: &str, limit: i64) -> Vec<Value> {
    read_events(
        conn,
        "SELECT id,user_id,event,resource_id,created_at FROM relay_security_events WHERE user_id=?1 ORDER BY created_at DESC,id DESC LIMIT ?2",
        params![user, limit],
    )
}

/// The events of a category as an SQL condition.
fn category_condition(category: &str) -> &'static str {
    match category {
        "login" => "event IN ('login.mfa','login.password_or_oauth')",
        "challenge" => "event='login.challenge'",
        "reauth" => "event='security.reauthenticated'",
        "failure" => "event='login.failed'",
        "activity" => "event IN ('registration.submitted','device.created','device.deleted','share.created','share.revoked','grant.created','grant.revoked')",
        "credential" => "event NOT IN ('login.mfa','login.password_or_oauth','login.challenge','security.reauthenticated','login.failed','registration.submitted','device.created','device.deleted','share.created','share.revoked','grant.created','grant.revoked')",
        _ => "1",
    }
}

pub(super) async fn events(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let paging = query.paging();
    let category = query.kind.as_deref().unwrap_or("all");
    let user = query.user.clone().unwrap_or_default();
    let since = now_ms() - query.range(Range::Quarter).millis();
    let condition = format!(
        "{} AND (?1='' OR user_id=?1) AND created_at>=?2",
        category_condition(category)
    );
    let total = count(
        &conn,
        &format!("SELECT COUNT(*) FROM relay_security_events WHERE {condition}"),
        params![user, since],
    );
    let items = read_events(
        &conn,
        &format!(
            "SELECT id,user_id,event,resource_id,created_at FROM relay_security_events
             WHERE {condition} ORDER BY created_at DESC,id DESC LIMIT ?3 OFFSET ?4"
        ),
        params![user, since, paging.size as i64, paging.offset() as i64],
    );
    Json(Value::Object(paging.wrap(items, total as usize))).into_response()
}

/// Logins, second factors, sessions and per-day event counts for the range.
pub(super) async fn summary(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let range = query.range(Range::Week);
    let now = now_ms();
    let since = now - range.millis();
    let (logins, mfa_logins) = conn
        .query_row(
            "SELECT COALESCE(SUM(event IN ('login.mfa','login.password_or_oauth')),0),COALESCE(SUM(event='login.mfa'),0)
             FROM relay_security_events WHERE created_at>=?1",
            [since],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap_or((0, 0));
    let (sessions, trusted) = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM relay_auth_sessions WHERE expires_at>?1),
                    (SELECT COUNT(*) FROM relay_trusted_browsers WHERE expires_at>?1)",
            [now],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap_or((0, 0));
    let first_day_ms = now - (range.days() - 1) * DAY_MS;
    let first_day = day_of(first_day_ms);
    let start = ms_from_rfc3339(&format!("{first_day}T00:00:00Z")).unwrap_or(first_day_ms);
    let per_day: Vec<(String, String, i64)> = conn
        .prepare(
            "SELECT strftime('%Y-%m-%d',created_at/1000,'unixepoch'),event,COUNT(*)
             FROM relay_security_events WHERE created_at>=?1 GROUP BY 1,2",
        )
        .and_then(|mut stmt| {
            stmt.query_map([start], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect()
        })
        .unwrap_or_default();
    let daily: Vec<Value> = (0..range.days())
        .map(|offset| {
            let day = day_of(first_day_ms + offset * DAY_MS);
            let mut categories: HashMap<&str, i64> = HashMap::new();
            for (row_day, event, count) in &per_day {
                if *row_day == day {
                    *categories.entry(event_category(event)).or_insert(0) += count;
                }
            }
            json!({ "day": day, "categories": categories })
        })
        .collect();
    let environments: Vec<Value> = conn
        .prepare(
            "SELECT CASE WHEN environment='' THEN NULL ELSE environment END,COUNT(*)
             FROM relay_auth_sessions WHERE expires_at>?1 GROUP BY 1 ORDER BY 2 DESC",
        )
        .and_then(|mut stmt| {
            stmt.query_map([now], |row| {
                Ok(json!({ "label": row.get::<_, Option<String>>(0)?, "count": row.get::<_, i64>(1)? }))
            })?
            .collect()
        })
        .unwrap_or_default();
    let oldest = conn
        .query_row(
            "SELECT MIN(created_at),COUNT(*) FROM relay_security_events",
            [],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap_or((None, 0));
    let stats = super::stats::security(&conn, range);
    Json(json!({
        "loginFailures": stats["loginFailures"],
        "rateLimited": stats["rateLimited"],
        "retention": stats["retention"],
        "adminActions": stats["adminActions"],
        "range": range.key(),
        "logins": logins,
        "mfaLogins": mfa_logins,
        "activeSessions": sessions,
        "trustedBrowsers": trusted,
        "factors": super::users::login_methods(&conn),
        "daily": daily,
        "environments": environments,
        "history": {
            "oldestEventAt": oldest.0.map(rfc3339_from_ms),
            "events": oldest.1,
        },
    }))
    .into_response()
}

pub(super) async fn sessions(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let paging = query.paging();
    let now = now_ms();
    let pattern = query
        .search()
        .map(|search| like_pattern(&search))
        .unwrap_or_default();
    let condition = "s.expires_at>?1 AND (?2='' OR lower(u.username) LIKE ?2 ESCAPE '\\' OR lower(s.environment) LIKE ?2 ESCAPE '\\')";
    let total = count(
        &conn,
        &format!("SELECT COUNT(*) FROM relay_auth_sessions s JOIN relay_users u ON u.id=s.user_id WHERE {condition}"),
        params![now, pattern],
    );
    let items: Vec<Value> = conn
        .prepare(&format!(
            "SELECT s.token_hash,s.environment,s.created_at,s.expires_at,s.strong_auth_at,s.trusted_browser_id,s.account_mode,
                    u.id,u.username,u.role
             FROM relay_auth_sessions s JOIN relay_users u ON u.id=s.user_id
             WHERE {condition} ORDER BY s.created_at DESC LIMIT ?3 OFFSET ?4"
        ))
        .and_then(|mut stmt| {
            stmt.query_map(
                params![now, pattern, paging.size as i64, paging.offset() as i64],
                |row| {
                    let mut session = super::users::session_json(row)?;
                    session["userId"] = json!(row.get::<_, String>(7)?);
                    session["username"] = json!(row.get::<_, String>(8)?);
                    session["role"] = json!(row.get::<_, String>(9)?);
                    Ok(session)
                },
            )?
            .collect()
        })
        .unwrap_or_default();
    Json(Value::Object(paging.wrap(items, total as usize))).into_response()
}

/// Ends one session of a non-admin user.
pub(super) async fn end_session(
    Path(session_id): Path<String>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, admin) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let owner: Option<(String, String, String)> = conn
        .query_row(
            "SELECT u.username,u.role,u.id FROM relay_auth_sessions s JOIN relay_users u ON u.id=s.user_id WHERE s.token_hash=?1",
            [&session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .ok()
        .flatten();
    let Some((username, role, user_id)) = owner else {
        return not_found("Session not found");
    };
    if role == "admin" {
        return bad_request("End the admin's sessions from its own security settings");
    }
    match conn.execute(
        "DELETE FROM relay_auth_sessions WHERE token_hash=?1",
        [&session_id],
    ) {
        Ok(_) => {
            super::log_action(
                &conn,
                &admin,
                "session.ended",
                Some(("user", &user_id, &username)),
                Value::Null,
            );
            Json(json!({ "id": session_id })).into_response()
        }
        Err(_) => internal_error(),
    }
}
