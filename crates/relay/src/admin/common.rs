//! Pieces every admin endpoint shares: the admin check, paging, time ranges,
//! the in-memory tunnel snapshot and small JSON helpers.
use super::*;

pub(super) type Conn<'a> = crate::store::StoreGuard<'a>;

/// Locks the store and checks the admin session (bearer token or the admin
/// cookie; a static Web origin passes the resolved session). The caller takes
/// in-memory snapshots before this, so the store lock is never held while
/// waiting for the socket maps.
pub(super) async fn admin_conn<'a>(
    state: &'a AppState,
    headers: &HeaderMap,
) -> Result<(Conn<'a>, UserRow), Response> {
    let conn = state.store.conn.lock().await;
    match authenticated_admin_user(
        &conn,
        &state.store.session_secret,
        headers,
        &TokenQuery::default(),
    ) {
        Some(admin) => Ok((conn, admin)),
        None => Err(unauthorized()),
    }
}

/// A connection for admin reads: the store's read-only connection, or the
/// main one when the store has none (in-memory stores).
pub(super) enum ReadConn<'a> {
    Reader(tokio::sync::MutexGuard<'a, Connection>),
    Main(Conn<'a>),
}

impl std::ops::Deref for ReadConn<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        match self {
            Self::Reader(conn) => conn,
            Self::Main(conn) => conn,
        }
    }
}

/// Checks the admin session on the main connection (briefly: it also notes
/// the admin's activity and writes queued tunnel events, so lists read right
/// after a reconnect count it), then hands out the read-only connection. A
/// long report therefore never holds the lock tunnels and heartbeats need.
pub(super) async fn admin_read<'a>(
    state: &'a AppState,
    headers: &HeaderMap,
) -> Result<(ReadConn<'a>, UserRow), Response> {
    let (conn, admin) = admin_conn(state, headers).await?;
    crate::metrics::flush_connections(state, &conn);
    match state.store.read.as_ref() {
        Some(reader) => {
            drop(conn);
            Ok((ReadConn::Reader(reader.lock().await), admin))
        }
        None => Ok((ReadConn::Main(conn), admin)),
    }
}

/// Runs an admin report's SQLite work without holding up the async runtime:
/// on the multi-threaded runtime the worker hands its other tasks (tunnels,
/// heartbeats) to another thread while this one reads.
pub(super) fn blocking<R>(work: impl FnOnce() -> R) -> R {
    match tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(work),
        _ => work(),
    }
}

pub(super) fn not_found(message: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ApiError::new("not_found", message)),
    )
        .into_response()
}

pub(super) fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiError::new("bad_request", message)),
    )
        .into_response()
}

pub(super) fn internal_error() -> Response {
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct ListQuery {
    pub(super) page: Option<usize>,
    pub(super) page_size: Option<usize>,
    pub(super) q: Option<String>,
    pub(super) filter: Option<String>,
    pub(super) sort: Option<String>,
    pub(super) range: Option<String>,
    #[serde(rename = "type")]
    pub(super) kind: Option<String>,
    pub(super) user: Option<String>,
    /// `asc` or `desc` for `sort`; each list has its own default direction.
    pub(super) dir: Option<String>,
    // Column filters. Times are epoch milliseconds; `seen=never` asks for
    // rows never seen.
    pub(super) owner: Option<String>,
    pub(super) recipient: Option<String>,
    pub(super) state: Option<String>,
    pub(super) protection: Option<String>,
    pub(super) version: Option<String>,
    pub(super) role: Option<String>,
    pub(super) status: Option<String>,
    pub(super) key: Option<String>,
    pub(super) factor: Option<String>,
    pub(super) method: Option<String>,
    pub(super) scope: Option<String>,
    pub(super) access: Option<String>,
    pub(super) expiry: Option<String>,
    pub(super) seen: Option<String>,
    pub(super) seen_from: Option<i64>,
    pub(super) seen_to: Option<i64>,
    pub(super) created_from: Option<i64>,
    pub(super) created_to: Option<i64>,
    pub(super) from: Option<i64>,
    pub(super) to: Option<i64>,
    /// Which per-device list of the Performance page (`reconnects`, `traffic`).
    pub(super) view: Option<String>,
}

/// A time window `[from, to)` in epoch milliseconds; either end may be open.
#[derive(Clone, Copy, Default)]
pub(super) struct Window {
    pub(super) from: Option<i64>,
    pub(super) to: Option<i64>,
}

impl Window {
    pub(super) fn is_open(&self) -> bool {
        self.from.is_none() && self.to.is_none()
    }
    /// The bounds as RFC 3339 text, for tables that store text times ('' when open).
    pub(super) fn text(&self) -> (String, String) {
        (
            self.from.map(rfc3339_from_ms).unwrap_or_default(),
            self.to.map(rfc3339_from_ms).unwrap_or_default(),
        )
    }
}

/// Which way a list is sorted.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Dir {
    Asc,
    Desc,
}

