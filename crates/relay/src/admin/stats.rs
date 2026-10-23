//! The recorded statistics in the console (metrics.rs): series for the
//! overview, tunnels and traffic of devices, failed sign-ins and rate limits,
//! push results, versions and storage growth, plus the admin action log and
//! the retention settings.
use super::common::*;
use super::*;
use crate::metrics::{self, Agg};

fn history(conn: &Connection) -> Option<i64> {
    metrics::history_start(conn)
}

/// Online devices over the range, tunnels and forwarding, versions, recent
/// admin actions and daily active people for the overview.
pub(super) fn overview(
    conn: &Connection,
    range: Range,
    online_ids: &[String],
) -> serde_json::Map<String, Value> {
    let now = now_ms();
    let start = now - range.millis();
    let step = metrics::step_for(range);
    let started = history(conn);
    let mut out = serde_json::Map::new();
    let online = metrics::series(conn, "online_devices", None, start, now, step, Agg::Mean);
    out.insert(
        "onlineDevices".into(),
        started.map_or(Value::Null, |_| {
            metrics::series_json(&online, step, started)
        }),
    );
    // Tunnels: reconnects, forwarding errors and p95 latency per hour (or wider).
    let tunnel_step = step.max(HOUR_MS);
    let connects = metrics::series(conn, "connects", None, start, now, tunnel_step, Agg::Sum);
    let requests = metrics::series(conn, "requests", None, start, now, tunnel_step, Agg::Sum);
    let failures = sum_series(
        ERROR_METRICS
            .iter()
            .map(|(_, name)| metrics::series(conn, name, None, start, now, tunnel_step, Agg::Sum))
            .collect(),
    );
    let error_rate = rate_series(&failures, &requests);
    let latency = metrics::latency_series(conn, start, now, tunnel_step);
    let total_requests = metrics::total(conn, "requests", start, now);
    let total_failed: f64 = ERROR_METRICS
        .iter()
        .map(|(_, name)| metrics::total(conn, name, start, now))
        .sum();
    let unstable = unstable_devices(conn, online_ids);
    out.insert(
        "tunnels".into(),
        if started.is_none() {
            Value::Null
        } else {
            json!({
                "reconnects": metrics::total(conn, "connects", start, now),
                "unstableDevices": unstable.len(),
                "requests": total_requests,
                "errorRate": (total_requests > 0.0).then(|| total_failed / total_requests),
                "p95Ms": metrics::latency_p95(conn, start, now),
                "latencyBoundsMs": metrics::LATENCY_BOUNDS,
                "reconnectSeries": metrics::series_json(&connects, tunnel_step, started),
                "errorSeries": metrics::series_json(&error_rate, tunnel_step, started),
                "latencySeries": metrics::series_json(&latency, tunnel_step, started),
            })
        },
    );
    out.insert(
        "versions".into(),
        Value::Array(version_counts(conn, online_ids)),
    );
    out.insert("adminActions".into(), Value::Array(recent_actions(conn, 5)));
    out.insert("unstableDevices".into(), json!(unstable.len()));
    out.insert("dailyActive".into(), Value::Array(daily_active(conn, 14)));
    out
}

/// The kinds of forwarding errors and their counters: the relay could not
/// reach the device (offline), it did not answer in time (timeout), too many
/// requests were waiting (busy), or it answered with a 5xx status (other).
pub(super) const ERROR_METRICS: [(&str, &str); 4] = [
    ("timeout", "forward_timeout"),
    ("busy", "forward_busy"),
    ("offline", "forward_offline"),
    ("other", "device_errors"),
];

/// A series: (bucket start in ms, value or None where unknown).
type Points = Vec<(i64, Option<f64>)>;

