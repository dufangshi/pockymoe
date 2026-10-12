use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::auth::PeerCaller;
use crate::bounded_channel as mpsc;
use anyhow::{anyhow, Result};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use pockymoe_protocol::now_rfc3339;
use pockymoe_runtime::Supervisor;
use serde_json::{json, Value};
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest, protocol::WebSocketConfig, Message,
};
use tower::ServiceExt;
use url::Url;

const RELAY_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);
const RELAY_RECEIVE_TIMEOUT: Duration = Duration::from_secs(5);
const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RELAY_RECONNECT_INITIAL_DELAY: Duration = Duration::from_secs(1);
const RELAY_RECONNECT_MAX_DELAY: Duration = Duration::from_secs(3);
const TUNNEL_MESSAGE_LIMIT: usize = 128 * 1024 * 1024;

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
        .ok_or_else(|| anyhow!("POCKYMOE_RELAY_SERVER_URL is required"))?;
    let token = state
        .config
        .relay_agent_token
        .as_deref()
        .ok_or_else(|| anyhow!("POCKYMOE_RELAY_AGENT_TOKEN is required"))?;
    let tunnel_url = relay_tunnel_url(server_url)?;
    let mut reconnect_delay = RELAY_RECONNECT_INITIAL_DELAY;

    loop {
        let mut handshake = tunnel_url.as_str().into_client_request()?;
        handshake
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse()?);
        let socket_config = WebSocketConfig::default()
            .max_message_size(Some(TUNNEL_MESSAGE_LIMIT))
            .max_frame_size(Some(TUNNEL_MESSAGE_LIMIT));
        match tokio::time::timeout(
            RELAY_CONNECT_TIMEOUT,
            connect_async_with_config(handshake, Some(socket_config), false),
        )
        .await
        {
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
    let peer_connection = crate::peer_link::TunnelConnection::new(&state, outgoing.clone());
    let mut clients = HashMap::<String, RelayClientSession>::new();
    let mut heartbeat = tokio::time::interval(RELAY_HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut activity = state.bus.subscribe();

    outgoing
        .send(json!({ "type": "relay.heartbeat", "timestamp": now_rfc3339(), "threadLineage":thread_lineage(&state), "portMappings":crate::ports::snapshot(&state) }))
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
                            if let Some(id) = message["deviceId"].as_str() {
                                crate::owner_trust::remember_device(&state, id)?;
                            }
                            state.relay_connected.store(true, std::sync::atomic::Ordering::SeqCst);
                            let identity = message["deviceId"].as_str().map(|id| crate::peer_link::RelayIdentity {
                                device_id: id.into(),
                                device_name: message["deviceName"].as_str().unwrap_or("").into(),
                                port_preview_base_url: message["portPreviewBaseUrl"].as_str().map(str::to_owned),
                                public_base_url: message["publicBaseUrl"].as_str().map(str::to_owned),
                            });
                            peer_connection.connected(identity);
                        }
                        if matches!(message["type"].as_str(), Some("peer.response" | "peer.directory.result")) {
                            peer_connection.receive(&message);
                            continue;
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
                crate::ports::close_unauthorized(&state);
                clients.retain(|id, client| {
                    if channel_authorized(&state, client.crypto.as_ref().map(|(_, s)| s.as_ref())) {
                        true
                    } else {
                        let _ = outgoing.send(json!({"type":"relay.client.close","clientId":id}));
                        false
                    }
                });
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
                    "threadLineage":thread_lineage(&state), "portMappings":crate::ports::snapshot(&state)
                })).is_err() {
                    return Err(anyhow!("relay tunnel writer closed"));
                }
            }
            event = activity.recv() => {
                match event {
                    Ok(event) => {
                        if event.payload["reason"] == "port_mappings_changed" {
                            let _ = outgoing.send(json!({"type":"relay.heartbeat","portMappings":crate::ports::snapshot(&state)}));
                        }
                        if event.payload["reason"] == "thread_created" || event.payload["reason"] == "child_deleted" {
                            let _ = outgoing.send(json!({"type":"relay.heartbeat","timestamp":now_rfc3339(),"threadLineage":thread_lineage(&state)}));
                        }
                        if matches!(event.event_type.as_str(), "thread.turn.started" | "thread.turn.completed") {
                            let _ = outgoing.send(json!({"type":"relay.heartbeat","timestamp":now_rfc3339(),"threadLineage":thread_lineage(&state)}));
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
        Some("relay.revocations") => {
            if let Ok(entries) = serde_json::from_value::<crate::owner_trust::Revocations>(message)
            {
                if crate::owner_trust::apply_revocations(&state, &entries.revocations).is_ok() {
                    clients.retain(|id, client| {
                        if channel_authorized(
                            &state,
                            client.crypto.as_ref().map(|(_, s)| s.as_ref()),
                        ) {
                            true
                        } else {
                            let _ =
                                outgoing.send(json!({"type":"relay.client.close","clientId":id}));
                            false
                        }
                    });
                    let _ = outgoing.send(json!({"type":"relay.heartbeat","portMappings":crate::ports::snapshot(&state)}));
                }
            }
        }
        Some("preview.open") => crate::ports::open(state, &message),
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
            let peer = if let Some(value) = message.get("peer") {
                let Some(peer) = peer_caller(value) else {
                    let _ = outgoing.send(json!({"type":"relay.response","requestId":request_id,"payload":relay_error_response(403,"Invalid peer identity")}));
                    return;
                };
                Some(peer)
            } else {
                None
            };
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
                let mappings_changed = payload["path"]
                    .as_str()
                    .is_some_and(|p| p.starts_with("/api/port-mappings"));
                let payload = bounded_forward(
                    forward_local(&state, payload, peer),
                    Duration::from_secs(60),
                )
                .await;
                if mappings_changed {
                    // Announce the committed permission before returning the API
                    // result, so the owner's immediate Open request can resolve it.
                    let _ = outgoing.send(json!({"type":"relay.heartbeat","portMappings":crate::ports::snapshot(&state)}));
                }
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
                    if !channel_authorized(&state, client.crypto.as_ref().map(|(_, s)| s.as_ref()))
                    {
                        let _ = outgoing
                            .send(json!({"type":"relay.client.close","clientId":client_id}));
                        clients.remove(client_id);
                        return;
                    }
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

fn peer_caller(value: &Value) -> Option<PeerCaller> {
    Some(PeerCaller {
        device_id: value["deviceId"]
            .as_str()
            .filter(|id| !id.is_empty())?
            .into(),
        device_name: value["deviceName"].as_str()?.into(),
        user_id: value["userId"].as_str().filter(|id| !id.is_empty())?.into(),
    })
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
    if !channel_authorized(&state, crypto.as_ref().map(|(_, s)| s.as_ref())) {
        let _ = outgoing.send(json!({"type":"relay.client.close","clientId":client_id}));
        return;
    }
    let output_crypto = crypto.as_ref().map(|(_, session)| session.clone());
    let (session_output, mut output) = mpsc::channel::<Value>();
    let socket = crate::socket::SocketSession::spawn_authorized(
        state.clone(),
        session_output,
        output_crypto.as_ref().and_then(|s| s.principal.clone()),
        output_crypto.as_ref().and_then(|s| s.thread_id.clone()),
    );
    let relay_output = outgoing.clone();
    let output_client_id = client_id.clone();
    let bridge = tokio::spawn(async move {
        let mut auth_tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            let payload = tokio::select! {
                _ = auth_tick.tick() => {
                    if !channel_authorized(&state,output_crypto.as_deref()) {
                        let _=relay_output.send(json!({"type":"relay.client.close","clientId":output_client_id}));
                        break;
                    }
                    continue;
                }
                payload=output.recv() => { let Some(payload)=payload else {
                    let _=relay_output.send(json!({"type":"relay.client.close","clientId":output_client_id}));
                    break;
                }; payload }
            };
            if !channel_authorized(&state, output_crypto.as_deref()) {
                let _ = relay_output
                    .send(json!({"type":"relay.client.close","clientId":output_client_id}));
                break;
            }
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

fn channel_authorized(
    state: &Supervisor,
    channel: Option<&crate::secure_transport::Session>,
) -> bool {
    if channel.is_some_and(|c| !c.alive()) {
        return false;
    }
    match channel.and_then(|c| c.principal.as_ref()) {
        Some(p) => {
            p.valid(state)
                && (p.is_owner()
                    || channel.is_some_and(|c| {
                        crate::grants::effective_access(state, p, c.thread_id.as_deref(), None)
                            .ok()
                            .flatten()
                            .is_some()
                    }))
        }
        None => !crate::owner_trust::strict(state),
    }
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

pub(crate) async fn forward_local(
    state: &Arc<Supervisor>,
    payload: Value,
    peer: Option<PeerCaller>,
) -> Value {
    let path = payload["path"]
        .as_str()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("");
    let strict = crate::owner_trust::strict(state);
    let bound = crate::owner_trust::bound(state);
    let method = payload["method"].as_str().unwrap_or("GET");
    let encrypted = payload["headers"]["x-rcd-key"].is_string();
    let bootstrap = crate::owner_trust::bootstrap(method, path);
    let public = method == "GET"
        && (path == "/healthz"
            || path.ends_with("/transport/key")
            || path
                .strip_prefix("/api/publications/")
                .is_some_and(|token| !token.is_empty() && !token.contains('/')));
    if strict && peer.is_none() && path.starts_with("/api/peer/") {
        return crate::owner_trust::error(403, "device_strict_mode");
    }
    if strict && peer.is_none() && !encrypted && !public {
        return crate::owner_trust::error(403, "device_strict_mode");
    }
    if bootstrap && !encrypted {
        return crate::owner_trust::error(403, "device_strict_mode");
    }
    if peer.is_some() != path.starts_with("/api/peer/") {
        return relay_error_response(403, "Peer credentials only allow /api/peer/ requests");
    }
    // Peer routes are literal; mirror the relay so neither side depends on the other
    // normalizing dot segments or encodings into a different route.
    if peer.is_some()
        && (path.contains(['%', '\\'])
            || path
                .split('/')
                .skip(1)
                .any(|segment| matches!(segment, "" | "." | "..")))
    {
        return relay_error_response(403, "Peer requests must use literal /api/peer/ paths");
    }
    if peer.is_some() && !state.peer_access_enabled() {
        return json!({"statusCode":403,"headers":{"content-type":"application/json"},"body":json!({"code":"peer_access_disabled","message":"Peer access is disabled on this device."}).to_string()});
    }
    let key_path = path.ends_with("/transport/key");
    let session_path = peer.is_none() && path.ends_with("/transport/session");
    if peer.is_some()
        && !encrypted
        && !(key_path && payload["method"] == "GET")
        && !path.starts_with("/api/peer/transport/stream/")
    {
        return relay_error_response(400, "An encrypted peer request is required");
    }
    if !strict
        && !encrypted
        && path.ends_with("/transport/shell-scope")
        && payload["method"] == "GET"
    {
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
                Ok(mut value) => {
                    value["trust"] = crate::owner_trust::hint(state)
                        .unwrap_or(json!({"bound":true,"strict":true,"accountKeyId":null}));
                    value["certificate"] = match crate::owner_trust::device_certificate(state) {
                        Ok(certificate) => json!(certificate),
                        Err(_) => {
                            return relay_error_response(503, "Device certificate is unavailable")
                        }
                    };
                    json_response(value)
                }
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
        let mut opened = opened;
        let peer_bootstrap =
            peer.is_some() && method == "POST" && path == "/api/peer/transport/certificate";
        let peer_principal = if strict && !peer_bootstrap {
            if let Some(caller) = &peer {
                match crate::owner_trust::authorize_peer(
                    state,
                    caller,
                    &payload,
                    &opened.aad,
                    &opened.sealed,
                ) {
                    Ok(proof) => Some(proof),
                    Err(_) => {
                        return opened
                            .response(crate::owner_trust::error(401, "peer_certificate_required"))
                            .unwrap_or_else(|_| {
                                relay_error_response(502, "Device response encryption failed")
                            })
                    }
                }
            } else {
                None
            }
        } else {
            None
        };
        let authorization = if bound && peer.is_none() && !bootstrap {
            Some(crate::owner_trust::authorize(
                state,
                &payload,
                &opened.aad,
                &opened.sealed,
            ))
        } else {
            None
        };
        let principal = authorization
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .cloned();
        let owner_endpoint = path == "/api/transport/trust"
            || path == "/api/transport/device-certificate"
            || path == "/api/transport/grants"
            || path.starts_with("/api/transport/grants/");
        let signature_supplied = payload["headers"]["x-rcd-session"].is_string()
            || payload["headers"]["x-rcd-signature"].is_string();
        if peer.is_none()
            && principal.is_none()
            && ((!public && !bootstrap && (strict || (bound && signature_supplied)))
                || owner_endpoint)
        {
            return opened
                .response({
                    let (status, code) = authorization
                        .as_ref()
                        .and_then(|r| r.as_ref().err())
                        .map(crate::owner_trust::rejection_code)
                        .unwrap_or((401, "browser_registration_required"));
                    crate::owner_trust::error(status, code)
                })
                .unwrap_or_else(|_| {
                    relay_error_response(502, "Device response encryption failed")
                });
        }
        if let Some(p) = principal.as_ref().filter(|_| !bootstrap) {
            if let Err(error) = crate::grants::authorize_path(
                state,
                p,
                method,
                opened.request["path"].as_str().unwrap_or(path),
            ) {
                let response = crate::owner_trust::response(Err(error));
                let status = response.status().as_u16();
                let body = axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap_or_default();
                return opened.response(json!({"statusCode":status,"headers":{"content-type":"application/json"},"body":String::from_utf8_lossy(&body)})).unwrap_or_else(|_|relay_error_response(502,"Device response encryption failed"));
            }
        }
        opened.principal = principal.clone();
        if principal.is_none() {
            opened.request["fileActor"] = payload["headers"]["x-rcd-file-actor"].clone();
        }
        let response = if path.ends_with("/transport/shell-scope") && method == "GET" {
            let thread = path
                .strip_prefix("/api/threads/")
                .and_then(|p| p.strip_suffix("/transport/shell-scope"));
            match thread.and_then(|id| state.get_thread(id).ok()) {
                Some(thread) => json_response(
                    json!({"shells":crate::shells::hub().list_for_thread(&thread.id).iter().map(|shell|json!({"id":shell["id"]})).collect::<Vec<_>>()}),
                ),
                None => relay_error_response(404, "Thread not found"),
            }
        } else if path.contains("/transport/stream/") {
            match transport
                .streams
                .read(
                    opened.request["path"].as_str().unwrap_or(""),
                    &crate::secure_transport::StreamPrincipal::new(
                        principal.as_ref(),
                        peer_principal.as_ref(),
                    ),
                )
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
        } else {
            dispatch_streaming(
                state,
                opened.request.clone(),
                &transport,
                peer.clone(),
                principal,
                peer_principal,
            )
            .await
        };
        return opened
            .response(response)
            .unwrap_or_else(|_| relay_error_response(502, "Device response encryption failed"));
    }
    let mut payload = payload;
    payload["fileActor"] = payload["headers"]["x-rcd-file-actor"].clone();
    dispatch_local(state, payload, peer).await
}
fn json_response(value: Value) -> Value {
    json!({"statusCode":200,"headers":{"content-type":"application/json","cache-control":"no-store"},"body":value.to_string()})
}
async fn dispatch_raw(
    state: &Arc<Supervisor>,
    payload: Value,
    peer: Option<PeerCaller>,
    encrypted: bool,
    principal: Option<crate::owner_trust::BrowserPrincipal>,
    peer_principal: Option<crate::owner_trust::PeerPrincipal>,
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
    if let Some(peer) = peer {
        request = request.extension(peer);
    }
    if let Some(proof) = peer_principal {
        request = request.extension(proof);
    }
    if encrypted {
        request = request.extension(crate::owner_trust::EncryptedOwnerRequest);
    }
    if let Some(principal) = principal {
        request = request
            .extension(crate::file_documents::FileActor(principal.actor()))
            .extension(principal);
    } else if let Some(actor) = payload["fileActor"].as_str().filter(|s| !s.is_empty()) {
        request = request.extension(crate::file_documents::FileActor(format!("relay:{actor}")));
    }
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
    peer: Option<PeerCaller>,
    principal: Option<crate::owner_trust::BrowserPrincipal>,
    peer_principal: Option<crate::owner_trust::PeerPrincipal>,
) -> Value {
    let path = payload["path"].as_str().unwrap_or("").to_string();
    let stream_principal =
        crate::secure_transport::StreamPrincipal::new(principal.as_ref(), peer_principal.as_ref());
    let response = match dispatch_raw(state, payload, peer, true, principal, peer_principal).await {
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
                    | "etag"
                    | "content-security-policy"
                    | "referrer-policy"
                    | "x-content-type-options"
            )
        })
        .filter_map(|(key, value)| value.to_str().ok().map(|v| (key.to_string(), json!(v))))
        .collect::<serde_json::Map<_, _>>();
    match transport
        .streams
        .begin(&path, response.into_body(), stream_principal)
        .await
    {
        Ok((bytes, next)) => {
            json!({"statusCode":status,"headers":headers,"body":base64::engine::general_purpose::STANDARD.encode(bytes),"bodyEncoding":"base64","streamNext":next})
        }
        Err(_) => relay_error_response(502, "Device download could not continue"),
    }
}
async fn dispatch_local(
    state: &Arc<Supervisor>,
    payload: Value,
    peer: Option<PeerCaller>,
) -> Value {
    match dispatch_raw(state, payload, peer, false, None, None).await {
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
                            | "etag"
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
pub(crate) mod tests {
    use super::*;
    use pockymoe_protocol::{Mode, Provider};
    use pockymoe_runtime::actor::SharedRuntime;
    use pockymoe_runtime::config::RuntimeConfig;
    use pockymoe_runtime::db::Database;
    use pockymoe_runtime::fake::FakeRuntime;
    use tempfile::TempDir;

    pub(crate) fn state_with_relay_url(relay_url: &str) -> (TempDir, Arc<Supervisor>) {
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
            web_origin: None,
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

    fn peer() -> PeerCaller {
        PeerCaller {
            device_id: "source".into(),
            device_name: "Source device".into(),
            user_id: "owner".into(),
        }
    }
    async fn relay_frame(
        socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        kind: &str,
    ) -> Value {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                match socket.next().await.unwrap().unwrap() {
                    Message::Text(text) => {
                        let value: Value = serde_json::from_str(&text).unwrap();
                        if value["type"] == kind {
                            return value;
                        }
                    }
                    Message::Ping(data) => socket.send(Message::Pong(data)).await.unwrap(),
                    other => panic!("unexpected frame: {other:?}"),
                }
            }
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn peer_forwarding_requires_identity_opt_in_and_ciphertext() {
        let (_dir, state) = state_with_relay_url("http://127.0.0.1:1");
        let key = json!({"method":"GET","path":"/api/peer/transport/key?challenge=fresh"});
        assert_eq!(
            forward_local(&state, key.clone(), None).await["statusCode"],
            403
        );
        let disabled = forward_local(&state, key.clone(), Some(peer())).await;
        assert_eq!(disabled["statusCode"], 403);
        assert!(disabled["body"]
            .as_str()
            .unwrap()
            .contains("peer_access_disabled"));
        state.set_peer_access(true).unwrap();
        let response = forward_local(&state, key, Some(peer())).await;
        assert_eq!(response["statusCode"], 200);
        let descriptor: Value = serde_json::from_str(response["body"].as_str().unwrap()).unwrap();
        assert_eq!(descriptor["challenge"], "fresh");
        for path in [
            "/api/cli",
            "/api/config/peer-access",
            "/api/transport/key",
            "/healthz",
            "/api/peer/../cli",
            "/api/peer/./cli",
            "/api/peer//cli",
            "/api/peer/%2e%2e/cli",
        ] {
            assert_eq!(
                forward_local(&state, json!({"method":"GET","path":path}), Some(peer())).await
                    ["statusCode"],
                403,
                "{path}"
            );
        }
        assert_eq!(
            forward_local(
                &state,
                json!({"method":"POST","path":"/api/peer/cli","body":"{}"}),
                Some(peer())
            )
            .await["statusCode"],
            400
        );
        assert_eq!(
            forward_local(
                &state,
                json!({"method":"GET","path":"/api/peer/transport/key?challenge=bad%2Fchallenge"}),
                Some(peer())
            )
            .await["statusCode"],
            400
        );
        assert!(
            peer_caller(&json!({"deviceId":"source","deviceName":"Source","userId":"owner"}))
                .is_some()
        );
        assert!(peer_caller(&json!({"deviceId":"source","deviceName":"Source"})).is_none());
    }

    #[tokio::test]
    async fn peer_tunnel_routes_frames_accepts_large_messages_and_fails_pending_on_close() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (_dir, state) =
            state_with_relay_url(&format!("ws://{}", listener.local_addr().unwrap()));
        state.set_peer_access(true).unwrap();
        let tunnel = tokio::spawn(run_relay_tunnel(state.clone()));
        let (tcp, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut relay = tokio_tungstenite::accept_async(tcp).await.unwrap();
        relay.send(Message::Text(json!({"type":"relay.connected","deviceId":"local-device","deviceName":"Local device"}).to_string().into())).await.unwrap();
        // A single relay frame over the old 16 MiB limit must survive decoding.
        relay.send(Message::Text(json!({"type":"relay.request","requestId":"large","payload":{"method":"GET","path":"/healthz","body":"x".repeat(20*1024*1024)}}).to_string().into())).await.unwrap();
        let response = relay_frame(&mut relay, "relay.response").await;
        assert_eq!(response["requestId"], "large");
        assert_eq!(response["payload"]["statusCode"], 200);
        let identity = crate::peer_link::relay_identity(&state).unwrap();
        assert_eq!(identity.device_id, "local-device");
        assert_eq!(identity.device_name, "Local device");
        state
            .db
            .with(|conn| {
                assert_eq!(
                    conn.query_row(
                        "SELECT relay_device_id FROM device_owner_trust WHERE singleton=1",
                        [],
                        |row| row.get::<_, String>(0)
                    )?,
                    "local-device"
                );
                Ok(())
            })
            .unwrap();
        relay.send(Message::Text(json!({"type":"relay.request","requestId":"peer-key","peer":{"deviceId":"source","deviceName":"Source device","userId":"owner"},"payload":{"method":"GET","path":"/api/peer/transport/key?challenge=from-relay-frame"}}).to_string().into())).await.unwrap();
        let response = relay_frame(&mut relay, "relay.response").await;
        assert_eq!(response["requestId"], "peer-key");
        assert_eq!(response["payload"]["statusCode"], 200);
        relay.send(Message::Text(json!({"type":"relay.request","requestId":"invalid-peer","peer":{"deviceId":"source"},"payload":{"method":"GET","path":"/api/peer/transport/key"}}).to_string().into())).await.unwrap();
        let response = relay_frame(&mut relay, "relay.response").await;
        assert_eq!(response["requestId"], "invalid-peer");
        assert_eq!(response["payload"]["statusCode"], 403);
        let directory = tokio::spawn({
            let state = state.clone();
            async move { crate::peer_link::directory(&state).await }
        });
        let request = relay_frame(&mut relay, "peer.directory").await;
        relay.send(Message::Text(json!({"type":"peer.directory.result","requestId":request["requestId"],"devices":[{"deviceId":"local-device","name":"Local device","online":true,"self":true}]}).to_string().into())).await.unwrap();
        assert_eq!(directory.await.unwrap().unwrap()[0]["self"], true);
        let pending = tokio::spawn({
            let state = state.clone();
            async move {
                crate::peer_link::relay_request(
                    &state,
                    "remote-device",
                    json!({"method":"GET","path":"/api/peer/transport/key"}),
                )
                .await
            }
        });
        let request = relay_frame(&mut relay, "peer.request").await;
        assert_eq!(request["targetDeviceId"], "remote-device");
        relay.send(Message::Text(json!({"type":"peer.response","requestId":request["requestId"],"payload":{"statusCode":503,"headers":{},"body":"offline"}}).to_string().into())).await.unwrap();
        assert_eq!(pending.await.unwrap().unwrap()["statusCode"], 503);
        let pending = tokio::spawn({
            let state = state.clone();
            async move { crate::peer_link::directory(&state).await }
        });
        relay_frame(&mut relay, "peer.directory").await;
        relay.close(None).await.unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), pending)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err()
                .downcast_ref(),
            Some(crate::peer_link::PeerError::RelayUnavailable)
        ));
        tunnel.abort();
        let _ = tunnel.await;
    }

    #[tokio::test]
    async fn publication_consent_cannot_be_forged_by_plaintext_relay_forwarding() {
        let (_dir, state) = state_with_relay_url("http://localhost:8788");
        let response = forward_local(&state, json!({"method":"POST","path":format!("/api/threads/{}/publications", uuid::Uuid::new_v4()),"body":"{}"}), None).await;
        assert_eq!(response["statusCode"], 400);
        assert!(response["body"]
            .as_str()
            .unwrap()
            .contains("encrypted owner request"));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn file_receipts_bind_to_trusted_relay_actor_and_ignore_inner_actor_forgery() {
        let (_dir, state) = state_with_relay_url("http://localhost:8788");
        let workspace = state
            .create_workspace(pockymoe_protocol::CreateWorkspaceInput {
                abs_path: Some(state.config.workspace_root.to_string_lossy().into()),
                git_url: None,
                label: Some("files".into()),
            })
            .unwrap();
        std::fs::write(state.config.workspace_root.join("a.txt"), "base").unwrap();
        let doc = state.file_document(&workspace.id, "a.txt").unwrap();
        let operation = uuid::Uuid::new_v4().to_string();
        let saved = state
            .file_save(
                "relay:alice",
                &workspace.id,
                pockymoe_runtime::file_documents::SaveDocument {
                    path: doc.path,
                    workspace_revision: doc.workspace_revision,
                    file_identity: doc.file_identity,
                    expected_hash: doc.content_hash.unwrap(),
                    content: "mine".into(),
                    draft_revision: 1,
                    operation_id: operation.clone(),
                    operation_created_at: chrono::Utc::now().timestamp_millis() as u64,
                },
            )
            .unwrap();
        assert_eq!(saved["status"], "saved");
        let path = format!(
            "/api/workspaces/{}/files/operations/{operation}",
            workspace.id
        );
        let own = forward_local(
            &state,
            json!({"method":"GET","path":path,"headers":{"x-rcd-file-actor":"alice"}}),
            None,
        )
        .await;
        assert_eq!(own["statusCode"], 200);
        let forged=forward_local(&state,json!({"method":"GET","path":path,"fileActor":"alice","headers":{"x-rcd-file-actor":"bob"}}),None).await;
        assert_eq!(forged["statusCode"], 410);
        let unbound = forward_local(
            &state,
            json!({"method":"GET","path":path,"fileActor":"alice"}),
            None,
        )
        .await;
        assert_eq!(unbound["statusCode"], 403);
    }

    #[tokio::test]
    async fn forged_forward_header_is_denied_but_tunnel_dispatch_is_authorized() {
        let (_dir, state) = state_with_relay_url("http://localhost:8788");
        let response = crate::http::router(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/workspaces")
                    .header("x-pockymoe-relay-forwarded", "1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
        let forwarded = forward_local(
            &state,
            json!({"method":"GET","path":"/api/workspaces"}),
            None,
        )
        .await;
        assert_eq!(forwarded["statusCode"], 200);
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

#[cfg(test)]
mod owner_channel_tests {
    use super::*;
    use crate::owner_trust::tests::{decoded, encrypted, live_owner};
    #[tokio::test]
    async fn grantee_open_channels_recheck_deletion_expiry_and_scope_changes() {
        for mutation in ["delete", "expire", "scope"] {
            let (_dir, state, owner, mut grant, _registration, key, p) =
                crate::grants::tests::fixture().await;
            let path = "/api/threads/root/transport/session";
            let (r, response_key, aad) = crate::owner_trust::tests::encrypted_with_key(
                &state,
                "POST",
                path,
                Value::Null,
                Some(&p),
                &key,
            );
            let (status, body) = decoded(forward_local(&state, r, None).await, &response_key, &aad);
            assert_eq!(status, 200);
            let transport = crate::secure_transport::transport(&state).unwrap();
            let channel = transport
                .session(body["channelId"].as_str().unwrap())
                .unwrap();
            assert_eq!(
                channel.principal.as_ref().unwrap().actor(),
                "grantee:recipient"
            );
            assert!(channel_authorized(&state, Some(&channel)));
            let (outgoing, mut output) = mpsc::channel();
            let mut clients = HashMap::new();
            connect_relay_client(
                state.clone(),
                &mut clients,
                &outgoing,
                "grantee".into(),
                Some(channel.id()),
            );
            let connected = tokio::time::timeout(Duration::from_secs(1), output.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(connected["type"], "relay.server.message");
            match mutation {
                "delete" => {
                    let (r, k, aad) = encrypted(
                        &state,
                        "DELETE",
                        &format!("/api/transport/grants/{}", grant.grant_id),
                        Value::Null,
                        Some(&owner),
                    );
                    assert_eq!(
                        decoded(forward_local(&state, r, None).await, &k, &aad).0,
                        200
                    );
                }
                "expire" => {
                    grant.expires_at = Some(crate::owner_trust::now() - 1);
                    state
                        .db
                        .with(|c| {
                            c.execute(
                                "UPDATE device_grants SET grant_json=?1 WHERE grant_id=?2",
                                rusqlite::params![serde_json::to_string(&grant)?, grant.grant_id],
                            )?;
                            Ok(())
                        })
                        .unwrap();
                }
                _ => {
                    grant.thread_id = Some("other".into());
                    state
                        .db
                        .with(|c| {
                            c.execute(
                                "UPDATE device_grants SET grant_json=?1 WHERE grant_id=?2",
                                rusqlite::params![serde_json::to_string(&grant)?, grant.grant_id],
                            )?;
                            Ok(())
                        })
                        .unwrap();
                }
            }
            assert!(!channel_authorized(&state, Some(&channel)), "{mutation}");
            let closed = tokio::time::timeout(Duration::from_secs(6), output.recv())
                .await
                .unwrap()
                .expect("idle channel closed");
            assert_eq!(closed["type"], "relay.client.close", "{mutation}");
        }
    }
    #[tokio::test]
    async fn strict_channels_inherit_owner_principal_and_reject_legacy_or_revoked_channels() {
        let (_dir, state) = super::tests::state_with_relay_url("http://127.0.0.1:1");
        let (outgoing, mut output) = mpsc::channel();
        let mut clients = HashMap::new();
        connect_relay_client(
            state.clone(),
            &mut clients,
            &outgoing,
            "legacy".into(),
            None,
        );
        assert!(clients.contains_key("legacy"));
        let (r, key, aad) = encrypted(&state, "POST", "/api/transport/session", Value::Null, None);
        let (status, body) = decoded(forward_local(&state, r, None).await, &key, &aad);
        assert_eq!(status, 200);
        let unsigned_id = body["channelId"].as_str().unwrap();
        let transport = crate::secure_transport::transport(&state).unwrap();
        assert!(transport.session(unsigned_id).unwrap().principal.is_none());
        let owner = live_owner(&state);
        assert!(!channel_authorized(&state, None));
        connect_relay_client(state.clone(), &mut clients, &outgoing, "plain".into(), None);
        assert!(!clients.contains_key("plain"));
        connect_relay_client(
            state.clone(),
            &mut clients,
            &outgoing,
            "unsigned".into(),
            Some(unsigned_id),
        );
        assert!(!clients.contains_key("unsigned"));
        let (r, key, aad) = encrypted(
            &state,
            "POST",
            "/api/transport/session",
            Value::Null,
            Some(&owner),
        );
        let (status, body) = decoded(forward_local(&state, r, None).await, &key, &aad);
        assert_eq!(status, 200);
        let signed_id = body["channelId"].as_str().unwrap();
        let channel = transport.session(signed_id).unwrap();
        assert_eq!(channel.principal.as_ref().unwrap().actor(), owner.actor());
        connect_relay_client(
            state.clone(),
            &mut clients,
            &outgoing,
            "signed".into(),
            Some(signed_id),
        );
        assert!(clients.contains_key("signed"));
        let (request, key, aad) = encrypted(
            &state,
            "POST",
            "/api/port-mappings",
            json!({"port":3009}),
            Some(&owner),
        );
        assert_eq!(
            decoded(forward_local(&state, request, None).await, &key, &aad).0,
            200
        );
        assert_eq!(crate::ports::snapshot(&state).len(), 1);
        let mut invalid = crate::owner_trust::tests::browser_revocation(&owner);
        invalid.kind = "device".into();
        handle_relay_message(
            state.clone(),
            &mut clients,
            &outgoing,
            json!({"type":"relay.revocations","revocations":[invalid]}),
        );
        assert!(channel.alive());
        assert!(clients.contains_key("signed"));
        // Even a legacy client connected before binding cannot send plaintext input.
        handle_relay_message(
            state.clone(),
            &mut clients,
            &outgoing,
            json!({"type":"relay.client.message","clientId":"legacy","payload":{"type":"shell.input","shellId":"unused","data":"x"}}),
        );
        assert!(!clients.contains_key("legacy"));
        handle_relay_message(
            state.clone(),
            &mut clients,
            &outgoing,
            json!({"type":"relay.revocations","revocations":[crate::owner_trust::tests::browser_revocation(&owner)]}),
        );
        assert!(!channel.alive());
        assert!(!clients.contains_key("signed"));
        assert!(crate::ports::snapshot(&state).is_empty());
        assert!(!channel_authorized(&state, Some(&channel)));
        handle_relay_message(
            state.clone(),
            &mut clients,
            &outgoing,
            json!({"type":"relay.client.message","clientId":"signed","payload":{}}),
        );
        assert!(!clients.contains_key("signed"));
        let mut closes = Vec::new();
        while let Ok(Some(value)) =
            tokio::time::timeout(Duration::from_millis(10), output.recv()).await
        {
            if value["type"] == "relay.client.close" {
                closes.push(value["clientId"].as_str().unwrap().to_owned());
            }
        }
        for id in ["plain", "unsigned", "legacy", "signed"] {
            assert!(closes.iter().any(|closed| closed == id));
        }
    }
}
