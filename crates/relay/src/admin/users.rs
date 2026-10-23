//! Users: a paged, filtered list, one user's details, ending sessions, and
//! registration requests.
use super::common::*;
use super::*;

// One row per user with its security and device facts, computed in SQL so a
// page costs one query instead of several per user.
const USER_COLUMNS: &str = "u.id,u.username,u.email,u.role,u.enabled,u.created_at,u.last_seen_at,
    EXISTS(SELECT 1 FROM relay_account_vaults v WHERE v.user_id=u.id),
    COALESCE((SELECT a.oauth_only FROM relay_account_auth a WHERE a.user_id=u.id),0),
    EXISTS(SELECT 1 FROM relay_account_auth a WHERE a.user_id=u.id AND a.reset_pending=1 AND a.reset_consumed=0),
    EXISTS(SELECT 1 FROM relay_totp t WHERE t.user_id=u.id AND t.enabled=1),
    (SELECT COUNT(*) FROM relay_passkeys p WHERE p.user_id=u.id),
    (SELECT group_concat(i.provider) FROM relay_user_identities i WHERE i.user_id=u.id),
    (SELECT COUNT(*) FROM relay_devices d WHERE d.owner_user_id=u.id),
    (SELECT s.at FROM relay_user_sign_ins s WHERE s.user_id=u.id),
    (SELECT s.environment FROM relay_user_sign_ins s WHERE s.user_id=u.id)";

const HAS_KEY: &str = "EXISTS(SELECT 1 FROM relay_account_vaults v WHERE v.user_id=u.id)";
const HAS_FACTOR: &str = "(EXISTS(SELECT 1 FROM relay_totp t WHERE t.user_id=u.id AND t.enabled=1) OR EXISTS(SELECT 1 FROM relay_passkeys p WHERE p.user_id=u.id))";
const HAS_TOTP: &str = "EXISTS(SELECT 1 FROM relay_totp t WHERE t.user_id=u.id AND t.enabled=1)";
const HAS_PASSKEY: &str = "EXISTS(SELECT 1 FROM relay_passkeys p WHERE p.user_id=u.id)";
const OAUTH_ONLY: &str =
    "COALESCE((SELECT a.oauth_only FROM relay_account_auth a WHERE a.user_id=u.id),0)=1";
const RESET_PENDING: &str = "EXISTS(SELECT 1 FROM relay_account_auth a WHERE a.user_id=u.id AND a.reset_pending=1 AND a.reset_consumed=0)";
const SIGN_IN_AT: &str = "(SELECT s.at FROM relay_user_sign_ins s WHERE s.user_id=u.id)";

/// The SQL condition of a list filter. `?1` is the 7-day cutoff.
fn filter_condition(filter: &str) -> String {
    match filter {
        "active7d" => "u.last_seen_at>=?1".into(),
        "noAccountKey" => format!("u.role='user' AND NOT {HAS_KEY}"),
        "noTwoFactor" => format!("u.role='user' AND NOT {HAS_FACTOR}"),
        "disabled" => "u.enabled=0".into(),
        "neverSeen" => "u.last_seen_at IS NULL".into(),
        "admins" => "u.role='admin'".into(),
        _ => "1".into(),
    }
}