/// Adds series point by point; a bucket is unknown only when every part is.
fn sum_series(parts: Vec<Vec<(i64, Option<f64>)>>) -> Vec<(i64, Option<f64>)> {
    parts.into_iter().fold(Vec::new(), |sum, series| {
        if sum.is_empty() {
            return series;
        }
        sum.into_iter()
            .zip(series)
            .map(|((at, a), (_, b))| {
                (
                    at,
                    match (a, b) {
                        (None, None) => None,
                        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
                    },
                )
            })
            .collect()
    })
}

/// failed / requested per bucket (0 without requests, unknown when not sampled).
fn rate_series(
    failed: &[(i64, Option<f64>)],
    requested: &[(i64, Option<f64>)],
) -> Vec<(i64, Option<f64>)> {
    failed
        .iter()
        .zip(requested)
        .map(|((at, failed), (_, requested))| {
            (
                *at,
                match (failed, requested) {
                    (Some(failed), Some(requested)) if *requested > 0.0 => Some(failed / requested),
                    (Some(_), Some(_)) => Some(0.0),
                    _ => None,
                },
            )
        })
        .collect()
}

/// A performance window: a preset range or explicit `from`/`to` times.
pub(super) struct PerformanceWindow {
    pub(super) key: &'static str,
    pub(super) start: i64,
    pub(super) end: i64,
    pub(super) step: i64,
}

/// Picks a step that gives at most about 200 points: five-minute steps
/// within the two days minute rows are kept, whole hours (or days) beyond.
pub(super) fn performance_window(
    query: &ListQuery,
    now: i64,
) -> Result<PerformanceWindow, &'static str> {
    let (key, start, end) = match (query.from, query.to) {
        (Some(from), to) => {
            let end = to.unwrap_or(now).min(now);
            if from >= end {
                return Err("The window must end after it starts");
            }
            ("custom", from.max(end - 400 * DAY_MS), end)
        }
        (None, _) => {
            let range = query.range(Range::Day);
            (range.key(), now - range.millis(), now)
        }
    };
    let span = end - start;
    let within_minutes = now - start <= metrics::MINUTE_RETENTION_MS;
    let raw = (span / 200).max(MINUTE_MS);
    let step = if within_minutes && raw <= HOUR_MS {
        // 5, 10, 15 or 30 minutes.
        [5, 10, 15, 30, 60]
            .into_iter()
            .map(|minutes| minutes * MINUTE_MS)
            .find(|step| *step >= raw)
            .unwrap_or(HOUR_MS)
    } else {
        [1, 2, 3, 4, 6, 12, 24, 48, 72, 168]
            .into_iter()
            .map(|hours| hours * HOUR_MS)
            .find(|step| *step >= raw)
            .unwrap_or(7 * DAY_MS)
    };
    Ok(PerformanceWindow {
        key,
        start,
        end,
        step,
    })
}

