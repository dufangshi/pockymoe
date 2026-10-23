//! System: the relay process, configuration checks, push delivery queue and
//! storage. Slow checks (integrity, table sizes) are cached.
use super::common::*;
use super::*;

const INTEGRITY_TTL: Duration = Duration::from_secs(60 * 60);
const STORAGE_TTL: Duration = Duration::from_secs(10 * 60);

pub(super) async fn system(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let window = match chart_window(&query, Range::Month, now_ms()) {
        Ok(window) => window,
        Err(message) => return bad_request(message),
    };
    // Integrity and table sizes read the whole database: measured on their
    // own connection off the async threads, then cached.
    let (integrity, storage) = match slow_checks(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let online: Vec<String> = state.sockets.read().await.keys().cloned().collect();
    let tunnels = online.len();
    let browsers = state.clients.read().await.len();
    let pending = state
        .pending
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .len();
    let mappings: usize = state.preview.mapping_counts().values().sum();
    let (_, preview_sessions, preview_streams) = state.preview.usage();
    let (conn, _) = match admin_read(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    blocking(|| {
        let data_dir = state
            .store
            .path
            .parent()
            .map(|path| path.display().to_string());
        let links = super::shares::public_link_summary(&conn);
        let mut push = push_summary(&conn);
        let mut storage = storage;
        let stats = super::stats::system(&conn, &online, &window);
        push["delivery"] = stats["delivery"].clone();
        storage["growth"] = stats["growth"].clone();
        drop(conn);
        let started = ms_from_rfc3339(&state.admin.started_at).unwrap_or_else(now_ms);
        let checks = vec![
            json!({ "key": "integrity", "ok": integrity == "ok", "detail": integrity }),
            json!({
                "key": "sessionSecret",
                "ok": true,
                "detail": if std::env::var_os("POCKYMOE_RELAY_SESSION_SECRET").is_some() { "environment" } else { "dataDir" },
            }),
            json!({ "key": "publicBaseUrl", "ok": state.oauth.public_base_url.is_some(), "detail": state.oauth.public_base_url }),
            json!({ "key": "webOrigin", "ok": true, "detail": state.oauth.web_origin.clone().or_else(|| state.web_dist.as_ref().map(|_| "relay".to_string())) }),
            json!({ "key": "github", "ok": state.oauth.available(OAuthProvider::Github), "detail": Value::Null }),
            json!({ "key": "google", "ok": state.oauth.available(OAuthProvider::Google), "detail": Value::Null }),
            json!({ "key": "portPreviews", "ok": state.preview.base.is_some(), "detail": state.preview.base.as_ref().map(url::Url::as_str) }),
            json!({ "key": "apns", "ok": crate::apns::configured(), "detail": Value::Null }),
            json!({ "key": "legacyPublicLinks", "ok": links["legacy"].as_i64().unwrap_or(0) == 0, "detail": links["legacy"] }),
        ];
        Json(json!({
            "relay": {
                "version": RELAY_VERSION,
                "startedAt": state.admin.started_at,
                "uptimeSeconds": (now_ms() - started).max(0) / 1000,
                "publicBaseUrl": state.oauth.public_base_url,
                "webOrigin": state.oauth.web_origin,
                "servesWeb": state.web_dist.is_some(),
                "dataDir": data_dir,
            },
            "connections": {
                "deviceTunnels": tunnels,
                "browserSockets": browsers,
                "pendingRequests": pending,
                "previewMappings": mappings,
                "previewSessions": preview_sessions,
                "previewStreams": preview_streams,
            },
            "checks": checks,
            "push": push,
            "storage": storage,
            "versions": stats["versions"],
            "metrics": stats["metrics"],
            "window": window.json(),
        }))
        .into_response()
    })
}

fn cached<T: Clone>(slot: &StdMutex<Option<(std::time::Instant, T)>>, ttl: Duration) -> Option<T> {
    slot.lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .filter(|(at, _)| at.elapsed() < ttl)
        .map(|(_, value)| value.clone())
}

/// The cached integrity check and storage sizes, measured again when stale on
/// a read-only connection in a blocking thread (one measurement at a time).
async fn slow_checks(
    state: &Arc<AppState>,
    headers: &HeaderMap,
) -> Result<(String, Value), Response> {
    // Only the admin may start a measurement.
    drop(admin_conn(state, headers).await?);
    let fresh = |state: &AppState| {
        cached(&state.admin.integrity, INTEGRITY_TTL).zip(cached(&state.admin.storage, STORAGE_TTL))
    };
    if let Some(value) = fresh(state) {
        return Ok(value);
    }
    let _measuring = state.admin.measuring.lock().await;
    if let Some(value) = fresh(state) {
        return Ok(value);
    }
    let need_integrity = cached(&state.admin.integrity, INTEGRITY_TTL).is_none();
    let need_storage = cached(&state.admin.storage, STORAGE_TTL).is_none();
    let (integrity, storage) = if state.store.read.is_some() {
        let path = state.store.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            conn.busy_timeout(Duration::from_secs(5))?;
            Ok::<_, rusqlite::Error>((
                need_integrity.then(|| integrity(&conn)),
                need_storage.then(|| storage(&conn, Some(&path))),
            ))
        })
        .await
        .map_err(|_| internal_error())?
        .map_err(|_| internal_error())?
    } else {
        // An in-memory store (tests) has no file to open again.
        let conn = state.store.conn.lock().await;
        (
            need_integrity.then(|| integrity(&conn)),
            need_storage.then(|| storage(&conn, None)),
        )
    };
    let now = std::time::Instant::now();
    if let Some(value) = integrity {
        *state
            .admin
            .integrity
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some((now, value));
    }
    if let Some(value) = storage {
        *state
            .admin
            .storage
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some((now, value));
    }
    Ok((
        cached(&state.admin.integrity, INTEGRITY_TTL).unwrap_or_default(),
        cached(&state.admin.storage, STORAGE_TTL).unwrap_or(Value::Null),
    ))
}

