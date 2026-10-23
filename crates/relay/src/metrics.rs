//! Operational statistics for the admin console (docs/relay-admin.md).
//!
//! Only counts, sizes, durations and times are recorded: never request or
//! response bodies, paths beyond their purpose, thread titles, IP addresses
//! or user agents. Counters live in memory and a sampler writes them once a
//! minute in one transaction, so a heartbeat or a forwarded request never
//! writes to SQLite by itself.
//!
//! Tables (all additive, created if missing):
//! - `relay_metrics`: per-minute rows kept 48 hours, per-hour rows kept the
//!   metrics retention (30, 90 or 365 days, default 90), per-day version rows
//!   kept the same; `value` sums the samples, `peak` is the largest and
//!   `samples` counts them, so a gauge's mean is `value / samples`.
//! - `relay_metrics` also holds per-day rows (`d`) of every metric, so long
//!   ranges read a day per row instead of 24 hour rows.
//! - `relay_device_traffic`: forwarded requests and bytes per device and hour,
//!   kept 30 days; `relay_device_traffic_daily` per device and day, kept the
//!   metrics retention.
//! - `relay_public_link_stats`: each public link's size and whether it is
//!   encrypted, written by triggers so the console never parses snapshots.
//! - `relay_device_connections`: each tunnel's connect and disconnect time and
//!   why it ended, kept 30 days (at most 200,000 rows).
//! - `relay_device_versions`: the version each device's `/healthz` reports.
//! - `relay_daily_active`: which accounts and devices were active each day.
//! - `relay_admin_actions` (written by `admin::log_action`): kept 365 days, at
//!   most 50,000 rows.
use super::admin::{compare_versions, DAY_MS, HOUR_MS, MINUTE_MS, RELAY_VERSION};
use super::*;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const MINUTE_RETENTION_MS: i64 = 48 * HOUR_MS;
pub(crate) const CONNECTION_RETENTION_DAYS: i64 = 30;
/// Hourly device traffic is kept this long; daily rows the metrics retention.
pub(crate) const DEVICE_TRAFFIC_HOURS_DAYS: i64 = 30;
const CONNECTION_MAX_ROWS: i64 = 200_000;
pub(crate) const ADMIN_ACTION_RETENTION_DAYS: i64 = 365;
const ADMIN_ACTION_MAX_ROWS: i64 = 50_000;
pub(crate) const SECURITY_MAX_EVENTS: i64 = 100_000;
pub(crate) const METRICS_DAYS: [i64; 3] = [30, 90, 365];
pub(crate) const SECURITY_DAYS: [i64; 3] = [90, 180, 365];
const DEFAULT_METRICS_DAYS: i64 = 90;
const DEFAULT_SECURITY_DAYS: i64 = 180;
/// Connection events waiting for the next flush; a flapping device cannot
/// grow memory without bound.
const MAX_QUEUED_EVENTS: usize = 10_000;
/// Five or more tunnel connections within an hour reads as unstable.
pub(crate) const UNSTABLE_CONNECTS: i64 = 5;
const VERSION_PROBE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const VERSION_PROBES_PER_MINUTE: usize = 10;
/// Upper bounds (ms) of the forwarding latency histogram; a last, open bucket
/// holds slower answers. Percentiles are interpolated inside the bucket that
/// holds them (`percentile`), so their error is at most that bucket's width.
/// Older relays recorded coarser bounds (5, 10, 25, 50, 100, 250, 500, 1000,
/// 2500, 5000, 10000, 30000); those rows still read correctly.
pub(crate) const LATENCY_BOUNDS: [u64; 26] = [
    5, 10, 20, 30, 50, 75, 100, 150, 200, 300, 400, 500, 650, 800, 1000, 1250, 1500, 2000, 2500,
    3000, 4000, 5000, 7500, 10000, 20000, 30000,
];
/// The bounds older relays recorded.
const LEGACY_LATENCY_BOUNDS: [u64; 12] =
    [5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000, 30000];

