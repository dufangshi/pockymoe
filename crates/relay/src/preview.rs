//! Private HTTP previews on a distinct origin per explicitly approved device port.
//! The ordinary relay API remains encrypted; only this separate data path is plaintext.
use super::*;
use axum::body::{Bytes, HttpBody};
use hyper_util::rt::TokioIo;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio_util::sync::CancellationToken;

const COOKIE: &str = "__Host-rc_port_preview";
const LOCAL_COOKIE: &str = "rc_port_preview";

#[derive(Clone, PartialEq, Eq)]
struct Mapping {
    id: String,
    device: String,
    connection: Uuid,
    port: u16,
    /// The conversation whose agent created the mapping, as the device reports
    /// it. People that conversation is shared with may open the mapping too.
    thread: Option<String>,
}
#[derive(Clone, PartialEq, Eq)]
struct Grant {
    mapping: String,
    device: String,
    session: String,
    lease: Option<String>,
    expires: std::time::Instant,
}
impl Grant {
    fn same_authority(&self, other: &Self) -> bool {
        self.mapping == other.mapping
            && self.device == other.device
            && self.session == other.session
            && self.lease == other.lease
    }
}
#[derive(Clone)]
struct Launch {
    grant: Grant,
    expires: std::time::Instant,
}
struct Stream {
    mapping: Mapping,
    grant: Grant,
    tx: Option<tokio::sync::oneshot::Sender<DuplexStream>>,
    cancel: CancellationToken,
    closed: CancellationToken,
}
#[derive(Default)]
pub(super) struct Hub {
    pub(super) base: Option<url::Url>,
    mappings: StdMutex<HashMap<String, Mapping>>,
    launches: StdMutex<HashMap<String, Launch>>,
    sessions: StdMutex<HashMap<String, Grant>>,
    streams: StdMutex<HashMap<String, Stream>>,
    idle: StdMutex<IdlePool>,
}

// Reuse only fully consumed HTTP/1 responses, within the very same capability.
// The device still watches the lease throughout the lifetime of the raw TCP stream.
// This needs no wire change: older Supervisors already bridge arbitrary TCP bytes.
const HTTP_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_PREWARM_TARGET: usize = 2;
const HTTP_PREWARM_GLOBAL: usize = 16;
const HTTP_PREWARM_DEVICE: usize = 4;
struct HttpClient {
    sender: hyper::client::conn::http1::SendRequest<Body>,
    cancel: CancellationToken,
    closed: CancellationToken,
    mapping: Mapping,
    grant: Grant,
}
struct IdleClient {
    client: HttpClient,
    expires: std::time::Instant,
}
struct ClosingClient {
    device: String,
    cancel: CancellationToken,
    closed: CancellationToken,
}
struct WarmingClient {
    id: Uuid,
    mapping: Mapping,
    grant: Grant,
    done: CancellationToken,
}
#[derive(Default)]
struct IdlePool {
    clients: Vec<IdleClient>,
    warming: Vec<WarmingClient>,
    closing: Vec<ClosingClient>,
    reaping: bool,
}
impl IdlePool {
    fn discard(&mut self, client: HttpClient) {
        // Dropping the sender begins asynchronous bridge shutdown. Keep only
        // its routing/completion tokens so full-capacity requests can await the
        // real slot release after pruning, rejection by an idle limit or eviction.
        self.closing
            .retain(|c| !c.closed.is_cancelled() && !c.cancel.is_cancelled());
        if !client.closed.is_cancelled() && !client.cancel.is_cancelled() {
            self.closing.push(ClosingClient {
                device: client.mapping.device.clone(),
                cancel: client.cancel.clone(),
                closed: client.closed.clone(),
            });
        }
    }
    fn prune(&mut self) {
        self.closing
            .retain(|c| !c.closed.is_cancelled() && !c.cancel.is_cancelled());
        let now = std::time::Instant::now();
        let mut index = 0;
        while index < self.clients.len() {
            let c = &self.clients[index];
            if c.expires <= now
                || c.client.grant.expires <= now
                || c.client.cancel.is_cancelled()
                || c.client.closed.is_cancelled()
                || c.client.sender.is_closed()
            {
                let client = self.clients.swap_remove(index).client;
                self.discard(client);
            } else {
                index += 1;
            }
        }
    }
}

async fn http_client(
    state: &Arc<AppState>,
    mapping: &Mapping,
    grant: &Grant,
    reuse: bool,
) -> Result<HttpClient> {
    if reuse {
        loop {
            let (idle, warming) = {
                let mut pool = state.preview.idle.lock().unwrap();
                pool.prune();
                let idle = pool
                    .clients
                    .iter()
                    .rposition(|c| c.client.mapping == *mapping && c.client.grant == *grant)
                    .map(|i| pool.clients.swap_remove(i).client);
                let warming = pool
                    .warming
                    .iter()
                    .find(|c| c.mapping == *mapping && c.grant == *grant)
                    .map(|c| c.done.clone());
                (idle, warming)
            };
            let Some(client) = idle else {
                if let Some(done) = warming {
                    // Join the bounded in-flight handshake instead of opening a
                    // duplicate connection on the first browser request.
                    done.cancelled().await;
                    if !valid_grant(state, grant).await {
                        bail!("Preview authorization ended");
                    }
                    continue;
                }
                break;
            };
            // A peer may close an idle keepalive socket. Fall back before sending
            // anything; never retry a request after it may have reached the app.
            if client.sender.is_ready() && !client.cancel.is_cancelled() {
                return Ok(client);
            }
            state.preview.idle.lock().unwrap().discard(client);
        }
    }
    new_http_client(state, mapping, grant, false).await
}

async fn new_http_client(
    state: &Arc<AppState>,
    mapping: &Mapping,
    grant: &Grant,
    opportunistic: bool,
) -> Result<HttpClient> {
    let (stream, cancel, released) = open_stream(state, mapping, grant, opportunistic).await?;
    let (sender, connection) =
        hyper::client::conn::http1::handshake::<_, Body>(TokioIo::new(stream)).await?;
    let closed = cancel.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = connection.with_upgrades() => {},
            _ = closed.cancelled() => {},
        }
    });
    Ok(HttpClient {
        sender,
        cancel,
        closed: released,
        mapping: mapping.clone(),
        grant: grant.clone(),
    })
}

fn return_http_client(state: &Arc<AppState>, client: HttpClient) {
    let mut pool = state.preview.idle.lock().unwrap();
    pool.prune();
    if client.cancel.is_cancelled()
        || client.closed.is_cancelled()
        || client.sender.is_closed()
        || client.grant.expires <= std::time::Instant::now()
    {
        pool.discard(client);
        return;
    }
    // Idle sockets still count against the existing active stream limits. Keep
    // room for concurrent requests and other ports on the same device.
    if pool.clients.len() >= 64
        || pool
            .clients
            .iter()
            .filter(|c| c.client.mapping.device == client.mapping.device)
            .count()
            >= 8
        || pool
            .clients
            .iter()
            .filter(|c| c.client.mapping == client.mapping && c.client.grant == client.grant)
            .count()
            >= 6
    {
        pool.discard(client);
        return;
    }
    pool.clients.push(IdleClient {
        client,
        expires: std::time::Instant::now() + HTTP_IDLE_TIMEOUT,
    });
    if !pool.reaping {
        pool.reaping = true;
        let weak = Arc::downgrade(state);
        tokio::spawn(async move {
            loop {
                let until = {
                    let Some(state) = weak.upgrade() else { return };
                    let mut pool = state.preview.idle.lock().unwrap();
                    pool.prune();
                    let Some(until) = pool.clients.iter().map(|c| c.expires).min() else {
                        pool.reaping = false;
                        return;
                    };
                    until
                };
                tokio::time::sleep_until(tokio::time::Instant::from_std(until)).await;
            }
        });
    }
}

// Speculation uses the same capability and idle budgets as ordinary reuse,
// plus smaller pending-handshake budgets. It never evicts a useful connection
// and sends no HTTP request to the application.
fn prewarm_http(state: &Arc<AppState>, mapping: &Mapping, grant: &Grant) {
    let streams = state.preview.streams.lock().unwrap();
    let mut pool = state.preview.idle.lock().unwrap();
    pool.prune();
    let same = pool
        .clients
        .iter()
        .filter(|c| c.client.mapping == *mapping && c.client.grant == *grant)
        .count()
        + pool
            .warming
            .iter()
            .filter(|c| c.mapping == *mapping && c.grant == *grant)
            .count();
    let device_idle = pool
        .clients
        .iter()
        .filter(|c| c.client.mapping.device == mapping.device)
        .count();
    let device_warming = pool
        .warming
        .iter()
        .filter(|c| c.mapping.device == mapping.device)
        .count();
    let device_streams = streams
        .values()
        .filter(|c| c.mapping.device == mapping.device)
        .count();
    let count = HTTP_PREWARM_TARGET
        .saturating_sub(same)
        .min(HTTP_PREWARM_GLOBAL.saturating_sub(pool.warming.len()))
        .min(HTTP_PREWARM_DEVICE.saturating_sub(device_warming))
        .min(64usize.saturating_sub(pool.clients.len() + pool.warming.len()))
        .min(8usize.saturating_sub(device_idle + device_warming))
        .min(256usize.saturating_sub(streams.len() + pool.warming.len()))
        .min(32usize.saturating_sub(device_streams + device_warming));
    for _ in 0..count {
        let id = Uuid::new_v4();
        let done = CancellationToken::new();
        pool.warming.push(WarmingClient {
            id,
            mapping: mapping.clone(),
            grant: grant.clone(),
            done: done.clone(),
        });
        let state = state.clone();
        let mapping = mapping.clone();
        let grant = grant.clone();
        tokio::spawn(async move {
            let _guard = WarmingGuard {
                state: state.clone(),
                id,
                done,
            };
            let result = tokio::time::timeout(HTTP_IDLE_TIMEOUT, async {
                if !valid_grant(&state, &grant).await {
                    bail!("Preview authorization ended");
                }
                new_http_client(&state, &mapping, &grant, true).await
            })
            .await;
            if let Ok(Ok(client)) = result {
                if valid_grant(&state, &grant).await {
                    return_http_client(&state, client);
                }
            }
        });
    }
}
struct WarmingGuard {
    state: Arc<AppState>,
    id: Uuid,
    done: CancellationToken,
}
impl Drop for WarmingGuard {
    fn drop(&mut self) {
        self.state
            .preview
            .idle
            .lock()
            .unwrap()
            .warming
            .retain(|c| c.id != self.id);
        self.done.cancel();
    }
}

// Preserve frames (including trailers) and streaming/backpressure. Dropping an
// unfinished/error response drops its sender instead of pooling a busy stream.
struct HttpResponseBody {
    body: hyper::body::Incoming,
    client: Option<HttpClient>,
    state: Arc<AppState>,
}
impl HttpResponseBody {
    fn complete(&mut self) {
        if let Some(client) = self.client.take() {
            return_http_client(&self.state, client);
        }
    }
}
impl HttpBody for HttpResponseBody {
    type Data = Bytes;
    type Error = hyper::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Bytes>, hyper::Error>>> {
        let frame = Pin::new(&mut self.body).poll_frame(cx);
        match &frame {
            Poll::Ready(Some(Err(_))) => {
                self.client.take();
            }
            Poll::Ready(None) => self.complete(),
            Poll::Ready(Some(Ok(_))) if self.body.is_end_stream() => self.complete(),
            _ => {}
        }
        frame
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> hyper::body::SizeHint {
        self.body.size_hint()
    }
}

impl Hub {
    pub(super) fn from_env() -> Result<Self> {
        let base = std::env::var("POCKYMOE_PORT_PREVIEW_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| parse_base(&s))
            .transpose()?;
        Ok(Self {
            base,
            ..Self::default()
        })
    }
    fn origin(&self, id: &str) -> Option<url::Url> {
        let mut url = self.base.clone()?;
        url.set_host(Some(&format!("p-{id}.{}", url.host_str()?)))
            .ok()?;
        Some(url)
    }
    fn id_for_host(&self, host: &str) -> Option<String> {
        let base = self.base.as_ref()?;
        let url = url::Url::parse(&format!("{}://{host}", base.scheme())).ok()?;
        if url.port_or_known_default() != base.port_or_known_default()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        let prefix = url
            .host_str()?
            .strip_suffix(&format!(".{}", base.host_str()?))?;
        let id = prefix.strip_prefix("p-")?;
        (id.len() == 32
            && id
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
        .then(|| id.to_string())
    }
    /// Port mappings per device, for the admin console. Counts only.
    pub(super) fn mapping_counts(&self) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for mapping in self
            .mappings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            *counts.entry(mapping.device.clone()).or_insert(0) += 1;
        }
        counts
    }
    /// Open preview sessions and data streams per device, and in total.
    pub(super) fn usage(&self) -> (HashMap<String, (usize, usize)>, usize, usize) {
        let mut per_device: HashMap<String, (usize, usize)> = HashMap::new();
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        for grant in sessions.values() {
            per_device.entry(grant.device.clone()).or_default().0 += 1;
        }
        let session_total = sessions.len();
        drop(sessions);
        let streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        for stream in streams.values() {
            per_device.entry(stream.grant.device.clone()).or_default().1 += 1;
        }
        (per_device, session_total, streams.len())
    }
}

fn parse_base(value: &str) -> Result<url::Url> {
    let url = url::Url::parse(value.trim())?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host(), Some(url::Host::Domain(_)))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("POCKYMOE_PORT_PREVIEW_BASE_URL must be an HTTP(S) domain origin, for example https://lnz-study.com");
    }
    Ok(url)
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/relay/port-mappings/config", get(config))
        // Existing CLI links already point at the Relay, including split-origin
        // deployments. A fully authorized navigation needs no Web bundle.
        .route("/devices/{device}/ports/{mapping}", get(browser_open))
        .route(
            "/relay/devices/{device}/port-mappings/{mapping}/open",
            post(open),
        )
        .route("/supervisor/port-stream/{ticket}", get(device_stream))
}

