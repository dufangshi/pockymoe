//! The overview: what needs attention, key figures, recent activity and
//! sign-ups, and the latest security events.
use super::common::*;
use super::*;

pub(super) async fn overview(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let live = live(&state).await;
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let range = query.range(Range::Day);
    let now = now_ms();
    let range_start = rfc3339_from_ms(now - range.millis());
    let hour = rfc3339_ago(HOUR_MS);
    let day = rfc3339_ago(DAY_MS);
    let week = rfc3339_ago(7 * DAY_MS);
    let month = rfc3339_ago(30 * DAY_MS);

    // People: admins sign in differently and are left out of these figures.
    let users = conn
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(enabled=1),0),
                    COALESCE(SUM(last_seen_at>=?1),0),COALESCE(SUM(last_seen_at>=?2),0),
                    COALESCE(SUM(last_seen_at>=?3),0),COALESCE(SUM(last_seen_at>=?4),0),
                    COALESCE(SUM(last_seen_at IS NULL),0),COALESCE(SUM(created_at>=?5),0),
                    COALESCE(SUM(EXISTS(SELECT 1 FROM relay_account_vaults v WHERE v.user_id=relay_users.id)),0),
                    COALESCE(SUM(EXISTS(SELECT 1 FROM relay_totp t WHERE t.user_id=relay_users.id AND t.enabled=1)
                              OR EXISTS(SELECT 1 FROM relay_passkeys p WHERE p.user_id=relay_users.id)),0),
                    COALESCE(SUM(EXISTS(SELECT 1 FROM relay_account_auth a WHERE a.user_id=relay_users.id AND a.reset_pending=1 AND a.reset_consumed=0)),0)
             FROM relay_users WHERE role='user'",
            params![hour, day, week, month, range_start],
            |row| {
                let values: Vec<i64> = (0..11).map(|i| row.get(i)).collect::<rusqlite::Result<_>>()?;
                Ok(values)
            },
        )
        .unwrap_or_else(|_| vec![0; 11]);
    let [total, enabled, active_hour, active_day, active_week, active_month, never, new_in_range, with_key, two_factor, reset_pending] =
        <[i64; 11]>::try_from(users).unwrap_or([0; 11]);
    // Exclusive buckets: each person in the most recent one that applies.
    let activity = json!([
        { "key": "1h", "count": active_hour },
        { "key": "24h", "count": active_day - active_hour },
        { "key": "7d", "count": active_week - active_day },
        { "key": "30d", "count": active_month - active_week },
        { "key": "older", "count": total - active_month - never },
        { "key": "never", "count": never },
    ]);

    let (devices_total, devices_protected, devices_new_week, devices_new_range) = conn
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(EXISTS(SELECT 1 FROM relay_device_certificates c WHERE c.device_id=d.id)),0),
                    COALESCE(SUM(d.created_at>=?1),0),COALESCE(SUM(d.created_at>=?2),0)
             FROM relay_devices d",
            params![week, range_start],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)),
        )
        .unwrap_or((0, 0, 0, 0));
    let online = conn
        .prepare("SELECT id FROM relay_devices")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
        .iter()
        .filter(|id| live.tunnels.contains_key(*id))
        .count();

    let now_text = now_rfc3339();
    let soon = rfc3339_from_ms(now + 7 * DAY_MS);
    let shares = conn
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM relay_shares WHERE revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?1))
             + (SELECT COUNT(*) FROM relay_access_grants WHERE revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?1)),
               (SELECT COUNT(*) FROM relay_shares WHERE created_at>=?2) + (SELECT COUNT(*) FROM relay_access_grants WHERE created_at>=?2),
               (SELECT COUNT(*) FROM relay_shares WHERE revoked_at>=?2) + (SELECT COUNT(*) FROM relay_access_grants WHERE revoked_at>=?2),
               (SELECT COUNT(*) FROM relay_shares WHERE revoked_at IS NULL AND expires_at>?1 AND expires_at<=?3)
             + (SELECT COUNT(*) FROM relay_access_grants WHERE revoked_at IS NULL AND expires_at>?1 AND expires_at<=?3)",
            params![now_text, week, soon],
            |row| {
                Ok(json!({
                    "active": row.get::<_, i64>(0)?,
                    "createdThisWeek": row.get::<_, i64>(1)?,
                    "revokedThisWeek": row.get::<_, i64>(2)?,
                    "expiringSoon": row.get::<_, i64>(3)?,
                }))
            },
        )
        .unwrap_or(Value::Null);

    let registrations = super::users::registration_summary(&conn);
    let weekly = weekly_signups(&conn, 12);
    let links = super::shares::public_link_summary(&conn);
    let legacy_links = links["legacy"].as_i64().unwrap_or(0);
    let events = super::audit::recent(&conn, 6);
    super::devices::super_flush(&state, &conn);
    let online_ids: Vec<String> = live.tunnels.keys().cloned().collect();
    let stats = super::stats::overview(&conn, range, &online_ids);
    drop(conn);

    Json(json!({
        "generatedAt": now_text,
        "range": range.key(),
        "rangeStart": range_start,
        "relay": {
            "version": RELAY_VERSION,
            "startedAt": state.admin.started_at,
            "publicBaseUrl": state.oauth.public_base_url,
        },
        "attention": {
            "pendingRegistrations": registrations["pending"],
            "oldestPendingAt": registrations["oldestPendingAt"],
            "usersWithoutAccountKey": total - with_key,
            "legacyPublicLinks": legacy_links,
            // Known only once the relay records tunnel connections.
            "unstableDevices": if stats["tunnels"].is_null() { Value::Null } else { stats["unstableDevices"].clone() },
        },
        "users": {
            "total": total,
            "enabled": enabled,
            "disabled": total - enabled,
            "active1h": active_hour,
            "active24h": active_day,
            "active7d": active_week,
            "active30d": active_month,
            "neverSeen": never,
            "newInRange": new_in_range,
            "withAccountKey": with_key,
            "twoFactor": two_factor,
            "resetPending": reset_pending,
            "dailyActive": stats["dailyActive"],
        },
        "activity": activity,
        "devices": {
            "total": devices_total,
            "online": online,
            "protected": devices_protected,
            "unprotected": devices_total - devices_protected,
            "newThisWeek": devices_new_week,
            "newInRange": devices_new_range,
        },
        "shares": shares,
        "registrations": registrations,
        "signupsWeekly": weekly,
        "publicLinks": links,
        "securityEvents": events,
        "onlineDevices": stats["onlineDevices"],
        "versions": stats["versions"],
        "tunnels": stats["tunnels"],
        "adminActions": stats["adminActions"],
    }))
    .into_response()
}

/// Accounts created in each of the last `weeks` seven-day spans, oldest first.
fn weekly_signups(conn: &Connection, weeks: i64) -> Value {
    let now = now_ms();
    let start = now - weeks * 7 * DAY_MS;
    let created: Vec<String> = conn
        .prepare("SELECT created_at FROM relay_users WHERE role='user' AND created_at>=?1")
        .and_then(|mut stmt| {
            stmt.query_map([rfc3339_from_ms(start)], |row| row.get(0))?
                .collect()
        })
        .unwrap_or_default();
    let mut counts = vec![0_i64; weeks as usize];
    for at in created.iter().filter_map(|value| ms_from_rfc3339(value)) {
        let index = ((at - start) / (7 * DAY_MS)).clamp(0, weeks - 1) as usize;
        counts[index] += 1;
    }
    Value::Array(
        counts
            .into_iter()
            .enumerate()
            .map(|(index, count)| {
                json!({ "weekStart": day_of(start + index as i64 * 7 * DAY_MS), "count": count })
            })
            .collect(),
    )
}
