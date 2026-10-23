//! The recorded statistics in the console (metrics.rs): series for the
//! overview, tunnels and traffic of devices, failed sign-ins and rate limits,
//! push results, versions and storage growth, plus the admin action log and
//! the retention settings.
//!
//! Every chart reads a window (a preset range or a custom one) at a step of at
//! most about 200 points, from minute, hour or day rows as the step allows;
//! per-device lists are grouped and paged in SQL.
use super::common::*;
use super::*;
use crate::metrics::{self, Agg};
use std::collections::HashSet;

fn history(conn: &Connection) -> Option<i64> {
    metrics::history_start(conn)
}

/// Online devices over the window, tunnels and forwarding, versions, recent
/// admin actions and daily active people for the overview.
pub(super) fn overview(
    state: &AppState,
    conn: &Connection,
    window: &ChartWindow,
    online_ids: &[String],
) -> serde_json::Map<String, Value> {
    let (start, end, step) = (window.start, window.end, window.step);
    let started = history(conn);
    let mut out = serde_json::Map::new();
    let online = metrics::series(conn, "online_devices", None, start, end, step, Agg::Mean);
    out.insert(
        "onlineDevices".into(),
        started.map_or(Value::Null, |_| {
            metrics::series_json(&online, step, started)
        }),
    );
    // Tunnels: reconnects, forwarding errors and p95 latency per hour (or wider).
    let tunnel_step = step.max(HOUR_MS);
    let connects = metrics::series(conn, "connects", None, start, end, tunnel_step, Agg::Sum);
    let requests = metrics::series(conn, "requests", None, start, end, tunnel_step, Agg::Sum);
    let failures = sum_series(
        ERROR_METRICS
            .iter()
            .map(|(_, name)| metrics::series(conn, name, None, start, end, tunnel_step, Agg::Sum))
            .collect(),
    );
    let error_rate = rate_series(&failures, &requests);
    let latency = metrics::latency_series(conn, start, end, tunnel_step);
    let total_requests = metrics::total(conn, "requests", start, end);
    let total_failed: f64 = ERROR_METRICS
        .iter()
        .map(|(_, name)| metrics::total(conn, name, start, end))
        .sum();
    let unstable = unstable_devices(conn, online_ids);
    out.insert(
        "tunnels".into(),
        if started.is_none() {
            Value::Null
        } else {
            json!({
                "reconnects": metrics::total(conn, "connects", start, end),
                "unstableDevices": unstable.len(),
                "requests": total_requests,
                "errorRate": (total_requests > 0.0).then(|| total_failed / total_requests),
                "p95Ms": metrics::latency_p95(conn, start, end),
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
    // Daily active people: the window's days (at least the last two weeks
    // for the figure's trend line).
    let days = window.days().max(14);
    out.insert(
        "dailyActive".into(),
        Value::Array(daily_active(state, conn, end, days)),
    );
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

/// Rows of the per-device lists in the performance response itself; the rest
/// page through `/relay/admin/performance/devices`.
const FIRST_DEVICES: usize = 10;

/// Forwarding in depth for a window (docs/relay-admin.md "Performance"):
/// volume, errors by kind, latency percentiles and histogram, reconnects,
/// tunnel durations and the devices behind them. Counts, sizes and times only.
pub(super) fn performance(conn: &Connection, window: &ChartWindow) -> Value {
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
    let first = DeviceListQuery::first();
    let top = reconnect_devices(conn, window, &first);
    let traffic = traffic_devices(conn, window, &first);
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
        "connections": connection_stats(conn, window, top),
        "deviceTraffic": traffic,
    })
}

/// Paging and sorting of a per-device list on the Performance page.
pub(super) struct DeviceListQuery {
    offset: usize,
    size: usize,
    sort: Option<(String, Dir)>,
}

impl DeviceListQuery {
    fn first() -> Self {
        Self {
            offset: 0,
            size: FIRST_DEVICES,
            sort: None,
        }
    }
    fn from(query: &ListQuery) -> Self {
        let paging = query.paging();
        Self {
            offset: paging.offset(),
            size: paging.size,
            sort: query
                .sort_by(Dir::Desc)
                .map(|(key, dir)| (key.to_string(), dir)),
        }
    }
}

/// Adds each row's device and owner (name, username, email, ids).
fn with_devices(
    conn: &Connection,
    rows: Vec<(String, serde_json::Map<String, Value>)>,
) -> Vec<Value> {
    let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
    let refs = device_refs(conn, &ids);
    rows.into_iter()
        .filter_map(|(id, mut row)| {
            let device = refs.get(&id)?;
            row.insert("deviceId".into(), json!(id));
            row.extend(device.fields());
            Some(Value::Object(row))
        })
        .collect()
}

/// Devices by tunnels opened in the window, with the short ones (under ten
/// minutes, ended in the window), busiest first; one page and the total.
fn reconnect_devices(
    conn: &Connection,
    window: &ChartWindow,
    page: &DeviceListQuery,
) -> (Vec<Value>, i64) {
    let order = match page.sort.as_ref() {
        Some((key, dir)) if key == "short" => {
            format!("short {}, connects DESC, device_id", dir.sql())
        }
        Some((key, dir)) if key == "connects" => format!("connects {}, device_id", dir.sql()),
        Some((key, dir)) if key == "device" => format!(
            "(SELECT name FROM relay_devices WHERE id=device_id) COLLATE NOCASE {}, device_id",
            dir.sql()
        ),
        _ => "connects DESC, device_id".into(),
    };
    let rows: Vec<(String, serde_json::Map<String, Value>)> = conn
        .prepare(&format!(
            "SELECT device_id,COUNT(*) AS connects,
                    COALESCE(SUM(ended_at IS NOT NULL AND ended_at<=?2 AND ended_at-connected_at<600000),0) AS short
             FROM relay_device_connections WHERE connected_at>=?1 AND connected_at<?2
               AND device_id IN (SELECT id FROM relay_devices)
             GROUP BY device_id ORDER BY {order} LIMIT ?3 OFFSET ?4"
        ))
        .and_then(|mut stmt| {
            stmt.query_map(
                params![window.start, window.end, page.size as i64, page.offset as i64],
                |row| {
                    let mut map = serde_json::Map::new();
                    map.insert("connects".into(), json!(row.get::<_, i64>(1)?));
                    map.insert("shortConnections".into(), json!(row.get::<_, i64>(2)?));
                    Ok((row.get::<_, String>(0)?, map))
                },
            )?
            .collect()
        })
        .unwrap_or_default();
    let total = count(
        conn,
        "SELECT COUNT(DISTINCT device_id) FROM relay_device_connections WHERE connected_at>=?1 AND connected_at<?2
           AND device_id IN (SELECT id FROM relay_devices)",
        params![window.start, window.end],
    );
    (with_devices(conn, rows), total)
}

/// Tunnel connections around the window: how long those that ended in it
/// lasted, how many were open, and the devices that reconnected most.
fn connection_stats(conn: &Connection, window: &ChartWindow, top: (Vec<Value>, i64)) -> Value {
    let (start, end) = (window.start, window.end);
    let kept_from = now_ms() - metrics::CONNECTION_RETENTION_DAYS * DAY_MS;
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
    if let Ok(mut stmt) = conn.prepare(
        "SELECT CASE WHEN ended_at-connected_at<60000 THEN 0 WHEN ended_at-connected_at<600000 THEN 1
                     WHEN ended_at-connected_at<3600000 THEN 2 WHEN ended_at-connected_at<21600000 THEN 3
                     WHEN ended_at-connected_at<86400000 THEN 4 ELSE 5 END,COUNT(*)
         FROM relay_device_connections WHERE ended_at>?1 AND ended_at<=?2 GROUP BY 1",
    ) {
        if let Ok(rows) = stmt.query_map(params![start, end], |row| {
            Ok((row.get::<_, usize>(0)?, row.get::<_, i64>(1)?))
        }) {
            for (bucket, count) in rows.flatten() {
                if let Some(slot) = durations.get_mut(bucket) {
                    *slot = count;
                }
            }
        }
    }
    // Open at the window's end: not ended, or ended after it.
    let open = count(
        conn,
        "SELECT (SELECT COUNT(*) FROM relay_device_connections WHERE ended_at IS NULL AND connected_at<?2)
              + (SELECT COUNT(*) FROM relay_device_connections WHERE ended_at>?2 AND connected_at<?2)",
        params![start, end],
    );
    let connects = count(
        conn,
        "SELECT COUNT(*) FROM relay_device_connections WHERE connected_at>=?1 AND connected_at<?2",
        params![start, end],
    );
    let (top, top_total) = top;
    json!({
        "historyStart": (kept_from > start).then(|| rfc3339_from_ms(kept_from)),
        "durations": BUCKETS.iter().zip(durations).map(|((key, _), count)| json!({ "key": key, "count": count })).collect::<Vec<_>>(),
        "open": open,
        "connects": connects,
        "topDevices": top,
        "topDevicesTotal": top_total,
    })
}

/// Forwarded requests, errors and bytes per device in the window, busiest
/// first: whole hours for two days or less, whole days beyond (hour rows are
/// kept 30 days, day rows the metrics retention).
fn traffic_devices(conn: &Connection, window: &ChartWindow, page: &DeviceListQuery) -> Value {
    let (start, end) = (window.start, window.end);
    let hourly = window.span() <= 2 * DAY_MS
        && start >= now_ms() - metrics::DEVICE_TRAFFIC_HOURS_DAYS * DAY_MS;
    let (table, column, unit) = if hourly {
        ("relay_device_traffic", "hour", HOUR_MS)
    } else {
        ("relay_device_traffic_daily", "day", DAY_MS)
    };
    let first = start - start.rem_euclid(unit);
    let order = match page.sort.as_ref() {
        Some((key, dir)) => {
            let d = dir.sql();
            match key.as_str() {
                "errors" => format!("errors {d}, requests DESC, device_id"),
                "rate" => format!("CAST(errors AS REAL)/MAX(requests,1) {d}, requests DESC, device_id"),
                "sent" => format!("request_bytes {d}, device_id"),
                "received" => format!("response_bytes {d}, device_id"),
                "device" => format!(
                    "(SELECT name FROM relay_devices WHERE id=device_id) COLLATE NOCASE {d}, device_id"
                ),
                _ => format!("requests {d}, device_id"),
            }
        }
        None => "requests DESC, device_id".into(),
    };
    let rows: Vec<(String, serde_json::Map<String, Value>)> = conn
        .prepare(&format!(
            "SELECT device_id,SUM(requests) AS requests,SUM(errors) AS errors,
                    SUM(request_bytes) AS request_bytes,SUM(response_bytes) AS response_bytes
             FROM {table} WHERE {column}>=?1 AND {column}<?2 GROUP BY device_id
             ORDER BY {order} LIMIT ?3 OFFSET ?4"
        ))
        .and_then(|mut stmt| {
            stmt.query_map(
                params![first, end, page.size as i64, page.offset as i64],
                |row| {
                    let mut map = serde_json::Map::new();
                    map.insert("requests".into(), json!(row.get::<_, i64>(1)?));
                    map.insert("errors".into(), json!(row.get::<_, i64>(2)?));
                    map.insert("requestBytes".into(), json!(row.get::<_, i64>(3)?));
                    map.insert("responseBytes".into(), json!(row.get::<_, i64>(4)?));
                    Ok((row.get::<_, String>(0)?, map))
                },
            )?
            .collect()
        })
        .unwrap_or_default();
    let (total_devices, total_requests): (i64, i64) = conn
        .query_row(
            &format!(
                "SELECT COUNT(DISTINCT device_id),COALESCE(SUM(requests),0) FROM {table} WHERE {column}>=?1 AND {column}<?2"
            ),
            params![first, end],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0, 0));
    json!({
        "resolutionSeconds": unit / 1000,
        "devices": with_devices(conn, rows),
        "total": total_devices,
        "requests": total_requests,
    })
}

pub(super) async fn performance_handler(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match admin_read(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    blocking(|| {
        let window = match chart_window(&query, Range::Day, now_ms()) {
            Ok(window) => window,
            Err(message) => return bad_request(message),
        };
        Json(performance(&conn, &window)).into_response()
    })
}

/// One page of a per-device list of the Performance page: `view=reconnects`
/// (sort `connects`, `short`, `device`) or `view=traffic` (sort `requests`,
/// `errors`, `rate`, `sent`, `received`, `device`).
pub(super) async fn performance_devices(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (conn, _) = match admin_read(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    blocking(|| {
        let window = match chart_window(&query, Range::Day, now_ms()) {
            Ok(window) => window,
            Err(message) => return bad_request(message),
        };
        let page = DeviceListQuery::from(&query);
        let paging = query.paging();
        let body = if query.view.as_deref() == Some("traffic") {
            let traffic = traffic_devices(&conn, &window, &page);
            let total = traffic["total"].as_u64().unwrap_or(0) as usize;
            let items = traffic["devices"].as_array().cloned().unwrap_or_default();
            let mut body = paging.wrap(items, total);
            body.insert(
                "resolutionSeconds".into(),
                traffic["resolutionSeconds"].clone(),
            );
            body.insert("requests".into(), traffic["requests"].clone());
            body
        } else {
            let (items, total) = reconnect_devices(&conn, &window, &page);
            paging.wrap(items, total as usize)
        };
        let mut body = body;
        body.insert("window".into(), window.json());
        Json(Value::Object(body)).into_response()
    })
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
    let online: HashSet<&str> = online.iter().map(String::as_str).collect();
    let mut rows = metrics::version_totals(conn, &online);
    rows.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(a), Some(b)) => compare_versions(b, a),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    rows.into_iter()
        .take(100)
        .map(|(version, devices, online)| {
            let outdated = version.as_deref().is_some_and(|version| {
                compare_versions(version, RELAY_VERSION) == std::cmp::Ordering::Less
            });
            json!({ "version": version, "devices": devices, "online": online, "outdated": outdated })
        })
        .collect()
}

/// Distinct people and devices active on each of the `days` days ending with
/// `end`. A finished day never changes, so its counts are kept in memory.
pub(super) fn daily_active(state: &AppState, conn: &Connection, end: i64, days: i64) -> Vec<Value> {
    let today = day_of(now_ms());
    let mut cache = state
        .admin
        .active_days
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if cache.len() > 1000 {
        cache.clear();
    }
    let mut stmt = match conn
        .prepare_cached("SELECT COUNT(*) FROM relay_daily_active WHERE day=?1 AND kind=?2")
    {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    let mut rows = Vec::new();
    let mut any = false;
    for offset in 0..days {
        let day = day_of(end - (days - 1 - offset) * DAY_MS);
        let counts = match cache.get(&day) {
            Some(counts) if day != today => *counts,
            _ => {
                let mut read = |kind: &str| {
                    stmt.query_row(params![day, kind], |row| row.get::<_, i64>(0))
                        .unwrap_or(0)
                };
                let counts = (read("user"), read("device"));
                if day != today {
                    cache.insert(day.clone(), counts);
                }
                counts
            }
        };
        any |= counts != (0, 0);
        rows.push(json!({ "day": day, "users": counts.0, "devices": counts.1 }));
    }
    if any {
        rows
    } else {
        Vec::new()
    }
}

/// One device over the last day, week or month: its tunnel connections and
/// forwarded requests (per hour for a day, per day beyond).
pub(super) fn device_detail(conn: &Connection, device: &str, range: Range) -> (Value, Value) {
    let now = now_ms();
    let start = now - range.millis();
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
        // The newest 500 connections are enough to draw and list.
        "spans": spans.iter().rev().take(500).rev().map(|span| json!({
            "from": rfc3339_from_ms(span.from),
            "to": span.to.map(rfc3339_from_ms),
            "reason": span.reason,
        })).collect::<Vec<_>>(),
        "uptime": online as f64 / window as f64,
        "reconnects": spans.iter().filter(|span| span.from >= start).count(),
        "longestOfflineSeconds": longest_gap.max(0) / 1000,
    });
    let (table, column, unit, buckets) = match range {
        Range::Day => ("relay_device_traffic", "hour", HOUR_MS, 24),
        Range::Week => ("relay_device_traffic_daily", "day", DAY_MS, 7),
        _ => ("relay_device_traffic_daily", "day", DAY_MS, 30),
    };
    // The buckets ending with the current one.
    let first = now - now.rem_euclid(unit) - (buckets - 1) * unit;
    let rows: HashMap<i64, (i64, i64, i64, i64)> = conn
        .prepare(&format!(
            "SELECT {column},requests,request_bytes,response_bytes,errors FROM {table} WHERE device_id=?1 AND {column}>=?2"
        ))
        .and_then(|mut stmt| {
            stmt.query_map(params![device, first], |row| {
                Ok((row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
            })?
            .collect()
        })
        .unwrap_or_default();
    let points: Vec<Value> = (0..buckets)
        .map(|index| {
            let at = first + index * unit;
            let (requests, request_bytes, response_bytes, errors) = rows.get(&at).copied().unwrap_or_default();
            json!({ "at": rfc3339_from_ms(at), "requests": requests, "requestBytes": request_bytes, "responseBytes": response_bytes, "errors": errors })
        })
        .collect();
    let traffic = json!({
        "resolutionSeconds": unit / 1000,
        "historyStart": started.filter(|begun| *begun > first).map(rfc3339_from_ms),
        "hours": points,
    });
    (connections, traffic)
}

/// Failed sign-ins, refusals by kind and per day, retention, and admin
/// actions in the window for the security page.
pub(super) fn security(conn: &Connection, window: &ChartWindow) -> serde_json::Map<String, Value> {
    let (start, end) = (window.start, window.end);
    let started = history(conn);
    let first_day = window.first_day();
    let per_day = |metric: &str| -> Vec<Value> {
        metrics::series(conn, metric, None, first_day, end + 1, DAY_MS, Agg::Sum)
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
                "total": metrics::total(conn, "login_failed", start, end),
                "daily": per_day("login_failed"),
                "historyStart": history_start,
            })
        },
    );
    let by_kind: serde_json::Map<String, Value> =
        metrics::totals_by_label(conn, "rate_limited", start, end)
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
            "SELECT COUNT(*) FROM relay_admin_actions WHERE created_at>=?1 AND created_at<?2",
            [start, end]
        )),
    );
    out
}

