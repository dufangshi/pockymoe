use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::bounded_channel as mpsc;
use anyhow::{anyhow, Result};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use remote_codex_protocol::{now_rfc3339, ThreadEventEnvelope};
use remote_codex_runtime::Supervisor;
use serde_json::{json, Value};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tower::ServiceExt;
use url::Url;

const RELAY_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);
const RELAY_RECEIVE_TIMEOUT: Duration = Duration::from_secs(5);
const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RELAY_RECONNECT_INITIAL_DELAY: Duration = Duration::from_secs(1);
const RELAY_RECONNECT_MAX_DELAY: Duration = Duration::from_secs(3);

struct RelayClientSession {
    socket: crate::socket::SocketSession,
    bridge: tokio::task::JoinHandle<()>,
    crypto: Option<(
        Arc<crate::secure_transport::Transport>,
        Arc<crate::secure_transport::Session>,
    )>,
}

// Reset readiness on every exit, including timeout, errors and task cancellation.
struct RelayReadiness(Arc<Supervisor>);
impl Drop for RelayReadiness {
    fn drop(&mut self) {
        self.0
            .relay_connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Drop for RelayClientSession {
    fn drop(&mut self) {
        self.bridge.abort();
        if let Some((transport, session)) = &self.crypto {
            transport.remove_session(session.id());
        }
    }
}

pub async fn run_relay_tunnel(state: Arc<Supervisor>) -> Result<()> {
    let server_url = state
        .config
        .relay_server_url
        .as_deref()
        .ok_or_else(|| anyhow!("REMOTE_CODEX_RELAY_SERVER_URL is required"))?;
    let token = state
        .config
        .relay_agent_token
        .as_deref()
        .ok_or_else(|| anyhow!("REMOTE_CODEX_RELAY_AGENT_TOKEN is required"))?;
    let tunnel_url = relay_tunnel_url(server_url)?;
    let mut reconnect_delay = RELAY_RECONNECT_INITIAL_DELAY;

    loop {
        let mut handshake = tunnel_url.as_str().into_client_request()?;
        handshake
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse()?);
        match tokio::time::timeout(RELAY_CONNECT_TIMEOUT, connect_async(handshake)).await {
            Ok(Ok((socket, _))) => {
                tracing::info!(relay_origin = %tunnel_url.origin().ascii_serialization(), "relay tunnel connected");
                reconnect_delay = RELAY_RECONNECT_INITIAL_DELAY;
                if let Err(error) = run_connected_tunnel(state.clone(), socket).await {
                    tracing::warn!(%error, "relay tunnel connection ended");
                }
                // Reconnect an established tunnel immediately. Only failed
                // connection attempts back off; the runtime remains alive.
                continue;
            }
            Ok(Err(_)) => {
                // Do not log the websocket error verbatim: some implementations
                // include the credential-bearing URL in connection errors.
                tracing::warn!("relay tunnel connect failed");
            }
            Err(_) => {
                tracing::warn!("relay tunnel connection attempt timed out");
            }
        }
        tokio::time::sleep(reconnect_delay).await;
        reconnect_delay = reconnect_delay
            .checked_mul(2)
            .unwrap_or(RELAY_RECONNECT_MAX_DELAY)
            .min(RELAY_RECONNECT_MAX_DELAY);
    }
}

fn relay_tunnel_url(server_url: &str) -> Result<Url> {
    let mut url = Url::parse(server_url)?;
    match url.scheme() {
        "http" => url
            .set_scheme("ws")
            .map_err(|_| anyhow!("invalid relay server URL scheme"))?,
        "https" => url
            .set_scheme("wss")
            .map_err(|_| anyhow!("invalid relay server URL scheme"))?,
        "ws" | "wss" => {}
        scheme => return Err(anyhow!("unsupported relay server URL scheme: {scheme}")),
    }
    url.set_path("/supervisor/tunnel");
    url.set_query(None);
    Ok(url)
}

async fn run_connected_tunnel(
    state: Arc<Supervisor>,
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<()> {
    run_connected_tunnel_with_deadline(state, socket, RELAY_RECEIVE_TIMEOUT).await
}

async fn run_connected_tunnel_with_deadline(
    state: Arc<Supervisor>,
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    receive_timeout: Duration,
) -> Result<()> {
    let (mut sink, mut stream) = socket.split();
    let _readiness = RelayReadiness(state.clone());
    let mut last_received = tokio::time::Instant::now();
    let mut last_tick = SystemTime::now();
    let (outgoing, mut outbound) = mpsc::channel::<Value>();
    let mut clients = HashMap::<String, RelayClientSession>::new();
    let mut heartbeat = tokio::time::interval(RELAY_HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut activity = state.bus.subscribe();

    outgoing
        .send(json!({ "type": "relay.heartbeat", "timestamp": now_rfc3339(), "threadLineage":thread_lineage(&state) }))
        .map_err(|_| anyhow!("relay tunnel writer closed"))?;
    // Consume the interval's immediate first tick because the initial heartbeat
    // was queued explicitly above.
    heartbeat.tick().await;

    loop {
        tokio::select! {
            message = outbound.recv() => {
                let Some(message) = message else {
                    return Err(anyhow!("relay tunnel writer closed"));
                };
                tokio::time::timeout(RELAY_RECEIVE_TIMEOUT,sink.send(Message::Text(message.to_string().into()))).await??;
            }
            _ = tokio::time::sleep_until(last_received + receive_timeout) => {
                return Err(anyhow!("relay stopped responding; reconnecting"));
            }
            incoming = stream.next() => {
                last_received = tokio::time::Instant::now();
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(message) = serde_json::from_str::<Value>(&text) else {
                            continue;
                        };
                        if message["type"] == "relay.notification.ack" {
                            if let Some(turn) = message["turnId"].as_str() { let _ = state.acknowledge_relay_notification(turn); }
                            continue;
                        }
                        if message["type"] == "relay.connected" {
                            state.relay_connected.store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                        handle_relay_message(
                            state.clone(),
                            &mut clients,
                            &outgoing,
                            message,
                        );
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        tokio::time::timeout(RELAY_RECEIVE_TIMEOUT, sink.send(Message::Pong(payload))).await??;
                    }
                    Some(Ok(Message::Close(frame))) => {
                        return Err(anyhow!("relay tunnel closed: {frame:?}"));
                    }
                    Some(Err(error)) => return Err(error.into()),
                    None => return Err(anyhow!("relay tunnel closed")),
                    _ => {}
                }
            }
            _ = heartbeat.tick() => {
                if let Ok(notices) = state.pending_relay_notifications() {
                    for payload in notices { let _ = outgoing.send(json!({"type":"relay.notification","payload":payload})); }
                }
                let now = SystemTime::now();
                if resumed_after_pause(last_tick, now) {
                    return Err(anyhow!("supervisor resumed after a pause; reconnecting relay"));
                }
                last_tick = now;
                // TCP writes can succeed after a network change even though the
                // peer is unreachable. A WebSocket ping requires a return path.
                tokio::time::timeout(RELAY_RECEIVE_TIMEOUT, sink.send(Message::Ping(Vec::new().into()))).await??;
                if outgoing.send(json!({
                    "type": "relay.heartbeat",
                    "timestamp": now_rfc3339(),
                    "threadLineage":thread_lineage(&state)
                })).is_err() {
                    return Err(anyhow!("relay tunnel writer closed"));
                }
            }
            event = activity.recv() => {
                match event {
                    Ok(event) => {
                        if event.event_type == "thread.request.created" {
                            // Notification failures/rejections are deliberately independent
                            // of the encrypted request flow, including with older relays.
                            if let Ok(notices) = state.pending_relay_notifications() {
                                for payload in notices { let _ = outgoing.send(json!({"type":"relay.notification","payload":payload})); }
                            }
                        }
                        if event.payload["reason"] == "thread_created" || event.payload["reason"] == "child_deleted" {
                            let _ = outgoing.send(json!({"type":"relay.heartbeat","timestamp":now_rfc3339(),"threadLineage":thread_lineage(&state)}));
                        }
                        if let Some(activity) = relay_activity(&event) {
                            let _ = outgoing.send(json!({"type":"relay.heartbeat","timestamp":now_rfc3339(),"threadLineage":thread_lineage(&state)}));
                            let _ = outgoing.send(activity);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "relay tunnel lagged turn lifecycle activity");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
                }
            }
        }
    }
}

fn thread_lineage(state: &Supervisor) -> Value {
    state
        .list_threads(None, true)
        .map(|threads| {
            json!(threads
                .iter()
                .map(|t| json!({"id":t.id,"parentThreadId":t.parent_thread_id}))
                .collect::<Vec<_>>())
        })
        .unwrap_or(Value::Null)
}

fn resumed_after_pause(previous: SystemTime, now: SystemTime) -> bool {
    // macOS's monotonic timer can exclude time asleep. A wall-clock gap forces
    // a fresh socket on the first heartbeat after wake instead of trusting TCP.
    now.duration_since(previous)
        .is_ok_and(|gap| gap > RELAY_HEARTBEAT_INTERVAL + RELAY_RECEIVE_TIMEOUT)
}

fn handle_relay_message(
    state: Arc<Supervisor>,
    clients: &mut HashMap<String, RelayClientSession>,
    outgoing: &mpsc::Sender<Value>,
    message: Value,
) {
    match message.get("type").and_then(Value::as_str) {
        Some("relay.request") => {
            let Some(request_id) = message
                .get("requestId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
            else {
                return;
            };
            let payload = message.get("payload").cloned().unwrap_or_else(|| json!({}));
            static REQUESTS: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> =
                std::sync::OnceLock::new();
            static HANDSHAKES: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> =
                std::sync::OnceLock::new();
            static HEALTH: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> =
                std::sync::OnceLock::new();
            // Reserve handshake capacity so slow application requests cannot make
            // an online device appear to have broken encryption.
            let handshake = payload["path"].as_str().is_some_and(|path| {
                let path = path.split('?').next().unwrap_or("");
                path.ends_with("/transport/key") || path.ends_with("/transport/session")
            });
            let health = payload["path"] == "/healthz";
            let pool = if health {
                &HEALTH
            } else if handshake {
                &HANDSHAKES
            } else {
                &REQUESTS
            };
            let permit = pool
                .get_or_init(|| {
                    Arc::new(tokio::sync::Semaphore::new(if handshake || health {
                        4
                    } else {
                        32
                    }))
                })
                .clone()
                .try_acquire_owned();
            let Ok(permit) = permit else {
                let _=outgoing.send(json!({"type":"relay.response","requestId":request_id,"payload":relay_error_response(429,"Device is busy; retry shortly")}));
                return;
            };
            let outgoing = outgoing.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let payload =
                    bounded_forward(forward_local(&state, payload), Duration::from_secs(60)).await;
                let _ = outgoing.send(json!({
                    "type": "relay.response",
                    "timestamp": now_rfc3339(),
                    "requestId": request_id,
                    "payload": payload
                }));
            });
        }
        Some("relay.client.connected") => {
            if let Some(client_id) = message
                .get("clientId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                connect_relay_client(
                    state,
                    clients,
                    outgoing,
                    client_id.to_string(),
                    message.get("secureChannelId").and_then(Value::as_str),
                );
            }
        }
        Some("relay.client.message") => {
            if let (Some(client_id), Some(payload)) = (
                message.get("clientId").and_then(Value::as_str),
                message.get("payload"),
            ) {
                if let Some(client) = clients.get(client_id) {
                    let clear = match &client.crypto {
                        Some((_, crypto)) => crypto.open(payload),
                        None if payload.get("encrypted").is_none() => Ok(payload.clone()),
                        None => Err(anyhow!("encrypted channel unavailable")),
                    };
                    match clear {
                        Ok(payload) => {
                            if !client.socket.send(payload) {
                                let _ = outgoing.send(
                                    json!({"type":"relay.client.close","clientId":client_id}),
                                );
                                clients.remove(client_id);
                            }
                        }
                        Err(_) => {
                            let _ = outgoing
                                .send(json!({"type":"relay.client.close","clientId":client_id}));
                            clients.remove(client_id);
                        }
                    }
                }
            }
        }
        Some("relay.client.disconnected") => {
            if let Some(client_id) = message.get("clientId").and_then(Value::as_str) {
                clients.remove(client_id);
            }
        }
        _ => {}
    }
}

fn connect_relay_client(
    state: Arc<Supervisor>,
    clients: &mut HashMap<String, RelayClientSession>,
    outgoing: &mpsc::Sender<Value>,
    client_id: String,
    secure_channel_id: Option<&str>,
) {
    clients.remove(&client_id);
    if clients.len() >= 128 {
        let _ = outgoing.send(json!({"type":"relay.client.close","clientId":client_id}));
        return;
    }
    let crypto = if let Some(id) = secure_channel_id {
        let Ok(transport) = crate::secure_transport::transport(&state) else {
            return;
        };
        let Some(session) = transport.session(id) else {
            let _ = outgoing.send(json!({"type":"relay.client.close","clientId":client_id}));
            return;
        };
        Some((transport, session))
    } else {
        None
    };
    let output_crypto = crypto.as_ref().map(|(_, session)| session.clone());
    let (session_output, mut output) = mpsc::channel::<Value>();
    let socket = crate::socket::SocketSession::spawn(state, session_output);
    let relay_output = outgoing.clone();
    let output_client_id = client_id.clone();
    let bridge = tokio::spawn(async move {
        while let Some(payload) = output.recv().await {
            let payload = if let Some(crypto) = &output_crypto {
                match crypto.seal(&payload) {
                    Ok(payload) => payload,
                    Err(_) => {
                        let _ = relay_output
                            .send(json!({"type":"relay.client.close","clientId":output_client_id}));
                        break;
                    }
                }
            } else {
                payload
            };
            if relay_output
                .send(json!({
                    "type": "relay.server.message",
                    "timestamp": now_rfc3339(),
                    "clientId": output_client_id,
                    "payload": payload
                }))
                .is_err()
            {
                break;
            }
        }
    });
    clients.insert(
        client_id,
        RelayClientSession {
            socket,
            bridge,
            crypto,
        },
    );
}

fn relay_activity(event: &ThreadEventEnvelope) -> Option<Value> {
    let kind = match event.event_type.as_str() {
        "thread.turn.started" => "turn_started",
        "thread.turn.completed" => "turn_terminal",
        _ => return None,
    };
    let turn_id = event.payload.get("turnId").and_then(Value::as_str)?;
    Some(json!({
        "type": "relay.activity",
        "timestamp": now_rfc3339(),
        "payload": {
            "kind": kind,
            "threadId": event.thread_id,
            "turnId": turn_id
        }
    }))
}

async fn bounded_forward(
    future: impl std::future::Future<Output = Value>,
    timeout: Duration,
) -> Value {
    tokio::time::timeout(timeout, future)
        .await
        .unwrap_or_else(|_| {
            relay_error_response(
                504,
                "Device request timed out; check its status before retrying",
            )
        })
}

async fn forward_local(state: &Arc<Supervisor>, payload: Value) -> Value {
    let path = payload["path"]
        .as_str()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("");
    if path.ends_with("/transport/shell-scope") && payload["method"] == "GET" {
        let thread = path
            .strip_prefix("/api/threads/")
            .and_then(|p| p.strip_suffix("/transport/shell-scope"));
        return match thread.and_then(|id| state.get_thread(id).ok()) {
            Some(thread) => json_response(
                json!({"shells":crate::shells::hub().list_for_thread(&thread.id).iter().map(|shell|json!({"id":shell["id"]})).collect::<Vec<_>>() }),
            ),
            None => relay_error_response(404, "Thread not found"),
        };
    }
    let key_path = path.ends_with("/transport/key");
    let session_path = path.ends_with("/transport/session");
    let encrypted = payload["headers"]["x-rcd-key"].is_string();
    if path.starts_with("/api/threads/") && path.ends_with("/publications") && !encrypted {
        return relay_error_response(
            400,
            "Publication consent requires an encrypted owner request",
        );
    }
    if key_path || encrypted || session_path {
        let transport = match crate::secure_transport::transport(state) {
            Ok(t) => t,
            Err(_) => return relay_error_response(503, "Device encryption is unavailable"),
        };
        if key_path && payload["method"] == "GET" {
            let query = payload["path"]
                .as_str()
                .unwrap_or("")
                .split_once('?')
                .map(|(_, q)| q)
                .unwrap_or("");
            let challenge = url::form_urlencoded::parse(query.as_bytes())
                .find(|(key, _)| key == "challenge")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            if challenge.len() > 128
                || !challenge
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                return relay_error_response(400, "Invalid key challenge");
            }
            return match transport.descriptor(&challenge) {
                Ok(value) => json_response(value),
                Err(_) => relay_error_response(503, "Device encryption is unavailable"),
            };
        }
        if !encrypted {
            return relay_error_response(400, "An encrypted request is required");
        }
        if !transport.has_key(payload["headers"]["x-rcd-key"].as_str().unwrap_or("")) {
            return json!({"statusCode":409,"headers":{"content-type":"application/json","cache-control":"no-store"},"body":json!({"code":"transport_reconnect_required","message":"The device reconnected. Retry this action to use its new encryption key."}).to_string()});
        }
        let opened =
            match transport.open(&payload) {
                Ok(o) => o,
                Err(_) => return relay_error_response(
                    400,
                    "Encrypted request is invalid, expired, or replayed. Reconnect to the device.",
                ),
            };
        let response = if path.contains("/transport/stream/") {
            match transport
                .streams
                .read(opened.request["path"].as_str().unwrap_or(""))
                .await
            {
                Ok(value) => value,
                Err(_) => relay_error_response(410, "Download expired; start it again"),
            }
        } else if session_path {
            match transport.create_session(&opened) {
                Ok(value) => json_response(value),
                Err(_) => relay_error_response(429, "Too many encrypted connections"),
            }
        } else if payload["headers"]["x-rcd-hosted-workspaces"].is_string()
            && matches!(
                path,
                "/api/workspaces" | "/api/threads" | "/api/threads/start"
            )
        {
            dispatch_local(state, opened.request.clone()).await
        } else {
            dispatch_streaming(state, opened.request.clone(), &transport).await
        };
        let (response, resources) = filter_hosted_response(&payload, response);
        let mut sealed = opened
            .response(response)
            .unwrap_or_else(|_| relay_error_response(502, "Device response encryption failed"));
        if let Some(resources) = resources {
            sealed["headers"]["x-rcd-result-resource"] = json!(resources.to_string());
        }
        return sealed;
    }
    dispatch_local(state, payload).await
}
// Policy is inserted by the relay after authentication, never copied from client headers.
fn filter_hosted_response(request: &Value, mut response: Value) -> (Value, Option<Value>) {
    let Some(policy) = request["headers"]["x-rcd-hosted-workspaces"].as_str() else {
        return (response, None);
    };
    let Ok(owned) = serde_json::from_str::<std::collections::HashSet<String>>(policy) else {
        return (relay_error_response(403, "Invalid hosted policy"), None);
    };
    let path = request["path"]
        .as_str()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("");
    let method = request["method"].as_str().unwrap_or("");
    if !matches!(
        (method, path),
        ("GET", "/api/workspaces" | "/api/threads")
            | ("POST", "/api/workspaces" | "/api/threads/start")
    ) || response["statusCode"].as_u64().unwrap_or(500) >= 300
    {
        return (response, None);
    }
    let Ok(mut data) = serde_json::from_str::<Value>(response["body"].as_str().unwrap_or(""))
    else {
        return (relay_error_response(502, "Invalid hosted response"), None);
    };
    if method == "GET" {
        let Some(rows) = data.as_array_mut() else {
            return (relay_error_response(502, "Invalid hosted list"), None);
        };
        rows.retain(|row| {
            row[if path == "/api/workspaces" {
                "id"
            } else {
                "workspaceId"
            }]
            .as_str()
            .is_some_and(|id| owned.contains(id))
        });
    }
    let rows = if let Some(rows) = data.as_array() {
        rows.clone()
    } else {
        vec![data.clone()]
    };
    let resources = rows
        .iter()
        .map(|row| json!({"id":row["id"],"workspaceId":row["workspaceId"]}))
        .collect::<Vec<_>>();
    response["body"] = json!(data.to_string());
    (response, Some(json!(resources)))
}
fn json_response(value: Value) -> Value {
    json!({"statusCode":200,"headers":{"content-type":"application/json","cache-control":"no-store"},"body":value.to_string()})
}
async fn dispatch_raw(
    state: &Arc<Supervisor>,
    payload: Value,
) -> Result<axum::response::Response, Value> {
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET");
    let path = payload.get("path").and_then(Value::as_str).unwrap_or("/");
    if !path.starts_with('/') || !(path == "/healthz" || path.starts_with("/api/")) {
        return Err(relay_error_response(403, "This relay path is not allowed."));
    }
    let Ok(method) = reqwest::Method::from_bytes(method.as_bytes()) else {
        return Err(relay_error_response(400, "Invalid relay request method."));
    };
    let mut request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .extension(crate::auth::TrustedRelayForward);
    if let Some(headers) = payload.get("headers").and_then(Value::as_object) {
        for name in ["content-type", "accept", "if-none-match", "range"] {
            if let Some(value) = headers.get(name).and_then(Value::as_str) {
                request = request.header(name, value);
            }
        }
    }
    let body = match decode_relay_request_body(&payload) {
        Ok(body) => body,
        Err(message) => return Err(relay_error_response(400, message)),
    };
    let request = match request.body(axum::body::Body::from(body.unwrap_or_default())) {
        Ok(request) => request,
        Err(_) => return Err(relay_error_response(400, "Invalid relay request headers.")),
    };
    // In-process dispatch avoids both a forgeable authorization header and a loopback hop.
    crate::http::router(state.clone())
        .oneshot(request)
        .await
        .map_err(|_| relay_error_response(502, "Device request failed"))
}
async fn dispatch_streaming(
    state: &Arc<Supervisor>,
    payload: Value,
    transport: &crate::secure_transport::Transport,
) -> Value {
    let path = payload["path"].as_str().unwrap_or("").to_string();
    let response = match dispatch_raw(state, payload).await {
        Ok(response) => response,
        Err(error) => return error,
    };
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter(|(key, _)| {
            matches!(
                key.as_str(),
                "content-type"
                    | "content-length"
                    | "content-disposition"
                    | "content-range"
                    | "accept-ranges"
                    | "cache-control"
                    | "content-security-policy"
                    | "referrer-policy"
                    | "x-content-type-options"
            )
        })
        .filter_map(|(key, value)| value.to_str().ok().map(|v| (key.to_string(), json!(v))))
        .collect::<serde_json::Map<_, _>>();
    match transport.streams.begin(&path, response.into_body()).await {
        Ok((bytes, next)) => {
            json!({"statusCode":status,"headers":headers,"body":base64::engine::general_purpose::STANDARD.encode(bytes),"bodyEncoding":"base64","streamNext":next})
        }
        Err(_) => relay_error_response(502, "Device download could not continue"),
    }
}
async fn dispatch_local(state: &Arc<Supervisor>, payload: Value) -> Value {
    match dispatch_raw(state, payload).await {
        Ok(response) => {
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    matches!(
                        name.as_str(),
                        "content-type"
                            | "content-disposition"
                            | "cache-control"
                            | "x-content-type-options"
                            | "content-security-policy"
                            | "referrer-policy"
                    )
                    .then(|| {
                        value.to_str().ok().map(|value| {
                            (name.as_str().to_string(), Value::String(value.to_string()))
                        })
                    })
                    .flatten()
                })
                .collect::<serde_json::Map<String, Value>>();
            let content_type = headers
                .get("content-type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let bytes = match axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024).await {
                Ok(bytes) => bytes,
                Err(_) => {
                    return relay_error_response(502, "Relay response exceeds the size limit.")
                }
            };
            let text_response = content_type.starts_with("text/")
                || content_type.contains("application/json")
                || content_type.contains("+json")
                || content_type.contains("application/javascript")
                || content_type.contains("application/xml")
                || content_type.contains("+xml")
                || content_type.contains("image/svg+xml");
            if text_response {
                json!({
                    "statusCode": status,
                    "headers": headers,
                    "body": String::from_utf8_lossy(&bytes)
                })
            } else {
                json!({
                    "statusCode": status,
                    "headers": headers,
                    "body": base64::engine::general_purpose::STANDARD.encode(bytes),
                    "bodyEncoding": "base64"
                })
            }
        }
        Err(error) => error,
    }
}