/// SQL conditions of the column filters. Values come from fixed lists, never
/// from the request text; times are bound as parameters ?6–?9.
fn column_conditions(query: &ListQuery) -> Vec<String> {
    let mut conditions = Vec::new();
    match ListQuery::pick(&query.role) {
        Some("admin") => conditions.push("u.role='admin'".to_string()),
        Some("user") => conditions.push("u.role='user'".to_string()),
        _ => {}
    }
    match ListQuery::pick(&query.status) {
        Some("active") => conditions.push(format!("u.enabled=1 AND NOT {RESET_PENDING}")),
        Some("disabled") => conditions.push("u.enabled=0".to_string()),
        Some("resetPending") => conditions.push(RESET_PENDING.to_string()),
        _ => {}
    }
    match ListQuery::pick(&query.key) {
        Some("set") => conditions.push(format!("u.role='user' AND {HAS_KEY}")),
        Some("missing") => conditions.push(format!("u.role='user' AND NOT {HAS_KEY}")),
        _ => {}
    }
    match ListQuery::pick(&query.factor) {
        Some("any") => conditions.push(HAS_FACTOR.to_string()),
        Some("none") => conditions.push(format!("NOT {HAS_FACTOR}")),
        Some("authenticator") => conditions.push(HAS_TOTP.to_string()),
        Some("passkey") => conditions.push(HAS_PASSKEY.to_string()),
        _ => {}
    }
    match ListQuery::pick(&query.method) {
        Some("password") => conditions.push(format!("NOT {OAUTH_ONLY}")),
        Some("oauthOnly") => conditions.push(OAUTH_ONLY.to_string()),
        Some(provider @ ("google" | "github")) => conditions.push(format!(
            "EXISTS(SELECT 1 FROM relay_user_identities i WHERE i.user_id=u.id AND i.provider='{provider}')"
        )),
        _ => {}
    }
    if ListQuery::pick(&query.seen) == Some("never") {
        conditions.push("u.last_seen_at IS NULL".to_string());
    } else {
        // Text times compare as text in the same RFC 3339 form.
        conditions
            .push("(?6='' OR u.last_seen_at>=?6) AND (?7='' OR u.last_seen_at<?7)".to_string());
    }
    conditions.push("(?8='' OR u.created_at>=?8) AND (?9='' OR u.created_at<?9)".to_string());
    conditions
}

pub(super) struct UserFacts {
    pub(super) id: String,
    pub(super) username: String,
    pub(super) email: String,
    pub(super) role: String,
    pub(super) enabled: bool,
    pub(super) created_at: String,
    pub(super) last_seen_at: Option<String>,
    pub(super) account_key: bool,
    pub(super) oauth_only: bool,
    pub(super) reset_pending: bool,
    pub(super) authenticator: bool,
    pub(super) passkeys: i64,
    pub(super) providers: Vec<String>,
    pub(super) devices: i64,
    pub(super) last_sign_in_at: Option<i64>,
    pub(super) last_sign_in_browser: Option<String>,
}