async fn config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
) -> Response {
    let conn = state.store.conn.lock().await;
    if authenticated_user(&conn, &state.store.session_secret, &headers, &query).is_none() {
        return unauthorized();
    }
    Json(json!({"available":state.preview.base.is_some(),"baseUrl":state.preview.base.as_ref().map(url::Url::as_str)})).into_response()
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OpenInput {
    #[serde(default)]
    path: String,
}
#[derive(Deserialize, Default)]
struct OpenQuery {
    #[serde(flatten)]
    token: TokenQuery,
    lease: Option<String>,
}
async fn open(
    State(state): State<Arc<AppState>>,
    Path((device, id)): Path<(String, String)>,
    headers: HeaderMap,
    Query(query): Query<OpenQuery>,
    Json(input): Json<OpenInput>,
) -> Response {
    match issue_launch(&state, device, id, &headers, query, input).await {
        Ok(url) => {
            let mut response = Json(json!({"url":url.as_str()})).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
            response
        }
        Err(response) => response,
    }
}

// Navigation is a separate capability handoff, not a CSRF exemption for POST.
// Only a top-level document can use it; it consumes an existing device lease
// and the browser's own HttpOnly account cookie, never a URL/bearer credential.
pub(super) fn browser_entry_path(path: &str) -> bool {
    let parts: Vec<_> = path.split('/').collect();
    matches!(parts.as_slice(), ["", "devices", device, "ports", mapping] if !device.is_empty() && !mapping.is_empty())
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct BrowserOpenQuery {
    #[serde(default)]
    path: String,
    lease: Option<String>,
    lease_expires_at: Option<String>,
}
fn document_navigation(request: &Request) -> bool {
    request.method() == Method::GET
        && request
            .headers()
            .get("sec-fetch-mode")
            .is_some_and(|v| v == "navigate")
        && request
            .headers()
            .get("sec-fetch-dest")
            .is_some_and(|v| v == "document")
        && request
            .headers()
            .get(header::ACCEPT)
            .is_some_and(|v| v.to_str().is_ok_and(|v| v.contains("text/html")))
        && !request.headers().contains_key("purpose")
        && !request.headers().contains_key("sec-purpose")
}
async fn browser_open(
    State(state): State<Arc<AppState>>,
    Path((device, id)): Path<(String, String)>,
    Query(query): Query<BrowserOpenQuery>,
    request: Request,
) -> Response {
    let lease_live = query.lease.as_ref().is_some_and(|lease| valid_lease(lease))
        && query.lease_expires_at.as_ref().is_some_and(|expires| {
            expires
                .parse::<i64>()
                .is_ok_and(|expires| expires > chrono::Utc::now().timestamp_millis())
        });
    if document_navigation(&request) && lease_live {
        let mut cookie_headers = HeaderMap::new();
        if let Some(cookie) = request.headers().get(header::COOKIE) {
            cookie_headers.insert(header::COOKIE, cookie.clone());
        }
        let full_session = {
            let conn = state.store.conn.lock().await;
            relay_cookie(&cookie_headers)
                .is_some_and(|token| account::session_mode(&conn, &token) == account::FULL)
        };
        if full_session {
            if let Ok(url) = issue_launch(
                &state,
                device,
                id,
                &cookie_headers,
                OpenQuery {
                    lease: query.lease,
                    ..Default::default()
                },
                OpenInput { path: query.path },
            )
            .await
            {
                return preview_redirect(&url);
            }
        }
    }
    // Same-origin installations serve their entry bundle; split-origin Relays
    // redirect to the configured Web site. Neither branch loops back here.
    let mut response = spa_fallback(State(state), request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
}
fn preview_redirect(destination: &url::Url) -> Response {
    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::LOCATION, destination.as_str())
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::REFERRER_POLICY, "no-referrer")
        .body(Body::empty())
        .unwrap()
}
fn valid_lease(lease: &str) -> bool {
    lease.len() == 43
        && lease
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
async fn issue_launch(
    state: &Arc<AppState>,
    device: String,
    id: String,
    headers: &HeaderMap,
    query: OpenQuery,
    input: OpenInput,
) -> std::result::Result<url::Url, Response> {
    let mapping = state
        .preview
        .mappings
        .lock()
        .unwrap()
        .get(&id)
        .filter(|m| m.device == device)
        .cloned();
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query.token)
    else {
        return Err(unauthorized());
    };
    // Refuse before revealing whether the mapping exists.
    if !may_open(
        &conn,
        &user.id,
        &device,
        mapping.as_ref().and_then(|m| m.thread.as_deref()),
    ) {
        return Err(StatusCode::FORBIDDEN.into_response());
    }
    let Some(session) = extract_session_token(&headers, &query.token) else {
        return Err(unauthorized());
    };
    drop(conn);
    let Some(mut url) = state.preview.origin(&id) else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                json!({"message":"Port preview DNS and gateway are not configured on this Relay."}),
            ),
        )
            .into_response());
    };
    let Some(mapping) = mapping else {
        return Err((StatusCode::CONFLICT, Json(json!({"message":"This port is not enabled on the online device. Update its Supervisor if port mappings are unavailable."}))).into_response());
    };
    if query
        .lease
        .as_ref()
        .is_some_and(|lease| !valid_lease(lease))
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"code":"invalid_lease","message":"Invalid preview lease"})),
        )
            .into_response());
    }
    if query.lease.is_none() {
        // The device descriptor is only a routing hint; the device enforces its own lease.
        let key = internal_forward_json(&state, &device, "GET", "/api/transport/key", None).await;
        if !key.is_ok_and(|key| key["trust"]["strict"] != true) {
            return Err((StatusCode::FORBIDDEN,Json(json!({"code":"preview_lease_required","message":"Obtain a signed device preview lease first"}))).into_response());
        }
    }
    let path = if input.path.is_empty() {
        "/"
    } else {
        &input.path
    };
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains(['\\', '\r', '\n'])
        || path.len() > 8192
    {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }
    let Ok(destination) = url.join(path) else {
        return Err(StatusCode::BAD_REQUEST.into_response());
    };
    if destination.origin() != url.origin() {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }
    url = destination;
    if let Some(lease) = &query.lease {
        url.query_pairs_mut().append_pair("lease", lease);
    }
    let requested_grant = Grant {
        mapping: id.clone(),
        device,
        session,
        lease: query.lease,
        expires: std::time::Instant::now() + Duration::from_secs(8 * 3600),
    };
    // Reopening the same permission must not create a second speculative pool.
    // Reuse its exact deadline, never extend a live channel's authorization.
    let grant = {
        let mut pool = state.preview.idle.lock().unwrap();
        pool.prune();
        pool.clients
            .iter()
            .map(|c| (&c.client.mapping, &c.client.grant))
            .chain(pool.warming.iter().map(|c| (&c.mapping, &c.grant)))
            .filter(|(m, g)| {
                **m == mapping
                    && g.same_authority(&requested_grant)
                    && g.expires > std::time::Instant::now() + Duration::from_secs(60)
            })
            .max_by_key(|(_, g)| g.expires)
            .map(|(_, g)| g.clone())
            .unwrap_or(requested_grant)
    };
    let ticket = Uuid::new_v4().to_string();
    {
        let mut launches = state.preview.launches.lock().unwrap();
        launches.retain(|_, launch| launch.expires > std::time::Instant::now());
        if launches.len() >= 1024 {
            return Err(StatusCode::TOO_MANY_REQUESTS.into_response());
        }
        launches.insert(
            ticket.clone(),
            Launch {
                grant: grant.clone(),
                expires: std::time::Instant::now() + Duration::from_secs(60),
            },
        );
    }
    url.query_pairs_mut().append_pair("__rc_launch", &ticket);
    prewarm_http(state, &mapping, &grant);
    Ok(url)
}

fn owns(conn: &Connection, user: &str, device: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM relay_devices WHERE id=?1 AND owner_user_id=?2)",
        params![device, user],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

/// The device owner opens every mapping. A mapping an agent created from a
/// thread also opens for anyone with live access to that thread: a share or a
/// thread grant on it or an ancestor, a grant on the workspace the device
/// reports it in, or a device grant, at any access level.
fn may_open(conn: &Connection, user: &str, device: &str, thread: Option<&str>) -> bool {
    owns(conn, user, device)
        || thread.is_some_and(|thread| {
            effective_access(conn, user, device, Some(thread), None).is_some()
        })
}

fn valid_thread_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub(super) fn update(state: &AppState, device: &str, connection: Uuid, value: &Value) {
    let Some(items) = value.as_array().filter(|a| a.len() <= 32) else {
        return;
    };
    let mut catalog = state.preview.mappings.lock().unwrap();
    catalog.retain(|_, m| m.device != device);
    for item in items {
        let (Some(id), Some(port)) = (
            item["id"].as_str(),
            item["port"].as_u64().filter(|p| *p > 0 && *p <= 65535),
        ) else {
            continue;
        };
        if id.len() != 32
            || !id
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            || catalog.contains_key(id)
        {
            continue;
        }
        catalog.insert(
            id.into(),
            Mapping {
                id: id.into(),
                device: device.into(),
                connection,
                port: port as u16,
                // An unusable thread ID leaves the mapping owner-only.
                thread: item["threadId"]
                    .as_str()
                    .filter(|thread| valid_thread_id(thread))
                    .map(Into::into),
            },
        );
    }
    state.preview.streams.lock().unwrap().retain(|_, s| {
        let live = catalog.get(&s.mapping.id).is_some_and(|m| m == &s.mapping);
        if !live {
            s.cancel.cancel();
        }
        live
    });
    state.preview.idle.lock().unwrap().clients.retain(|c| {
        catalog
            .get(&c.client.mapping.id)
            .is_some_and(|m| m == &c.client.mapping)
    });
}

pub(super) fn disconnected(state: &AppState, device: &str, connection: Uuid) {
    let mut catalog = state.preview.mappings.lock().unwrap();
    catalog.retain(|_, m| !(m.device == device && m.connection == connection));
    state.preview.streams.lock().unwrap().retain(|_, s| {
        let live = !(s.mapping.device == device && s.mapping.connection == connection);
        if !live {
            s.cancel.cancel();
        }
        live
    });
    state.preview.idle.lock().unwrap().clients.retain(|c| {
        catalog
            .get(&c.client.mapping.id)
            .is_some_and(|m| m == &c.client.mapping)
    });
}