impl Dir {
    pub(super) fn sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

impl ListQuery {
    /// The requested sort key and direction (`default` when `dir` is absent).
    pub(super) fn sort_by(&self, default: Dir) -> Option<(&str, Dir)> {
        let key = self.sort.as_deref().filter(|key| !key.is_empty())?;
        let dir = match self.dir.as_deref() {
            Some("asc") => Dir::Asc,
            Some("desc") => Dir::Desc,
            _ => default,
        };
        Some((key, dir))
    }
    pub(super) fn seen_window(&self) -> Window {
        Window {
            from: self.seen_from,
            to: self.seen_to,
        }
    }
    pub(super) fn created_window(&self) -> Window {
        Window {
            from: self.created_from,
            to: self.created_to,
        }
    }
    pub(super) fn window(&self) -> Window {
        Window {
            from: self.from,
            to: self.to,
        }
    }
    /// A filter value, or None when absent, empty or `all`.
    pub(super) fn pick(value: &Option<String>) -> Option<&str> {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "all")
    }
    pub(super) fn paging(&self) -> Paging {
        Paging {
            page: self.page.unwrap_or(1).max(1),
            size: self.page_size.unwrap_or(25).clamp(1, 100),
        }
    }
    /// The search text, lowercased, or None when empty.
    pub(super) fn search(&self) -> Option<String> {
        self.q
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(|q| q.chars().take(120).collect::<String>().to_lowercase())
    }
    pub(super) fn filter(&self) -> &str {
        self.filter.as_deref().unwrap_or("all")
    }
    pub(super) fn range(&self, default: Range) -> Range {
        self.range
            .as_deref()
            .and_then(Range::parse)
            .unwrap_or(default)
    }
}

pub(super) struct Paging {
    pub(super) page: usize,
    pub(super) size: usize,
}

impl Paging {
    pub(super) fn offset(&self) -> usize {
        (self.page - 1) * self.size
    }
    pub(super) fn wrap(&self, items: Vec<Value>, total: usize) -> serde_json::Map<String, Value> {
        let mut map = serde_json::Map::new();
        map.insert("items".into(), Value::Array(items));
        map.insert("total".into(), json!(total));
        map.insert("page".into(), json!(self.page));
        map.insert("pageSize".into(), json!(self.size));
        map
    }
}

/// The time ranges the console offers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Range {
    Day,
    Week,
    Month,
    Quarter,
}

impl Range {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "24h" => Some(Self::Day),
            "7d" => Some(Self::Week),
            "30d" => Some(Self::Month),
            "90d" => Some(Self::Quarter),
            _ => None,
        }
    }
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
            Self::Quarter => "90d",
        }
    }
    pub(crate) fn millis(self) -> i64 {
        let hours = match self {
            Self::Day => 24,
            Self::Week => 24 * 7,
            Self::Month => 24 * 30,
            Self::Quarter => 24 * 90,
        };
        hours * HOUR_MS
    }
}

pub(crate) const MINUTE_MS: i64 = 60_000;
pub(crate) const HOUR_MS: i64 = 60 * MINUTE_MS;
pub(crate) const DAY_MS: i64 = 24 * HOUR_MS;

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(crate) fn rfc3339_from_ms(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub(crate) fn ms_from_rfc3339(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.timestamp_millis())
}