pub(crate) fn decode_relay_request_body(payload: &Value) -> Result<Option<Vec<u8>>, &'static str> {
    let Some(body) = payload.get("body") else {
        return Ok(None);
    };
    if body.is_null() {
        return Ok(None);
    }
    let Some(body) = body.as_str() else {
        return Err("Invalid relay request body.");
    };
    if payload.get("bodyEncoding").and_then(Value::as_str) == Some("base64") {
        base64::engine::general_purpose::STANDARD
            .decode(body)
            .map(Some)
            .map_err(|_| "Invalid base64 relay request body.")
    } else {
        Ok(Some(body.as_bytes().to_vec()))
    }
}

fn relay_error_response(status_code: u16, message: &str) -> Value {
    json!({
        "statusCode": status_code,
        "headers": { "content-type": "application/json" },
        "body": json!({
            "code": "gateway_unavailable",
            "message": message
        }).to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use remote_codex_protocol::{Mode, Provider};
    use remote_codex_runtime::actor::SharedRuntime;
    use remote_codex_runtime::config::RuntimeConfig;
    use remote_codex_runtime::db::Database;
    use remote_codex_runtime::fake::FakeRuntime;
    use tempfile::TempDir;

    fn state_with_relay_url(relay_url: &str) -> (TempDir, Arc<Supervisor>) {
        let directory = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            mode: Mode::Relay,
            host: "127.0.0.1".into(),
            port: 0,
            workspace_root: directory.path().join("workspaces"),
            database_url: directory.path().join("supervisor.sqlite"),
            app_name: "test".into(),
            app_version: "0.12.0".into(),
            environment: "test".into(),
            auth_required: true,
            admin_username: Some("admin".into()),
            admin_password: Some("secret123".into()),
            session_secret: Some("0123456789abcdef".into()),
            relay_server_url: Some(relay_url.into()),
            relay_agent_token: Some("agent token/+".into()),
            enabled_providers: vec![Provider::Codex],
            acp_command: None,
            acp_startup_timeout_ms: 1_000,
            fake_runtime: true,
        };
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let database = Database::open(&config.database_url).unwrap();
        let runtime = Arc::new(FakeRuntime::new(Provider::Codex)) as SharedRuntime;
        (
            directory,
            Arc::new(Supervisor::new(config, database, vec![runtime])),
        )
    }

    #[tokio::test]
    async fn publication_consent_cannot_be_forged_by_plaintext_relay_forwarding() {
        let (_dir, state) = state_with_relay_url("http://localhost:8788");
        let response = forward_local(&state, json!({"method":"POST","path":format!("/api/threads/{}/publications", uuid::Uuid::new_v4()),"body":"{}"})).await;
        assert_eq!(response["statusCode"], 400);
        assert!(response["body"]
            .as_str()
            .unwrap()
            .contains("encrypted owner request"));
    }

    #[tokio::test]
    async fn forged_forward_header_is_denied_but_tunnel_dispatch_is_authorized() {
        let (_dir, state) = state_with_relay_url("http://localhost:8788");
        let response = crate::http::router(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/workspaces")
                    .header("x-remote-codex-relay-forwarded", "1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
        let forwarded =
            forward_local(&state, json!({"method":"GET","path":"/api/workspaces"})).await;
        assert_eq!(forwarded["statusCode"], 200);
    }

    #[tokio::test]
    async fn stats_summary_routes_through_authenticated_device_tunnel() {
        let (_dir, state) = state_with_relay_url("http://localhost:8788");
        let forwarded = forward_local(
            &state,
            json!({"method":"GET","path":"/api/stats/summary?range=7d&tz=America%2FToronto"}),
        )
        .await;
        assert_eq!(forwarded["statusCode"], 200);
        let body: Value = serde_json::from_str(forwarded["body"].as_str().unwrap()).unwrap();
        assert_eq!(body["range"], "7d");
        assert_eq!(body["totals"]["turns"], 0);
        assert_eq!(body["daily"].as_array().unwrap().len(), 7);
    }

    #[tokio::test]
    async fn half_open_tunnel_reconnects_automatically_with_the_same_supervisor() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay_url = format!("ws://{}", listener.local_addr().unwrap());
        let (_directory, state) = state_with_relay_url(&relay_url);
        let tunnel = tokio::spawn(run_relay_tunnel(state.clone()));
        let (tcp, _) = listener.accept().await.unwrap();
        let _silent = tokio_tungstenite::accept_async(tcp).await.unwrap();
        // TCP remains open and writable. No return frames reach the supervisor.
        let (tcp, _) = tokio::time::timeout(Duration::from_secs(7), listener.accept())
            .await
            .expect("reconnect without user intervention or process restart")
            .unwrap();
        let mut replacement = tokio_tungstenite::accept_async(tcp).await.unwrap();
        assert!(!state
            .relay_connected
            .load(std::sync::atomic::Ordering::SeqCst));
        replacement
            .send(Message::Text(
                json!({"type":"relay.connected"}).to_string().into(),
            ))
            .await
            .unwrap();
        replacement.send(Message::Text(json!({
            "type":"relay.request", "requestId":"health", "payload":{"method":"GET","path":"/healthz"}
        }).to_string().into())).await.unwrap();
        let response = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(Ok(message)) = replacement.next().await {
                if let Message::Text(text) = message {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["type"] == "relay.response" {
                        return value;
                    }
                }
            }
            panic!("replacement tunnel closed");
        })
        .await
        .unwrap();
        assert_eq!(response["requestId"], "health");
        assert_eq!(response["payload"]["statusCode"], 200);
        let body: Value =
            serde_json::from_str(response["payload"]["body"].as_str().unwrap()).unwrap();
        assert_eq!(body["processId"], std::process::id());
        assert_eq!(body["relayConnected"], true);
        assert!(!tunnel.is_finished());
        tunnel.abort();
        let _ = tunnel.await;
        assert!(!state
            .relay_connected
            .load(std::sync::atomic::Ordering::SeqCst));
    }
}