fn browser_entry(state: &AppState, request: &Request, mapping: &Mapping) -> Option<url::Url> {
    if !matches!(*request.method(), Method::GET | Method::HEAD)
        || request.headers().get("sec-fetch-mode")?.to_str().ok()? != "navigate"
        || !request
            .headers()
            .get(header::ACCEPT)?
            .to_str()
            .ok()?
            .contains("text/html")
    {
        return None;
    }
    // Use configured control-plane origin, never the caller's Host/Forwarded headers.
    let mut destination = url::Url::parse(state.oauth.public_base_url.as_deref()?).ok()?;
    if !matches!(destination.scheme(), "http" | "https")
        || destination.host_str().is_none()
        || !destination.username().is_empty()
        || destination.password().is_some()
    {
        return None;
    }
    destination.set_path(&format!("/devices/{}/ports/{}", mapping.device, mapping.id));
    destination.set_query(None);
    destination.set_fragment(None);
    let query = request.uri().query().unwrap_or("");
    let app = app_query(query);
    let path = if app.is_empty() {
        request.uri().path().to_owned()
    } else {
        format!("{}?{app}", request.uri().path())
    };
    destination.query_pairs_mut().append_pair("path", &path);
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if matches!(key.as_ref(), "lease" | "leaseExpiresAt") {
            destination.query_pairs_mut().append_pair(&key, &value);
        }
    }
    Some(destination)
}

fn require_browser_login(state: &AppState, request: &Request, mapping: &Mapping) -> Response {
    // Only HTML navigations enter login. API/assets/WS retain 401.
    if let Some(destination) = browser_entry(state, request, mapping) {
        return Response::builder()
            .status(StatusCode::SEE_OTHER)
            .header(header::LOCATION, destination.as_str())
            .header(header::CACHE_CONTROL, "no-store")
            .header(header::REFERRER_POLICY, "no-referrer")
            .body(Body::empty())
            .unwrap();
    }
    (
        StatusCode::UNAUTHORIZED,
        "Open this port from your signed-in Pockymoe device page.",
    )
        .into_response()
}

// Strip control-plane parameters without normalizing application query bytes,
// duplicate keys or bare flags. Match decoded names so encoded aliases cannot leak.
fn app_query(query: &str) -> String {
    query
        .split('&')
        .filter(|pair| {
            url::form_urlencoded::parse(pair.as_bytes())
                .next()
                .is_none_or(|(key, _)| {
                    !matches!(key.as_ref(), "__rc_launch" | "lease" | "leaseExpiresAt")
                })
        })
        .collect::<Vec<_>>()
        .join("&")
}

// Run outside the control-plane security middleware: local applications own their
// CSP and websocket Origin, and must never be served on the control-plane origin.
pub(super) async fn middleware(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: axum::middleware::Next,
) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(id) = state.preview.id_for_host(host) else {
        return next.run(request).await;
    };
    let Some(mapping) = state.preview.mappings.lock().unwrap().get(&id).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(origin) = state.preview.origin(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let launch = request.uri().query().and_then(|q| {
        url::form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == "__rc_launch")
            .map(|(_, v)| v.into_owned())
    });
    if let Some(ticket) = launch {
        if request.method() != Method::GET {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let launch = state.preview.launches.lock().unwrap().remove(&ticket);
        let Some(launch) = launch.filter(|launch| {
            launch.grant.mapping == id
                && launch.grant.device == mapping.device
                && launch.expires > std::time::Instant::now()
        }) else {
            return unauthorized();
        };
        let grant = launch.grant;
        if grant.expires <= std::time::Instant::now() || !valid_grant(&state, &grant).await {
            return unauthorized();
        }
        let session = Uuid::new_v4().to_string();
        let mut sessions = state.preview.sessions.lock().unwrap();
        sessions.retain(|_, g| g.expires > std::time::Instant::now());
        if sessions.len() >= 4096 {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
        let max_age = grant
            .expires
            .saturating_duration_since(std::time::Instant::now())
            .as_secs()
            .max(1);
        sessions.insert(session.clone(), grant);
        let mut destination = origin.clone();
        destination.set_path(request.uri().path());
        let query = app_query(request.uri().query().unwrap_or(""));
        destination.set_query((!query.is_empty()).then_some(query.as_str()));
        let secure = origin.scheme() == "https";
        let cookie_name = if secure { COOKIE } else { LOCAL_COOKIE };
        return Response::builder()
            .status(StatusCode::SEE_OTHER)
            .header(header::LOCATION, destination.as_str())
            .header(header::CACHE_CONTROL, "no-store")
            .header(header::REFERRER_POLICY, "no-referrer")
            .header(
                header::SET_COOKIE,
                format!(
                    "{cookie_name}={session}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
                    if secure { "; Secure" } else { "" }
                ),
            )
            .body(Body::empty())
            .unwrap();
    }
    let cookie_name = if origin.scheme() == "https" {
        COOKIE
    } else {
        LOCAL_COOKIE
    };
    let session = security::cookie(request.headers(), cookie_name);
    let grant = session
        .as_ref()
        .and_then(|s| state.preview.sessions.lock().unwrap().get(s).cloned());
    let Some(grant) = grant.filter(|g| {
        g.mapping == id && g.device == mapping.device && g.expires > std::time::Instant::now()
    }) else {
        return require_browser_login(&state, &request, &mapping);
    };
    if !valid_grant(&state, &grant).await {
        return require_browser_login(&state, &request, &mapping);
    }
    if let Some(value) = request.headers().get(header::ORIGIN) {
        if !value
            .to_str()
            .ok()
            .and_then(|s| url::Url::parse(s).ok())
            .is_some_and(|o| o.origin() == origin.origin())
        {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let upstream = format!("http://127.0.0.1:{}", mapping.port);
    let websocket = request
        .headers()
        .get(header::UPGRADE)
        .is_some_and(|s| s.as_bytes().eq_ignore_ascii_case(b"websocket"));
    let browser_upgrade = websocket.then(|| hyper::upgrade::on(&mut request));
    let original_host = origin[url::Position::BeforeHost..url::Position::AfterPort].to_string();
    let query = app_query(request.uri().query().unwrap_or(""));
    let target = if query.is_empty() {
        request.uri().path().to_string()
    } else {
        format!("{}?{query}", request.uri().path())
    };
    *request.uri_mut() = target.parse().unwrap();
    strip_hop_headers(request.headers_mut(), websocket);
    let headers = request.headers_mut();
    headers.insert(
        header::HOST,
        format!("127.0.0.1:{}", mapping.port).parse().unwrap(),
    );
    headers.insert("x-forwarded-host", original_host.parse().unwrap());
    headers.insert("x-forwarded-proto", origin.scheme().parse().unwrap());
    if headers.contains_key(header::ORIGIN) {
        headers.insert(header::ORIGIN, upstream.parse().unwrap());
    }
    let cookies = app_cookies(headers);
    headers.remove(header::COOKIE);
    if !cookies.is_empty() {
        headers.insert(header::COOKIE, cookies.parse().unwrap());
    }
    if websocket {
        headers.insert(header::CONNECTION, "upgrade".parse().unwrap());
    }
    let result = async {
        // Upgrades own their connection permanently and never enter the HTTP pool.
        let mut client = http_client(&state, &mapping, &grant, !websocket).await?;
        let mut response =
            tokio::time::timeout(Duration::from_secs(30), client.sender.send_request(request))
                .await??;
        if websocket && response.status() == StatusCode::SWITCHING_PROTOCOLS {
            let upstream_upgrade = hyper::upgrade::on(&mut response);
            tokio::spawn(async move {
                if let (Ok(browser), Ok(upstream)) =
                    tokio::join!(browser_upgrade.unwrap(), upstream_upgrade)
                {
                    let _ = tokio::io::copy_bidirectional(
                        &mut TokioIo::new(browser),
                        &mut TokioIo::new(upstream),
                    )
                    .await;
                }
            });
        }
        let upgraded = websocket && response.status() == StatusCode::SWITCHING_PROTOCOLS;
        strip_hop_headers(response.headers_mut(), upgraded);
        rewrite_response_headers(response.headers_mut(), &origin, mapping.port);
        let (parts, body) = response.into_parts();
        let mut body = HttpResponseBody {
            body,
            client: (!websocket).then_some(client),
            state: state.clone(),
        };
        if body.is_end_stream() {
            body.complete();
        }
        Ok::<_, anyhow::Error>(Response::from_parts(parts, Body::new(body)))
    }
    .await;
    result.unwrap_or_else(|error| {
        tracing::debug!(%error, mapping_id = %id, "port preview request failed");
        (StatusCode::BAD_GATEWAY,"The device's local web service is unavailable. Check that the service and Supervisor are running.").into_response()
    })
}

async fn valid_grant(state: &AppState, grant: &Grant) -> bool {
    // Use the thread the device reports now, so moving the mapping to another
    // thread or revoking a share ends a recipient's access at the next check.
    let thread = state
        .preview
        .mappings
        .lock()
        .unwrap()
        .get(&grant.mapping)
        .filter(|m| m.device == grant.device)
        .and_then(|m| m.thread.clone());
    let conn = state.store.conn.lock().await;
    load_user_by_session(&conn, &state.store.session_secret, &grant.session).is_some_and(|u| {
        u.role != "admin" && may_open(&conn, &u.id, &grant.device, thread.as_deref())
    })
}

fn app_cookies(headers: &HeaderMap) -> String {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .map(str::trim)
        .filter(|c| {
            let name = c.split('=').next().unwrap_or("");
            name != COOKIE && name != LOCAL_COOKIE && !name.starts_with("remote_codex_relay_")
        })
        .collect::<Vec<_>>()
        .join("; ")
}
fn strip_hop_headers(headers: &mut HeaderMap, websocket: bool) {
    let nominated = headers
        .get(header::CONNECTION)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .collect::<Vec<_>>();
    for name in nominated {
        if !(websocket && name.eq_ignore_ascii_case("upgrade")) {
            headers.remove(name.as_str());
        }
    }
    for name in [
        "connection",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "keep-alive",
        "te",
        "trailer",
        "transfer-encoding",
    ] {
        headers.remove(name);
    }
    if !websocket {
        headers.remove(header::UPGRADE);
    } else {
        headers.insert(header::CONNECTION, "upgrade".parse().unwrap());
    }
}
fn rewrite_response_headers(headers: &mut HeaderMap, origin: &url::Url, port: u16) {
    if let Some(url) = headers
        .get(header::LOCATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| url::Url::parse(s).ok())
    {
        if matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
            && url.port_or_known_default() == Some(port)
        {
            let mut rewritten = origin.clone();
            rewritten.set_path(url.path());
            rewritten.set_query(url.query());
            rewritten.set_fragment(url.fragment());
            headers.insert(header::LOCATION, rewritten.as_str().parse().unwrap());
        }
    }
    let cookies = headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    headers.remove(header::SET_COOKIE);
    for cookie in cookies {
        let name = cookie.split('=').next().unwrap_or("").trim();
        if name == COOKIE || name == LOCAL_COOKIE || name.starts_with("remote_codex_relay_") {
            continue;
        }
        let rewritten = cookie
            .split(';')
            .filter(|part| !part.trim().to_ascii_lowercase().starts_with("domain="))
            .collect::<Vec<_>>()
            .join(";");
        if let Ok(value) = rewritten.parse() {
            headers.append(header::SET_COOKIE, value);
        }
    }
}

struct Pending<'a> {
    hub: &'a Hub,
    ticket: String,
}
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        let mut streams = self.hub.streams.lock().unwrap();
        if streams.get(&self.ticket).is_some_and(|s| s.tx.is_some()) {
            streams.remove(&self.ticket);
        }
    }
}
// The upgrade callback owns this guard even if the handshake fails or is
// cancelled. Do not leave a claimed ticket occupying a slot indefinitely.
struct StreamGuard {
    state: Arc<AppState>,
    ticket: String,
    closed: CancellationToken,
}
impl Drop for StreamGuard {
    fn drop(&mut self) {
        if let Some(stream) = self
            .state
            .preview
            .streams
            .lock()
            .unwrap()
            .remove(&self.ticket)
        {
            stream.cancel.cancel();
        }
        self.closed.cancel();
    }
}
async fn open_stream(
    state: &AppState,
    mapping: &Mapping,
    grant: &Grant,
    opportunistic: bool,
) -> Result<(DuplexStream, CancellationToken, CancellationToken)> {
    let cancel = CancellationToken::new();
    let closed = CancellationToken::new();
    let ticket = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let pending = Pending {
        hub: &state.preview,
        ticket: ticket.clone(),
    };
    loop {
        let released = {
            let mut streams = state.preview.streams.lock().unwrap();
            let device_full = streams
                .values()
                .filter(|s| s.mapping.device == mapping.device)
                .count()
                >= 32;
            if streams.len() < 256 && !device_full {
                streams.insert(
                    ticket.clone(),
                    Stream {
                        mapping: mapping.clone(),
                        grant: grant.clone(),
                        tx: Some(tx),
                        cancel: cancel.clone(),
                        closed: closed.clone(),
                    },
                );
                break;
            }
            if opportunistic {
                bail!("No spare preview connection capacity");
            }
            // Idle connections are opportunistic: reclaim the oldest eligible
            // one before refusing live traffic, including another capability.
            let mut pool = state.preview.idle.lock().unwrap();
            pool.prune();
            let oldest = pool
                .clients
                .iter()
                .enumerate()
                .filter(|(_, c)| !device_full || c.client.mapping.device == mapping.device)
                .min_by_key(|(_, c)| c.expires)
                .map(|(i, _)| i);
            if let Some(oldest) = oldest {
                let client = pool.clients.swap_remove(oldest).client;
                let released = client.closed.clone();
                pool.discard(client);
                released
            } else {
                pool.closing
                    .iter()
                    .find(|c| !device_full || c.device == mapping.device)
                    .map(|c| c.closed.clone())
                    .ok_or_else(|| anyhow::anyhow!("Too many preview connections"))?
            }
        };
        // Dropping the idle HTTP sender closes its TCP bridge normally. Wait for
        // its WS close handshake/guard, so the device can also free its slot.
        // Never remove a still-running stream just to bypass either limit.
        tokio::time::timeout(Duration::from_secs(2), released.cancelled()).await?;
    }
    {
        let sockets = state.sockets.read().await;
        let socket = sockets
            .get(&mapping.device)
            .filter(|s| s.connection_id == mapping.connection)
            .ok_or_else(|| anyhow::anyhow!("Device offline"))?;
        socket.tx.try_send(
            json!({"type":"preview.open","mappingId":mapping.id,"ticket":ticket,"leaseId":grant.lease}).to_string(),
        )?;
    }
    let stream = tokio::time::timeout(Duration::from_secs(10), rx).await??;
    drop(pending);
    Ok((stream, cancel, closed))
}