/// Forwarding in depth for a window (docs/relay-admin.md "Performance"):
/// volume, errors by kind, latency percentiles and histogram, reconnects,
/// tunnel durations and the devices behind them. Counts, sizes and times only.
pub(super) fn performance(conn: &Connection, window: &PerformanceWindow) -> Value {
    let (start, end, step) = (window.start, window.end, window.step);
    let started = history(conn);
    let series =
        |metric: &str, agg: Agg| metrics::series(conn, metric, None, start, end, step, agg);
    let json_series = |points: &[(i64, Option<f64>)]| metrics::series_json(points, step, started);
    let requests = series("requests", Agg::Sum);
    let errors: Vec<(&str, Points)> = ERROR_METRICS
        .iter()
        .map(|(kind, metric)| (*kind, series(metric, Agg::Sum)))
        .collect();
    let failed = sum_series(errors.iter().map(|(_, points)| points.clone()).collect());
    let quantiles = metrics::latency_quantile_series(conn, start, end, step, &[0.5, 0.95, 0.99]);
    let total = |metric: &str| metrics::total(conn, metric, start, end);
    let total_requests = total("requests");
    let error_totals: serde_json::Map<String, Value> = ERROR_METRICS
        .iter()
        .map(|(kind, metric)| (kind.to_string(), json!(total(metric))))
        .collect();
    let total_failed: f64 = error_totals.values().filter_map(Value::as_f64).sum();
    let histogram = metrics::latency_histogram(conn, start, end);
    let counts: Vec<(String, f64)> = histogram
        .iter()
        .map(|(bound, count)| {
            (
                if bound.is_finite() {
                    bound.to_string()
                } else {
                    "inf".into()
                },
                *count,
            )
        })
        .collect();
    let mut lower = 0.0;
    let histogram_json: Vec<Value> = histogram
        .iter()
        .map(|(bound, count)| {
            let row = json!({
                "fromMs": lower,
                "toMs": bound.is_finite().then_some(*bound),
                "count": count,
            });
            if bound.is_finite() {
                lower = *bound;
            }
            row
        })
        .collect();
    let last_bound = *metrics::LATENCY_BOUNDS.last().unwrap_or(&30_000) as f64;
    let quantile = |q: f64| metrics::percentile(&counts, q);
    json!({
        "range": window.key,
        "start": rfc3339_from_ms(start),
        "end": rfc3339_from_ms(end),
        "resolutionSeconds": step / 1000,
        "historyStart": started.map(rfc3339_from_ms),
        "latency": {
            "boundsMs": metrics::LATENCY_BOUNDS,
            // A percentile at the last bound means "this slow or slower".
            "openAboveMs": last_bound,
            "method": "interpolated",
        },
        "totals": {
            "requests": total_requests,
            "errors": error_totals,
            "errorRate": (total_requests > 0.0).then(|| total_failed / total_requests),
            "requestBytes": total("request_bytes"),
            "responseBytes": total("response_bytes"),
            "reconnects": total("connects"),
            "wsFrames": total("ws_frames"),
            "wsBytes": total("ws_bytes"),
            "p50Ms": quantile(0.5),
            "p95Ms": quantile(0.95),
            "p99Ms": quantile(0.99),
        },
        "series": {
            "requests": json_series(&requests),
            "errors": errors.iter().map(|(kind, points)| (kind.to_string(), json_series(points))).collect::<serde_json::Map<_, _>>(),
            "errorRate": json_series(&rate_series(&failed, &requests)),
            "p50": json_series(&quantiles[0]),
            "p95": json_series(&quantiles[1]),
            "p99": json_series(&quantiles[2]),
            "reconnects": json_series(&series("connects", Agg::Sum)),
            "onlineDevices": json_series(&series("online_devices", Agg::Mean)),
            "requestBytes": json_series(&series("request_bytes", Agg::Sum)),
            "responseBytes": json_series(&series("response_bytes", Agg::Sum)),
        },
        "latencyHistogram": histogram_json,
        "connections": connection_stats(conn, start, end),
        "deviceTraffic": device_traffic(conn, start, end),
    })
}