fn user_facts(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserFacts> {
    let providers: Option<String> = row.get(12)?;
    Ok(UserFacts {
        id: row.get(0)?,
        username: row.get(1)?,
        email: row.get(2)?,
        role: row.get(3)?,
        enabled: row.get::<_, i64>(4)? == 1,
        created_at: row.get(5)?,
        last_seen_at: row.get(6)?,
        account_key: row.get(7)?,
        oauth_only: row.get::<_, i64>(8)? == 1,
        reset_pending: row.get(9)?,
        authenticator: row.get(10)?,
        passkeys: row.get(11)?,
        providers: providers
            .map(|value| value.split(',').map(str::to_string).collect())
            .unwrap_or_default(),
        devices: row.get(13)?,
        last_sign_in_at: row.get(14)?,
        last_sign_in_browser: row
            .get::<_, Option<String>>(15)?
            .filter(|label| !label.is_empty()),
    })
}

impl UserFacts {
    pub(super) fn json(&self, devices_online: usize) -> Value {
        let admin = self.role == "admin";
        let mut methods: Vec<&str> = Vec::new();
        if !self.oauth_only {
            methods.push("password");
        }
        for provider in &self.providers {
            if !methods.contains(&provider.as_str()) {
                methods.push(provider);
            }
        }
        json!({
            "id": self.id,
            "username": self.username,
            "email": self.email,
            "role": self.role,
            "enabled": self.enabled,
            "createdAt": self.created_at,
            "lastSeenAt": self.last_seen_at,
            "loginMethods": methods,
            "oauthOnly": self.oauth_only,
            // The relay admin signs in with its configured password and has no
            // account key, so the question does not apply.
            "accountKey": if admin { Value::Null } else { json!(self.account_key) },
            "resetPending": self.reset_pending,
            "authenticator": self.authenticator,
            "passkeys": self.passkeys,
            "devices": self.devices,
            "devicesOnline": devices_online,
            // The latest sign-in and its browser label; the relay keeps no
            // addresses.
            "lastSignInAt": self.last_sign_in_at.map(rfc3339_from_ms),
            "lastSignInBrowser": self.last_sign_in_browser,
        })
    }
}

fn online_devices_by_owner(conn: &Connection, live: &Live) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT id,owner_user_id FROM relay_devices") {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) {
            for (device, owner) in rows.flatten() {
                if live.tunnels.contains_key(&device) {
                    *counts.entry(owner).or_insert(0) += 1;
                }
            }
        }
    }
    counts
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
    let paging = query.paging();
    let week = rfc3339_ago(7 * DAY_MS);
    let pattern = query.search().map(|search| like_pattern(&search));
    // Every statement names ?1–?3, so the same parameters bind whatever the
    // filter; an empty pattern matches every user.
    // Every statement names ?1–?9, so the same parameters bind whatever the
    // filters; empty text leaves a condition open.
    let condition = format!(
        "({}) AND ?1=?1 AND ?4=?4 AND ?5=?5 AND (?2='' OR lower(u.username) LIKE ?2 ESCAPE '\\' OR lower(u.email) LIKE ?2 ESCAPE '\\' OR u.id=?3) AND {}",
        filter_condition(query.filter()),
        column_conditions(&query).join(" AND ")
    );
    let order = match query.sort_by(Dir::Asc) {
        Some((key, dir)) => {
            let d = dir.sql();
            match key {
                "name" => format!("u.username {d}"),
                "email" => format!("u.email {d}, u.username ASC"),
                "role" => format!("u.role {d}, u.username ASC"),
                "status" => format!("u.enabled {d}, {RESET_PENDING} {d}, u.username ASC"),
                "created" => format!("u.created_at {d}, u.username ASC"),
                "lastSeen" => format!("u.last_seen_at IS NULL, u.last_seen_at {d}, u.username ASC"),
                "lastSignIn" => format!("{SIGN_IN_AT} IS NULL, {SIGN_IN_AT} {d}, u.username ASC"),
                "browser" => format!(
                    "COALESCE((SELECT s.environment FROM relay_user_sign_ins s WHERE s.user_id=u.id),'')='', (SELECT s.environment FROM relay_user_sign_ins s WHERE s.user_id=u.id) {d}, u.username ASC"
                ),
                "devices" => format!(
                    "(SELECT COUNT(*) FROM relay_devices d WHERE d.owner_user_id=u.id) {d}, u.username ASC"
                ),
                "key" => format!("u.role='admin', {HAS_KEY} {d}, u.username ASC"),
                "factors" => format!(
                    "{HAS_TOTP} + (SELECT COUNT(*) FROM relay_passkeys p WHERE p.user_id=u.id) {d}, u.username ASC"
                ),
                "method" => format!("{OAUTH_ONLY} {d}, u.username ASC"),
                _ => "u.last_seen_at IS NULL, u.last_seen_at DESC, u.username ASC".to_string(),
            }
        }
        None => "u.last_seen_at IS NULL, u.last_seen_at DESC, u.username ASC".to_string(),
    };
    let raw = query.q.clone().unwrap_or_default();
    let (seen_from, seen_to) = query.seen_window().text();
    let (created_from, created_to) = query.created_window().text();
    let search_params = (week.as_str(), pattern.as_deref().unwrap_or(""), raw.trim());
    let bind = |limit: i64, offset: i64| {
        [
            search_params.0.to_string(),
            search_params.1.to_string(),
            search_params.2.to_string(),
            limit.to_string(),
            offset.to_string(),
            seen_from.clone(),
            seen_to.clone(),
            created_from.clone(),
            created_to.clone(),
        ]
    };
    let total = count(
        &conn,
        &format!("SELECT COUNT(*) FROM relay_users u WHERE {condition}"),
        rusqlite::params_from_iter(bind(0, 0)),
    );
    let users: Vec<UserFacts> = conn
        .prepare(&format!(
            "SELECT {USER_COLUMNS} FROM relay_users u WHERE {condition} ORDER BY {order} LIMIT CAST(?4 AS INTEGER) OFFSET CAST(?5 AS INTEGER)"
        ))
        .and_then(|mut stmt| {
            stmt.query_map(
                rusqlite::params_from_iter(bind(paging.size as i64, paging.offset() as i64)),
                user_facts,
            )?
            .collect()
        })
        .unwrap_or_default();
    let online = online_devices_by_owner(&conn, &live);
    let items = users
        .iter()
        .map(|user| user.json(online.get(&user.id).copied().unwrap_or(0)))
        .collect();
    let counts = conn
        .query_row(
            &format!(
                "SELECT COUNT(*),
                  COALESCE(SUM(u.last_seen_at>=?1),0),
                  COALESCE(SUM(u.role='user' AND NOT {HAS_KEY}),0),
                  COALESCE(SUM(u.role='user' AND NOT {HAS_FACTOR}),0),
                  COALESCE(SUM(u.enabled=0),0),
                  COALESCE(SUM(u.last_seen_at IS NULL),0)
                 FROM relay_users u"
            ),
            [week.as_str()],
            |row| {
                Ok(json!({
                    "all": row.get::<_, i64>(0)?,
                    "active7d": row.get::<_, i64>(1)?,
                    "noAccountKey": row.get::<_, i64>(2)?,
                    "noTwoFactor": row.get::<_, i64>(3)?,
                    "disabled": row.get::<_, i64>(4)?,
                    "neverSeen": row.get::<_, i64>(5)?,
                }))
            },
        )
        .unwrap_or(Value::Null);
    let mut body = paging.wrap(items, total as usize);
    body.insert("counts".into(), counts);
    body.insert("methods".into(), login_methods(&conn));
    body.insert(
        "pendingRegistrations".into(),
        json!(count(
            &conn,
            "SELECT COUNT(*) FROM relay_pending_registrations WHERE status='pending'",
            []
        )),
    );
    Json(Value::Object(body)).into_response()
}