fn integrity(conn: &Connection) -> String {
    let value = conn
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .unwrap_or_else(|error| error.to_string());
    let foreign_keys = count(conn, "SELECT COUNT(*) FROM pragma_foreign_key_check", []);
    if value == "ok" && foreign_keys > 0 {
        format!("{foreign_keys} foreign key violations")
    } else {
        value
    }
}

/// Database and WAL size, and the largest tables (by pages when SQLite has
/// the dbstat table, otherwise by rows).
fn storage(conn: &Connection, database: Option<&FsPath>) -> Value {
    let file = |suffix: &str| {
        database
            .map(|path| {
                let mut name = path.as_os_str().to_os_string();
                name.push(suffix);
                std::fs::metadata(PathBuf::from(name))
                    .map(|meta| meta.len())
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    };
    let sized: Vec<(String, i64)> = conn
        .prepare(
            "SELECT m.tbl_name,SUM(s.pgsize) FROM dbstat s JOIN sqlite_master m ON m.name=s.name
             GROUP BY m.tbl_name ORDER BY 2 DESC",
        )
        .and_then(|mut stmt| {
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .unwrap_or_default();
    let tables: Vec<Value> = if sized.is_empty() {
        let names: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'relay_%'")
            .and_then(|mut stmt| stmt.query_map([], |row| row.get(0))?.collect())
            .unwrap_or_default();
        let mut rows: Vec<(String, i64)> = names
            .into_iter()
            .map(|name| {
                let rows = count(conn, &format!("SELECT COUNT(*) FROM \"{name}\""), []);
                (name, rows)
            })
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        rows.into_iter()
            .map(|(name, rows)| json!({ "name": name, "rows": rows, "bytes": Value::Null }))
            .collect()
    } else {
        sized
            .into_iter()
            .map(|(name, bytes)| json!({ "name": name, "bytes": bytes, "rows": Value::Null }))
            .collect()
    };
    let value = json!({
        "databaseBytes": file(""),
        "walBytes": file("-wal"),
        "tables": tables,
        "measuredAt": now_rfc3339(),
    });
    value
}

/// Subscriptions by channel, the delivery queue and turn results of the last
/// day (push events are kept for one day).
fn push_summary(conn: &Connection) -> Value {
    let (web, apns) = conn
        .query_row(
            "SELECT COALESCE(SUM(subscription NOT LIKE '%\"deviceToken\"%'),0),
                    COALESCE(SUM(subscription LIKE '%\"deviceToken\"%'),0)
             FROM relay_push_subscriptions",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap_or((0, 0));
    let (queued, retrying) = conn
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(attempts>0),0) FROM relay_push_deliveries",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap_or((0, 0));
    let since = now_ms() - DAY_MS;
    let mut results = serde_json::Map::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT json_extract(payload,'$.status'),COUNT(*) FROM relay_push_events WHERE created_at>=?1 GROUP BY 1",
    ) {
        if let Ok(rows) = stmt.query_map([since], |row| {
            Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?))
        }) {
            for (status, count) in rows.flatten() {
                results.insert(status.unwrap_or_else(|| "unknown".into()), json!(count));
            }
        }
    }
    json!({
        "subscriptions": { "webPush": web, "apns": apns },
        "queued": queued,
        "retrying": retrying,
        "turns24h": results,
        "apnsConfigured": crate::apns::configured(),
    })
}