/// Tunnel connections that overlap the window: how long they lasted, and the
/// devices that reconnected most.
fn connection_stats(conn: &Connection, start: i64, end: i64) -> Value {
    let kept_from = now_ms() - metrics::CONNECTION_RETENTION_DAYS * DAY_MS;
    let rows: Vec<(String, i64, Option<i64>)> = conn
        .prepare(
            "SELECT device_id,connected_at,ended_at FROM relay_device_connections
             WHERE connected_at<?2 AND (ended_at IS NULL OR ended_at>?1)",
        )
        .and_then(|mut stmt| {
            stmt.query_map(params![start, end], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect()
        })
        .unwrap_or_default();
    // Durations of the connections that ended in the window.
    const BUCKETS: [(&str, i64); 6] = [
        ("lt1m", MINUTE_MS),
        ("lt10m", 10 * MINUTE_MS),
        ("lt1h", HOUR_MS),
        ("lt6h", 6 * HOUR_MS),
        ("lt24h", DAY_MS),
        ("gte24h", i64::MAX),
    ];
    let mut durations = [0_i64; 6];
    let mut open = 0_i64;
    let mut per_device: HashMap<String, (i64, i64, i64)> = HashMap::new();
    for (device, connected, ended) in &rows {
        let entry = per_device.entry(device.clone()).or_default();
        if *connected >= start {
            entry.0 += 1;
        }
        match ended {
            Some(ended) if *ended <= end => {
                let length = ended - connected;
                if let Some(index) = BUCKETS.iter().position(|(_, limit)| length < *limit) {
                    durations[index] += 1;
                }
                if length < 10 * MINUTE_MS {
                    entry.1 += 1;
                }
                entry.2 = entry.2.max(*ended);
            }
            _ => {
                open += 1;
                entry.2 = entry.2.max(end);
            }
        }
    }
    let mut top: Vec<(String, (i64, i64, i64))> = per_device
        .into_iter()
        .filter(|(_, (connects, _, _))| *connects > 0)
        .collect();
    top.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then_with(|| a.0.cmp(&b.0)));
    top.truncate(15);
    let ids: Vec<String> = top.iter().map(|(id, _)| id.clone()).collect();
    let names = device_names(conn, &ids);
    let owners = device_owners(conn, &ids);
    json!({
        "historyStart": (kept_from > start).then(|| rfc3339_from_ms(kept_from)),
        "durations": BUCKETS.iter().zip(durations).map(|((key, _), count)| json!({ "key": key, "count": count })).collect::<Vec<_>>(),
        "open": open,
        "connects": rows.iter().filter(|(_, connected, _)| *connected >= start).count(),
        "topDevices": top.iter().filter(|(id, _)| names.contains_key(id)).map(|(id, (connects, short, _))| json!({
            "deviceId": id,
            "name": names.get(id),
            "ownerUsername": owners.get(id),
            "connects": connects,
            "shortConnections": short,
        })).collect::<Vec<_>>(),
    })
}

fn device_owners(conn: &Connection, ids: &[String]) -> HashMap<String, String> {
    let mut owners = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT u.username FROM relay_devices d JOIN relay_users u ON u.id=d.owner_user_id WHERE d.id=?1",
    ) {
        for id in ids {
            if let Ok(name) = stmt.query_row([id], |row| row.get::<_, String>(0)) {
                owners.insert(id.clone(), name);
            }
        }
    }
    owners
}

/// Forwarded requests, errors and bytes per device in the window (whole
/// hours), busiest first.
fn device_traffic(conn: &Connection, start: i64, end: i64) -> Value {
    let first_hour = start - start.rem_euclid(HOUR_MS);
    let rows: Vec<(String, String, String, i64, i64, i64, i64)> = conn
        .prepare(
            "SELECT t.device_id,d.name,u.username,SUM(t.requests),SUM(t.errors),SUM(t.request_bytes),SUM(t.response_bytes)
             FROM relay_device_traffic t JOIN relay_devices d ON d.id=t.device_id JOIN relay_users u ON u.id=d.owner_user_id
             WHERE t.hour>=?1 AND t.hour<?2 GROUP BY t.device_id ORDER BY 4 DESC LIMIT 100",
        )
        .and_then(|mut stmt| {
            stmt.query_map(params![first_hour, end], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })?
            .collect()
        })
        .unwrap_or_default();
    json!({
        "resolutionSeconds": 3600,
        "devices": rows.iter().map(|(id, name, owner, requests, errors, request_bytes, response_bytes)| json!({
            "deviceId": id,
            "name": name,
            "ownerUsername": owner,
            "requests": requests,
            "errors": errors,
            "requestBytes": request_bytes,
            "responseBytes": response_bytes,
        })).collect::<Vec<_>>(),
    })
}

pub(super) async fn performance_handler(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    crate::metrics::flush_connections(&state, &conn);
    let window = match performance_window(&query, now_ms()) {
        Ok(window) => window,
        Err(message) => return bad_request(message),
    };
    Json(performance(&conn, &window)).into_response()
}