/// Push delivery results, version history and database growth for the system page.
pub(super) fn system(
    conn: &Connection,
    online: &[String],
    window: &ChartWindow,
) -> serde_json::Map<String, Value> {
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
    let growth = metrics::series(
        conn,
        "db_bytes",
        None,
        window.start,
        window.end,
        window.step,
        Agg::Mean,
    );
    out.insert(
        "growth".into(),
        started.map_or(Value::Null, |_| {
            metrics::series_json(&growth, window.step, started)
        }),
    );
    // Online devices per version per day (the day's largest count) over the
    // window's days.
    let days = window.days().min(400);
    let first_day = window.end - window.end.rem_euclid(DAY_MS) - (days - 1) * DAY_MS;
    let rows: Vec<(i64, String, f64)> = conn
        .prepare("SELECT bucket,label,peak FROM relay_metrics WHERE resolution='d' AND metric='online_by_version' AND bucket>=?1 AND bucket<?2")
        .and_then(|mut stmt| stmt.query_map([first_day, window.end], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect())
        .unwrap_or_default();
    // Days the relay sampled at all; on the others the versions are unknown.
    let sampled: HashSet<i64> = conn
        .prepare("SELECT bucket FROM relay_metrics WHERE resolution='d' AND metric='up' AND label='' AND bucket>=?1 AND bucket<?2")
        .and_then(|mut stmt| stmt.query_map([first_day, window.end], |row| row.get(0))?.collect())
        .unwrap_or_default();
    let history_days: Vec<Value> = (0..days)
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

type ActionRow = (
    i64,
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn action_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ActionRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

/// Actions as JSON; a device target also names its owner (when it still
/// exists), since device names repeat across people.
fn actions_json(conn: &Connection, rows: Vec<ActionRow>) -> Vec<Value> {
    let devices: Vec<String> = rows
        .iter()
        .filter(|row| row.4.as_deref() == Some("device"))
        .filter_map(|row| row.5.clone())
        .collect();
    let refs = device_refs(conn, &devices);
    rows.into_iter()
        .map(
            |(id, at, admin, action, target_type, target_id, target_label, detail)| {
                let device = target_id
                    .as_ref()
                    .filter(|_| target_type.as_deref() == Some("device"))
                    .and_then(|id| refs.get(id));
                json!({
                    "id": id,
                    "at": rfc3339_from_ms(at),
                    "adminUsername": admin,
                    "action": action,
                    "targetType": target_type,
                    "targetId": target_id,
                    "targetLabel": target_label,
                    "targetOwnerUserId": device.map(|device| &device.owner_user_id),
                    "targetOwnerUsername": device.map(|device| &device.owner_username),
                    "targetOwnerEmail": device.map(|device| &device.owner_email),
                    "detail": detail.and_then(|text| serde_json::from_str::<Value>(&text).ok()),
                })
            },
        )
        .collect()
}

const ACTION_COLUMNS: &str =
    "id,created_at,admin_username,action,target_type,target_id,target_label,detail";

pub(super) fn recent_actions(conn: &Connection, limit: i64) -> Vec<Value> {
    let rows: Vec<ActionRow> = conn
        .prepare(&format!(
            "SELECT {ACTION_COLUMNS} FROM relay_admin_actions ORDER BY id DESC LIMIT ?1"
        ))
        .and_then(|mut stmt| stmt.query_map([limit], action_row)?.collect())
        .unwrap_or_default();
    actions_json(conn, rows)
}

pub(super) async fn actions(
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
            &format!(
                "SELECT COUNT(*) FROM relay_admin_actions WHERE {condition} AND ?2=?2 AND ?3=?3"
            ),
            params![kind, 0, 0, from, to],
        );
        let rows: Vec<ActionRow> = conn
            .prepare(&format!(
                "SELECT {ACTION_COLUMNS} FROM relay_admin_actions WHERE {condition} ORDER BY {order} LIMIT ?2 OFFSET ?3"
            ))
            .and_then(|mut stmt| {
                stmt.query_map(params![kind, paging.size as i64, paging.offset() as i64, from, to], action_row)?
                    .collect()
            })
            .unwrap_or_default();
        let items = actions_json(&conn, rows);
        Json(Value::Object(paging.wrap(items, total as usize))).into_response()
    })
}

fn retention_json(conn: &Connection) -> Value {
    json!({
        "metricsDays": metrics::metrics_days(conn),
        "securityDays": metrics::security_days(conn),
        "options": { "metricsDays": metrics::METRICS_DAYS, "securityDays": metrics::SECURITY_DAYS },
        "fixed": {
            "connectionDays": metrics::CONNECTION_RETENTION_DAYS,
            "deviceTrafficHourDays": metrics::DEVICE_TRAFFIC_HOURS_DAYS,
            "adminActionDays": metrics::ADMIN_ACTION_RETENTION_DAYS,
            "securityMaxEvents": metrics::SECURITY_MAX_EVENTS,
        },
    })
}

pub(super) async fn retention(headers: HeaderMap, State(state): State<Arc<AppState>>) -> Response {
    let (conn, _) = match admin_read(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    blocking(move || Json(retention_json(&conn)).into_response())
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
