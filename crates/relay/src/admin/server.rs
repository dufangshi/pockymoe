//! Server: whether the relay's host keeps up (docs/relay-admin.md "Server").
//! The relay process (memory, CPU, threads, open files), the machine (CPU,
//! load, memory, swap, network), the cgroup's limits, the disk holding the
//! data directory and the database, and the relay's own load (tunnels,
//! requests in flight, send queues, the store lock, the async runtime).
//!
//! Current values come from the latest minute sample; history from the
//! metrics store over the window. Each check has a status from fixed
//! thresholds; a figure the system does not offer is `unknown`, not a failure.
use super::common::*;
use super::*;
use crate::host::HostSample;
use crate::metrics::{self, Agg};

/// A check's status and the value it was judged on.
struct Check {
    key: &'static str,
    value: Option<f64>,
    warn: f64,
    critical: f64,
}

impl Check {
    fn status(&self) -> &'static str {
        match self.value {
            None => "unknown",
            Some(value) if value >= self.critical => "critical",
            Some(value) if value >= self.warn => "warn",
            Some(_) => "ok",
        }
    }
    fn json(&self) -> Value {
        json!({
            "key": self.key,
            "status": self.status(),
            "value": self.value,
            "warnAt": self.warn,
            "criticalAt": self.critical,
        })
    }
}

/// Thresholds (docs/relay-admin.md "Server"): shares are 0–1.
pub(super) const DISK_WARN: f64 = 0.85;
pub(super) const DISK_CRITICAL: f64 = 0.95;
pub(super) const MEMORY_WARN: f64 = 0.85;
pub(super) const MEMORY_CRITICAL: f64 = 0.95;
pub(super) const CPU_WARN: f64 = 0.80;
pub(super) const CPU_CRITICAL: f64 = 0.95;

fn share(part: Option<u64>, whole: Option<u64>) -> Option<f64> {
    match (part, whole) {
        (Some(part), Some(whole)) if whole > 0 => Some(part as f64 / whole as f64),
        _ => None,
    }
}

/// The mean (or peak) of a gauge over the last five minutes.
fn recent(conn: &Connection, metric: &str, agg: Agg) -> Option<f64> {
    let now = now_ms();
    let step = 5 * MINUTE_MS;
    let start = now - now.rem_euclid(MINUTE_MS) - step;
    // One bucket starting five whole minutes ago.
    metrics::series(conn, metric, None, start, start + step, step, agg)
        .first()
        .and_then(|(_, value)| *value)
}