/// Devices with five or more tunnel connections in the last hour.
pub(super) fn unstable_devices(conn: &Connection, _online: &[String]) -> Vec<String> {
    metrics::connects_since(conn, now_ms() - HOUR_MS)
        .into_iter()
        .filter(|(_, count)| *count >= metrics::UNSTABLE_CONNECTS)
        .map(|(device, _)| device)
        .collect()
}

/// Devices per reported version (all, and online now), newest version first.
pub(super) fn version_counts(conn: &Connection, online: &[String]) -> Vec<Value> {
    let mut counts: HashMap<Option<String>, (i64, i64)> = HashMap::new();
    let versions = metrics::versions(conn);
    for (device, (version, _)) in &versions {
        let entry = counts.entry(version.clone()).or_default();
        entry.0 += 1;
        if online.contains(device) {
            entry.1 += 1;
        }
    }
    // Online devices the relay has not read yet.
    let unknown = online
        .iter()
        .filter(|device| !versions.contains_key(*device))
        .count() as i64;
    if unknown > 0 {
        let entry = counts.entry(None).or_default();
        entry.0 += unknown;
        entry.1 += unknown;
    }
    let mut rows: Vec<(Option<String>, (i64, i64))> = counts.into_iter().collect();
    rows.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(a), Some(b)) => compare_versions(b, a),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    rows.into_iter()
        .map(|(version, (devices, online))| {
            let outdated = version.as_deref().is_some_and(|version| {
                compare_versions(version, RELAY_VERSION) == std::cmp::Ordering::Less
            });
            json!({ "version": version, "devices": devices, "online": online, "outdated": outdated })
        })
        .collect()
}