/// Text timestamps compare as text: produce the cutoff in the same format.
pub(crate) fn rfc3339_ago(ms: i64) -> String {
    rfc3339_from_ms(now_ms() - ms)
}

/// The UTC day (`YYYY-MM-DD`) of a millisecond time.
pub(crate) fn day_of(ms: i64) -> String {
    rfc3339_from_ms(ms)[..10].to_string()
}

/// What the relay holds in memory about online devices and previews.
pub(super) struct Live {
    pub(super) tunnels: HashMap<String, (String, String)>,
    pub(super) previews: HashMap<String, usize>,
}

pub(super) async fn live(state: &AppState) -> Live {
    Live {
        tunnels: tunnel_times(state).await,
        previews: state.preview.mapping_counts(),
    }
}

/// Parameters of a statement built from optional pieces: `add` binds a value
/// and returns its placeholder.
#[derive(Default)]
pub(super) struct Binds {
    pub(super) values: Vec<rusqlite::types::Value>,
}

impl Binds {
    pub(super) fn add(&mut self, value: impl Into<rusqlite::types::Value>) -> String {
        self.values.push(value.into());
        format!("?{}", self.values.len())
    }
    /// A condition that is always true but names the last placeholder, so a
    /// statement that uses only some of the bound values still takes them all
    /// (SQLite counts parameters up to the highest one named).
    pub(super) fn keep_all(&self) -> String {
        if self.values.is_empty() {
            "1".into()
        } else {
            format!("(?{} IS NULL OR 1)", self.values.len())
        }
    }
    pub(super) fn params(
        &self,
    ) -> rusqlite::ParamsFromIter<std::slice::Iter<'_, rusqlite::types::Value>> {
        rusqlite::params_from_iter(self.values.iter())
    }
}

/// Text for `json_each`, so a set held in memory (online devices) can join a
/// query without one parameter per member.
pub(super) fn json_list<'a>(items: impl IntoIterator<Item = &'a String>) -> String {
    serde_json::to_string(&items.into_iter().collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into())
}

/// A chart window: a preset range or explicit `from`/`to` times, with a step
/// that gives at most about 200 points: five-minute steps within the two
/// days minute rows are kept, whole hours (or days) beyond.
#[derive(Clone, Copy, Debug)]
pub(super) struct ChartWindow {
    pub(super) key: &'static str,
    pub(super) start: i64,
    pub(super) end: i64,
    pub(super) step: i64,
}

impl ChartWindow {
    pub(super) fn span(&self) -> i64 {
        self.end - self.start
    }
    /// Days of per-day charts: a preset shows its days ending today (24 hours
    /// shows the last two), a custom window every UTC day it touches.
    pub(super) fn days(&self) -> i64 {
        match self.key {
            "24h" => 2,
            "7d" => 7,
            "30d" => 30,
            "90d" => 90,
            _ => {
                let first = self.start - self.start.rem_euclid(DAY_MS);
                ((self.end - first + DAY_MS - 1) / DAY_MS).max(1)
            }
        }
    }
    /// The first of those days (UTC midnight).
    pub(super) fn first_day(&self) -> i64 {
        let last = (self.end - 1) - (self.end - 1).rem_euclid(DAY_MS);
        last - (self.days() - 1) * DAY_MS
    }
    pub(super) fn json(&self) -> Value {
        json!({
            "range": self.key,
            "start": rfc3339_from_ms(self.start),
            "end": rfc3339_from_ms(self.end),
            "resolutionSeconds": self.step / 1000,
        })
    }
}