/// How non-admin users sign in and which second factors they have.
pub(super) fn login_methods(conn: &Connection) -> Value {
    conn.query_row(
        &format!(
            "SELECT COUNT(*),
              COALESCE(SUM(COALESCE((SELECT a.oauth_only FROM relay_account_auth a WHERE a.user_id=u.id),0)=0),0),
              COALESCE(SUM(EXISTS(SELECT 1 FROM relay_user_identities i WHERE i.user_id=u.id AND i.provider='google')),0),
              COALESCE(SUM(EXISTS(SELECT 1 FROM relay_user_identities i WHERE i.user_id=u.id AND i.provider='github')),0),
              COALESCE(SUM(COALESCE((SELECT a.oauth_only FROM relay_account_auth a WHERE a.user_id=u.id),0)=1),0),
              COALESCE(SUM(EXISTS(SELECT 1 FROM relay_totp t WHERE t.user_id=u.id AND t.enabled=1)),0),
              COALESCE(SUM(EXISTS(SELECT 1 FROM relay_passkeys p WHERE p.user_id=u.id)),0),
              COALESCE(SUM({HAS_FACTOR}),0),
              COALESCE(SUM({HAS_KEY}),0)
             FROM relay_users u WHERE u.role='user'"
        ),
        [],
        |row| {
            Ok(json!({
                "users": row.get::<_, i64>(0)?,
                "password": row.get::<_, i64>(1)?,
                "google": row.get::<_, i64>(2)?,
                "github": row.get::<_, i64>(3)?,
                "oauthOnly": row.get::<_, i64>(4)?,
                "authenticator": row.get::<_, i64>(5)?,
                "passkey": row.get::<_, i64>(6)?,
                "twoFactor": row.get::<_, i64>(7)?,
                "accountKey": row.get::<_, i64>(8)?,
            }))
        },
    )
    .unwrap_or(Value::Null)
}

