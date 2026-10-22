//! Pieces every admin endpoint shares: the admin check, paging, time ranges,
//! the in-memory tunnel snapshot and small JSON helpers.
use super::*;

pub(super) type Conn<'a> = tokio::sync::MutexGuard<'a, Connection>;

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
}

impl ListQuery {
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
    pub(super) fn slice<T: Clone>(&self, items: &[T]) -> Vec<T> {
        items
            .iter()
            .skip(self.offset())
            .take(self.size)
            .cloned()
            .collect()
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
    /// Days in the range, for per-day charts (24 hours shows the last two days).
    pub(crate) fn days(self) -> i64 {
        match self {
            Self::Day => 2,
            Self::Week => 7,
            Self::Month => 30,
            Self::Quarter => 90,
        }
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

/// Usernames by id, for labelling rows whose tables store only ids.
pub(super) fn usernames(conn: &Connection, ids: &[String]) -> HashMap<String, String> {
    let mut names = HashMap::new();
    let mut stmt = match conn.prepare("SELECT username FROM relay_users WHERE id=?1") {
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

pub(super) fn device_names(conn: &Connection, ids: &[String]) -> HashMap<String, String> {
    let mut names = HashMap::new();
    let mut stmt = match conn.prepare("SELECT name FROM relay_devices WHERE id=?1") {
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