pub(crate) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS relay_metrics (
           resolution TEXT NOT NULL, bucket INTEGER NOT NULL, metric TEXT NOT NULL,
           label TEXT NOT NULL DEFAULT '', value REAL NOT NULL, peak REAL NOT NULL,
           samples INTEGER NOT NULL, PRIMARY KEY(resolution,metric,label,bucket)
         ) WITHOUT ROWID;
         CREATE INDEX IF NOT EXISTS relay_metrics_bucket ON relay_metrics(resolution,bucket);
         CREATE TABLE IF NOT EXISTS relay_device_traffic (
           device_id TEXT NOT NULL REFERENCES relay_devices(id) ON DELETE CASCADE,
           hour INTEGER NOT NULL, requests INTEGER NOT NULL DEFAULT 0,
           request_bytes INTEGER NOT NULL DEFAULT 0, response_bytes INTEGER NOT NULL DEFAULT 0,
           errors INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(device_id,hour)
         ) WITHOUT ROWID;
         CREATE INDEX IF NOT EXISTS relay_device_traffic_hour ON relay_device_traffic(hour);
         CREATE TABLE IF NOT EXISTS relay_device_connections (
           id INTEGER PRIMARY KEY,
           device_id TEXT NOT NULL REFERENCES relay_devices(id) ON DELETE CASCADE,
           connection_id TEXT NOT NULL UNIQUE, connected_at INTEGER NOT NULL,
           ended_at INTEGER, reason TEXT
         );
         CREATE INDEX IF NOT EXISTS relay_device_connections_device ON relay_device_connections(device_id,connected_at);
         CREATE INDEX IF NOT EXISTS relay_device_connections_time ON relay_device_connections(connected_at);
         CREATE INDEX IF NOT EXISTS relay_device_connections_ended ON relay_device_connections(ended_at,connected_at);
         CREATE TABLE IF NOT EXISTS relay_device_versions (
           device_id TEXT PRIMARY KEY REFERENCES relay_devices(id) ON DELETE CASCADE,
           version TEXT, checked_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS relay_daily_active (
           day TEXT NOT NULL, kind TEXT NOT NULL CHECK (kind IN ('user','device')),
           subject_id TEXT NOT NULL, PRIMARY KEY(day,kind,subject_id)
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS relay_admin_actions (
           id INTEGER PRIMARY KEY, admin_user_id TEXT, admin_username TEXT NOT NULL,
           action TEXT NOT NULL, target_type TEXT, target_id TEXT, target_label TEXT,
           detail TEXT, created_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS relay_admin_actions_time ON relay_admin_actions(created_at);
         CREATE TABLE IF NOT EXISTS relay_user_sign_ins (
           user_id TEXT PRIMARY KEY REFERENCES relay_users(id) ON DELETE CASCADE,
           at INTEGER NOT NULL, environment TEXT NOT NULL DEFAULT ''
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS relay_device_traffic_daily (
           day INTEGER NOT NULL,
           device_id TEXT NOT NULL REFERENCES relay_devices(id) ON DELETE CASCADE,
           requests INTEGER NOT NULL DEFAULT 0,
           request_bytes INTEGER NOT NULL DEFAULT 0, response_bytes INTEGER NOT NULL DEFAULT 0,
           errors INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(day,device_id)
         ) WITHOUT ROWID;
         CREATE INDEX IF NOT EXISTS relay_device_traffic_daily_device ON relay_device_traffic_daily(device_id,day);
         CREATE INDEX IF NOT EXISTS relay_metrics_series ON relay_metrics(resolution,metric,bucket);
         CREATE TABLE IF NOT EXISTS relay_public_link_stats (
           link_id TEXT PRIMARY KEY REFERENCES relay_public_links(id) ON DELETE CASCADE,
           bytes INTEGER NOT NULL, encrypted INTEGER NOT NULL
         ) WITHOUT ROWID;
         CREATE TRIGGER IF NOT EXISTS relay_public_link_stats_insert AFTER INSERT ON relay_public_links
         BEGIN
           INSERT OR REPLACE INTO relay_public_link_stats(link_id,bytes,encrypted)
           VALUES (NEW.id,length(CAST(NEW.snapshot_json AS BLOB)),
                   json_valid(NEW.snapshot_json) AND COALESCE(json_extract(NEW.snapshot_json,'$.version'),1)=2);
         END;
         CREATE TRIGGER IF NOT EXISTS relay_public_link_stats_update AFTER UPDATE OF snapshot_json ON relay_public_links
         BEGIN
           INSERT OR REPLACE INTO relay_public_link_stats(link_id,bytes,encrypted)
           VALUES (NEW.id,length(CAST(NEW.snapshot_json AS BLOB)),
                   json_valid(NEW.snapshot_json) AND COALESCE(json_extract(NEW.snapshot_json,'$.version'),1)=2);
         END;
         CREATE INDEX IF NOT EXISTS relay_share_activity_time ON relay_share_activity(accessed_at);
         CREATE INDEX IF NOT EXISTS relay_users_created ON relay_users(created_at);",
    )?;
    // Links stored before the triggers existed.
    conn.execute(
        "INSERT OR IGNORE INTO relay_public_link_stats(link_id,bytes,encrypted)
         SELECT l.id,length(CAST(l.snapshot_json AS BLOB)),
                json_valid(l.snapshot_json) AND COALESCE(json_extract(l.snapshot_json,'$.version'),1)=2
         FROM relay_public_links l
         WHERE NOT EXISTS(SELECT 1 FROM relay_public_link_stats s WHERE s.link_id=l.id)",
        [],
    )?;
    // Daily rows for the history recorded before they existed, once.
    if relay_setting(conn, "metricsDailyRollup").is_none() {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO relay_metrics(resolution,bucket,metric,label,value,peak,samples)
             SELECT 'd',bucket-(bucket%86400000),metric,label,SUM(value),MAX(peak),SUM(samples)
             FROM relay_metrics WHERE resolution='h' AND metric<>'online_by_version'
             GROUP BY bucket-(bucket%86400000),metric,label",
            [],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO relay_device_traffic_daily(day,device_id,requests,request_bytes,response_bytes,errors)
             SELECT hour-(hour%86400000),device_id,SUM(requests),SUM(request_bytes),SUM(response_bytes),SUM(errors)
             FROM relay_device_traffic GROUP BY device_id,hour-(hour%86400000)",
            [],
        )?;
        tx.execute(
            "INSERT INTO relay_settings(key,value) VALUES ('metricsDailyRollup','1')",
            [],
        )?;
        tx.commit()?;
    }
    // The latest sign-in of each account and its browser label ("Chrome ·
    // macOS"), kept after the session itself ends. Every new session is a
    // sign-in; its label is written right after the row (auth_api).
    let backfill = !conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND name='relay_sign_in_insert')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false);
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS relay_sign_in_insert AFTER INSERT ON relay_auth_sessions
         BEGIN
           INSERT INTO relay_user_sign_ins(user_id,at,environment) VALUES (NEW.user_id,NEW.created_at,NEW.environment)
           ON CONFLICT(user_id) DO UPDATE SET at=excluded.at,environment=excluded.environment
           WHERE excluded.at>=relay_user_sign_ins.at;
         END;
         CREATE TRIGGER IF NOT EXISTS relay_sign_in_environment AFTER UPDATE OF environment ON relay_auth_sessions
         BEGIN
           UPDATE relay_user_sign_ins SET environment=NEW.environment
           WHERE user_id=NEW.user_id AND at=NEW.created_at;
         END;",
    )?;
    if backfill {
        conn.execute(
            "INSERT OR IGNORE INTO relay_user_sign_ins(user_id,at,environment)
             SELECT user_id,MAX(created_at),environment FROM relay_auth_sessions
             WHERE user_id IN (SELECT id FROM relay_users) GROUP BY user_id",
            [],
        )?;
    }
    Ok(())
}

/// What ended a tunnel connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndReason {
    Closed,
    Timeout,
    Replaced,
    QueueFull,
    Deleted,
    TokenRotated,
    Admin,
    SendFailed,
    Error,
}

impl EndReason {
    fn key(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Timeout => "timeout",
            Self::Replaced => "replaced",
            Self::QueueFull => "queue_full",
            Self::Deleted => "deleted",
            Self::TokenRotated => "token_rotated",
            Self::Admin => "admin",
            Self::SendFailed => "send_failed",
            Self::Error => "error",
        }
    }
}

enum ConnectionEvent {
    Open {
        device: String,
        connection: Uuid,
        at: i64,
    },
    Close {
        connection: Uuid,
        at: i64,
        reason: EndReason,
    },
}

#[derive(Default, Clone, Copy)]
struct Traffic {
    requests: u64,
    request_bytes: u64,
    response_bytes: u64,
    errors: u64,
}

/// How a forwarded request ended, for the counters.
pub(crate) enum Forwarded {
    Answered { status: u16, response_bytes: u64 },
    Offline,
    Timeout,
    Busy,
}

#[derive(Default)]
pub(crate) struct Metrics {
    requests: AtomicU64,
    request_bytes: AtomicU64,
    response_bytes: AtomicU64,
    offline: AtomicU64,
    timeout: AtomicU64,
    busy: AtomicU64,
    device_errors: AtomicU64,
    ws_frames: AtomicU64,
    ws_bytes: AtomicU64,
    login_failed: AtomicU64,
    push_sent: AtomicU64,
    push_failed: AtomicU64,
    push_expired: AtomicU64,
    latency: [AtomicU64; LATENCY_BOUNDS.len() + 1],
    per_device: StdMutex<HashMap<String, Traffic>>,
    events: StdMutex<Vec<ConnectionEvent>>,
    /// Devices recorded as active today (day, ids).
    active_devices: StdMutex<(String, HashSet<String>)>,
    /// When each online device's version was last read, for its connection.
    probes: StdMutex<HashMap<String, (Uuid, std::time::Instant)>>,
    /// The relay process and its host (Server section).
    pub(crate) host: crate::host::HostProbe,
    /// The longest the async runtime was late waking a timer since the last
    /// sample, in microseconds: how long ready work waited for a thread.
    loop_lag_us: AtomicU64,
    /// The latest relay internals, for the Server section's current values.
    latest_load: StdMutex<Option<RelayLoad>>,
}

