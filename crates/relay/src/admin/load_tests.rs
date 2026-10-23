//! A seeded load test of the admin API: a relay with 10,000 people, 50,000
//! devices and months of statistics, every admin endpoint timed through HTTP
//! while a "tunnel" task keeps taking the store lock the way heartbeats do.
//!
//! Ignored by default (it writes a database of about a gigabyte). Run it in
//! release mode, where SQLite is optimised:
//!
//! ```sh
//! RELAY_LOAD_DIR=$PWD/.scratch/relay-load \
//!   cargo test --release -p pockymoe-relay --lib admin_endpoints_on_a_large_relay -- --ignored --nocapture
//! ```
//!
//! `RELAY_LOAD_SCALE` (default 1.0) scales every row count; `RELAY_LOAD_KEEP=1`
//! keeps the seeded database for the next run (seeding takes most of the time).
use super::*;
use std::time::Instant;

/// Each endpoint's budget in release mode (median of three calls).
const BUDGET_MS: f64 = 400.0;
/// The longest a tunnel or heartbeat may wait for the store while admin
/// reads run.
const LOCK_BUDGET_MS: f64 = 100.0;

struct Scale {
    users: usize,
    devices: usize,
    online: usize,
    threads_per_device: usize,
    connections: usize,
    traffic_devices: usize,
    traffic_days: i64,
    events: usize,
    sessions: usize,
    shares: usize,
    links: usize,
    actions: usize,
}

impl Scale {
    fn from_env() -> Self {
        let factor: f64 = std::env::var("RELAY_LOAD_SCALE")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1.0);
        let n = |base: usize| ((base as f64) * factor).max(1.0) as usize;
        Self {
            users: n(10_000),
            devices: n(50_000),
            online: n(15_000),
            threads_per_device: 20,
            connections: n(200_000),
            traffic_devices: n(3_000),
            traffic_days: 90,
            events: n(100_000),
            sessions: n(20_000),
            shares: n(15_000),
            links: n(1_000),
            actions: n(50_000),
        }
    }
}

/// A small deterministic generator, so runs are comparable.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn user_id(index: usize) -> String {
    format!("u{index:06}")
}

fn device_id(index: usize) -> String {
    format!("d{index:06}")
}

const VERSIONS: [&str; 12] = [
    "0.13.39", "0.13.38", "0.13.37", "0.13.36", "0.13.30", "0.13.20", "0.13.9", "0.13.1", "0.12.4",
    "0.12.0", "0.11.2", "0.10.0",
];
const NAMES: [&str; 8] = [
    "mac", "wsl", "laptop", "desktop", "gpu-lab", "build-01", "pi", "nas",
];