pub(super) async fn detail(
    Path(user_id): Path<String>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let live = live(&state).await;
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(user) = conn
        .query_row(
            &format!("SELECT {USER_COLUMNS} FROM relay_users u WHERE u.id=?1"),
            [&user_id],
            user_facts,
        )
        .optional()
        .ok()
        .flatten()
    else {
        return not_found("User not found");
    };
    let devices = super::devices::rows(&conn, &live, Some(&user.id));
    let online = devices.iter().filter(|device| device.connected()).count();
    let now = now_ms();
    let sessions: Vec<Value> = conn
        .prepare(
            "SELECT token_hash,environment,created_at,expires_at,strong_auth_at,trusted_browser_id,account_mode
             FROM relay_auth_sessions WHERE user_id=?1 AND expires_at>?2 ORDER BY created_at DESC LIMIT 50",
        )
        .and_then(|mut stmt| {
            stmt.query_map(params![user.id, now], session_json)?
                .collect()
        })
        .unwrap_or_default();
    let events = super::audit::recent_for_user(&conn, &user.id, 12);
    let outgoing = count(
        &conn,
        "SELECT (SELECT COUNT(*) FROM relay_shares WHERE owner_user_id=?1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?2))
              + (SELECT COUNT(*) FROM relay_access_grants WHERE owner_user_id=?1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?2))",
        params![user.id, now_rfc3339()],
    );
    let incoming = count(
        &conn,
        "SELECT (SELECT COUNT(*) FROM relay_shares WHERE target_user_id=?1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?2))
              + (SELECT COUNT(*) FROM relay_access_grants WHERE target_user_id=?1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?2))",
        params![user.id, now_rfc3339()],
    );
    let browsers = count(
        &conn,
        "SELECT COUNT(*) FROM relay_account_browsers WHERE user_id=?1",
        [&user.id],
    );
    let trusted = count(
        &conn,
        "SELECT COUNT(*) FROM relay_trusted_browsers WHERE user_id=?1 AND expires_at>?2",
        params![user.id, now],
    );
    let registration = conn
        .query_row(
            "SELECT r.provider,r.created_at,r.reviewed_at,(SELECT username FROM relay_users WHERE id=r.reviewed_by_user_id)
             FROM relay_pending_registrations r
             WHERE r.status='approved' AND (r.username=?1 OR r.email=?2) ORDER BY r.reviewed_at DESC LIMIT 1",
            params![user.username, user.email],
            |row| {
                Ok(json!({
                    "provider": row.get::<_, String>(0)?,
                    "requestedAt": row.get::<_, String>(1)?,
                    "approvedAt": row.get::<_, Option<String>>(2)?,
                    "approvedBy": row.get::<_, Option<String>>(3)?,
                }))
            },
        )
        .optional()
        .ok()
        .flatten();
    Json(json!({
        "user": user.json(online),
        "devices": devices.iter().map(|device| device.json()).collect::<Vec<_>>(),
        "sessions": sessions,
        "shares": { "outgoing": outgoing, "incoming": incoming },
        "accountBrowsers": browsers,
        "trustedBrowsers": trusted,
        "events": events,
        "registration": registration,
    }))
    .into_response()
}

pub(super) fn session_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let environment: String = row.get(1)?;
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "environment": if environment.is_empty() { Value::Null } else { json!(environment) },
        "createdAt": rfc3339_from_ms(row.get(2)?),
        "expiresAt": rfc3339_from_ms(row.get(3)?),
        "strongAuthAt": row.get::<_, Option<i64>>(4)?.map(rfc3339_from_ms),
        "trustedBrowser": row.get::<_, Option<String>>(5)?.is_some(),
        "accountMode": row.get::<_, String>(6)?,
    }))
}

/// Ends every session of a user: they sign in again on each browser. Trusted
/// browsers and account keys are unaffected.
pub(super) async fn end_sessions(
    Path(user_id): Path<String>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, admin) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(user) = load_user_by_id(&conn, &user_id) else {
        return not_found("User not found");
    };
    if user.role == "admin" {
        return bad_request("End the admin's sessions from its own security settings");
    }
    let ended = match conn.execute(
        "DELETE FROM relay_auth_sessions WHERE user_id=?1",
        [&user.id],
    ) {
        Ok(ended) => ended,
        Err(_) => return internal_error(),
    };
    super::log_action(
        &conn,
        &admin,
        "user.sessions_ended",
        Some(("user", &user.id, &user.username)),
        json!({ "sessions": ended }),
    );
    Json(json!({ "id": user.id, "ended": ended })).into_response()
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct RegistrationQuery {
    status: Option<String>,
    page: Option<usize>,
    page_size: Option<usize>,
    sort: Option<String>,
    dir: Option<String>,
}

