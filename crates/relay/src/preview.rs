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
}
#[derive(Clone, PartialEq, Eq)]
struct Grant {
    mapping: String,
    device: String,
    session: String,
    lease: Option<String>,
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
    launches: StdMutex<HashMap<String, Grant>>,
    sessions: StdMutex<HashMap<String, Grant>>,
    streams: StdMutex<HashMap<String, Stream>>,
    idle: StdMutex<IdlePool>,
}

// Reuse only fully consumed HTTP/1 responses, within the very same capability.
// The device still watches the lease throughout the lifetime of the raw TCP stream.
// This needs no wire change: older Supervisors already bridge arbitrary TCP bytes.
const HTTP_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
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
#[derive(Default)]
struct IdlePool {
    clients: Vec<IdleClient>,
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
            let idle = {
                let mut pool = state.preview.idle.lock().unwrap();
                pool.prune();
                pool.clients
                    .iter()
                    .rposition(|c| c.client.mapping == *mapping && c.client.grant == *grant)
                    .map(|i| pool.clients.swap_remove(i).client)
            };
            let Some(client) = idle else { break };
            // A peer may close an idle keepalive socket. Fall back before sending
            // anything; never retry a request after it may have reached the app.
            if client.sender.is_ready() && !client.cancel.is_cancelled() {
                return Ok(client);
            }
            state.preview.idle.lock().unwrap().discard(client);
        }
    }
    let (stream, cancel, released) = open_stream(state, mapping, grant).await?;
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
    let conn = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&conn, &state.store.session_secret, &headers, &query.token)
    else {
        return unauthorized();
    };
    if !owns(&conn, &user.id, &device) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(session) = extract_session_token(&headers, &query.token) else {
        return unauthorized();
    };
    drop(conn);
    let Some(mut url) = state.preview.origin(&id) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                json!({"message":"Port preview DNS and gateway are not configured on this Relay."}),
            ),
        )
            .into_response();
    };
    if !state
        .preview
        .mappings
        .lock()
        .unwrap()
        .get(&id)
        .is_some_and(|m| m.device == device)
    {
        return (StatusCode::CONFLICT, Json(json!({"message":"This port is not enabled on the online device. Update its Supervisor if port mappings are unavailable."}))).into_response();
    }
    if query.lease.as_ref().is_some_and(|lease| {
        lease.len() != 43
            || !lease
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    }) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"code":"invalid_lease","message":"Invalid preview lease"})),
        )
            .into_response();
    }
    if query.lease.is_none() {
        // The device descriptor is only a routing hint; the device enforces its own lease.
        let key = internal_forward_json(&state, &device, "GET", "/api/transport/key", None).await;
        if !key.is_ok_and(|key| key["trust"]["strict"] != true) {
            return (StatusCode::FORBIDDEN,Json(json!({"code":"preview_lease_required","message":"Obtain a signed device preview lease first"}))).into_response();
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
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Ok(destination) = url.join(path) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if destination.origin() != url.origin() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    url = destination;
    if let Some(lease) = &query.lease {
        url.query_pairs_mut().append_pair("lease", lease);
    }
    let ticket = Uuid::new_v4().to_string();
    let mut launches = state.preview.launches.lock().unwrap();
    launches.retain(|_, g| g.expires > std::time::Instant::now());
    if launches.len() >= 1024 {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    launches.insert(
        ticket.clone(),
        Grant {
            mapping: id,
            device,
            session,
            lease: query.lease,
            expires: std::time::Instant::now() + Duration::from_secs(60),
        },
    );
    url.query_pairs_mut().append_pair("__rc_launch", &ticket);
    Json(json!({"url":url.as_str()})).into_response()
}

fn owns(conn: &Connection, user: &str, device: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM relay_devices WHERE id=?1 AND owner_user_id=?2)",
        params![device, user],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
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
    destination.query_pairs_mut().append_pair(
        "path",
        request
            .uri()
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/"),
    );
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
        let grant = state.preview.launches.lock().unwrap().remove(&ticket);
        let Some(mut grant) = grant.filter(|g| {
            g.mapping == id && g.device == mapping.device && g.expires > std::time::Instant::now()
        }) else {
            return unauthorized();
        };
        if !valid_grant(&state, &grant).await {
            return unauthorized();
        }
        grant.expires = std::time::Instant::now() + Duration::from_secs(8 * 3600);
        let session = Uuid::new_v4().to_string();
        let mut sessions = state.preview.sessions.lock().unwrap();
        sessions.retain(|_, g| g.expires > std::time::Instant::now());
        if sessions.len() >= 4096 {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
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
                    "{cookie_name}={session}; Path=/; HttpOnly; SameSite=Lax; Max-Age=28800{}",
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
    let conn = state.store.conn.lock().await;
    load_user_by_session(&conn, &state.store.session_secret, &grant.session)
        .is_some_and(|u| u.role != "admin" && owns(&conn, &u.id, &grant.device))
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
            .clone();
        let mapping = state.preview.mappings.lock().unwrap()[&mapping_id].clone();
        let caller = state.clone();
        let task = tokio::spawn(async move { open_stream(&caller, &mapping, &grant).await });
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
        tx.send(tokio::io::duplex(1024).0).unwrap();
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
                    let stream = streams.get_mut(&ticket).unwrap();
                    (stream.tx.take().unwrap(), stream.closed.clone())
                };
                tx.send(relay).unwrap();
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
            let opened =
                tokio::spawn(async move { open_stream(&caller, &mapping, &grant).await.unwrap() });
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
            Grant {
                mapping: mapping.clone(),
                device: "device".into(),
                session: token,
                lease: Some(lease.clone()),
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