async fn device_stream(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Path(ticket): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return unauthorized();
    };
    let device = {
        let conn = state.store.conn.lock().await;
        device_id_for_supervisor_token(&conn, &token, state.legacy_supervisor_token.as_deref())
    };
    let Some(device) = device else {
        return unauthorized();
    };
    let sockets = state.sockets.read().await;
    let connection = sockets.get(&device).map(|s| s.connection_id);
    let pairing = {
        let mut streams = state.preview.streams.lock().unwrap();
        streams
            .get_mut(&ticket)
            .filter(|s| s.mapping.device == device && Some(s.mapping.connection) == connection)
            .and_then(|s| {
                s.tx.take()
                    .map(|tx| (tx, s.cancel.clone(), s.closed.clone(), s.grant.clone()))
            })
    };
    drop(sockets);
    let Some((tx, cancel, closed, grant)) = pairing else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let guard = StreamGuard {
        state,
        ticket,
        closed,
    };
    ws.max_message_size(128 * 1024)
        .max_frame_size(128 * 1024)
        .on_upgrade(move |socket| async move {
            let (stream, bridge) = tokio::io::duplex(128 * 1024);
            if tx.send(stream).is_ok() {
                bridge_socket(socket, bridge, cancel, &guard.state, grant).await;
            }
            drop(guard);
        })
        .into_response()
}