/// Registration requests: the pending ones, or the reviewed history.
pub(super) async fn registrations(
    headers: HeaderMap,
    Query(query): Query<RegistrationQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let paging = ListQuery {
        page: query.page,
        page_size: query.page_size,
        ..Default::default()
    }
    .paging();
    let pending = query.status.as_deref() != Some("reviewed");
    let condition = if pending {
        "r.status='pending'"
    } else {
        "r.status IN ('approved','rejected')"
    };
    let order = match (pending, query.sort.as_deref(), query.dir.as_deref()) {
        (true, _, _) => "r.created_at ASC",
        (false, Some("requested"), Some("asc")) => "r.created_at ASC",
        (false, Some("requested"), _) => "r.created_at DESC",
        (false, Some("user"), Some("desc")) => "lower(r.username) DESC",
        (false, Some("user"), _) => "lower(r.username) ASC",
        (false, Some("result"), Some("desc")) => "r.status DESC, r.reviewed_at DESC",
        (false, Some("result"), _) => "r.status ASC, r.reviewed_at DESC",
        (false, _, Some("asc")) => "r.reviewed_at ASC",
        (false, _, _) => "r.reviewed_at DESC",
    };
    let total = count(
        &conn,
        &format!("SELECT COUNT(*) FROM relay_pending_registrations r WHERE {condition}"),
        [],
    );
    let items: Vec<Value> = conn
        .prepare(&format!(
            "SELECT r.id,r.username,r.email,r.provider,r.created_at,r.status,r.reviewed_at,
                    (SELECT username FROM relay_users WHERE id=r.reviewed_by_user_id)
             FROM relay_pending_registrations r WHERE {condition} ORDER BY {order} LIMIT ?1 OFFSET ?2"
        ))
        .and_then(|mut stmt| {
            stmt.query_map(params![paging.size as i64, paging.offset() as i64], |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "username": row.get::<_, String>(1)?,
                    "email": row.get::<_, String>(2)?,
                    "provider": row.get::<_, String>(3)?,
                    "createdAt": row.get::<_, String>(4)?,
                    "status": row.get::<_, String>(5)?,
                    "reviewedAt": row.get::<_, Option<String>>(6)?,
                    "reviewedBy": row.get::<_, Option<String>>(7)?,
                }))
            })?
            .collect()
        })
        .unwrap_or_default();
    let mut body = paging.wrap(items, total as usize);
    body.insert("summary".into(), registration_summary(&conn));
    Json(Value::Object(body)).into_response()
}

/// Pending count and age, decisions in the last 30 days and the median time
/// from request to approval over the last 90 days.
pub(super) fn registration_summary(conn: &Connection) -> Value {
    let month = rfc3339_ago(30 * DAY_MS);
    let quarter = rfc3339_ago(90 * DAY_MS);
    let (pending, oldest, approved, rejected) = conn
        .query_row(
            "SELECT COALESCE(SUM(status='pending'),0),
                    MIN(CASE WHEN status='pending' THEN created_at END),
                    COALESCE(SUM(status='approved' AND reviewed_at>=?1),0),
                    COALESCE(SUM(status='rejected' AND reviewed_at>=?1),0)
             FROM relay_pending_registrations",
            [&month],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .unwrap_or((0, None, 0, 0));
    let mut waits: Vec<i64> = conn
        .prepare(
            "SELECT created_at,reviewed_at FROM relay_pending_registrations
             WHERE status='approved' AND reviewed_at>=?1",
        )
        .and_then(|mut stmt| {
            stmt.query_map([&quarter], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(created, reviewed)| {
            Some(ms_from_rfc3339(&reviewed)? - ms_from_rfc3339(&created)?)
        })
        .filter(|wait| *wait >= 0)
        .collect();
    waits.sort_unstable();
    let median_hours = (!waits.is_empty()).then(|| waits[waits.len() / 2] as f64 / HOUR_MS as f64);
    json!({
        "pending": pending,
        "oldestPendingAt": oldest,
        "approved30d": approved,
        "rejected30d": rejected,
        "medianApprovalHours": median_hours,
    })
}
