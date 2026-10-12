//! Private HTTP previews on a distinct origin per explicitly approved device port.
//! The ordinary relay API remains encrypted; only this separate data path is plaintext.
use super::*;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio_util::sync::CancellationToken;

const COOKIE: &str = "__Host-rc_port_preview";
const LOCAL_COOKIE: &str = "rc_port_preview";

#[derive(Clone)]
struct Mapping {
    id: String,
    device: String,
    connection: Uuid,
    port: u16,
}
#[derive(Clone)]
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
}
#[derive(Default)]
pub(super) struct Hub {
    pub(super) base: Option<url::Url>,
    mappings: StdMutex<HashMap<String, Mapping>>,
    launches: StdMutex<HashMap<String, Grant>>,
    sessions: StdMutex<HashMap<String, Grant>>,
    streams: StdMutex<HashMap<String, Stream>>,
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
        let live = catalog
            .get(&s.mapping.id)
            .is_some_and(|m| m.connection == s.mapping.connection && m.device == s.mapping.device);
        if !live {
            s.cancel.cancel();
        }
        live
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
    headers.insert(
        header::CONNECTION,
        if websocket { "upgrade" } else { "close" }.parse().unwrap(),
    );
    let result = async {
        let stream = open_stream(&state, &mapping, &grant).await?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake::<_, Body>(TokioIo::new(stream)).await?;
        tokio::spawn(async move {
            let _ = connection.with_upgrades().await;
        });
        let mut response =
            tokio::time::timeout(Duration::from_secs(30), sender.send_request(request)).await??;
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
        Ok::<_, anyhow::Error>(response.map(Body::new))
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
    }
}
async fn open_stream(state: &AppState, mapping: &Mapping, grant: &Grant) -> Result<DuplexStream> {
    let ticket = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let pending = Pending {
        hub: &state.preview,
        ticket: ticket.clone(),
    };
    {
        let mut streams = state.preview.streams.lock().unwrap();
        if streams.len() >= 256
            || streams
                .values()
                .filter(|s| s.mapping.device == mapping.device)
                .count()
                >= 32
        {
            bail!("Too many preview connections");
        }
        streams.insert(
            ticket.clone(),
            Stream {
                mapping: mapping.clone(),
                grant: grant.clone(),
                tx: Some(tx),
                cancel: CancellationToken::new(),
            },
        );
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
    Ok(stream)
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
                    .map(|tx| (tx, s.cancel.clone(), s.grant.clone()))
            })
    };
    drop(sockets);
    let Some((tx, cancel, grant)) = pairing else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let guard = StreamGuard { state, ticket };
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
            },
        );
        drop(StreamGuard {
            state: state.clone(),
            ticket: "failed-upgrade".into(),
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
        let (tx, rx) = tokio::sync::mpsc::channel(4);
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
        upstream.await.unwrap();
        relay.abort();
        drop(state);
        let _ = std::fs::remove_dir_all(dir);
    }
}