pub(super) fn chart_window(
    query: &ListQuery,
    default: Range,
    now: i64,
) -> Result<ChartWindow, &'static str> {
    let (key, start, end) = match (query.from, query.to) {
        (Some(from), to) => {
            let end = to.unwrap_or(now).min(now);
            if from >= end {
                return Err("The window must end after it starts");
            }
            ("custom", from.max(end - 400 * DAY_MS), end)
        }
        (None, _) => {
            let range = query.range(default);
            (range.key(), now - range.millis(), now)
        }
    };
    let span = end - start;
    let within_minutes = now - start <= crate::metrics::MINUTE_RETENTION_MS;
    let raw = (span / 200).max(MINUTE_MS);
    let step = if within_minutes && raw <= HOUR_MS {
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
    Ok(ChartWindow {
        key,
        start,
        end,
        step,
    })
}

/// A LIKE pattern that matches the text anywhere, with wildcards escaped.
pub(super) fn like_pattern(search: &str) -> String {
    let escaped = search
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

pub(super) fn count(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> i64 {
    conn.query_row(sql, params, |row| row.get(0)).unwrap_or(0)
}

/// Compares dotted versions numerically (`0.13.9` < `0.13.38`); a suffix such
/// as `-beta` sorts before the plain release.
pub(crate) fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    fn parts(value: &str) -> (Vec<u64>, bool) {
        let value = value.trim().trim_start_matches('v');
        let (core, pre) = match value.split_once(['-', '+']) {
            Some((core, _)) => (core, true),
            None => (value, false),
        };
        (
            core.split('.')
                .map(|part| part.parse().unwrap_or(0))
                .collect(),
            pre,
        )
    }
    let (left, left_pre) = parts(left);
    let (right, right_pre) = parts(right);
    for index in 0..left.len().max(right.len()) {
        let ordering = left
            .get(index)
            .unwrap_or(&0)
            .cmp(right.get(index).unwrap_or(&0));
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    right_pre.cmp(&left_pre)
}

/// The relay's own version: a device older than it is "outdated".
pub(crate) const RELAY_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A device with its owner, for every list that names a device: the same
/// name can belong to many people ("mac", "wsl"), so the owner goes with it.
#[derive(Clone, Debug)]
pub(super) struct DeviceRef {
    pub(super) name: String,
    pub(super) owner_user_id: String,
    pub(super) owner_username: String,
    pub(super) owner_email: String,
}

impl DeviceRef {
    /// The fields a row carries next to its own `deviceId`.
    pub(super) fn fields(&self) -> serde_json::Map<String, Value> {
        let mut map = serde_json::Map::new();
        map.insert("name".into(), json!(self.name));
        map.insert("deviceName".into(), json!(self.name));
        map.insert("ownerUserId".into(), json!(self.owner_user_id));
        map.insert("ownerUsername".into(), json!(self.owner_username));
        map.insert("ownerEmail".into(), json!(self.owner_email));
        map
    }
}

/// Devices and their owners by id, one indexed lookup each.
pub(super) fn device_refs(conn: &Connection, ids: &[String]) -> HashMap<String, DeviceRef> {
    let mut refs = HashMap::new();
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT d.name,d.owner_user_id,u.username,u.email FROM relay_devices d
         JOIN relay_users u ON u.id=d.owner_user_id WHERE d.id=?1",
    ) else {
        return refs;
    };
    for id in ids {
        if refs.contains_key(id) {
            continue;
        }
        if let Ok(found) = stmt.query_row([id], |row| {
            Ok(DeviceRef {
                name: row.get(0)?,
                owner_user_id: row.get(1)?,
                owner_username: row.get(2)?,
                owner_email: row.get(3)?,
            })
        }) {
            refs.insert(id.clone(), found);
        }
    }
    refs
}

/// Usernames by id, for labelling rows whose tables store only ids.
pub(super) fn usernames(conn: &Connection, ids: &[String]) -> HashMap<String, String> {
    let mut names = HashMap::new();
    let mut stmt = match conn.prepare_cached("SELECT username FROM relay_users WHERE id=?1") {
        Ok(stmt) => stmt,
        Err(_) => return names,
    };
    for id in ids {
        if names.contains_key(id) {
            continue;
        }
        if let Ok(name) = stmt.query_row([id], |row| row.get::<_, String>(0)) {
            names.insert(id.clone(), name);
        }
    }
    names
}