pub(super) async fn server(
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let tunnels = state.sockets.read().await.len();
    let browsers = state.clients.read().await.len();
    let pending = state
        .pending
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .len();
    let (conn, _) = match admin_read(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    blocking(|| {
        let now = now_ms();
        let window = match chart_window(&query, Range::Day, now) {
            Ok(window) => window,
            Err(message) => return bad_request(message),
        };
        // The latest minute sample; right after a start, a reading without rates.
        let (sampled_at, host) = state.metrics.host.latest().unwrap_or_else(|| {
            (
                now,
                crate::host::HostProbe::default().sample(state.store.data_dir(), now),
            )
        });
        let load = state.metrics.latest_load();
        let (main_bytes, wal_bytes) = database_files(&state, &conn);
        let capacity = host.cpu_capacity();
        let started = metrics::history_start(&conn);
        let (start, end, step) = (window.start, window.end, window.step);
        let series =
            |metric: &str, agg: Agg| metrics::series(&conn, metric, None, start, end, step, agg);
        let json_series =
            |points: &[(i64, Option<f64>)]| metrics::series_json(points, step, started);
        let scaled =
            |points: Vec<(i64, Option<f64>)>, by: Option<f64>| -> Vec<(i64, Option<f64>)> {
                points
                    .into_iter()
                    .map(|(at, value)| {
                        (
                            at,
                            value
                                .zip(by)
                                .filter(|(_, by)| *by > 0.0)
                                .map(|(value, by)| value / by),
                        )
                    })
                    .collect()
            };

        // Checks over the last five minutes (sustained), or the latest reading.
        let process_cpu = recent(&conn, "proc_cpu", Agg::Mean)
            .or(host.process_cpu_cores)
            .zip(capacity)
            .filter(|(_, capacity)| *capacity > 0.0)
            .map(|(cores, capacity)| cores / capacity);
        let host_cpu = recent(&conn, "host_cpu", Agg::Mean).or(host.cpu);
        let (memory_used, memory_limit, memory_scope) = match host.cgroup_memory_limit {
            Some(limit) => (host.cgroup_memory, Some(limit), "cgroup"),
            None => (host.memory_used(), host.memory_total, "host"),
        };
        let disk = match (host.disk_used, host.disk_available) {
            (Some(used), Some(available)) if used + available > 0 => {
                Some(used as f64 / (used + available) as f64)
            }
            _ => None,
        };
        let store_busy = recent(&conn, "store_busy", Agg::Mean)
            .or(load.map(|load| (load.store.hold_ms / MINUTE_MS as f64).min(1.0)));
        let store_wait = recent(&conn, "store_wait_max_ms", Agg::Peak)
            .or(load.map(|load| load.store.max_wait_ms));
        let loop_lag =
            recent(&conn, "loop_lag_ms", Agg::Peak).or(load.map(|load| load.loop_lag_ms));
        let queue = load
            .filter(|load| load.queue_capacity > 0)
            .map(|load| load.tunnel_queue_max as f64 / load.queue_capacity as f64);
        let checks = [
            Check {
                key: "cpu",
                value: host_cpu,
                warn: CPU_WARN,
                critical: CPU_CRITICAL,
            },
            Check {
                key: "processCpu",
                value: process_cpu,
                warn: CPU_WARN,
                critical: CPU_CRITICAL,
            },
            Check {
                key: "load",
                value: host
                    .load
                    .zip(host.cores)
                    .filter(|(_, cores)| *cores > 0)
                    .map(|(load, cores)| load[1] / cores as f64),
                warn: 1.0,
                critical: 2.0,
            },
            Check {
                key: "memory",
                value: share(memory_used, memory_limit),
                warn: MEMORY_WARN,
                critical: MEMORY_CRITICAL,
            },
            Check {
                key: "swap",
                value: share(host.swap_used(), host.swap_total.filter(|total| *total > 0)),
                warn: 0.5,
                critical: 0.9,
            },
            Check {
                key: "disk",
                value: disk,
                warn: DISK_WARN,
                critical: DISK_CRITICAL,
            },
            Check {
                key: "openFiles",
                value: share(host.open_files, host.open_files_limit),
                warn: 0.8,
                critical: 0.95,
            },
            Check {
                key: "wal",
                value: wal_bytes.map(|bytes| bytes as f64),
                warn: 256e6,
                critical: 1e9,
            },
            Check {
                key: "storeBusy",
                value: store_busy,
                warn: 0.5,
                critical: 0.8,
            },
            Check {
                key: "storeWait",
                value: store_wait,
                warn: 1_000.0,
                critical: 5_000.0,
            },
            Check {
                key: "loopLag",
                value: loop_lag,
                warn: 200.0,
                critical: 1_000.0,
            },
            Check {
                key: "tunnelQueue",
                value: queue,
                warn: 0.5,
                critical: 0.9,
            },
        ];
        let worst = checks
            .iter()
            .map(Check::status)
            .max_by_key(|status| match *status {
                "critical" => 3,
                "warn" => 2,
                "ok" => 1,
                _ => 0,
            })
            .unwrap_or("unknown");

        let started_at = ms_from_rfc3339(&state.admin.started_at).unwrap_or(now);
        let body = json!({
            "platform": std::env::consts::OS,
            "sampledAt": rfc3339_from_ms(sampled_at),
            "status": worst,
            "window": window.json(),
            "historyStart": started.map(rfc3339_from_ms),
            "process": {
                "pid": host.pid,
                "startedAt": state.admin.started_at,
                "uptimeSeconds": (now - started_at).max(0) / 1000,
                "rssBytes": host.rss_bytes,
                "vmBytes": host.vm_bytes,
                "cpuCores": host.process_cpu_cores,
                "threads": host.threads,
                "openFiles": host.open_files,
                "openFilesLimit": host.open_files_limit,
            },
            "host": host_json(&host),
            "cgroup": host.cgroup.as_ref().map(|path| json!({
                "path": path,
                "memoryBytes": host.cgroup_memory,
                "memoryLimit": host.cgroup_memory_limit,
                "cpuLimitCores": host.cgroup_cpu_limit,
            })),
            "memoryScope": memory_scope,
            "cpuCapacityCores": capacity,
            "disk": {
                "path": state.store.data_dir().map(|dir| dir.display().to_string()),
                "totalBytes": host.disk_total,
                "usedBytes": host.disk_used,
                "availableBytes": host.disk_available,
                "databaseBytes": main_bytes,
                "walBytes": wal_bytes,
            },
            "relay": {
                "deviceTunnels": tunnels,
                "browserSockets": browsers,
                "pendingRequests": pending,
                "previewStreams": load.map(|load| load.preview_streams),
                "tunnelQueue": load.map(|load| load.tunnel_queue),
                "tunnelQueueMax": load.map(|load| load.tunnel_queue_max),
                "browserQueue": load.map(|load| load.browser_queue),
                "queueCapacity": load.map(|load| load.queue_capacity),
                "storeLocksPerMinute": load.map(|load| load.store.acquired),
                "storeBusy": load.map(|load| (load.store.hold_ms / MINUTE_MS as f64).min(1.0)),
                "storeWaitMaxMs": load.map(|load| load.store.max_wait_ms),
                "storeHoldMaxMs": load.map(|load| load.store.max_hold_ms),
                "loopLagMs": load.map(|load| load.loop_lag_ms),
            },
            "checks": checks.iter().map(Check::json).collect::<Vec<_>>(),
            "series": {
                "processCpu": json_series(&scaled(series("proc_cpu", Agg::Mean), capacity)),
                "hostCpu": json_series(&series("host_cpu", Agg::Mean)),
                "hostCpuPeak": json_series(&series("host_cpu", Agg::Peak)),
                "load1": json_series(&series("host_load1", Agg::Mean)),
                "processRss": json_series(&series("proc_rss", Agg::Mean)),
                "hostMemoryUsed": json_series(&series("host_mem_used", Agg::Mean)),
                "cgroupMemory": json_series(&series("cgroup_mem", Agg::Mean)),
                "swapUsed": json_series(&series("host_swap_used", Agg::Mean)),
                "diskUsed": json_series(&series("disk_used", Agg::Mean)),
                "databaseBytes": json_series(&series("db_main_bytes", Agg::Mean)),
                "walBytes": json_series(&series("db_wal_bytes", Agg::Peak)),
                "netRx": json_series(&series("net_rx", Agg::Mean)),
                "netTx": json_series(&series("net_tx", Agg::Mean)),
                "tunnels": json_series(&series("online_devices", Agg::Mean)),
                "browsers": json_series(&series("browser_sockets", Agg::Mean)),
                "pending": json_series(&series("pending_requests", Agg::Peak)),
                "tunnelQueue": json_series(&series("tunnel_queue_max", Agg::Peak)),
                "storeBusy": json_series(&series("store_busy", Agg::Mean)),
                "storeWaitMax": json_series(&series("store_wait_max_ms", Agg::Peak)),
                "loopLag": json_series(&series("loop_lag_ms", Agg::Peak)),
                "openFiles": json_series(&series("proc_fds", Agg::Mean)),
                "threads": json_series(&series("proc_threads", Agg::Mean)),
            },
        });
        drop(conn);
        Json(body).into_response()
    })
}

fn host_json(host: &HostSample) -> Value {
    json!({
        "cores": host.cores,
        "cpu": host.cpu,
        "perCore": host.per_core,
        "load": host.load,
        "memoryTotal": host.memory_total,
        "memoryAvailable": host.memory_available,
        "memoryUsed": host.memory_used(),
        "swapTotal": host.swap_total,
        "swapUsed": host.swap_used(),
        "netRxPerSecond": host.net_rx_per_second,
        "netTxPerSecond": host.net_tx_per_second,
    })
}

/// The database's pages and its write-ahead log, now.
fn database_files(state: &AppState, conn: &Connection) -> (Option<u64>, Option<u64>) {
    let main = conn
        .query_row(
            "SELECT page_count*page_size FROM pragma_page_count(),pragma_page_size()",
            [],
            |row| row.get::<_, i64>(0),
        )
        .ok()
        .map(|bytes| bytes.max(0) as u64);
    let wal = state.store.data_dir().map(|_| {
        let mut name = state.store.path.as_os_str().to_os_string();
        name.push("-wal");
        std::fs::metadata(PathBuf::from(name))
            .map(|meta| meta.len())
            .unwrap_or(0)
    });
    (main, wal)
}