/// What the relay itself was doing at the last sample.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RelayLoad {
    pub(crate) preview_streams: usize,
    /// Messages waiting in device tunnels' send queues: all, and the fullest.
    pub(crate) tunnel_queue: usize,
    pub(crate) tunnel_queue_max: usize,
    pub(crate) browser_queue: usize,
    pub(crate) queue_capacity: usize,
    /// The store lock over the last minute.
    pub(crate) store: crate::store::LockSample,
    pub(crate) loop_lag_ms: f64,
}

fn take(counter: &AtomicU64) -> u64 {
    counter.swap(0, Ordering::Relaxed)
}

impl Metrics {
    /// A request forwarded to a device. Health probes (`/healthz`) are the
    /// relay's own and are not counted.
    pub(crate) fn forwarded(
        &self,
        device: &str,
        request_bytes: u64,
        result: &Forwarded,
        latency: Duration,
    ) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.request_bytes
            .fetch_add(request_bytes, Ordering::Relaxed);
        let mut traffic = Traffic {
            requests: 1,
            request_bytes,
            ..Traffic::default()
        };
        match result {
            Forwarded::Answered {
                status,
                response_bytes,
            } => {
                self.response_bytes
                    .fetch_add(*response_bytes, Ordering::Relaxed);
                traffic.response_bytes = *response_bytes;
                if *status >= 500 {
                    self.device_errors.fetch_add(1, Ordering::Relaxed);
                    traffic.errors = 1;
                }
                let ms = latency.as_millis() as u64;
                let index = LATENCY_BOUNDS
                    .iter()
                    .position(|bound| ms <= *bound)
                    .unwrap_or(LATENCY_BOUNDS.len());
                self.latency[index].fetch_add(1, Ordering::Relaxed);
            }
            Forwarded::Offline => {
                self.offline.fetch_add(1, Ordering::Relaxed);
                traffic.errors = 1;
            }
            Forwarded::Timeout => {
                self.timeout.fetch_add(1, Ordering::Relaxed);
                traffic.errors = 1;
            }
            Forwarded::Busy => {
                self.busy.fetch_add(1, Ordering::Relaxed);
                traffic.errors = 1;
            }
        }
        let mut per_device = self.per_device.lock().unwrap_or_else(|e| e.into_inner());
        // Unknown device IDs (requests to devices that do not exist) are not kept.
        if per_device.len() < 50_000 || per_device.contains_key(device) {
            let entry = per_device.entry(device.to_string()).or_default();
            entry.requests += traffic.requests;
            entry.request_bytes += traffic.request_bytes;
            entry.response_bytes += traffic.response_bytes;
            entry.errors += traffic.errors;
        }
    }

    /// A WebSocket message relayed between a browser and a device.
    pub(crate) fn ws_frame(&self, bytes: usize) {
        self.ws_frames.fetch_add(1, Ordering::Relaxed);
        self.ws_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub(crate) fn login_failed(&self) {
        self.login_failed.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn push(&self, outcome: &str) {
        match outcome {
            "sent" => &self.push_sent,
            "expired" => &self.push_expired,
            _ => &self.push_failed,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn connected(&self, device: &str, connection: Uuid) {
        self.queue(ConnectionEvent::Open {
            device: device.to_string(),
            connection,
            at: now_ms(),
        });
    }

    pub(crate) fn disconnected(&self, connection: Uuid, reason: EndReason) {
        self.queue(ConnectionEvent::Close {
            connection,
            at: now_ms(),
            reason,
        });
    }

    fn queue(&self, event: ConnectionEvent) {
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if events.len() < MAX_QUEUED_EVENTS {
            events.push(event);
        }
    }

    pub(crate) fn latest_load(&self) -> Option<RelayLoad> {
        *self.latest_load.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn day_of(ms: i64) -> String {
    super::admin::day_of(ms)
}

/// One-minute samples for the next write.
struct Sample {
    minute: i64,
    gauges: Vec<(&'static str, String, f64)>,
    counters: Vec<(&'static str, String, f64)>,
    traffic: HashMap<String, Traffic>,
    events: Vec<ConnectionEvent>,
    online: Vec<String>,
}

/// Messages waiting in a socket's send queue.
fn queued(tx: &tokio::sync::mpsc::Sender<String>) -> usize {
    tx.max_capacity().saturating_sub(tx.capacity())
}

async fn collect(state: &AppState, minute: i64) -> Sample {
    let metrics = &state.metrics;
    let (online, tunnel_queue, tunnel_queue_max, queue_capacity) = {
        let sockets = state.sockets.read().await;
        let depths: Vec<usize> = sockets.values().map(|socket| queued(&socket.tx)).collect();
        (
            sockets.keys().cloned().collect::<Vec<String>>(),
            depths.iter().sum::<usize>(),
            depths.iter().copied().max().unwrap_or(0),
            sockets
                .values()
                .next()
                .map(|socket| socket.tx.max_capacity())
                .unwrap_or(0),
        )
    };
    let (browsers, browser_queue) = {
        let clients = state.clients.read().await;
        (
            clients.len(),
            clients
                .values()
                .map(|client| queued(&client.tx))
                .sum::<usize>(),
        )
    };
    let pending = state
        .pending
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .len();
    let (_, preview_sessions, preview_streams) = state.preview.usage();
    let store = state.store.conn.take_sample();
    let loop_lag_ms = metrics.loop_lag_us.swap(0, Ordering::Relaxed) as f64 / 1000.0;
    *metrics
        .latest_load
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(RelayLoad {
        preview_streams,
        tunnel_queue,
        tunnel_queue_max,
        browser_queue,
        queue_capacity,
        store,
        loop_lag_ms,
    });
    let mut gauges = vec![
        ("up", String::new(), 1.0),
        ("online_devices", String::new(), online.len() as f64),
        ("browser_sockets", String::new(), browsers as f64),
        ("pending_requests", String::new(), pending as f64),
        ("preview_sessions", String::new(), preview_sessions as f64),
        ("preview_streams", String::new(), preview_streams as f64),
        ("tunnel_queue", String::new(), tunnel_queue as f64),
        ("tunnel_queue_max", String::new(), tunnel_queue_max as f64),
        ("browser_queue", String::new(), browser_queue as f64),
        ("store_wait_max_ms", String::new(), store.max_wait_ms),
        ("store_hold_max_ms", String::new(), store.max_hold_ms),
        // The share of the minute the store lock was held.
        (
            "store_busy",
            String::new(),
            (store.hold_ms / MINUTE_MS as f64).min(1.0),
        ),
        ("loop_lag_ms", String::new(), loop_lag_ms),
    ];
    // The host: reading a few small files, outside the store lock.
    let host = metrics.host.sample(state.store.data_dir(), minute);
    let mut gauge = |name: &'static str, value: Option<f64>| {
        if let Some(value) = value {
            gauges.push((name, String::new(), value));
        }
    };
    let bytes = |value: Option<u64>| value.map(|value| value as f64);
    gauge("proc_rss", bytes(host.rss_bytes));
    gauge("proc_vm", bytes(host.vm_bytes));
    gauge("proc_threads", bytes(host.threads));
    gauge("proc_fds", bytes(host.open_files));
    gauge("proc_cpu", host.process_cpu_cores);
    gauge("host_cpu", host.cpu);
    gauge("host_load1", host.load.map(|load| load[0]));
    gauge("host_load5", host.load.map(|load| load[1]));
    gauge("host_mem_used", bytes(host.memory_used()));
    gauge("host_mem_total", bytes(host.memory_total));
    gauge("host_swap_used", bytes(host.swap_used()));
    gauge("cgroup_mem", bytes(host.cgroup_memory));
    gauge("cgroup_mem_max", bytes(host.cgroup_memory_limit));
    gauge("disk_used", bytes(host.disk_used));
    gauge("disk_available", bytes(host.disk_available));
    gauge("net_rx", host.net_rx_per_second);
    gauge("net_tx", host.net_tx_per_second);
    let mut counters: Vec<(&'static str, String, f64)> = [
        ("requests", take(&metrics.requests)),
        ("request_bytes", take(&metrics.request_bytes)),
        ("response_bytes", take(&metrics.response_bytes)),
        ("forward_offline", take(&metrics.offline)),
        ("forward_timeout", take(&metrics.timeout)),
        ("forward_busy", take(&metrics.busy)),
        ("device_errors", take(&metrics.device_errors)),
        ("ws_frames", take(&metrics.ws_frames)),
        ("ws_bytes", take(&metrics.ws_bytes)),
        ("login_failed", take(&metrics.login_failed)),
        ("push_sent", take(&metrics.push_sent)),
        ("push_failed", take(&metrics.push_failed)),
        ("push_expired", take(&metrics.push_expired)),
    ]
    .into_iter()
    .filter(|(_, value)| *value > 0)
    .map(|(name, value)| (name, String::new(), value as f64))
    .collect();
    // Time callers waited for the store lock and held it this minute: held
    // 30,000 ms of 60,000 is a store busy half the time.
    counters.push(("store_wait_ms", String::new(), store.wait_ms));
    counters.push(("store_hold_ms", String::new(), store.hold_ms));
    counters.push(("store_locks", String::new(), store.acquired as f64));
    for (index, counter) in metrics.latency.iter().enumerate() {
        let value = take(counter);
        if value > 0 {
            let label = LATENCY_BOUNDS
                .get(index)
                .map(u64::to_string)
                .unwrap_or_else(|| "inf".into());
            counters.push(("latency", label, value as f64));
        }
    }
    for (kind, value) in state.admission.take_refusals() {
        counters.push(("rate_limited", kind, value as f64));
    }
    let traffic =
        std::mem::take(&mut *metrics.per_device.lock().unwrap_or_else(|e| e.into_inner()));
    let events = std::mem::take(&mut *metrics.events.lock().unwrap_or_else(|e| e.into_inner()));
    let reconnects = events
        .iter()
        .filter(|event| matches!(event, ConnectionEvent::Open { .. }))
        .count();
    if reconnects > 0 {
        counters.push(("connects", String::new(), reconnects as f64));
    }
    gauges.retain(|(_, _, value)| value.is_finite());
    Sample {
        minute,
        gauges,
        counters,
        traffic,
        events,
        online,
    }
}

fn upsert(
    tx: &Connection,
    resolution: &str,
    bucket: i64,
    metric: &str,
    label: &str,
    value: f64,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO relay_metrics(resolution,bucket,metric,label,value,peak,samples) VALUES (?1,?2,?3,?4,?5,?5,1)
         ON CONFLICT(resolution,metric,label,bucket) DO UPDATE SET
           value=value+excluded.value, peak=max(peak,excluded.peak), samples=samples+1",
        params![resolution, bucket, metric, label, value],
    )?;
    Ok(())
}

/// Writes queued tunnel connection events. Rows for devices deleted since
/// are skipped; a close only ends a connection that is still open.
pub(crate) fn flush_connections(state: &AppState, conn: &Connection) {
    let events = std::mem::take(
        &mut *state
            .metrics
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
    );
    if events.is_empty() {
        return;
    }
    if let Err(error) = write_events(conn, &events) {
        tracing::warn!(%error, "could not record tunnel connections");
    }
}

fn write_events(conn: &Connection, events: &[ConnectionEvent]) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    write_event_rows(&tx, events)?;
    tx.commit()
}

fn write_event_rows(tx: &Connection, events: &[ConnectionEvent]) -> rusqlite::Result<()> {
    for event in events {
        match event {
            ConnectionEvent::Open {
                device,
                connection,
                at,
            } => {
                tx.execute(
                    "INSERT OR IGNORE INTO relay_device_connections(device_id,connection_id,connected_at)
                     SELECT ?1,?2,?3 WHERE EXISTS(SELECT 1 FROM relay_devices WHERE id=?1)",
                    params![device, connection.to_string(), at],
                )?;
            }
            ConnectionEvent::Close {
                connection,
                at,
                reason,
            } => {
                tx.execute(
                    "UPDATE relay_device_connections SET ended_at=?2,reason=?3 WHERE connection_id=?1 AND ended_at IS NULL",
                    params![connection.to_string(), at, reason.key()],
                )?;
            }
        }
    }
    Ok(())
}

fn write(state: &AppState, conn: &Connection, sample: &Sample) -> rusqlite::Result<()> {
    let hour = sample.minute - sample.minute.rem_euclid(HOUR_MS);
    let day = sample.minute - sample.minute.rem_euclid(DAY_MS);
    let today = day_of(sample.minute);
    let tx = conn.unchecked_transaction()?;
    for (metric, label, value) in sample.gauges.iter().chain(sample.counters.iter()) {
        upsert(&tx, "m", sample.minute, metric, label, *value)?;
        upsert(&tx, "h", hour, metric, label, *value)?;
        upsert(&tx, "d", day, metric, label, *value)?;
    }
    for (table, column, conflict, bucket) in [
        ("relay_device_traffic", "hour", "device_id,hour", hour),
        ("relay_device_traffic_daily", "day", "day,device_id", day),
    ] {
        let mut stmt = tx.prepare_cached(&format!(
            "INSERT INTO {table}(device_id,{column},requests,request_bytes,response_bytes,errors)
             SELECT ?1,?2,?3,?4,?5,?6 WHERE EXISTS(SELECT 1 FROM relay_devices WHERE id=?1)
             ON CONFLICT({conflict}) DO UPDATE SET requests=requests+excluded.requests,
               request_bytes=request_bytes+excluded.request_bytes,
               response_bytes=response_bytes+excluded.response_bytes, errors=errors+excluded.errors"
        ))?;
        for (device, traffic) in &sample.traffic {
            stmt.execute(params![
                device,
                bucket,
                traffic.requests as i64,
                traffic.request_bytes as i64,
                traffic.response_bytes as i64,
                traffic.errors as i64
            ])?;
        }
    }
    // Online devices count as active today, once per device and day.
    {
        let mut active = state
            .metrics
            .active_devices
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if active.0 != today {
            *active = (today.clone(), HashSet::new());
        }
        let new: Vec<&String> = sample
            .online
            .iter()
            .filter(|device| active.1.insert((*device).clone()))
            .collect();
        if !new.is_empty() {
            // One statement for every device new today (thousands at midnight).
            tx.execute(
                "INSERT OR IGNORE INTO relay_daily_active(day,kind,subject_id)
                 SELECT ?1,'device',j.value FROM json_each(?2) j
                 WHERE EXISTS(SELECT 1 FROM relay_devices WHERE id=j.value)",
                params![
                    today,
                    serde_json::to_string(&new).unwrap_or_else(|_| "[]".into())
                ],
            )?;
        }
    }
    // Online devices per reported version, for the version history: one
    // statement for every online device.
    let by_version: Vec<(String, i64)> = if sample.online.is_empty() {
        Vec::new()
    } else {
        let mut stmt = tx.prepare_cached(
            "SELECT version,COUNT(*) FROM relay_device_versions
             WHERE device_id IN (SELECT value FROM json_each(?1)) AND version IS NOT NULL GROUP BY version",
        )?;
        let online = serde_json::to_string(&sample.online).unwrap_or_else(|_| "[]".into());
        let rows = stmt.query_map([online], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (version, count) in by_version {
        upsert(&tx, "d", day, "online_by_version", &version, count as f64)?;
    }
    if let Some((main, wal)) = database_bytes(&tx) {
        for (metric, value) in [
            ("db_bytes", main + wal),
            ("db_main_bytes", main),
            ("db_wal_bytes", wal),
        ] {
            upsert(&tx, "m", sample.minute, metric, "", value)?;
            upsert(&tx, "h", hour, metric, "", value)?;
            upsert(&tx, "d", day, metric, "", value)?;
        }
    }
    write_event_rows(&tx, &sample.events)?;
    tx.execute(
        "INSERT INTO relay_settings(key,value) VALUES ('metricsStartedAt',?1) ON CONFLICT(key) DO NOTHING",
        [sample.minute.to_string()],
    )?;
    tx.execute(
        "INSERT INTO relay_settings(key,value) VALUES ('metricsLastSampleAt',?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [now_ms().to_string()],
    )?;
    tx.commit()
}

/// The database's pages and its write-ahead log, in bytes.
fn database_bytes(conn: &Connection) -> Option<(f64, f64)> {
    let pages: i64 = conn
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .ok()?;
    let size: i64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .ok()?;
    let wal = conn
        .path()
        .and_then(|path| std::fs::metadata(format!("{path}-wal")).ok())
        .map(|meta| meta.len() as f64)
        .unwrap_or(0.0);
    Some(((pages * size) as f64, wal))
}

pub(crate) fn setting_days(conn: &Connection, key: &str, allowed: &[i64], default: i64) -> i64 {
    relay_setting(conn, key)
        .and_then(|value| value.parse().ok())
        .filter(|days| allowed.contains(days))
        .unwrap_or(default)
}

pub(crate) fn metrics_days(conn: &Connection) -> i64 {
    setting_days(
        conn,
        "metricsRetentionDays",
        &METRICS_DAYS,
        DEFAULT_METRICS_DAYS,
    )
}

pub(crate) fn security_days(conn: &Connection) -> i64 {
    setting_days(
        conn,
        "securityRetentionDays",
        &SECURITY_DAYS,
        DEFAULT_SECURITY_DAYS,
    )
}

/// Deletes what is past its retention and caps the log tables.
pub(crate) fn prune(conn: &Connection) -> rusqlite::Result<()> {
    let now = now_ms();
    let metrics_cutoff = now - metrics_days(conn) * DAY_MS;
    let security_cutoff = now - security_days(conn) * DAY_MS;
    conn.execute(
        "DELETE FROM relay_metrics WHERE resolution='m' AND bucket<?1",
        [now - MINUTE_RETENTION_MS],
    )?;
    conn.execute(
        "DELETE FROM relay_metrics WHERE resolution IN ('h','d') AND bucket<?1",
        [metrics_cutoff],
    )?;
    // Hours of device traffic serve the last month; days the whole retention.
    conn.execute(
        "DELETE FROM relay_device_traffic WHERE hour<?1",
        [metrics_cutoff.max(now - DEVICE_TRAFFIC_HOURS_DAYS * DAY_MS)],
    )?;
    conn.execute(
        "DELETE FROM relay_device_traffic_daily WHERE day<?1",
        [metrics_cutoff],
    )?;
    conn.execute(
        "DELETE FROM relay_daily_active WHERE day<?1",
        [day_of(metrics_cutoff)],
    )?;
    conn.execute(
        "DELETE FROM relay_device_connections WHERE connected_at<?1 AND ended_at IS NOT NULL",
        [now - CONNECTION_RETENTION_DAYS * DAY_MS],
    )?;
    conn.execute(
        "DELETE FROM relay_device_connections WHERE id<=(SELECT id FROM relay_device_connections ORDER BY id DESC LIMIT 1 OFFSET ?1)",
        [CONNECTION_MAX_ROWS],
    )?;
    conn.execute(
        "DELETE FROM relay_security_events WHERE created_at<?1",
        [security_cutoff],
    )?;
    conn.execute(
        "DELETE FROM relay_security_events WHERE id<=(SELECT id FROM relay_security_events ORDER BY id DESC LIMIT 1 OFFSET ?1)",
        [SECURITY_MAX_EVENTS],
    )?;
    conn.execute(
        "DELETE FROM relay_admin_actions WHERE created_at<?1",
        [now - ADMIN_ACTION_RETENTION_DAYS * DAY_MS],
    )?;
    conn.execute(
        "DELETE FROM relay_admin_actions WHERE id<=(SELECT id FROM relay_admin_actions ORDER BY id DESC LIMIT 1 OFFSET ?1)",
        [ADMIN_ACTION_MAX_ROWS],
    )?;
    Ok(())
}

/// Takes and writes one sample. Exposed for tests; the sampler calls it once a
/// minute.
pub(crate) async fn sample(state: &Arc<AppState>, minute: i64) {
    let sample = collect(state, minute).await;
    let online = sample.online.clone();
    {
        let conn = state.store.conn.lock().await;
        if let Err(error) = write(state, &conn, &sample) {
            tracing::warn!(%error, "could not record relay metrics");
        }
    }
    probe_versions(state, &online);
}

/// Reads `/healthz` of online devices whose version is unknown for this
/// connection or older than six hours. Older devices answer it too; a device
/// that does not report a version is stored without one.
fn probe_versions(state: &Arc<AppState>, online: &[String]) {
    let due: Vec<String> = {
        let Ok(sockets) = state.sockets.try_read() else {
            return;
        };
        let mut probes = state
            .metrics
            .probes
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        probes.retain(|device, _| sockets.contains_key(device));
        let mut due = Vec::new();
        for device in online {
            if due.len() >= VERSION_PROBES_PER_MINUTE {
                break;
            }
            let Some(socket) = sockets.get(device) else {
                continue;
            };
            let fresh = probes.get(device).is_some_and(|(connection, at)| {
                *connection == socket.connection_id && at.elapsed() < VERSION_PROBE_INTERVAL
            });
            if !fresh {
                probes.insert(
                    device.clone(),
                    (socket.connection_id, std::time::Instant::now()),
                );
                due.push(device.clone());
            }
        }
        due
    };
    for device in due {
        let state = state.clone();
        tokio::spawn(async move {
            let response = forward_device_with_timeout(
                state.clone(),
                device.clone(),
                "GET".into(),
                "/healthz".into(),
                None,
                None,
                json!({}),
                Duration::from_secs(10),
            )
            .await;
            if !response.status().is_success() {
                return;
            }
            let Ok(bytes) = to_bytes(response.into_body(), 64 * 1024).await else {
                return;
            };
            let version = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|value| {
                    value["runningVersion"]
                        .as_str()
                        .or_else(|| value["version"].as_str())
                        .map(str::to_string)
                })
                .filter(|version| {
                    version.len() <= 64
                        && version
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
                });
            let conn = state.store.conn.lock().await;
            let _ = conn.execute(
                "INSERT INTO relay_device_versions(device_id,version,checked_at)
                 SELECT ?1,?2,?3 WHERE EXISTS(SELECT 1 FROM relay_devices WHERE id=?1)
                 ON CONFLICT(device_id) DO UPDATE SET version=excluded.version,checked_at=excluded.checked_at",
                params![device, version, now_ms()],
            );
        });
    }
}

/// Starts the once-a-minute sampler and closes tunnel connections a previous
/// process left open (at its last sample, when known).
pub(crate) fn start(state: &Arc<AppState>) -> Result<()> {
    {
        let conn = state
            .store
            .conn
            .try_lock()
            .map_err(|_| anyhow::anyhow!("relay store is busy at startup"))?;
        let ended = relay_setting(&conn, "metricsLastSampleAt")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or_else(now_ms);
        conn.execute(
            "UPDATE relay_device_connections SET ended_at=max(connected_at,?1),reason='relay_stopped' WHERE ended_at IS NULL",
            [ended],
        )?;
        prune(&conn)?;
    }
    // How late the runtime wakes a timer: ready work waiting for a thread
    // (a blocked or saturated runtime) shows here before anywhere else.
    let lag = Arc::downgrade(state);
    tokio::spawn(async move {
        const TICK: Duration = Duration::from_millis(500);
        loop {
            let asked = std::time::Instant::now();
            tokio::time::sleep(TICK).await;
            let late = asked.elapsed().saturating_sub(TICK).as_micros() as u64;
            let Some(state) = lag.upgrade() else { break };
            state.metrics.loop_lag_us.fetch_max(late, Ordering::Relaxed);
        }
    });
    let weak = Arc::downgrade(state);
    tokio::spawn(async move {
        let mut samples = 0_u64;
        loop {
            let now = now_ms();
            let next = now - now.rem_euclid(MINUTE_MS) + MINUTE_MS;
            tokio::time::sleep(Duration::from_millis((next - now) as u64 + 50)).await;
            let Some(state) = weak.upgrade() else { break };
            sample(&state, next).await;
            samples += 1;
            if samples % 60 == 0 {
                let conn = state.store.conn.lock().await;
                if let Err(error) = prune(&conn) {
                    tracing::warn!(%error, "could not prune relay statistics");
                }
            }
        }
    });
    Ok(())
}

// Per-account activity: `last_seen_at` is written at most once a minute and
// the day's active row once a day, instead of on every request. Keyed by the
// database file so separate stores (tests) do not share state.
/// (database path, user ID) -> (last write in ms, day of the last daily row).
type ActivityMap = HashMap<(String, String), (i64, String)>;
static ACTIVITY: StdMutex<Option<ActivityMap>> = StdMutex::new(None);

pub(crate) fn note_user_activity(conn: &Connection, user_id: &str) {
    let now = now_ms();
    let today = day_of(now);
    let key = (
        conn.path().unwrap_or_default().to_string(),
        user_id.to_string(),
    );
    let (write_seen, write_day) = {
        let mut guard = ACTIVITY.lock().unwrap_or_else(|e| e.into_inner());
        let map = guard.get_or_insert_with(HashMap::new);
        if map.len() > 100_000 {
            map.clear();
        }
        let entry = map.entry(key).or_insert((i64::MIN, String::new()));
        let write_seen = now.saturating_sub(entry.0) >= MINUTE_MS;
        let write_day = entry.1 != today;
        if write_seen {
            entry.0 = now;
        }
        if write_day {
            entry.1 = today.clone();
        }
        (write_seen, write_day)
    };
    if write_seen {
        let _ = conn.execute(
            "UPDATE relay_users SET last_seen_at=?1 WHERE id=?2",
            params![now_rfc3339(), user_id],
        );
    }
    if write_day {
        let _ = conn.execute(
            "INSERT OR IGNORE INTO relay_daily_active(day,kind,subject_id) VALUES (?1,'user',?2)",
            params![today, user_id],
        );
    }
}

// ── Reads for the admin console ──────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agg {
    /// A gauge: the mean of its samples.
    Mean,
    /// A gauge: the largest sample.
    Peak,
    /// A counter: the sum.
    Sum,
}

pub(crate) fn history_start(conn: &Connection) -> Option<i64> {
    relay_setting(conn, "metricsStartedAt").and_then(|value| value.parse().ok())
}

/// Metrics recorded per label (latency bucket, refusal kind, version); every
/// other metric has the empty label.
fn labelled(metric: &str) -> bool {
    matches!(metric, "latency" | "rate_limited" | "online_by_version")
}

/// The rows a series of `step` buckets reads: minutes below an hour, days
/// for whole-day steps, hours otherwise.
fn resolution_for(step: i64) -> &'static str {
    if step < HOUR_MS {
        "m"
    } else if step % DAY_MS == 0 {
        "d"
    } else {
        "h"
    }
}

/// A metric over `[start, end)` in `step` buckets. Buckets the relay did not
/// sample (it was not running, or recording had not started) are null, not
/// zero; a counter in a sampled bucket without rows is zero.
pub(crate) fn series(
    conn: &Connection,
    metric: &str,
    label: Option<&str>,
    start: i64,
    end: i64,
    step: i64,
    agg: Agg,
) -> Vec<(i64, Option<f64>)> {
    let resolution = resolution_for(step);
    let start = start - start.rem_euclid(step);
    let buckets = ((end - start + step - 1) / step).max(1) as usize;
    let read = |name: &str| -> Vec<(i64, f64, f64, i64)> {
        // Name the label whenever it is known, so the read seeks the primary
        // key (resolution, metric, label, bucket) instead of scanning every
        // metric of the buckets.
        let sql = format!(
            "SELECT bucket,value,peak,samples FROM relay_metrics
             WHERE resolution=?1 AND metric=?2 AND bucket>=?3 AND bucket<?4{}",
            if label.is_some() && name == metric {
                " AND label=?5"
            } else if !labelled(name) {
                " AND label=''"
            } else {
                ""
            }
        );
        conn.prepare_cached(&sql)
            .and_then(|mut stmt| {
                let map = |row: &rusqlite::Row<'_>| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                };
                match label.filter(|_| name == metric) {
                    Some(label) => stmt
                        .query_map(params![resolution, name, start, end, label], map)?
                        .collect(),
                    None => stmt
                        .query_map(params![resolution, name, start, end], map)?
                        .collect(),
                }
            })
            .unwrap_or_default()
    };
    let mut covered = vec![false; buckets];
    for (bucket, _, _, _) in read("up") {
        let index = ((bucket - start) / step) as usize;
        if index < buckets {
            covered[index] = true;
        }
    }
    let mut sums = vec![(0.0_f64, 0_i64, f64::NEG_INFINITY); buckets];
    for (bucket, value, peak, samples) in read(metric) {
        let index = ((bucket - start) / step) as usize;
        if index < buckets {
            sums[index].0 += value;
            sums[index].1 += samples;
            sums[index].2 = sums[index].2.max(peak);
        }
    }
    (0..buckets)
        .map(|index| {
            let at = start + index as i64 * step;
            let (value, samples, peak) = sums[index];
            let point = if !covered[index] && samples == 0 {
                None
            } else {
                match agg {
                    Agg::Sum => Some(value),
                    Agg::Mean if samples > 0 => Some(value / samples as f64),
                    Agg::Peak if samples > 0 => Some(peak),
                    // A gauge the relay did not record in a sampled bucket
                    // (a host figure this system does not offer) is unknown.
                    Agg::Mean | Agg::Peak => None,
                }
            };
            (at, point)
        })
        .collect()
}

pub(crate) fn series_json(points: &[(i64, Option<f64>)], step: i64, history: Option<i64>) -> Value {
    json!({
        "resolutionSeconds": step / 1000,
        "points": points.iter().map(|(at, value)| json!([at, value])).collect::<Vec<_>>(),
        "historyStart": history.map(super::admin::rfc3339_from_ms),
    })
}

/// The rows that cover `[start, end)`: minutes while they are kept, else
/// whole days from day rows with the hours at either edge from hour rows
/// (to the hour, as before day rows existed).
fn segments(start: i64, end: i64) -> Vec<(&'static str, i64, i64)> {
    if start >= now_ms() - MINUTE_RETENTION_MS {
        return vec![("m", start, end)];
    }
    let start = start - start.rem_euclid(HOUR_MS);
    let first_day = start + (DAY_MS - start.rem_euclid(DAY_MS)) % DAY_MS;
    let last_day = end - end.rem_euclid(DAY_MS);
    if last_day - first_day < 2 * DAY_MS {
        return vec![("h", start, end)];
    }
    vec![
        ("h", start, first_day),
        ("d", first_day, last_day),
        ("h", last_day, end),
    ]
}

/// The sum of a counter over a time window.
pub(crate) fn total(conn: &Connection, metric: &str, start: i64, end: i64) -> f64 {
    segments(start, end)
        .into_iter()
        .filter(|(_, from, to)| from < to)
        .map(|(resolution, from, to)| {
            conn.prepare_cached(if labelled(metric) {
                "SELECT COALESCE(SUM(value),0) FROM relay_metrics WHERE resolution=?1 AND metric=?2 AND bucket>=?3 AND bucket<?4"
            } else {
                "SELECT COALESCE(SUM(value),0) FROM relay_metrics WHERE resolution=?1 AND metric=?2 AND label='' AND bucket>=?3 AND bucket<?4"
            })
            .and_then(|mut stmt| stmt.query_row(params![resolution, metric, from, to], |row| row.get::<_, f64>(0)))
            .unwrap_or(0.0)
        })
        .sum()
}

/// Counter totals per label over a window (rate limits by kind), largest first.
pub(crate) fn totals_by_label(
    conn: &Connection,
    metric: &str,
    start: i64,
    end: i64,
) -> Vec<(String, f64)> {
    let mut totals: Vec<(String, f64)> = Vec::new();
    for (resolution, from, to) in segments(start, end) {
        if from >= to {
            continue;
        }
        let rows: Vec<(String, f64)> = conn
            .prepare_cached(
                "SELECT label,SUM(value) FROM relay_metrics WHERE resolution=?1 AND metric=?2 AND bucket>=?3 AND bucket<?4 GROUP BY label",
            )
            .and_then(|mut stmt| {
                stmt.query_map(params![resolution, metric, from, to], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect()
            })
            .unwrap_or_default();
        for (label, value) in rows {
            match totals.iter_mut().find(|(known, _)| *known == label) {
                Some(entry) => entry.1 += value,
                None => totals.push((label, value)),
            }
        }
    }
    totals.sort_by(|a, b| b.1.total_cmp(&a.1));
    totals
}

/// The 95th percentile of forwarding latency (ms) over a window, from the
/// histogram buckets; None without requests.
pub(crate) fn latency_p95(conn: &Connection, start: i64, end: i64) -> Option<f64> {
    let counts = totals_by_label(conn, "latency", start, end);
    percentile(&counts, 0.95)
}

/// The upper bound of a histogram label; the open bucket (`inf`) is infinite.
fn bound_of(label: &str) -> f64 {
    label.parse::<f64>().unwrap_or(f64::INFINITY)
}

/// A quantile of a latency histogram (labels are bucket upper bounds in ms).
/// The value is interpolated linearly inside the bucket that holds it, from
/// the bucket below's bound to its own; in the open bucket it is the last
/// finite bound (a floor: "at least"). None without samples.
pub(crate) fn percentile(counts: &[(String, f64)], quantile: f64) -> Option<f64> {
    let mut buckets: Vec<(f64, f64)> = Vec::new();
    for (label, count) in counts {
        let bound = bound_of(label);
        match buckets.iter_mut().find(|(b, _)| *b == bound) {
            Some(entry) => entry.1 += count,
            None => buckets.push((bound, *count)),
        }
    }
    buckets.sort_by(|a, b| a.0.total_cmp(&b.0));
    let all: f64 = buckets.iter().map(|(_, count)| count).sum();
    if all <= 0.0 {
        return None;
    }
    // A bucket starts at the next lower bound any relay version records, not
    // at the previous bucket that happens to hold samples.
    let lower_of = |bound: f64| -> f64 {
        LATENCY_BOUNDS
            .iter()
            .chain(LEGACY_LATENCY_BOUNDS.iter())
            .map(|b| *b as f64)
            .chain(buckets.iter().map(|(b, _)| *b))
            .filter(|b| b.is_finite() && *b < bound)
            .fold(0.0, f64::max)
    };
    let rank = all * quantile;
    let mut seen = 0.0;
    for (bound, count) in &buckets {
        if *count > 0.0 && seen + count >= rank {
            let lower = lower_of(*bound);
            if !bound.is_finite() {
                return Some(lower);
            }
            let within = ((rank - seen) / count).clamp(0.0, 1.0);
            return Some(lower + (bound - lower) * within);
        }
        seen += count;
    }
    buckets.last().map(|(bound, _)| lower_of(*bound))
}

/// Latency histogram rows per series bucket.
fn latency_rows(conn: &Connection, start: i64, end: i64, step: i64) -> Vec<Vec<(String, f64)>> {
    let resolution = resolution_for(step);
    let buckets = ((end - start + step - 1) / step).max(1) as usize;
    let mut per: Vec<Vec<(String, f64)>> = vec![Vec::new(); buckets];
    if let Ok(mut stmt) = conn.prepare(
        "SELECT bucket,label,value FROM relay_metrics WHERE resolution=?1 AND metric='latency' AND bucket>=?2 AND bucket<?3",
    ) {
        if let Ok(rows) = stmt.query_map(params![resolution, start, end], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, f64>(2)?))
        }) {
            for (bucket, label, value) in rows.flatten() {
                let index = ((bucket - start) / step) as usize;
                if index < buckets {
                    per[index].push((label, value));
                }
            }
        }
    }
    per
}

/// p95 latency per bucket of a series.
pub(crate) fn latency_series(
    conn: &Connection,
    start: i64,
    end: i64,
    step: i64,
) -> Vec<(i64, Option<f64>)> {
    latency_quantile_series(conn, start, end, step, &[0.95])
        .pop()
        .unwrap_or_default()
}

/// Several latency quantiles per bucket of a series, one list per quantile.
pub(crate) fn latency_quantile_series(
    conn: &Connection,
    start: i64,
    end: i64,
    step: i64,
    quantiles: &[f64],
) -> Vec<Vec<(i64, Option<f64>)>> {
    let start = start - start.rem_euclid(step);
    let rows = latency_rows(conn, start, end, step);
    quantiles
        .iter()
        .map(|quantile| {
            rows.iter()
                .enumerate()
                .map(|(index, counts)| (start + index as i64 * step, percentile(counts, *quantile)))
                .collect()
        })
        .collect()
}

/// The latency histogram over a window with every bound the relay records
/// (zero where nothing fell), plus labels older relays recorded.
pub(crate) fn latency_histogram(conn: &Connection, start: i64, end: i64) -> Vec<(f64, f64)> {
    let mut buckets: Vec<(f64, f64)> = LATENCY_BOUNDS
        .iter()
        .map(|bound| (*bound as f64, 0.0))
        .chain(std::iter::once((f64::INFINITY, 0.0)))
        .collect();
    for (label, count) in totals_by_label(conn, "latency", start, end) {
        let bound = bound_of(&label);
        match buckets.iter_mut().find(|(b, _)| *b == bound) {
            Some(entry) => entry.1 += count,
            None => buckets.push((bound, count)),
        }
    }
    buckets.sort_by(|a, b| a.0.total_cmp(&b.0));
    buckets
}

/// Tunnel connections per device over a window, newest last.
pub(crate) struct Span {
    pub(crate) from: i64,
    pub(crate) to: Option<i64>,
    pub(crate) reason: Option<String>,
}

pub(crate) fn connection_spans(conn: &Connection, device: &str, start: i64) -> Vec<Span> {
    conn.prepare(
        "SELECT connected_at,ended_at,reason FROM relay_device_connections
         WHERE device_id=?1 AND (ended_at IS NULL OR ended_at>?2) ORDER BY connected_at",
    )
    .and_then(|mut stmt| {
        stmt.query_map(params![device, start], |row| {
            Ok(Span {
                from: row.get(0)?,
                to: row.get(1)?,
                reason: row.get(2)?,
            })
        })?
        .collect()
    })
    .unwrap_or_default()
}

/// Connections opened per device since `start` (reconnects).
pub(crate) fn connects_since(conn: &Connection, start: i64) -> HashMap<String, i64> {
    conn.prepare(
        "SELECT device_id,COUNT(*) FROM relay_device_connections WHERE connected_at>=?1 GROUP BY device_id",
    )
    .and_then(|mut stmt| {
        stmt.query_map([start], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect()
    })
    .unwrap_or_default()
}

/// When each of these devices' last tunnel ended (or began, while open):
/// the newest connection of each, one index lookup per device.
pub(crate) fn last_seen_of(conn: &Connection, ids: &[String]) -> HashMap<String, i64> {
    let mut seen = HashMap::new();
    if let Ok(mut stmt) = conn.prepare_cached(
        "SELECT COALESCE(ended_at,connected_at) FROM relay_device_connections WHERE device_id=?1
         ORDER BY connected_at DESC LIMIT 1",
    ) {
        for id in ids {
            if let Ok(at) = stmt.query_row([id], |row| row.get::<_, i64>(0)) {
                seen.insert(id.clone(), at);
            }
        }
    }
    seen
}

/// Every reported version with its device count, and how many of the online
/// devices report each (one pass over the versions table).
pub(crate) fn version_totals(
    conn: &Connection,
    online: &std::collections::HashSet<&str>,
) -> Vec<(Option<String>, i64, i64)> {
    let mut counts: HashMap<Option<String>, (i64, i64)> = HashMap::new();
    let mut known_online = 0_i64;
    if let Ok(mut stmt) = conn.prepare("SELECT device_id,version FROM relay_device_versions") {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        }) {
            for (device, version) in rows.flatten() {
                let entry = counts.entry(version).or_default();
                entry.0 += 1;
                if online.contains(device.as_str()) {
                    entry.1 += 1;
                    known_online += 1;
                }
            }
        }
    }
    // Online devices the relay has not read yet.
    let unknown = online.len() as i64 - known_online;
    if unknown > 0 {
        let entry = counts.entry(None).or_default();
        entry.0 += unknown;
        entry.1 += unknown;
    }
    counts
        .into_iter()
        .map(|(version, (devices, online))| (version, devices, online))
        .collect()
}