async fn bridge_socket(
    socket: WebSocket,
    bridge: DuplexStream,
    cancel: CancellationToken,
    state: &AppState,
    grant: Grant,
) {
    let (mut sink, mut source) = socket.split();
    let (mut read, mut write) = tokio::io::split(bridge);
    {
        let send = async {
            let mut buffer = vec![0; 64 * 1024];
            let mut ping = tokio::time::interval(Duration::from_secs(20));
            ping.tick().await;
            loop {
                tokio::select! {
                    count=read.read(&mut buffer)=>{let count=count?;if count==0 {sink.close().await?;break;}sink.send(Message::Binary(buffer[..count].to_vec().into())).await?;}
                    _=ping.tick()=>sink.send(Message::Ping(Vec::new().into())).await?,
                }
            }
            Ok::<_, anyhow::Error>(())
        };
        let receive = async {
            while let Some(message) = source.next().await {
                match message? {
                    Message::Binary(bytes) => write.write_all(&bytes).await?,
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            Ok::<_, anyhow::Error>(())
        };
        let authorization = async {
            let mut interval = tokio::time::interval(Duration::from_secs(15));
            loop {
                interval.tick().await;
                if grant.expires <= std::time::Instant::now() || !valid_grant(state, &grant).await {
                    break;
                }
            }
        };
        tokio::pin!(send, receive);
        tokio::select! {
            result=&mut send=>{
                if result.is_ok() { let _=tokio::time::timeout(Duration::from_secs(2),&mut receive).await; }
            },
            _=&mut receive=>{}, _=cancel.cancelled()=>{}, _=authorization=>{}
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), sink.close()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_hosts_are_exact_origins_and_do_not_accept_credentials_or_paths() {
        let hub = Hub {
            base: Some(parse_base("https://example.test").unwrap()),
            ..Hub::default()
        };
        let id = "a".repeat(32);
        assert_eq!(
            hub.origin(&id).unwrap().as_str(),
            format!("https://p-{id}.example.test/")
        );
        assert_eq!(
            hub.id_for_host(&format!("p-{id}.example.test")),
            Some(id.clone())
        );
        for host in [
            format!("p-{id}.example.test.evil"),
            format!("p-{id}.example.test:81"),
            "example.test".into(),
            "p-short.example.test".into(),
            format!("user@p-{id}.example.test"),
            format!("p-{id}.example.test/path"),
        ] {
            assert!(hub.id_for_host(&host).is_none());
        }
        for base in [
            "https://127.0.0.1",
            "http://[::1]",
            "https://user@example.test",
            "https://example.test/path",
            "https://example.test/?q=1",
            "ftp://example.test",
        ] {
            assert!(parse_base(base).is_err());
        }
    }

    #[test]
    fn preview_headers_preserve_app_cookies_without_leaking_gateway_credentials() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE,format!("{COOKIE}=secret; app=value; remote_codex_relay_session=relay; {LOCAL_COOKIE}=secret").parse().unwrap());
        assert_eq!(app_cookies(&headers), "app=value");
        headers.append(
            header::SET_COOKIE,
            "app=value; Domain=localhost; HttpOnly; Path=/"
                .parse()
                .unwrap(),
        );
        headers.append(
            header::SET_COOKIE,
            format!("{COOKIE}=forged").parse().unwrap(),
        );
        headers.insert(
            header::LOCATION,
            "http://localhost:4013/app?q=1#part".parse().unwrap(),
        );
        headers.insert(
            header::CONNECTION,
            "keep-alive, X-Private-Hop".parse().unwrap(),
        );
        headers.insert("x-private-hop", "remove".parse().unwrap());
        strip_hop_headers(&mut headers, false);
        rewrite_response_headers(
            &mut headers,
            &parse_base("https://p-map.example.test").unwrap(),
            4013,
        );
        assert!(!headers.contains_key("x-private-hop"));
        assert_eq!(
            headers[header::LOCATION],
            "https://p-map.example.test/app?q=1#part"
        );
        assert_eq!(headers.get_all(header::SET_COOKIE).iter().count(), 1);
        assert_eq!(headers[header::SET_COOKIE], "app=value; HttpOnly; Path=/");
    }

    #[tokio::test]
    async fn stopping_or_reconnecting_a_mapping_cancels_streams_and_upgrade_failure_releases_slots()
    {
        let (state, dir) = crate::tests::test_app_state("preview-cleanup");
        let id = "b".repeat(32);
        let connection = Uuid::new_v4();
        update(
            &state,
            "device",
            connection,
            &json!([{"id":id,"port":4013}]),
        );
        let mapping = state.preview.mappings.lock().unwrap()[&id].clone();
        let grant = Grant {
            mapping: id.clone(),
            device: "device".into(),
            session: String::new(),
            lease: None,
            expires: std::time::Instant::now() + Duration::from_secs(60),
        };
        let token = CancellationToken::new();
        state.preview.streams.lock().unwrap().insert(
            "ticket".into(),
            Stream {
                mapping: mapping.clone(),
                grant: grant.clone(),
                tx: None,
                cancel: token.clone(),
                closed: CancellationToken::new(),
            },
        );
        // An older connection's cleanup cannot erase a newer device connection.
        disconnected(&state, "device", Uuid::new_v4());
        assert!(!token.is_cancelled());
        update(
            &state,
            "device",
            Uuid::new_v4(),
            &json!([{"id":id,"port":4013}]),
        );
        assert!(token.is_cancelled());
        assert!(state.preview.streams.lock().unwrap().is_empty());
        let token = CancellationToken::new();
        state.preview.streams.lock().unwrap().insert(
            "failed-upgrade".into(),
            Stream {
                mapping,
                grant,
                tx: None,
                cancel: token.clone(),
                closed: CancellationToken::new(),
            },
        );
        drop(StreamGuard {
            state: state.clone(),
            ticket: "failed-upgrade".into(),
            closed: CancellationToken::new(),
        });
        assert!(token.is_cancelled());
        assert!(state.preview.streams.lock().unwrap().is_empty());
        update(&state, "device", connection, &json!([]));
        assert!(state.preview.mappings.lock().unwrap().is_empty());
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
    async fn lease_fixture(
        name: &str,
    ) -> (
        Arc<AppState>,
        PathBuf,
        HeaderMap,
        tokio::sync::mpsc::Receiver<String>,
        String,
    ) {
        let (mut state, dir) = crate::tests::test_app_state(name);
        Arc::get_mut(&mut state).unwrap().preview.base =
            Some(parse_base("https://preview.example.test").unwrap());
        let token = {
            let conn = state.store.conn.lock().await;
            conn.execute("INSERT INTO relay_users (id,email,username,role,enabled,created_at,password_salt,password_hash) VALUES ('owner','owner@example.test','owner','user',1,'now','','')",[]).unwrap();
            conn.execute("INSERT INTO relay_devices(id,owner_user_id,name,token_hash,token_preview,created_at) VALUES ('device','owner','device','hash','test','now')",[]).unwrap();
            create_session(&conn, &state.store.session_secret, "owner").unwrap()
        };
        let connection = Uuid::new_v4();
        // Allow a browser-sized burst without the fake control channel being
        // smaller than the production data-connection budget.
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        state.sockets.write().await.insert(
            "device".into(),
            DeviceSocket {
                tx,
                connection_id: connection,
                connected_at: now_rfc3339(),
                last_heartbeat_at: now_rfc3339(),
            },
        );
        let mapping = "c".repeat(32);
        update(
            &state,
            "device",
            connection,
            &json!([{"id":mapping,"port":4013}]),
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        (state, dir, headers, rx, mapping)
    }

    #[tokio::test]
    async fn security_revision_strict_preview_refuses_open_without_device_lease() {
        let (state, dir, headers, mut rx, mapping) =
            lease_fixture("security-preview-no-lease").await;
        let responder = state.clone();
        let task = tokio::spawn(async move {
            if let Ok(Some(frame)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                let frame: Value = serde_json::from_str(&frame).unwrap();
                assert_eq!(frame["payload"]["path"], "/api/transport/key");
                let id = frame["requestId"].as_str().unwrap();
                let pending = responder.pending.lock().unwrap().remove(id).unwrap();
                pending.tx.send(json!({"statusCode":200,"body":json!({"trust":{"strict":true}}).to_string()})).unwrap();
            }
        });
        let response = open(
            State(state.clone()),
            Path(("device".into(), mapping)),
            headers,
            Query(serde_json::from_value(json!({})).unwrap()),
            Json(OpenInput::default()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(state.preview.launches.lock().unwrap().is_empty());
        task.await.unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn security_revision_preview_lease_reaches_host_launch_and_every_device_open_frame() {
        let (state, dir, headers, mut rx, mapping_id) =
            lease_fixture("security-preview-lease").await;
        let lease = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([222; 32]);
        let response = open(
            State(state.clone()),
            Path(("device".into(), mapping_id.clone())),
            headers,
            Query(serde_json::from_value(json!({"lease":lease})).unwrap()),
            Json(OpenInput::default()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let url = url::Url::parse(value["url"].as_str().unwrap()).unwrap();
        assert!(url.query_pairs().any(|(k, v)| k == "lease" && v == lease));
        let grant = state
            .preview
            .launches
            .lock()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .grant
            .clone();
        let mapping = state.preview.mappings.lock().unwrap()[&mapping_id].clone();
        let caller = state.clone();
        let task = tokio::spawn(async move { http_client(&caller, &mapping, &grant, true).await });
        let frame: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(frame["type"], "preview.open");
        assert_eq!(frame["leaseId"], lease);
        let ticket = frame["ticket"].as_str().unwrap();
        let tx = state
            .preview
            .streams
            .lock()
            .unwrap()
            .get_mut(ticket)
            .unwrap()
            .tx
            .take()
            .unwrap();
        let (stream, _device) = tokio::io::duplex(1024);
        tx.send(stream).unwrap();
        assert!(task.await.unwrap().is_ok());
        drop(state);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn documentation_preview_lease_validation_uses_device_alphabet_and_length() {
        let (state, dir, headers, _rx, mapping) = lease_fixture("doc-preview-lease-shape").await;
        for lease in [
            Uuid::new_v4().to_string(),
            "a".repeat(42),
            "a".repeat(44),
            format!("{}+", "a".repeat(42)),
            format!("{}/", "a".repeat(42)),
            format!("{}=", "a".repeat(42)),
            format!("{}é", "a".repeat(41)),
        ] {
            let response = open(
                State(state.clone()),
                Path(("device".into(), mapping.clone())),
                headers.clone(),
                Query(serde_json::from_value(json!({"lease":lease})).unwrap()),
                Json(OpenInput::default()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let value: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
            assert_eq!(value["code"], "invalid_lease");
        }
        assert!(state.preview.launches.lock().unwrap().is_empty());
        // Alphabet and length are the wire contract; UUID or padding is not required.
        let response = open(
            State(state.clone()),
            Path(("device".into(), mapping)),
            headers,
            Query(serde_json::from_value(json!({"lease":format!("{}_-","A".repeat(41))})).unwrap()),
            Json(OpenInput::default()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        drop(state);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn browser_handoff_uses_owner_cookie_and_existing_lease_or_falls_back_to_web() {
        for separate in [false, true] {
            let (mut state, dir, headers, _rx, mapping) =
                lease_fixture("preview-browser-handoff").await;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let dist = dir.join("web");
            std::fs::create_dir_all(&dist).unwrap();
            std::fs::write(
                dist.join("index.html"),
                "<html><head></head><body>Web fallback</body></html>",
            )
            .unwrap();
            let mutable = Arc::get_mut(&mut state).unwrap();
            mutable.oauth.public_base_url = Some(base.clone());
            mutable.oauth.web_origin = separate.then(|| "https://web.example.test".into());
            mutable.web_dist = Some(dist);
            let session = extract_session_token(&headers, &TokenQuery::default()).unwrap();
            let cookie = format!("remote_codex_relay_session={session}");
            let other = {
                let conn = state.store.conn.lock().await;
                conn.execute("INSERT INTO relay_users (id,email,username,role,enabled,created_at,password_salt,password_hash) VALUES ('other','other@example.test','other','user',1,'now','','')", []).unwrap();
                let other = create_session(&conn, &state.store.session_secret, "other").unwrap();
                for (user, token) in [("owner", session.as_str()), ("other", other.as_str())] {
                    account::store_vault(&conn, user, &json!({"revision":1,"accountKeyId":"fixture-key","accountPublicKey":"fixture-public"})).unwrap();
                    account::mark_session(&conn, token, account::FULL).unwrap();
                }
                other
            };
            let app = Router::new()
                .merge(routes())
                .fallback(spa_fallback)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    security::browser_security,
                ))
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    middleware,
                ))
                .with_state(state.clone());
            let relay = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            let lease = "L".repeat(43);
            let entry = format!("{base}/devices/device/ports/{mapping}");
            let query = |path: &str, lease: &str, expires: &str| {
                [
                    ("path", path.to_owned()),
                    ("lease", lease.to_owned()),
                    ("leaseExpiresAt", expires.to_owned()),
                ]
            };
            let future = (chrono::Utc::now().timestamp_millis() + 3_600_000).to_string();
            let document = |path: &str, lease: &str, expires: &str| {
                client
                    .get(&entry)
                    .query(&query(path, lease, expires))
                    .header(header::ACCEPT, "text/html")
                    .header("sec-fetch-mode", "navigate")
                    .header("sec-fetch-dest", "document")
            };
            let owner_document =
                || document("/app?q=1#section", &lease, &future).header(header::COOKIE, &cookie);
            let header_override = |name: &'static str, value: &'static str| {
                let mut headers = HeaderMap::new();
                headers.insert(name, value.parse().unwrap());
                headers
            };
            // These must not mint a capability or start a device connection. The
            // mapping has no thread, so another account cannot open it.
            let cases = [
                ("anonymous", document("/", &lease, &future)),
                (
                    "fetch",
                    owner_document().headers(header_override("sec-fetch-mode", "cors")),
                ),
                (
                    "iframe",
                    owner_document().headers(header_override("sec-fetch-dest", "iframe")),
                ),
                ("prefetch", owner_document().header("purpose", "prefetch")),
                (
                    "speculative",
                    owner_document().header("sec-purpose", "prefetch"),
                ),
                (
                    "bearer",
                    document("/", &lease, &future)
                        .header(header::AUTHORIZATION, format!("Bearer {session}")),
                ),
                (
                    "URL token",
                    document("/", &lease, &future).query(&[("token", session.as_str())]),
                ),
                (
                    "other account",
                    document("/", &lease, &future).header(
                        header::COOKIE,
                        format!("remote_codex_relay_session={other}"),
                    ),
                ),
                (
                    "expired",
                    document("/", &lease, "1").header(header::COOKIE, &cookie),
                ),
                (
                    "unknown expiry",
                    document("/", &lease, "").header(header::COOKIE, &cookie),
                ),
                (
                    "invalid lease",
                    document("/", "invalid", &future).header(header::COOKIE, &cookie),
                ),
                (
                    "external path",
                    document("//evil.example/path", &lease, &future)
                        .header(header::COOKIE, &cookie),
                ),
            ];
            let fallback_status = if separate {
                StatusCode::TEMPORARY_REDIRECT
            } else {
                StatusCode::OK
            };
            for (case, request) in cases {
                let response = request.send().await.unwrap();
                assert_eq!(response.status(), fallback_status, "{case}");
                if separate {
                    assert!(response.headers()[header::LOCATION]
                        .to_str()
                        .unwrap()
                        .starts_with("https://web.example.test/devices/"));
                } else {
                    assert!(response.text().await.unwrap().contains("Web fallback"));
                }
                assert!(state.preview.launches.lock().unwrap().is_empty(), "{case}");
                assert!(
                    state.preview.idle.lock().unwrap().warming.is_empty(),
                    "{case}"
                );
            }
            // Incomplete/recovery sessions need the Web flow, not a raw API error.
            for mode in [account::SETUP, account::RECOVERY, account::RESET] {
                account::mark_session(&*state.store.conn.lock().await, &session, mode).unwrap();
                assert_eq!(
                    owner_document().send().await.unwrap().status(),
                    fallback_status
                );
                assert!(state.preview.launches.lock().unwrap().is_empty());
            }
            account::mark_session(&*state.store.conn.lock().await, &session, account::FULL)
                .unwrap();
            // The existing POST still requires the normal origin/CSRF proof.
            let denied = client
                .post(format!(
                    "{base}/relay/devices/device/port-mappings/{mapping}/open"
                ))
                .header(header::COOKIE, &cookie)
                .json(&json!({"path":"/"}))
                .send()
                .await
                .unwrap();
            assert_eq!(denied.status(), StatusCode::FORBIDDEN);
            let response = client
                .get(&entry)
                .query(&query("/app?q=1#section", &lease, &future))
                .header(header::ACCEPT, "text/html")
                .header("sec-fetch-mode", "navigate")
                .header("sec-fetch-dest", "document")
                .header(header::COOKIE, &cookie)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
            let destination =
                url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
            assert_eq!(destination.path(), "/app");
            assert_eq!(destination.fragment(), Some("section"));
            assert!(destination.host_str().unwrap().starts_with("p-"));
            let launch = state
                .preview
                .launches
                .lock()
                .unwrap()
                .values()
                .next()
                .unwrap()
                .clone();
            assert_eq!(launch.grant.session, session);
            assert_eq!(launch.grant.lease.as_deref(), Some(lease.as_str()));
            assert!(launch.expires < launch.grant.expires);
            let path = format!("/app?{}", destination.query().unwrap());
            let redeem = || {
                client
                    .get(format!("{base}{path}"))
                    .header(header::HOST, destination.host_str().unwrap())
            };
            let response = redeem().send().await.unwrap();
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert!(response.headers().contains_key(header::SET_COOKIE));
            assert_eq!(
                state
                    .preview
                    .sessions
                    .lock()
                    .unwrap()
                    .values()
                    .next()
                    .unwrap()
                    .expires,
                launch.grant.expires
            );
            assert_eq!(
                redeem().send().await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
            // Someone the creating thread is shared with follows the same CLI link.
            let thread = Uuid::new_v4().to_string();
            state.store.conn.lock().await.execute("INSERT INTO relay_shares(id,owner_user_id,target_user_id,device_id,thread_id,thread_access,created_at) VALUES ('share','owner','other','device',?1,'read','now')", [&thread]).unwrap();
            let connection = state.sockets.read().await["device"].connection_id;
            update(
                &state,
                "device",
                connection,
                &json!([{"id":mapping,"port":4013,"threadId":thread}]),
            );
            let response = document("/app", &lease, &future)
                .header(
                    header::COOKIE,
                    format!("remote_codex_relay_session={other}"),
                )
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let destination =
                url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
            assert!(destination.host_str().unwrap().starts_with("p-"));
            assert!(state
                .preview
                .launches
                .lock()
                .unwrap()
                .values()
                .any(|launch| launch.grant.session == other));
            update(&state, "device", Uuid::new_v4(), &json!([]));
            relay.abort();
            drop(state);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn browser_login_keeps_lease_separate_from_the_application_query() {
        let (mut state, dir) = crate::tests::test_app_state("preview-login-lease");
        Arc::get_mut(&mut state).unwrap().oauth.public_base_url =
            Some("https://relay.example.test".into());
        let mapping = Mapping {
            id: "a".repeat(32),
            device: "device".into(),
            connection: Uuid::new_v4(),
            port: 4013,
            thread: None,
        };
        let request = Request::builder()
            .uri("/work?app=a%2Fb&app=two&flag&lease=existing&leaseExpiresAt=123")
            .header("sec-fetch-mode", "navigate")
            .header(header::ACCEPT, "text/html")
            .body(Body::empty())
            .unwrap();
        let destination = browser_entry(&state, &request, &mapping).unwrap();
        let query: HashMap<_, _> = destination.query_pairs().into_owned().collect();
        assert_eq!(query["path"], "/work?app=a%2Fb&app=two&flag");
        assert_eq!(query["lease"], "existing");
        assert_eq!(query["leaseExpiresAt"], "123");
        drop(state);
        let _ = std::fs::remove_dir_all(dir);
    }

    struct HttpFixture {
        state: Arc<AppState>,
        dir: PathBuf,
        mapping: String,
        base: String,
        browser: reqwest::Client,
        opened: Arc<std::sync::atomic::AtomicUsize>,
        relay: tokio::task::JoinHandle<()>,
        device: tokio::task::JoinHandle<()>,
    }
    impl HttpFixture {
        fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
            self.browser
                .request(method, format!("{}{path}", self.base))
                .header(
                    header::HOST,
                    format!("p-{}.preview.example.test", self.mapping),
                )
                .header(header::COOKIE, "__Host-rc_port_preview=test-cookie")
        }
        fn opens(&self) -> usize {
            self.opened.load(std::sync::atomic::Ordering::Relaxed)
        }
        fn grant(&self) -> Grant {
            self.state.preview.sessions.lock().unwrap()["test-cookie"].clone()
        }
    }
    impl Drop for HttpFixture {
        fn drop(&mut self) {
            self.relay.abort();
            self.device.abort();
            for stream in self.state.preview.streams.lock().unwrap().values() {
                stream.cancel.cancel();
            }
            self.state.preview.idle.lock().unwrap().clients.clear();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    async fn http_fixture<F, Fut>(name: &str, delay: Duration, service: F) -> HttpFixture
    where
        F: Fn(hyper::Request<hyper::body::Incoming>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Response, std::io::Error>> + Send,
    {
        let (state, dir, headers, mut rx, mapping) = lease_fixture(name).await;
        state.preview.sessions.lock().unwrap().insert(
            "test-cookie".into(),
            Grant {
                mapping: mapping.clone(),
                device: "device".into(),
                session: extract_session_token(&headers, &TokenQuery::default()).unwrap(),
                lease: Some("test-lease".into()),
                expires: std::time::Instant::now() + Duration::from_secs(60),
            },
        );
        let opened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = opened.clone();
        let responder = state.clone();
        let service = Arc::new(service);
        let device = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                let frame: Value = serde_json::from_str(&frame).unwrap();
                assert_eq!(frame["type"], "preview.open");
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tokio::time::sleep(delay).await;
                let (relay, device) = tokio::io::duplex(128 * 1024);
                let ticket = frame["ticket"].as_str().unwrap().to_owned();
                let (tx, closed) = {
                    let mut streams = responder.preview.streams.lock().unwrap();
                    let Some(stream) = streams.get_mut(&ticket) else {
                        continue;
                    };
                    (stream.tx.take().unwrap(), stream.closed.clone())
                };
                if tx.send(relay).is_err() {
                    closed.cancel();
                    continue;
                }
                let responder = responder.clone();
                let service = service.clone();
                tokio::spawn(async move {
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(
                            TokioIo::new(device),
                            hyper::service::service_fn(move |request| {
                                let service = service.clone();
                                async move { service(request).await }
                            }),
                        )
                        .with_upgrades()
                        .await;
                    // This fixture stands in for the remote app, not the data WS.
                    // Do not cancel the reader before it drains a short close reply.
                    responder.preview.streams.lock().unwrap().remove(&ticket);
                    closed.cancel();
                });
            }
        });
        let app = Router::new()
            .fallback(|| async { StatusCode::IM_A_TEAPOT })
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                middleware,
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let relay = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        HttpFixture {
            state,
            dir,
            mapping,
            base,
            browser: reqwest::Client::new(),
            opened,
            relay,
            device,
        }
    }

    async fn warm_done(fixture: &HttpFixture) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let done = fixture
                    .state
                    .preview
                    .idle
                    .lock()
                    .unwrap()
                    .warming
                    .first()
                    .map(|c| c.done.clone());
                match done {
                    Some(done) => done.cancelled().await,
                    None => break,
                }
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn prewarm_joins_first_request_without_probing_the_app_or_duplicating_handshakes() {
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = requests.clone();
        let fixture = http_fixture(
            "preview-prewarm-first",
            Duration::from_millis(30),
            move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                async { Ok(Response::new(Body::from("ready"))) }
            },
        )
        .await;
        let mapping = fixture.state.preview.mappings.lock().unwrap()[&fixture.mapping].clone();
        let grant = fixture.grant();
        for _ in 0..20 {
            prewarm_http(&fixture.state, &mapping, &grant);
        }
        assert_eq!(fixture.state.preview.idle.lock().unwrap().warming.len(), 2);
        // The browser arrives during preparation, rather than after a fixed sleep.
        let response = fixture.request(Method::GET, "/first").send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "ready");
        warm_done(&fixture).await;
        assert_eq!(fixture.opens(), 2);
        assert_eq!(requests.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(fixture.state.preview.idle.lock().unwrap().clients.len(), 2);
        for index in 0..3 {
            assert_eq!(
                fixture
                    .request(Method::GET, &format!("/later-{index}"))
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap(),
                "ready"
            );
        }
        assert_eq!(fixture.opens(), 2);
        // Expiry discards speculative connections through the normal idle path.
        {
            let mut pool = fixture.state.preview.idle.lock().unwrap();
            for client in &mut pool.clients {
                client.expires = std::time::Instant::now();
            }
            pool.prune();
            assert!(pool.clients.is_empty());
        }
        assert_eq!(
            fixture
                .request(Method::GET, "/after-expiry")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(fixture.opens(), 3);
    }

    #[tokio::test]
    async fn reopening_the_same_authorization_reuses_preparation_without_extending_it() {
        let fixture = http_fixture("preview-prewarm-reopen", Duration::ZERO, |_| async {
            Ok(Response::new(Body::from("ready")))
        })
        .await;
        let mapping = fixture.state.preview.mappings.lock().unwrap()[&fixture.mapping].clone();
        let mut grant = fixture.grant();
        grant.lease = Some("L".repeat(43));
        grant.expires = std::time::Instant::now() + Duration::from_secs(3600);
        prewarm_http(&fixture.state, &mapping, &grant);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("remote_codex_relay_session={}", grant.session)
                .parse()
                .unwrap(),
        );
        for _ in 0..3 {
            assert!(issue_launch(
                &fixture.state,
                "device".into(),
                mapping.id.clone(),
                &headers,
                OpenQuery {
                    lease: grant.lease.clone(),
                    ..Default::default()
                },
                OpenInput::default()
            )
            .await
            .is_ok());
        }
        assert_eq!(fixture.state.preview.idle.lock().unwrap().warming.len(), 2);
        assert!(fixture
            .state
            .preview
            .launches
            .lock()
            .unwrap()
            .values()
            .all(|l| l.grant == grant));
        warm_done(&fixture).await;
        assert_eq!(fixture.opens(), 2);
        // A different device lease retains a separate pool and deadline.
        assert!(issue_launch(
            &fixture.state,
            "device".into(),
            mapping.id.clone(),
            &headers,
            OpenQuery {
                lease: Some("R".repeat(43)),
                ..Default::default()
            },
            OpenInput::default()
        )
        .await
        .is_ok());
        warm_done(&fixture).await;
        assert_eq!(fixture.opens(), 4);
        assert_eq!(
            fixture
                .state
                .preview
                .launches
                .lock()
                .unwrap()
                .values()
                .filter(|l| l.grant == grant)
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn thread_mappings_open_for_that_threads_recipients_until_access_ends() {
        let fixture = http_fixture("preview-thread-share", Duration::ZERO, |_| async {
            Ok(Response::new(Body::from("shared app")))
        })
        .await;
        let state = &fixture.state;
        let (root, child, other) = (
            Uuid::new_v4().to_string(),
            Uuid::new_v4().to_string(),
            Uuid::new_v4().to_string(),
        );
        let sessions: HashMap<&str, String> = {
            let conn = state.store.conn.lock().await;
            let mut sessions = HashMap::new();
            for user in [
                "reader", "stranger", "viewer", "files", "outsider", "nobody",
            ] {
                conn.execute("INSERT INTO relay_users (id,email,username,role,enabled,created_at,password_salt,password_hash) VALUES (?1,?1||'@example.test',?1,'user',1,'now','','')", [user]).unwrap();
                sessions.insert(
                    user,
                    create_session(&conn, &state.store.session_secret, user).unwrap(),
                );
            }
            sessions.insert(
                "owner",
                create_session(&conn, &state.store.session_secret, "owner").unwrap(),
            );
            crate::thread_groups::replace(
                &conn,
                "device",
                &json!([{"id":root,"workspaceId":"project"},{"id":child,"parentThreadId":root,"workspaceId":"project"},{"id":other,"workspaceId":"elsewhere"}]),
            )
            .unwrap();
            // A read-only share of the parent, a share of another thread, a
            // whole-device grant, a grant on the thread's workspace and one on
            // another workspace.
            conn.execute("INSERT INTO relay_shares(id,owner_user_id,target_user_id,device_id,thread_id,thread_access,created_at) VALUES ('share','owner','reader','device',?1,'read','now'),('elsewhere','owner','stranger','device',?2,'control','now')", params![root, other]).unwrap();
            conn.execute_batch("INSERT INTO relay_access_grants(id,owner_user_id,target_user_id,device_id,scope,thread_access,created_at) VALUES ('device-grant','owner','viewer','device','device','read','now');
                INSERT INTO relay_access_grants(id,owner_user_id,target_user_id,device_id,scope,workspace_id,workspace_access,created_at) VALUES ('files-grant','owner','files','device','workspace','project','write','now'),('outside-grant','owner','outsider','device','workspace','elsewhere','write','now');").unwrap();
            sessions
        };
        let launch = |user: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                format!("Bearer {}", sessions[user]).parse().unwrap(),
            );
            let state = state.clone();
            let mapping = fixture.mapping.clone();
            async move {
                open(
                    State(state),
                    Path(("device".into(), mapping)),
                    headers,
                    Query(serde_json::from_value(json!({"lease":"L".repeat(43)})).unwrap()),
                    Json(OpenInput::default()),
                )
                .await
                .status()
            }
        };
        // Without a thread, a mapping stays owner-only.
        assert_eq!(launch("owner").await, StatusCode::OK);
        for user in [
            "reader", "stranger", "viewer", "files", "outsider", "nobody",
        ] {
            assert_eq!(launch(user).await, StatusCode::FORBIDDEN, "{user}");
        }
        let connection = state.sockets.read().await["device"].connection_id;
        let report = |thread: &str| {
            update(
                state,
                "device",
                connection,
                &json!([{"id":fixture.mapping,"port":4013,"threadId":thread}]),
            )
        };
        report(&child);
        for (user, status) in [
            ("owner", StatusCode::OK),
            ("reader", StatusCode::OK),
            ("viewer", StatusCode::OK),
            ("stranger", StatusCode::FORBIDDEN),
            ("files", StatusCode::OK),
            ("outsider", StatusCode::FORBIDDEN),
            ("nobody", StatusCode::FORBIDDEN),
        ] {
            assert_eq!(launch(user).await, status, "{user}");
        }
        // The recipient's preview session reaches the device and application.
        let recipient = |cookie: &'static str, user: &str| {
            state.preview.sessions.lock().unwrap().insert(
                cookie.into(),
                Grant {
                    mapping: fixture.mapping.clone(),
                    device: "device".into(),
                    session: sessions[user].clone(),
                    lease: Some("test-lease".into()),
                    expires: std::time::Instant::now() + Duration::from_secs(60),
                },
            );
            fixture
                .browser
                .get(format!("{}/page", fixture.base))
                .header(
                    header::HOST,
                    format!("p-{}.preview.example.test", fixture.mapping),
                )
                .header(header::COOKIE, format!("{COOKIE}={cookie}"))
        };
        let response = recipient("reader-cookie", "reader").send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "shared app");
        let stranger = recipient("stranger-cookie", "stranger")
            .send()
            .await
            .unwrap();
        assert_eq!(stranger.status(), StatusCode::UNAUTHORIZED);
        // A mapping reported for another thread follows that thread's recipients.
        report(&other);
        assert_eq!(
            recipient("reader-cookie", "reader")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(launch("stranger").await, StatusCode::OK);
        report(&child);
        assert_eq!(launch("stranger").await, StatusCode::FORBIDDEN);
        // Revoking the share ends access at the next request.
        assert_eq!(
            recipient("reader-cookie", "reader")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        state
            .store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE relay_shares SET revoked_at='now' WHERE id='share'",
                [],
            )
            .unwrap();
        let revoked = recipient("reader-cookie", "reader").send().await.unwrap();
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(launch("reader").await, StatusCode::FORBIDDEN);
        // An expired grant ends like a revoked one.
        assert_eq!(
            recipient("viewer-cookie", "viewer")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        state
            .store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE relay_access_grants SET expires_at='2000-01-01T00:00:00Z' WHERE id='device-grant'",
                [],
            )
            .unwrap();
        assert_eq!(
            recipient("viewer-cookie", "viewer")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        // An unusable thread ID from the device leaves the mapping owner-only.
        report("../not a thread");
        assert_eq!(launch("stranger").await, StatusCode::FORBIDDEN);
        assert_eq!(launch("owner").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn prewarm_failed_handshakes_fall_back_before_sending_one_application_request() {
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = requests.clone();
        let fixture = http_fixture(
            "preview-prewarm-failure",
            Duration::from_millis(30),
            move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                async { Ok(Response::new(Body::from("fallback"))) }
            },
        )
        .await;
        let mapping = fixture.state.preview.mappings.lock().unwrap()[&fixture.mapping].clone();
        prewarm_http(&fixture.state, &mapping, &fixture.grant());
        let request = fixture.request(Method::POST, "/once");
        let browser = tokio::spawn(async move { request.send().await.unwrap() });
        tokio::time::timeout(Duration::from_secs(1), async {
            while fixture.state.preview.streams.lock().unwrap().len() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // Simulate the two device handoffs failing before HTTP bytes are sent.
        for (_, stream) in fixture.state.preview.streams.lock().unwrap().drain() {
            stream.cancel.cancel();
            stream.closed.cancel();
        }
        let response = tokio::time::timeout(Duration::from_secs(2), browser)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "fallback");
        assert_eq!(requests.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(fixture.opens(), 3);
        assert!(fixture
            .state
            .preview
            .idle
            .lock()
            .unwrap()
            .warming
            .is_empty());
    }

    #[tokio::test]
    async fn prewarm_is_bounded_and_does_not_evict_live_traffic() {
        let fixture = http_fixture("preview-prewarm-limits", Duration::ZERO, |_| async {
            Ok(Response::new(Body::empty()))
        })
        .await;
        let mapping = fixture.state.preview.mappings.lock().unwrap()[&fixture.mapping].clone();
        let mut grant = fixture.grant();
        for index in 0..20 {
            grant.lease = Some(format!("capability-{index}"));
            prewarm_http(&fixture.state, &mapping, &grant);
        }
        assert_eq!(
            fixture.state.preview.idle.lock().unwrap().warming.len(),
            HTTP_PREWARM_DEVICE
        );
        for index in 0..20 {
            let mut other = mapping.clone();
            other.device = format!("other-{index}");
            grant.device = other.device.clone();
            prewarm_http(&fixture.state, &other, &grant);
        }
        assert_eq!(
            fixture.state.preview.idle.lock().unwrap().warming.len(),
            HTTP_PREWARM_GLOBAL
        );
        warm_done(&fixture).await;
        // Once normal active streams fill a device budget, speculation adds none.
        {
            let mut streams = fixture.state.preview.streams.lock().unwrap();
            for index in streams.len()..32 {
                streams.insert(
                    format!("busy-{index}"),
                    Stream {
                        mapping: mapping.clone(),
                        grant: fixture.grant(),
                        tx: None,
                        cancel: CancellationToken::new(),
                        closed: CancellationToken::new(),
                    },
                );
            }
        }
        let clients = fixture.state.preview.idle.lock().unwrap().clients.len();
        prewarm_http(&fixture.state, &mapping, &fixture.grant());
        assert!(fixture
            .state
            .preview
            .idle
            .lock()
            .unwrap()
            .warming
            .is_empty());
        assert_eq!(
            fixture.state.preview.idle.lock().unwrap().clients.len(),
            clients
        );
    }

    #[tokio::test]
    async fn http_keepalive_reuses_authorized_stream_and_preserves_compressed_bytes() {
        let fixture = http_fixture(
            "preview-keepalive",
            Duration::from_millis(40),
            |request| async move {
                assert_eq!(request.headers()[header::ACCEPT_ENCODING], "gzip");
                Ok(Response::builder()
                    .header(header::CONTENT_ENCODING, "gzip")
                    .header(header::CACHE_CONTROL, "max-age=600")
                    .body(Body::from(vec![0x1f, 0x8b, 0x08, 0x00]))
                    .unwrap())
            },
        )
        .await;
        let start = std::time::Instant::now();
        for index in 0..12 {
            let response = fixture
                .request(Method::GET, &format!("/asset-{index}.js"))
                .header(header::ACCEPT_ENCODING, "gzip")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "max-age=600");
            assert_eq!(
                response.bytes().await.unwrap().as_ref(),
                &[0x1f, 0x8b, 0x08, 0x00]
            );
        }
        eprintln!(
            "preview keepalive: requests=12 opens={} modeled_open_delay_ms=40 elapsed_ms={}",
            fixture.opens(),
            start.elapsed().as_millis()
        );
        assert_eq!(fixture.opens(), 1);
    }

    #[tokio::test]
    async fn http_pool_isolates_capabilities_and_rechecks_account_authority() {
        let fixture = http_fixture("preview-pool-authority", Duration::ZERO, |_| async {
            Ok(Response::new(Body::from("ok")))
        })
        .await;
        async fn get(f: &HttpFixture) {
            let response = f.request(Method::GET, "/").send().await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.text().await.unwrap(), "ok");
        }
        let original = fixture.grant();
        get(&fixture).await;
        assert_eq!(fixture.opens(), 1);
        fixture
            .state
            .preview
            .sessions
            .lock()
            .unwrap()
            .get_mut("test-cookie")
            .unwrap()
            .lease = Some("different-lease".into());
        get(&fixture).await;
        assert_eq!(fixture.opens(), 2, "leases must not share a connection");
        fixture
            .state
            .preview
            .sessions
            .lock()
            .unwrap()
            .insert("test-cookie".into(), original.clone());
        get(&fixture).await;
        assert_eq!(fixture.opens(), 2);
        let session = {
            let conn = fixture.state.store.conn.lock().await;
            create_session(&conn, &fixture.state.store.session_secret, "owner").unwrap()
        };
        fixture
            .state
            .preview
            .sessions
            .lock()
            .unwrap()
            .get_mut("test-cookie")
            .unwrap()
            .session = session;
        get(&fixture).await;
        assert_eq!(
            fixture.opens(),
            3,
            "account sessions must not share a connection"
        );
        let connection = Uuid::new_v4();
        fixture
            .state
            .sockets
            .write()
            .await
            .get_mut("device")
            .unwrap()
            .connection_id = connection;
        update(
            &fixture.state,
            "device",
            connection,
            &json!([{"id":fixture.mapping,"port":4013}]),
        );
        assert!(fixture
            .state
            .preview
            .idle
            .lock()
            .unwrap()
            .clients
            .is_empty());
        get(&fixture).await;
        assert_eq!(
            fixture.opens(),
            4,
            "reconnected devices must not reuse previous sockets"
        );
        update(
            &fixture.state,
            "device",
            connection,
            &json!([{"id":fixture.mapping,"port":4014}]),
        );
        assert!(fixture
            .state
            .preview
            .idle
            .lock()
            .unwrap()
            .clients
            .is_empty());
        get(&fixture).await;
        assert_eq!(
            fixture.opens(),
            5,
            "changed target ports must invalidate previous sockets"
        );
        let forbidden = fixture
            .request(Method::POST, "/")
            .header(header::ORIGIN, "https://other.example.test")
            .send()
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        let session = fixture.grant().session;
        fixture
            .state
            .store
            .conn
            .lock()
            .await
            .execute(
                "DELETE FROM relay_auth_sessions WHERE token_hash=?1",
                [security::token_hash(&session)],
            )
            .unwrap();
        assert_eq!(
            fixture
                .request(Method::GET, "/")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            fixture.opens(),
            5,
            "logout must not reuse an authenticated idle connection"
        );
        fixture
            .state
            .preview
            .sessions
            .lock()
            .unwrap()
            .insert("test-cookie".into(), original);
        get(&fixture).await;
        assert_eq!(fixture.opens(), 6);
        let conn = fixture.state.store.conn.lock().await;
        conn.execute("UPDATE relay_users SET enabled=0 WHERE id='owner'", [])
            .unwrap();
        drop(conn);
        let revoked = fixture.request(Method::GET, "/").send().await.unwrap();
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            fixture.opens(),
            6,
            "pooled connections cannot bypass account authorization"
        );
    }

    #[tokio::test]
    async fn http_pool_keeps_sse_busy_and_stop_cancels_streaming_and_idle_connections() {
        let fixture = http_fixture("preview-pool-sse", Duration::ZERO, |request| async move {
            if request.uri().path() == "/events" {
                let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(
                    Bytes::from_static(b"data: ready\n\n"),
                )])
                .chain(futures_util::stream::pending());
                Ok(Response::builder()
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(stream))
                    .unwrap())
            } else {
                Ok(Response::new(Body::from("ok")))
            }
        })
        .await;
        let mut events = fixture
            .request(Method::GET, "/events")
            .send()
            .await
            .unwrap();
        assert_eq!(events.status(), StatusCode::OK);
        let first = tokio::time::timeout(Duration::from_secs(1), events.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(first, "data: ready\n\n");
        assert!(fixture
            .state
            .preview
            .idle
            .lock()
            .unwrap()
            .clients
            .is_empty());
        for _ in 0..2 {
            assert_eq!(
                fixture
                    .request(Method::GET, "/")
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap(),
                "ok"
            );
        }
        assert_eq!(
            fixture.opens(),
            2,
            "the active SSE connection cannot serve another request"
        );
        assert_eq!(fixture.state.preview.idle.lock().unwrap().clients.len(), 1);
        let connection = fixture.state.sockets.read().await["device"].connection_id;
        update(&fixture.state, "device", connection, &json!([]));
        assert!(fixture.state.preview.streams.lock().unwrap().is_empty());
        assert!(fixture
            .state
            .preview
            .idle
            .lock()
            .unwrap()
            .clients
            .is_empty());
        let ended = tokio::time::timeout(Duration::from_secs(1), events.chunk())
            .await
            .unwrap();
        assert!(ended.is_err() || ended.unwrap().is_none());
        assert_eq!(
            fixture
                .request(Method::GET, "/")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn http_pool_never_replays_a_post_after_the_application_receives_it() {
        let posts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = posts.clone();
        let fixture = http_fixture("preview-pool-no-replay", Duration::ZERO, move |request| {
            let observed = observed.clone();
            async move {
                if request.method() == Method::POST {
                    let _ = to_bytes(Body::new(request.into_body()), 4096)
                        .await
                        .unwrap();
                    observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Err(std::io::Error::other(
                        "fixture disconnects after accepting POST",
                    ));
                }
                let mut response = Response::new(Body::from("ok"));
                if request.uri().path() == "/close" {
                    response
                        .headers_mut()
                        .insert(header::CONNECTION, "close".parse().unwrap());
                }
                Ok(response)
            }
        })
        .await;
        assert_eq!(
            fixture
                .request(Method::GET, "/")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok"
        );
        let failed = fixture
            .request(Method::POST, "/mutate")
            .body("side effect")
            .send()
            .await
            .unwrap();
        assert_eq!(failed.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(posts.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(
            fixture.opens(),
            1,
            "an already sent request must never trigger another open/replay"
        );
        assert_eq!(
            fixture
                .request(Method::GET, "/close")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok"
        );
        assert_eq!(
            fixture
                .request(Method::GET, "/")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok"
        );
        assert_eq!(
            fixture.opens(),
            3,
            "an upstream Connection: close must establish a fresh stream"
        );
    }

    #[tokio::test]
    async fn websocket_upgrade_uses_a_dedicated_stream_and_preserves_http_keepalive() {
        use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message as ClientMessage};
        let fixture = http_fixture(
            "preview-pool-websocket",
            Duration::ZERO,
            |mut request| async move {
                if request.uri().path() != "/ws" {
                    return Ok(Response::new(Body::from("ok")));
                }
                assert_eq!(request.headers()[header::ORIGIN], "http://127.0.0.1:4013");
                let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(
                    request.headers()["sec-websocket-key"].as_bytes(),
                );
                let upgrade = hyper::upgrade::on(&mut request);
                tokio::spawn(async move {
                    let mut socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
                        TokioIo::new(upgrade.await.unwrap()),
                        tokio_tungstenite::tungstenite::protocol::Role::Server,
                        None,
                    )
                    .await;
                    while let Some(Ok(message)) = socket.next().await {
                        if message.is_close() {
                            break;
                        }
                        if socket.send(message).await.is_err() {
                            break;
                        }
                    }
                });
                Ok(Response::builder()
                    .status(StatusCode::SWITCHING_PROTOCOLS)
                    .header(header::UPGRADE, "websocket")
                    .header(header::CONNECTION, "upgrade")
                    .header("sec-websocket-accept", accept)
                    .body(Body::empty())
                    .unwrap())
            },
        )
        .await;
        assert_eq!(
            fixture
                .request(Method::GET, "/")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok"
        );
        let mut request = format!("{}/ws", fixture.base.replacen("http:", "ws:", 1))
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            header::HOST,
            format!("p-{}.preview.example.test", fixture.mapping)
                .parse()
                .unwrap(),
        );
        request.headers_mut().insert(
            header::COOKIE,
            "__Host-rc_port_preview=test-cookie".parse().unwrap(),
        );
        request.headers_mut().insert(
            header::ORIGIN,
            format!("https://p-{}.preview.example.test", fixture.mapping)
                .parse()
                .unwrap(),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        socket
            .send(ClientMessage::Text("interactive echo".into()))
            .await
            .unwrap();
        let echo = tokio::time::timeout(Duration::from_secs(1), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(echo.into_text().unwrap(), "interactive echo");
        assert_eq!(
            fixture
                .request(Method::GET, "/")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok"
        );
        assert_eq!(fixture.opens(), 2);
        assert_eq!(fixture.state.preview.idle.lock().unwrap().clients.len(), 1);
        let _ = socket.close(None).await;
    }

    #[tokio::test]
    async fn http_pool_bounds_idle_sockets_and_reaps_them_without_new_traffic() {
        let barrier = Arc::new(tokio::sync::Barrier::new(8));
        let fixture = http_fixture("preview-pool-bounds", Duration::ZERO, move |request| {
            let barrier = barrier.clone();
            async move {
                if request.uri().path() == "/parallel" {
                    barrier.wait().await;
                }
                Ok(Response::new(Body::from("ok")))
            }
        })
        .await;
        let responses = futures_util::future::join_all((0..8).map(|_| async {
            fixture
                .request(Method::GET, "/parallel")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        }))
        .await;
        assert!(responses.iter().all(|s| s == "ok"));
        assert_eq!(fixture.opens(), 8);
        assert_eq!(
            fixture.state.preview.idle.lock().unwrap().clients.len(),
            6,
            "retain a browser's HTTP/1 concurrency within the per-device budget"
        );
        let warm = futures_util::future::join_all((0..6).map(|_| async {
            fixture
                .request(Method::GET, "/warm")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        }))
        .await;
        assert!(warm.iter().all(|s| s == "ok"));
        assert_eq!(
            fixture.opens(),
            8,
            "a warm browser burst should reuse its connections"
        );
        for i in 0..9 {
            fixture
                .state
                .preview
                .sessions
                .lock()
                .unwrap()
                .get_mut("test-cookie")
                .unwrap()
                .lease = Some(format!("lease-{i}"));
            assert_eq!(
                fixture
                    .request(Method::GET, "/")
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap(),
                "ok"
            );
        }
        assert_eq!(
            fixture.state.preview.idle.lock().unwrap().clients.len(),
            8,
            "idle streams leave 24 of 32 device slots available"
        );
        let start = std::time::Instant::now();
        tokio::time::timeout(HTTP_IDLE_TIMEOUT + Duration::from_secs(2), async {
            loop {
                if fixture
                    .state
                    .preview
                    .idle
                    .lock()
                    .unwrap()
                    .clients
                    .is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        eprintln!(
            "preview idle reaper: evicted=8 elapsed_ms={}",
            start.elapsed().as_millis()
        );
    }

    #[tokio::test]
    async fn device_websocket_close_releases_stream_slots_without_two_second_stalls() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let (state, dir, headers, mut rx, mapping_id) =
            lease_fixture("preview-real-ws-close").await;
        let device_token = "fixture-preview-device-token";
        state
            .store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE relay_devices SET token_hash=?1 WHERE id='device'",
                [hash_device_token(device_token)],
            )
            .unwrap();
        let mapping = state.preview.mappings.lock().unwrap()[&mapping_id].clone();
        let grant = Grant {
            mapping: mapping_id,
            device: "device".into(),
            session: extract_session_token(&headers, &TokenQuery::default()).unwrap(),
            lease: None,
            expires: std::time::Instant::now() + Duration::from_secs(60),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("ws://{}", listener.local_addr().unwrap());
        let app = routes().with_state(state.clone());
        let relay = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let start = std::time::Instant::now();
        for _ in 0..40 {
            let caller = state.clone();
            let mapping = mapping.clone();
            let grant = grant.clone();
            let opened = tokio::spawn(async move {
                open_stream(&caller, &mapping, &grant, false).await.unwrap()
            });
            let frame: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
            let mut request = format!(
                "{base}/supervisor/port-stream/{}",
                frame["ticket"].as_str().unwrap()
            )
            .into_client_request()
            .unwrap();
            request.headers_mut().insert(
                header::AUTHORIZATION,
                format!("Bearer {device_token}").parse().unwrap(),
            );
            let (mut device, _) = tokio_tungstenite::connect_async(request).await.unwrap();
            let (mut io, _, _) = opened.await.unwrap();
            // Exercise real binary bridging and its close handshake, not a mocked slot.
            io.write_all(b"request").await.unwrap();
            assert_eq!(
                device.next().await.unwrap().unwrap().into_data().as_ref(),
                b"request"
            );
            device
                .send(tokio_tungstenite::tungstenite::Message::Binary(
                    b"reply".to_vec().into(),
                ))
                .await
                .unwrap();
            let mut reply = [0; 5];
            io.read_exact(&mut reply).await.unwrap();
            assert_eq!(&reply, b"reply");
            drop(io);
            assert!(device.next().await.unwrap().unwrap().is_close());
            device.flush().await.unwrap();
            tokio::time::timeout(Duration::from_secs(1), async {
                while !state.preview.streams.lock().unwrap().is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
        eprintln!(
            "preview real WS close: streams=40 remaining={} elapsed_ms={}",
            state.preview.streams.lock().unwrap().len(),
            start.elapsed().as_millis()
        );
        relay.abort();
        drop(state);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn http_pool_evicts_idle_from_another_lease_before_refusing_a_full_device() {
        for expired in [false, true] {
            let fixture = http_fixture("preview-pool-eviction", Duration::ZERO, |_| async {
                Ok(Response::new(Body::from("ok")))
            })
            .await;
            assert_eq!(
                fixture
                    .request(Method::GET, "/")
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap(),
                "ok"
            );
            let mapping = fixture.state.preview.mappings.lock().unwrap()[&fixture.mapping].clone();
            let grant = fixture.grant();
            let idle_closed = fixture.state.preview.idle.lock().unwrap().clients[0]
                .client
                .closed
                .clone();
            if expired {
                fixture.state.preview.idle.lock().unwrap().clients[0].expires =
                    std::time::Instant::now() - Duration::from_secs(1);
            }
            let busy = CancellationToken::new();
            {
                let mut streams = fixture.state.preview.streams.lock().unwrap();
                for i in 0..31 {
                    streams.insert(
                        format!("busy-{i}"),
                        Stream {
                            mapping: mapping.clone(),
                            grant: grant.clone(),
                            tx: None,
                            cancel: busy.clone(),
                            closed: CancellationToken::new(),
                        },
                    );
                }
                assert_eq!(streams.len(), 32);
            }
            fixture
                .state
                .preview
                .sessions
                .lock()
                .unwrap()
                .get_mut("test-cookie")
                .unwrap()
                .lease = Some("new-lease".into());
            let response = tokio::time::timeout(
                Duration::from_secs(1),
                fixture.request(Method::GET, "/").send(),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.text().await.unwrap(), "ok");
            assert_eq!(fixture.opens(), 2);
            assert!(
                idle_closed.is_cancelled(),
                "previous capability's idle connection must finish closing first"
            );
            assert!(
                !busy.is_cancelled(),
                "active traffic must never be preempted"
            );
            assert_eq!(fixture.state.preview.streams.lock().unwrap().len(), 32);
        }
    }

    #[tokio::test]
    async fn dropping_an_unfinished_response_discards_its_http_connection() {
        let fixture = http_fixture("preview-pool-drop", Duration::ZERO, |request| async move {
            if request.uri().path() == "/stream" {
                let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(
                    Bytes::from_static(b"first chunk"),
                )])
                .chain(futures_util::stream::pending());
                Ok(Response::new(Body::from_stream(stream)))
            } else {
                Ok(Response::new(Body::from("ok")))
            }
        })
        .await;
        let mut response = fixture
            .request(Method::GET, "/stream")
            .send()
            .await
            .unwrap();
        assert_eq!(response.chunk().await.unwrap().unwrap(), "first chunk");
        assert!(fixture
            .state
            .preview
            .idle
            .lock()
            .unwrap()
            .clients
            .is_empty());
        drop(response);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !fixture.state.preview.streams.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(fixture
            .state
            .preview
            .idle
            .lock()
            .unwrap()
            .clients
            .is_empty());
        assert_eq!(
            fixture
                .request(Method::GET, "/")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok"
        );
        assert_eq!(fixture.opens(), 2);
    }

    #[tokio::test]
    async fn documentation_preview_redirect_and_forwarding_strip_lease_parameters() {
        let (state, dir, headers, mut rx, mapping) = lease_fixture("doc-preview-lease-query").await;
        let lease = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([224; 32]);
        let token = extract_session_token(&headers, &TokenQuery::default()).unwrap();
        let ticket = Uuid::new_v4().to_string();
        state.preview.launches.lock().unwrap().insert(
            ticket.clone(),
            Launch {
                grant: Grant {
                    mapping: mapping.clone(),
                    device: "device".into(),
                    session: token,
                    lease: Some(lease.clone()),
                    expires: std::time::Instant::now() + Duration::from_secs(60),
                },
                expires: std::time::Instant::now() + Duration::from_secs(60),
            },
        );
        let host = state
            .preview
            .origin(&mapping)
            .unwrap()
            .host_str()
            .unwrap()
            .to_owned();
        let app = Router::new()
            .fallback(|| async { StatusCode::IM_A_TEAPOT })
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                middleware,
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let relay = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let response=client.get(format!("{base}/work?app=a%2Fb&app=two&flag&lease={lease}&leaseExpiresAt=123&%6cease=duplicate&__rc_launch={ticket}")).header(header::HOST,&host).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let redirect =
            url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        assert_eq!(redirect.path(), "/work");
        assert_eq!(redirect.query(), Some("app=a%2Fb&app=two&flag"));
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        assert_eq!(
            state
                .preview
                .sessions
                .lock()
                .unwrap()
                .values()
                .next()
                .unwrap()
                .lease
                .as_deref(),
            Some(lease.as_str())
        );
        // Also filter manually supplied lease parameters on subsequent HTTP/WS paths.
        let request=client.get(format!("{base}/work?app=a%2Fb&app=two&flag&lease=forged&leaseExpiresAt=1&%6ceaseExpiresAt=2")).header(header::HOST,&host).header(header::COOKIE,cookie);
        let caller = tokio::spawn(async move { request.send().await.unwrap() });
        let frame: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(frame["type"], "preview.open");
        assert_eq!(frame["leaseId"], lease);
        let observed = Arc::new(StdMutex::new(Vec::<String>::new()));
        let recorder = observed.clone();
        let (browser, device) = tokio::io::duplex(4096);
        let upstream = tokio::spawn(async move {
            hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    TokioIo::new(device),
                    hyper::service::service_fn(
                        move |request: hyper::Request<hyper::body::Incoming>| {
                            recorder.lock().unwrap().push(request.uri().to_string());
                            async {
                                Ok::<_, std::convert::Infallible>(Response::new(Body::from("ok")))
                            }
                        },
                    ),
                )
                .await
                .unwrap();
        });
        let stream_ticket = frame["ticket"].as_str().unwrap();
        let sender = state
            .preview
            .streams
            .lock()
            .unwrap()
            .get_mut(stream_ticket)
            .unwrap()
            .tx
            .take()
            .unwrap();
        sender.send(browser).unwrap();
        let response = caller.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "ok");
        assert_eq!(
            *observed.lock().unwrap(),
            vec!["/work?app=a%2Fb&app=two&flag".to_owned()]
        );
        // The upstream now stays alive for subsequent authorized HTTP requests.
        upstream.abort();
        relay.abort();
        drop(state);
        let _ = std::fs::remove_dir_all(dir);
    }
}