fn seed(conn: &Connection, scale: &Scale) -> rusqlite::Result<()> {
    let now = common::now_ms();
    let iso = common::rfc3339_from_ms;
    let mut rng = Lcg(42);
    let tx = conn.unchecked_transaction()?;
    {
        let mut user = tx.prepare(
            "INSERT INTO relay_users(id,email,username,role,enabled,last_seen_at,created_at,password_salt,password_hash)
             VALUES (?1,?2,?3,?4,1,?5,?6,'','')",
        )?;
        user.execute(params![
            "admin",
            "admin@example.test",
            "admin",
            "admin",
            iso(now),
            iso(now - 400 * DAY_MS)
        ])?;
        for index in 0..scale.users {
            let seen = (index % 7 != 0).then(|| iso(now - rng.below(60 * DAY_MS as u64) as i64));
            user.execute(params![
                user_id(index),
                format!("person{index}@example.test"),
                format!("person{index}"),
                "user",
                seen,
                iso(now - rng.below(365 * DAY_MS as u64) as i64)
            ])?;
        }
        let mut vault = tx.prepare(
            "INSERT INTO relay_account_vaults(user_id,vault_json,revision,account_key_id,account_public_key) VALUES (?1,'{}',1,?1,'p')",
        )?;
        let mut totp = tx.prepare(
            "INSERT INTO relay_totp(user_id,secret,enabled,expires_at) VALUES (?1,'s',1,0)",
        )?;
        for index in (0..scale.users).filter(|index| index % 3 != 0) {
            vault.execute([user_id(index)])?;
            if index % 5 == 0 {
                totp.execute([user_id(index)])?;
            }
        }
        let mut device = tx.prepare(
            "INSERT INTO relay_devices(id,owner_user_id,name,token_hash,token_preview,created_at) VALUES (?1,?2,?3,?4,'rcd_x',?5)",
        )?;
        let mut certificate = tx.prepare(
            "INSERT INTO relay_device_certificates(device_id,certificate_json) VALUES (?1,'{\"createdAt\":\"2026-05-24T00:00:00.000Z\"}')",
        )?;
        let mut version = tx.prepare(
            "INSERT INTO relay_device_versions(device_id,version,checked_at) VALUES (?1,?2,?3)",
        )?;
        let mut lineage = tx.prepare(
            "INSERT INTO relay_thread_lineage(device_id,thread_id,parent_thread_id,workspace_id) VALUES (?1,?2,?3,?4)",
        )?;
        for index in 0..scale.devices {
            // One power user owns 500 devices; the rest five each.
            let owner = if index < 500 { 0 } else { index % scale.users };
            let id = device_id(index);
            device.execute(params![
                id,
                user_id(owner),
                NAMES[index % NAMES.len()],
                format!("hash-{index}"),
                iso(now - rng.below(300 * DAY_MS as u64) as i64)
            ])?;
            if index % 2 == 0 {
                certificate.execute([&id])?;
            }
            if index % 10 != 0 {
                version.execute(params![
                    id,
                    VERSIONS[(rng.below(VERSIONS.len() as u64)) as usize],
                    now
                ])?;
            }
            for thread in 0..scale.threads_per_device {
                lineage.execute(params![
                    id,
                    format!("{index}-{thread}"),
                    (thread % 4 == 3).then(|| format!("{index}-{}", thread - 1)),
                    format!("w{}", thread % 3)
                ])?;
            }
        }
        // Tunnel connections over 30 days, a few devices flapping.
        let mut connection = tx.prepare(
            "INSERT INTO relay_device_connections(device_id,connection_id,connected_at,ended_at,reason) VALUES (?1,?2,?3,?4,?5)",
        )?;
        for index in 0..scale.connections {
            let device = if index % 10 == 0 {
                rng.below(50)
            } else {
                rng.below(scale.devices as u64)
            } as usize;
            let at = now - rng.below(30 * DAY_MS as u64) as i64;
            let ended = at + rng.below(6 * HOUR_MS as u64) as i64;
            connection.execute(params![
                device_id(device),
                format!("c{index}"),
                at,
                (ended < now).then_some(ended),
                (ended < now).then_some("closed")
            ])?;
        }
        // Forwarded traffic per device and hour.
        let mut traffic = tx.prepare(
            "INSERT INTO relay_device_traffic(device_id,hour,requests,request_bytes,response_bytes,errors) VALUES (?1,?2,?3,?4,?5,?6)",
        )?;
        let this_hour = now - now.rem_euclid(HOUR_MS);
        for index in 0..scale.traffic_devices {
            let id = device_id(index * 7 % scale.devices);
            for hour in 0..scale.traffic_days * 24 {
                let requests = rng.below(400) as i64;
                traffic.execute(params![
                    id,
                    this_hour - hour * HOUR_MS,
                    requests,
                    requests * 900,
                    requests * 4000,
                    requests / 50
                ])?;
            }
        }
        // Statistics: minutes for 48 hours, hours for 365 days.
        let mut metric = tx.prepare(
            "INSERT OR IGNORE INTO relay_metrics(resolution,bucket,metric,label,value,peak,samples) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        )?;
        let gauges = [
            "up",
            "online_devices",
            "browser_sockets",
            "pending_requests",
            "preview_sessions",
            "preview_streams",
            "db_bytes",
        ];
        let counters = [
            "requests",
            "request_bytes",
            "response_bytes",
            "forward_offline",
            "forward_timeout",
            "forward_busy",
            "device_errors",
            "ws_frames",
            "ws_bytes",
            "login_failed",
            "connects",
        ];
        let this_minute = now - now.rem_euclid(MINUTE_MS);
        for (resolution, step, count, samples) in
            [("m", MINUTE_MS, 48 * 60, 1), ("h", HOUR_MS, 365 * 24, 60)]
        {
            let last = if resolution == "m" {
                this_minute
            } else {
                this_hour
            };
            for slot in 0..count {
                let bucket = last - slot as i64 * step;
                for name in gauges {
                    let value = rng.below(15_000) as f64;
                    metric.execute(params![
                        resolution,
                        bucket,
                        name,
                        "",
                        value * samples as f64,
                        value,
                        samples
                    ])?;
                }
                for name in counters {
                    metric.execute(params![
                        resolution,
                        bucket,
                        name,
                        "",
                        rng.below(10_000) as f64,
                        0.0,
                        samples
                    ])?;
                }
                for bound in metrics::LATENCY_BOUNDS
                    .iter()
                    .map(u64::to_string)
                    .chain(["inf".to_string()])
                {
                    metric.execute(params![
                        resolution,
                        bucket,
                        "latency",
                        bound,
                        rng.below(500) as f64,
                        0.0,
                        samples
                    ])?;
                }
                for kind in ["login", "auth-ip"] {
                    metric.execute(params![
                        resolution,
                        bucket,
                        "rate_limited",
                        kind,
                        rng.below(5) as f64,
                        0.0,
                        samples
                    ])?;
                }
            }
        }
        let today = now - now.rem_euclid(DAY_MS);
        for day in 0..365 {
            for version in VERSIONS {
                metric.execute(params![
                    "d",
                    today - day * DAY_MS,
                    "online_by_version",
                    version,
                    1000.0,
                    1000.0,
                    1440
                ])?;
            }
        }
        // Who was active each day, for 90 days.
        let mut active = tx.prepare(
            "INSERT OR IGNORE INTO relay_daily_active(day,kind,subject_id) VALUES (?1,?2,?3)",
        )?;
        for day in 0..90 {
            let label = common::day_of(now - day * DAY_MS);
            for index in 0..scale.users * 3 / 10 {
                active.execute(params![label, "user", user_id(index)])?;
            }
            for index in 0..scale.online {
                active.execute(params![label, "device", device_id(index)])?;
            }
        }
        // The security audit, sessions and sign-ins.
        let mut event = tx.prepare(
            "INSERT INTO relay_security_events(user_id,event,resource_id,created_at) VALUES (?1,?2,?3,?4)",
        )?;
        let kinds = [
            "login.password_or_oauth",
            "login.mfa",
            "login.challenge",
            "device.created",
            "device.token_rotated",
            "passkey.added",
            "login.failed",
        ];
        for index in 0..scale.events {
            let kind = kinds[index % kinds.len()];
            let user = rng.below(scale.users as u64) as usize;
            event.execute(params![
                user_id(user),
                kind,
                kind.starts_with("device.")
                    .then(|| device_id(rng.below(scale.devices as u64) as usize)),
                now - rng.below(180 * DAY_MS as u64) as i64
            ])?;
        }
        let mut session = tx.prepare(
            "INSERT INTO relay_auth_sessions(token_hash,user_id,created_at,expires_at,environment) VALUES (?1,?2,?3,?4,?5)",
        )?;
        for index in 0..scale.sessions {
            session.execute(params![
                format!("session-{index}"),
                user_id(index % scale.users),
                now - rng.below(14 * DAY_MS as u64) as i64,
                now + DAY_MS,
                ["Chrome · macOS", "Safari · iOS", "Firefox · Linux", ""][index % 4]
            ])?;
        }
        // Shares, grants and their recorded visits.
        let mut share = tx.prepare(
            "INSERT INTO relay_shares(id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,thread_id,workspace_label,thread_access,workspace_access,created_at,expires_at)
             VALUES (?1,?2,?3,?4,?5,?6,'mac','t','w','read','none',?7,?8)",
        )?;
        let mut grant = tx.prepare(
            "INSERT INTO relay_access_grants(id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,scope,thread_access,workspace_access,can_create_threads,created_at,expires_at)
             VALUES (?1,?2,?3,?4,?5,?6,'mac',?7,'control','write',1,?8,?9)",
        )?;
        let mut visit = tx.prepare(
            "INSERT INTO relay_share_activity(id,resource_id,resource_type,user_id,username,kind,accessed_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        )?;
        for index in 0..scale.shares {
            let device = (index * 13) % scale.devices;
            let owner = if device < 500 {
                0
            } else {
                device % scale.users
            };
            let target = (owner + 1 + index % 50) % scale.users;
            let created = iso(now - rng.below(200 * DAY_MS as u64) as i64);
            let expires = (index % 4 == 0).then(|| iso(now + rng.below(30 * DAY_MS as u64) as i64));
            share.execute(params![
                format!("s{index}"),
                user_id(owner),
                format!("person{owner}"),
                user_id(target),
                format!("person{target}"),
                device_id(device),
                created,
                expires
            ])?;
            grant.execute(params![
                format!("g{index}"),
                user_id(owner),
                format!("person{owner}"),
                user_id(target),
                format!("person{target}"),
                device_id(device),
                ["device", "workspace", "thread"][index % 3],
                created,
                expires
            ])?;
            for (kind, id) in [
                ("share", format!("s{index}")),
                ("grant", format!("g{index}")),
            ] {
                for visit_index in 0..10 {
                    visit.execute(params![
                        format!("{kind}-{index}-{visit_index}"),
                        id,
                        kind,
                        user_id(target),
                        format!("person{target}"),
                        ["open_thread", "send_prompt", "read_workspace_file"][visit_index % 3],
                        iso(now - rng.below(30 * DAY_MS as u64) as i64)
                    ])?;
                }
            }
        }
        // Public links with 30 KB snapshots, a fifth of them legacy plaintext.
        let mut link = tx.prepare(
            "INSERT INTO relay_public_links(id,owner_user_id,device_id,thread_id,snapshot_json,created_at) VALUES (?1,?2,?3,'t',?4,?5)",
        )?;
        let body = "x".repeat(30_000);
        for index in 0..scale.links {
            let device = (index * 11) % scale.devices;
            let owner = if device < 500 {
                0
            } else {
                device % scale.users
            };
            let snapshot = if index % 5 == 0 {
                format!("{{\"title\":\"t\",\"turns\":[\"{body}\"]}}")
            } else {
                format!("{{\"version\":2,\"iv\":\"x\",\"ciphertext\":\"{body}\"}}")
            };
            link.execute(params![
                format!("l{index}"),
                user_id(owner),
                device_id(device),
                snapshot,
                iso(now - rng.below(200 * DAY_MS as u64) as i64)
            ])?;
        }
        let mut action = tx.prepare(
            "INSERT INTO relay_admin_actions(admin_user_id,admin_username,action,target_type,target_id,target_label,detail,created_at)
             VALUES ('admin','admin',?1,?2,?3,?4,NULL,?5)",
        )?;
        for index in 0..scale.actions {
            let device = device_id(index % scale.devices);
            action.execute(params![
                "device.disconnected",
                "device",
                device,
                "mac",
                now - rng.below(365 * DAY_MS as u64) as i64
            ])?;
        }
        let mut registration = tx.prepare(
            "INSERT INTO relay_pending_registrations(id,email,username,password_salt,password_hash,created_at,status,reviewed_at,reviewed_by_user_id)
             VALUES (?1,?2,?3,'s','h',?4,?5,?6,?7)",
        )?;
        for index in 0..2_000 {
            let pending = index < 100;
            registration.execute(params![
                format!("r{index}"),
                format!("new{index}@example.test"),
                format!("new{index}"),
                iso(now - rng.below(120 * DAY_MS as u64) as i64),
                if pending { "pending" } else { "approved" },
                (!pending).then(|| iso(now - rng.below(60 * DAY_MS as u64) as i64)),
                (!pending).then_some("admin")
            ])?;
        }
    }
    // As on a relay upgraded from a version without the rollups: the next
    // start computes them once.
    tx.execute(
        "DELETE FROM relay_settings WHERE key IN ('lineageCounts','metricsDailyRollup')",
        [],
    )?;
    tx.execute(
        "INSERT INTO relay_settings(key,value) VALUES ('metricsStartedAt',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [(now - 365 * DAY_MS).to_string()],
    )?;
    tx.commit()
}

fn app_state(path: PathBuf) -> Arc<AppState> {
    let store = RelayStore::open(path, "load-test-secret".into()).unwrap();
    Arc::new(AppState {
        store,
        sockets: RwLock::new(HashMap::new()),
        clients: RwLock::new(HashMap::new()),
        pending: StdMutex::new(HashMap::new()),
        web_dist: None,
        legacy_supervisor_token: None,
        oauth: OAuthConfig::default(),
        oauth_client: reqwest::Client::new(),
        admission: security::Admission::default(),
        preview: preview::Hub::default(),
        admin: AdminState::default(),
        metrics: metrics::Metrics::default(),
    })
}

struct Timing {
    path: String,
    median_ms: f64,
    max_ms: f64,
    bytes: usize,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "seeds a database of about a gigabyte; run in release mode"]
async fn admin_endpoints_on_a_large_relay() {
    let scale = Scale::from_env();
    let root = std::env::var("RELAY_LOAD_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("pockymoe-relay-load"));
    let keep = std::env::var("RELAY_LOAD_KEEP").is_ok_and(|value| value == "1");
    let path = root.join("relay-store.sqlite");
    let seeded = root.join("seeded");
    if !keep || !seeded.exists() {
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let started = Instant::now();
        {
            let store = RelayStore::open(path.clone(), "load-test-secret".into()).unwrap();
            let conn = store.conn.try_lock().unwrap();
            seed(&conn, &scale).unwrap();
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); ANALYZE;")
                .unwrap();
        }
        std::fs::write(&seeded, "").unwrap();
        println!("seeded in {:.1} s", started.elapsed().as_secs_f64());
    }
    // Opening again runs the schema steps (indexes, rollups) a restart runs.
    let opened = Instant::now();
    let state = app_state(path.clone());
    println!("opened in {:.1} s", opened.elapsed().as_secs_f64());
    let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    println!("database {:.0} MB", size as f64 / 1e6);
    let admin = {
        let conn = state.store.conn.lock().await;
        create_session(&conn, &state.store.session_secret, "admin").unwrap()
    };
    // Online devices live in memory.
    {
        let mut sockets = state.sockets.write().await;
        for index in 0..scale.online {
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            std::mem::forget(rx);
            sockets.insert(
                device_id(index),
                DeviceSocket {
                    tx,
                    connection_id: Uuid::new_v4(),
                    connected_at: common::rfc3339_ago(HOUR_MS),
                    last_heartbeat_at: common::rfc3339_ago(0),
                },
            );
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().merge(routes()).with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // A tunnel's view: take the store lock every few milliseconds, as
    // heartbeats and forwarded requests do, and note the longest wait.
    let probe_state = state.clone();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let probe_stop = stop.clone();
    let current = Arc::new(StdMutex::new(String::new()));
    let probe_current = current.clone();
    let probe = tokio::spawn(async move {
        let mut longest = Duration::ZERO;
        let mut during = String::new();
        while !probe_stop.load(std::sync::atomic::Ordering::Relaxed) {
            let asked = Instant::now();
            let conn = probe_state.store.conn.lock().await;
            if asked.elapsed() > longest {
                longest = asked.elapsed();
                during = probe_current.lock().unwrap().clone();
            }
            let _ = conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0));
            drop(conn);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        (longest, during)
    });
    let client = reqwest::Client::new();
    let now = common::now_ms();
    let paths: Vec<String> = [
        "/relay/admin",
        "/relay/admin/overview?range=24h",
        "/relay/admin/overview?range=7d",
        "/relay/admin/overview?range=90d",
        "/relay/admin/search?q=person12",
        "/relay/admin/users",
        "/relay/admin/users?sort=devices&dir=desc",
        "/relay/admin/users?sort=lastSignIn&dir=desc",
        "/relay/admin/users?key=missing&page=10",
        "/relay/admin/users/u000000",
        "/relay/admin/registrations?status=reviewed",
        "/relay/admin/devices",
        "/relay/admin/devices?sort=traffic&dir=desc",
        "/relay/admin/devices?sort=lastSeen&dir=desc",
        "/relay/admin/devices?sort=threads&dir=desc",
        "/relay/admin/devices?sort=version&dir=desc",
        "/relay/admin/devices?state=offline&page=40",
        "/relay/admin/devices?owner=u000000",
        "/relay/admin/devices?q=gpu",
        "/relay/admin/devices/d000001",
        "/relay/admin/shares",
        "/relay/admin/shares?sort=lastUsed&dir=desc",
        "/relay/admin/shares?seen=never",
        "/relay/admin/public-links",
        "/relay/admin/public-links?sort=size&dir=desc",
        "/relay/admin/previews",
        "/relay/admin/security?range=7d",
        "/relay/admin/security?range=90d",
        "/relay/admin/security/events",
        "/relay/admin/security/events?sort=user",
        "/relay/admin/sessions",
        "/relay/admin/system",
        "/relay/admin/actions",
        "/relay/admin/performance?range=24h",
        "/relay/admin/performance?range=7d",
        "/relay/admin/performance?range=90d",
    ]
    .into_iter()
    .map(str::to_string)
    .chain([format!(
        "/relay/admin/performance?from={}&to={now}",
        now - 365 * DAY_MS
    )])
    .chain(
        [
            "/relay/admin/performance/devices?view=traffic&range=90d&page=5",
            "/relay/admin/performance/devices?view=traffic&range=24h&sort=rate",
            "/relay/admin/performance/devices?view=reconnects&range=30d",
            "/relay/admin/devices/d000001?range=30d",
            "/relay/admin/shares?range=90d",
            "/relay/admin/system?range=90d",
            "/relay/admin/server?range=24h",
            "/relay/admin/server?range=90d",
        ]
        .into_iter()
        .map(str::to_string),
    )
    .chain([format!(
        "/relay/admin/overview?from={}&to={now}",
        now - 365 * DAY_MS
    )])
    .collect();
    let mut timings = Vec::new();
    for path in &paths {
        *current.lock().unwrap() = path.clone();
        let mut runs = Vec::new();
        let mut bytes = 0;
        let mut status = StatusCode::OK;
        for _ in 0..3 {
            let started = Instant::now();
            let response = client
                .get(format!("{base}{path}"))
                .bearer_auth(&admin)
                .send()
                .await
                .unwrap();
            status = response.status();
            bytes = response.bytes().await.unwrap().len();
            runs.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        if status == StatusCode::NOT_FOUND {
            println!("{path:<60} (not served)");
            continue;
        }
        assert_eq!(status, StatusCode::OK, "{path}");
        runs.sort_by(f64::total_cmp);
        timings.push(Timing {
            path: path.clone(),
            median_ms: runs[1],
            max_ms: runs[2],
            bytes,
        });
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let (lock_wait, lock_during) = probe.await.unwrap();
    // The minute sampler with every online device: how long it holds the store.
    let _ = state.store.conn.take_sample();
    let sampled = Instant::now();
    let minute = now - now.rem_euclid(MINUTE_MS);
    metrics::sample(&state, minute).await;
    let sampler_ms = sampled.elapsed().as_secs_f64() * 1000.0;
    let held = state.store.conn.take_sample();
    println!(
        "sampler with {} online devices, first minute of the day: {sampler_ms:.1} ms, store lock held {:.1} ms at most",
        scale.online, held.max_hold_ms
    );
    let sampled = Instant::now();
    metrics::sample(&state, minute + MINUTE_MS).await;
    let sampler_ms = sampled.elapsed().as_secs_f64() * 1000.0;
    let held = state.store.conn.take_sample();
    println!(
        "sampler, any later minute: {sampler_ms:.1} ms, store lock held {:.1} ms at most",
        held.max_hold_ms
    );
    server.abort();
    println!(
        "\n{:<60} {:>10} {:>10} {:>10}",
        "endpoint", "median ms", "max ms", "KB"
    );
    for timing in &timings {
        println!(
            "{:<60} {:>10.1} {:>10.1} {:>10.1}",
            timing.path,
            timing.median_ms,
            timing.max_ms,
            timing.bytes as f64 / 1000.0
        );
    }
    let lock_ms = lock_wait.as_secs_f64() * 1000.0;
    println!(
        "\nlongest store-lock wait of the tunnel probe: {lock_ms:.1} ms (during {lock_during})"
    );
    if !keep {
        let _ = std::fs::remove_dir_all(&root);
    }
    let slow: Vec<&Timing> = timings
        .iter()
        .filter(|timing| timing.median_ms > BUDGET_MS)
        .collect();
    assert!(
        slow.is_empty(),
        "over the {BUDGET_MS} ms budget: {:?}",
        slow.iter()
            .map(|timing| (&timing.path, timing.median_ms))
            .collect::<Vec<_>>()
    );
    assert!(
        lock_ms <= LOCK_BUDGET_MS,
        "a tunnel waited {lock_ms:.1} ms for the store"
    );
    // Lists stay bounded: no endpoint answers with megabytes.
    for timing in &timings {
        // The legacy summary for older Web builds is capped at 500 rows per list.
        let limit = if timing.path == "/relay/admin" {
            4_000_000
        } else {
            600_000
        };
        assert!(
            timing.bytes < limit,
            "{} answered {} bytes",
            timing.path,
            timing.bytes
        );
    }
}