pub(crate) fn outdated(version: Option<&str>) -> Option<bool> {
    version.map(|version| compare_versions(version, RELAY_VERSION) == std::cmp::Ordering::Less)
}

/// Forwarded requests per day for the last `days` days (oldest first) of
/// these devices, from the daily rows.
pub(crate) fn daily_requests_of(
    conn: &Connection,
    ids: &[String],
    days: i64,
) -> HashMap<String, Vec<i64>> {
    let now = now_ms();
    let first = now - now.rem_euclid(DAY_MS) - (days - 1) * DAY_MS;
    let mut result: HashMap<String, Vec<i64>> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare_cached(
        "SELECT day,requests FROM relay_device_traffic_daily WHERE device_id=?1 AND day>=?2",
    ) {
        for id in ids {
            let mut values = vec![0; days as usize];
            if let Ok(rows) = stmt.query_map(params![id, first], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            }) {
                for (day, requests) in rows.flatten() {
                    let index = ((day - first) / DAY_MS) as usize;
                    if index < values.len() {
                        values[index] += requests;
                    }
                }
            }
            result.insert(id.clone(), values);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_reads_the_bucket_holding_the_quantile() {
        let counts = vec![
            ("10".to_string(), 90.0),
            ("250".to_string(), 8.0),
            ("inf".to_string(), 2.0),
        ];
        // 95 of 100 falls 5/8 of the way through the 200–250 ms bucket.
        assert_eq!(percentile(&counts, 0.95), Some(231.25));
        let median = percentile(&counts, 0.5).unwrap();
        assert!((median - (5.0 + 5.0 * 50.0 / 90.0)).abs() < 1e-9);
        // In the open bucket the value is a floor: the last finite bound.
        assert_eq!(percentile(&counts, 0.99), Some(30_000.0));
        assert_eq!(percentile(&[], 0.95), None);
    }

    #[test]
    fn percentile_is_not_the_bucket_ceiling() {
        // Every answer took 600–1000 ms: p95 sits inside the bucket, not at 1000.
        let counts = vec![("1000".to_string(), 100.0)];
        let p95 = percentile(&counts, 0.95).unwrap();
        assert!(p95 > 800.0 && p95 < 1000.0, "{p95}");
        // Old (coarse) and new (fine) labels merge by bound.
        let mixed = vec![
            ("250".to_string(), 50.0),
            ("300".to_string(), 50.0),
            ("250".to_string(), 0.0),
        ];
        assert_eq!(percentile(&mixed, 0.5), Some(250.0));
        assert_eq!(percentile(&mixed, 1.0), Some(300.0));
    }
}