pub(super) fn daily_active(conn: &Connection, days: i64) -> Vec<Value> {
    let now = now_ms();
    let first = day_of(now - (days - 1) * DAY_MS);
    let rows: Vec<(String, String, i64)> = conn
        .prepare("SELECT day,kind,COUNT(*) FROM relay_daily_active WHERE day>=?1 GROUP BY day,kind")
        .and_then(|mut stmt| {
            stmt.query_map([&first], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect()
        })
        .unwrap_or_default();
    if rows.is_empty() {
        return Vec::new();
    }
    (0..days)
        .map(|offset| {
            let day = day_of(now - (days - 1 - offset) * DAY_MS);
            let count = |kind: &str| {
                rows.iter()
                    .find(|row| row.0 == day && row.1 == kind)
                    .map_or(0, |row| row.2)
            };
            json!({ "day": day, "users": count("user"), "devices": count("device") })
        })
        .collect()
}

pub(super) struct DeviceStats {
    pub(super) versions: HashMap<String, (Option<String>, i64)>,
    pub(super) last_seen: HashMap<String, i64>,
    pub(super) connects_hour: HashMap<String, i64>,
    pub(super) traffic: HashMap<String, Vec<i64>>,
    pub(super) recorded: bool,
}

pub(super) fn device_stats(conn: &Connection) -> DeviceStats {
    DeviceStats {
        versions: metrics::versions(conn),
        last_seen: metrics::last_seen(conn),
        connects_hour: metrics::connects_since(conn, now_ms() - HOUR_MS),
        traffic: metrics::daily_requests(conn, 7),
        recorded: history(conn).is_some(),
    }
}

/// The last 24 hours of one device: its tunnel connections and forwarded
/// requests per hour.
pub(super) fn device_detail(conn: &Connection, device: &str) -> (Value, Value) {
    let now = now_ms();
    let start = now - DAY_MS;
    let started = history(conn);
    if started.is_none() {
        return (Value::Null, Value::Null);
    }
    let spans = metrics::connection_spans(conn, device, start);
    let window_start = started.map_or(start, |begun| begun.max(start));
    let mut online = 0_i64;
    let mut longest_gap = 0_i64;
    let mut cursor = window_start;
    for span in &spans {
        let from = span.from.max(window_start);
        let to = span.to.unwrap_or(now).min(now);
        if to > from {
            online += to - from;
            longest_gap = longest_gap.max(from - cursor);
            cursor = cursor.max(to);
        }
    }
    longest_gap = longest_gap.max(now - cursor);
    let window = (now - window_start).max(1);
    let connections = json!({
        "since": rfc3339_from_ms(start),
        "historyStart": started.filter(|begun| *begun > start).map(rfc3339_from_ms),
        "spans": spans.iter().map(|span| json!({
            "from": rfc3339_from_ms(span.from),
            "to": span.to.map(rfc3339_from_ms),
            "reason": span.reason,
        })).collect::<Vec<_>>(),
        "uptime": online as f64 / window as f64,
        "reconnects": spans.iter().filter(|span| span.from >= start).count(),
        "longestOfflineSeconds": longest_gap.max(0) / 1000,
    });
    // The 24 hours ending with the current one.
    let first_hour = now - now.rem_euclid(HOUR_MS) - 23 * HOUR_MS;
    let rows: HashMap<i64, (i64, i64, i64, i64)> = conn
        .prepare(
            "SELECT hour,requests,request_bytes,response_bytes,errors FROM relay_device_traffic WHERE device_id=?1 AND hour>=?2",
        )
        .and_then(|mut stmt| {
            stmt.query_map(params![device, first_hour], |row| {
                Ok((row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
            })?
            .collect()
        })
        .unwrap_or_default();
    let hours: Vec<Value> = (0..24)
        .map(|index| {
            let hour = first_hour + index * HOUR_MS;
            let (requests, request_bytes, response_bytes, errors) = rows.get(&hour).copied().unwrap_or_default();
            json!({ "at": rfc3339_from_ms(hour), "requests": requests, "requestBytes": request_bytes, "responseBytes": response_bytes, "errors": errors })
        })
        .collect();
    let traffic = json!({
        "resolutionSeconds": 3600,
        "historyStart": started.filter(|begun| *begun > start).map(rfc3339_from_ms),
        "hours": hours,
    });
    (connections, traffic)
}

/// Failed sign-ins, refusals by kind and per day, retention, and admin
/// actions in the range for the security page.
pub(super) fn security(conn: &Connection, range: Range) -> serde_json::Map<String, Value> {
    let now = now_ms();
    let start = now - range.millis();
    let started = history(conn);
    let first_day = now - now.rem_euclid(DAY_MS) - (range.days() - 1) * DAY_MS;
    let per_day = |metric: &str| -> Vec<Value> {
        metrics::series(conn, metric, None, first_day, now + 1, DAY_MS, Agg::Sum)
            .into_iter()
            .map(|(at, value)| json!({ "day": day_of(at), "count": value.unwrap_or(0.0) }))
            .collect()
    };
    let history_start = started.map(rfc3339_from_ms);
    let mut out = serde_json::Map::new();
    out.insert(
        "loginFailures".into(),
        if started.is_none() {
            Value::Null
        } else {
            json!({
                "total": metrics::total(conn, "login_failed", start, now),
                "daily": per_day("login_failed"),
                "historyStart": history_start,
            })
        },
    );
    let by_kind: serde_json::Map<String, Value> =
        metrics::totals_by_label(conn, "rate_limited", start, now)
            .into_iter()
            .map(|(kind, count)| (kind, json!(count)))
            .collect();
    out.insert(
        "rateLimited".into(),
        if started.is_none() {
            Value::Null
        } else {
            json!({
                "total": by_kind.values().filter_map(Value::as_f64).sum::<f64>(),
                "byKind": by_kind,
                "daily": per_day("rate_limited"),
                "historyStart": history_start,
            })
        },
    );
    out.insert(
        "retention".into(),
        json!({ "securityDays": metrics::security_days(conn), "securityMaxEvents": metrics::SECURITY_MAX_EVENTS }),
    );
    out.insert(
        "adminActions".into(),
        json!(count(
            conn,
            "SELECT COUNT(*) FROM relay_admin_actions WHERE created_at>=?1",
            [start]
        )),
    );
    out
}

/// Push delivery results, version history and database growth for the system page.
pub(super) fn system(conn: &Connection, online: &[String]) -> serde_json::Map<String, Value> {
    let now = now_ms();
    let started = history(conn);
    let mut out = serde_json::Map::new();
    let day_ago = now - DAY_MS;
    out.insert(
        "delivery".into(),
        if started.is_none() {
            Value::Null
        } else {
            json!({
                "sent": metrics::total(conn, "push_sent", day_ago, now),
                "failed": metrics::total(conn, "push_failed", day_ago, now),
                "expired": metrics::total(conn, "push_expired", day_ago, now),
                "historyStart": started.filter(|begun| *begun > day_ago).map(rfc3339_from_ms),
            })
        },
    );
    let month = now - 30 * DAY_MS;
    let growth = metrics::series(conn, "db_bytes", None, month, now, DAY_MS, Agg::Mean);
    out.insert(
        "growth".into(),
        started.map_or(Value::Null, |_| {
            metrics::series_json(&growth, DAY_MS, started)
        }),
    );
    // Online devices per version per day (the day's largest count), 30 days.
    let first_day = now - now.rem_euclid(DAY_MS) - 29 * DAY_MS;
    let rows: Vec<(i64, String, f64)> = conn
        .prepare("SELECT bucket,label,peak FROM relay_metrics WHERE resolution='d' AND metric='online_by_version' AND bucket>=?1")
        .and_then(|mut stmt| stmt.query_map([first_day], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect())
        .unwrap_or_default();
    // Days the relay sampled at all; on the others the versions are unknown.
    let sampled: std::collections::HashSet<i64> = conn
        .prepare("SELECT DISTINCT bucket-(bucket%86400000) FROM relay_metrics WHERE resolution='h' AND metric='up' AND label='' AND bucket>=?1")
        .and_then(|mut stmt| stmt.query_map([first_day], |row| row.get(0))?.collect())
        .unwrap_or_default();
    let history_days: Vec<Value> = (0..30)
        .map(|offset| first_day + offset * DAY_MS)
        .filter(|day| started.is_some_and(|begun| *day + DAY_MS > begun))
        .map(|day| {
            if !sampled.contains(&day) {
                return json!({ "day": day_of(day), "versions": Value::Null });
            }
            let versions: serde_json::Map<String, Value> = rows
                .iter()
                .filter(|row| row.0 == day)
                .map(|row| (row.1.clone(), json!(row.2)))
                .collect();
            json!({ "day": day_of(day), "versions": versions })
        })
        .collect();
    out.insert(
        "versions".into(),
        if started.is_none() {
            Value::Null
        } else {
            json!({
                "current": version_counts(conn, online),
                "history": history_days,
                "historyStart": started.map(rfc3339_from_ms),
            })
        },
    );
    out.insert(
        "metrics".into(),
        json!({ "retentionDays": metrics::metrics_days(conn), "historyStart": started.map(rfc3339_from_ms) }),
    );
    out
}

fn action_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let detail: Option<String> = row.get(7)?;
    Ok(json!({
        "id": row.get::<_, i64>(0)?,
        "at": rfc3339_from_ms(row.get(1)?),
        "adminUsername": row.get::<_, String>(2)?,
        "action": row.get::<_, String>(3)?,
        "targetType": row.get::<_, Option<String>>(4)?,
        "targetId": row.get::<_, Option<String>>(5)?,
        "targetLabel": row.get::<_, Option<String>>(6)?,
        "detail": detail.and_then(|text| serde_json::from_str::<Value>(&text).ok()),
    }))
}

const ACTION_COLUMNS: &str =
    "id,created_at,admin_username,action,target_type,target_id,target_label,detail";

pub(super) fn recent_actions(conn: &Connection, limit: i64) -> Vec<Value> {
    conn.prepare(&format!(
        "SELECT {ACTION_COLUMNS} FROM relay_admin_actions ORDER BY id DESC LIMIT ?1"
    ))
    .and_then(|mut stmt| stmt.query_map([limit], action_json)?.collect())
    .unwrap_or_default()
}

pub(super) async fn actions(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let paging = query.paging();
    let kind = query.kind.clone().unwrap_or_default();
    let window = query.window();
    let (from, to) = (window.from.unwrap_or(0), window.to.unwrap_or(i64::MAX));
    let condition = "(?1='' OR action=?1) AND created_at>=?4 AND created_at<?5";
    let order = match query.sort_by(Dir::Desc) {
        Some(("action", dir)) => format!("action {}, id DESC", dir.sql()),
        Some(("admin", dir)) => format!("lower(admin_username) {}, id DESC", dir.sql()),
        Some(("target", dir)) => format!(
            "target_label IS NULL, lower(target_label) {}, id DESC",
            dir.sql()
        ),
        Some((_, dir)) => format!("id {}", dir.sql()),
        None => "id DESC".to_string(),
    };
    let total = count(
        &conn,
        &format!("SELECT COUNT(*) FROM relay_admin_actions WHERE {condition} AND ?2=?2 AND ?3=?3"),
        params![kind, 0, 0, from, to],
    );
    let items: Vec<Value> = conn
        .prepare(&format!(
            "SELECT {ACTION_COLUMNS} FROM relay_admin_actions WHERE {condition} ORDER BY {order} LIMIT ?2 OFFSET ?3"
        ))
        .and_then(|mut stmt| {
            stmt.query_map(params![kind, paging.size as i64, paging.offset() as i64, from, to], action_json)?
                .collect()
        })
        .unwrap_or_default();
    Json(Value::Object(paging.wrap(items, total as usize))).into_response()
}

fn retention_json(conn: &Connection) -> Value {
    json!({
        "metricsDays": metrics::metrics_days(conn),
        "securityDays": metrics::security_days(conn),
        "options": { "metricsDays": metrics::METRICS_DAYS, "securityDays": metrics::SECURITY_DAYS },
        "fixed": {
            "connectionDays": metrics::CONNECTION_RETENTION_DAYS,
            "adminActionDays": metrics::ADMIN_ACTION_RETENTION_DAYS,
            "securityMaxEvents": metrics::SECURITY_MAX_EVENTS,
        },
    })
}

pub(super) async fn retention(headers: HeaderMap, State(state): State<Arc<AppState>>) -> Response {
    let (conn, _) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    Json(retention_json(&conn)).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RetentionInput {
    metrics_days: Option<i64>,
    security_days: Option<i64>,
}

pub(super) async fn update_retention(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(body): Json<RetentionInput>,
) -> Response {
    let (conn, admin) = match admin_conn(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut changes = serde_json::Map::new();
    for (key, setting, allowed, value, before) in [
        (
            "metricsDays",
            "metricsRetentionDays",
            &metrics::METRICS_DAYS,
            body.metrics_days,
            metrics::metrics_days(&conn),
        ),
        (
            "securityDays",
            "securityRetentionDays",
            &metrics::SECURITY_DAYS,
            body.security_days,
            metrics::security_days(&conn),
        ),
    ] {
        let Some(value) = value else { continue };
        if !allowed.contains(&value) {
            return bad_request("Unsupported retention period");
        }
        if set_relay_setting(&conn, setting, &value.to_string()).is_err() {
            return internal_error();
        }
        if before != value {
            changes.insert(key.into(), json!([before, value]));
        }
    }
    if !changes.is_empty() {
        super::log_action(
            &conn,
            &admin,
            "settings.retention",
            None,
            Value::Object(changes),
        );
        // A shorter period takes effect now, not at the next hourly pass.
        if let Err(error) = metrics::prune(&conn) {
            tracing::warn!(%error, "could not apply the new retention");
        }
    }
    Json(retention_json(&conn)).into_response()
}
