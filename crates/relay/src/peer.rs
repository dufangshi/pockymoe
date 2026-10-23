use super::*;

pub(super) const MAX_IN_FLIGHT: usize = 32;
const MAX_BODY: usize = 8 * 1024 * 1024;
const SOURCE_SHARED_MESSAGE: &str = "This device is shared with other people who can control its agents, so your other devices do not accept its requests.";

pub(super) struct Caller {
    pub device_id: String,
    pub connection_id: Uuid,
    device_name: String,
    user_id: String,
    shared_control: bool,
}

impl Caller {
    pub(super) fn identity(&self) -> Value {
        json!({
            "deviceId": self.device_id,
            "deviceName": self.device_name,
            "userId": self.user_id,
            "sharedControl": self.shared_control
        })
    }
}

/// Whether someone other than the owner can drive agents on the device: an active
/// share or access grant with thread control or permission to create threads.
/// Read-only shares and grants do not count. Such a device cannot reach the
/// owner's other devices (docs/cross-device-peer.zh.md §5).
pub(super) fn device_shared_for_control(conn: &Connection, device_id: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM relay_shares s JOIN relay_devices d ON d.id=s.device_id
           WHERE s.device_id=?1 AND s.target_user_id<>d.owner_user_id
             AND s.revoked_at IS NULL AND (s.expires_at IS NULL OR s.expires_at>?2)
             AND s.pending=0 AND s.thread_access='control'
           UNION ALL
           SELECT 1 FROM relay_access_grants g JOIN relay_devices d ON d.id=g.device_id
           WHERE g.device_id=?1 AND g.target_user_id<>d.owner_user_id
             AND g.revoked_at IS NULL AND (g.expires_at IS NULL OR g.expires_at>?2)
             AND g.pending=0 AND (g.thread_access='control' OR g.can_create_threads=1))",
        params![device_id, now_rfc3339()],
        |row| row.get(0),
    )
    // Fail closed: an unreadable answer must not open the owner's other devices.
    .unwrap_or(true)
}

/// Requests a device shared for control may still send, because none of them lets
/// it act: the key handshake and certificate registration carry no command, and a
/// credential export needs a single-use authorization the owner's browser issued.
fn shared_source_allowed(method: &str, pathname: &str) -> bool {
    match method {
        "GET" => pathname == "/api/peer/transport/key",
        "POST" => {
            pathname == "/api/peer/transport/certificate"
                || pathname
                    .strip_prefix("/api/peer/connections/")
                    .and_then(|rest| rest.strip_suffix("/export"))
                    .is_some_and(|id| !id.is_empty() && !id.contains('/'))
        }
        _ => false,
    }
}

#[derive(Debug)]
struct Failure {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
}

impl Failure {
    fn forbidden() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "peer_forbidden",
            message: "peer request is not allowed",
        }
    }

    fn source_shared() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "peer_source_shared",
            message: SOURCE_SHARED_MESSAGE,
        }
    }

    fn payload(self) -> Value {
        json!({
            "statusCode": self.status.as_u16(),
            "headers": {"content-type": "application/json"},
            "body": json!({"code": self.code, "message": self.message}).to_string()
        })
    }
}

impl From<DeviceForwardError> for Failure {
    fn from(error: DeviceForwardError) -> Self {
        let (status, code, message) = match error {
            DeviceForwardError::Offline => (
                StatusCode::SERVICE_UNAVAILABLE,
                "device_offline",
                "device is offline",
            ),
            DeviceForwardError::Timeout => (
                StatusCode::GATEWAY_TIMEOUT,
                "timeout",
                "device did not respond",
            ),
            DeviceForwardError::Busy => (
                StatusCode::TOO_MANY_REQUESTS,
                "busy",
                "too many requests in flight",
            ),
        };
        Self {
            status,
            code,
            message,
        }
    }
}

fn caller(conn: &Connection, device_id: &str, connection_id: Uuid) -> Result<Caller, Failure> {
    conn.query_row(
        "SELECT d.name,d.owner_user_id FROM relay_devices d
         JOIN relay_users u ON u.id=d.owner_user_id
         WHERE d.id=?1 AND u.enabled=1",
        params![device_id],
        |row| {
            Ok(Caller {
                device_id: device_id.to_string(),
                connection_id,
                device_name: row.get(0)?,
                user_id: row.get(1)?,
                shared_control: false,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
    .map(|caller| Caller {
        shared_control: device_shared_for_control(conn, device_id),
        ..caller
    })
    .ok_or_else(Failure::forbidden)
}

pub(super) async fn device_name(state: &AppState, device_id: &str) -> String {
    state
        .store
        .conn
        .lock()
        .await
        .query_row(
            "SELECT name FROM relay_devices WHERE id=?1",
            params![device_id],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| device_id.to_string())
}

/// Peer routes are literal. Dot segments, empty segments and encodings could name a
/// different route than the prefix check saw, depending on who normalizes the path.
pub(super) fn literal_peer_path(pathname: &str) -> bool {
    pathname.starts_with("/api/peer/")
        && !pathname.contains(['%', '\\'])
        && !pathname
            .split('/')
            .skip(1)
            .any(|segment| matches!(segment, "" | "." | ".."))
}

fn request_payload(payload: &Value) -> Result<Value, Failure> {
    let method = payload["method"].as_str().ok_or_else(Failure::forbidden)?;
    let path = payload["path"].as_str().ok_or_else(Failure::forbidden)?;
    let pathname = path.split('?').next().unwrap_or(path);
    if !matches!(method, "GET" | "POST" | "PUT") || !literal_peer_path(pathname) {
        return Err(Failure::forbidden());
    }
    let uri = path.parse::<Uri>().map_err(|_| Failure::forbidden())?;
    if uri.scheme().is_some() || uri.authority().is_some() {
        return Err(Failure::forbidden());
    }
    let mut headers = serde_json::Map::new();
    if let Some(values) = payload["headers"].as_object() {
        for (name, value) in values {
            let name = name.to_ascii_lowercase();
            if matches!(
                name.as_str(),
                "content-type"
                    | "accept"
                    | "x-rcd-key"
                    | "x-rcd-request"
                    | "x-rcd-enc"
                    | "x-rcd-sealed"
                    | "x-rcd-peer-device"
                    | "x-rcd-peer-signature"
            ) && value.is_string()
            {
                headers.insert(name, value.clone());
            }
        }
    }
    if !(method == "GET" && pathname.ends_with("/transport/key"))
        && !headers
            .get("x-rcd-key")
            .and_then(Value::as_str)
            .is_some_and(|key| !key.is_empty())
    {
        return Err(Failure::forbidden());
    }
    let body = match payload.get("body") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(body)) if body.len() <= MAX_BODY => Value::String(body.clone()),
        Some(Value::String(_)) => {
            return Err(Failure {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                code: "payload_too_large",
                message: "peer request body exceeds 8 MiB",
            });
        }
        _ => return Err(Failure::forbidden()),
    };
    let body_encoding = match payload.get("bodyEncoding") {
        None | Some(Value::Null) => None,
        Some(Value::String(encoding)) => Some(Value::String(encoding.clone())),
        _ => return Err(Failure::forbidden()),
    };
    let mut payload = json!({"method": method, "path": path, "headers": headers, "body": body});
    if let Some(encoding) = body_encoding {
        payload["bodyEncoding"] = encoding;
    }
    Ok(payload)
}

pub(super) fn request(state: Arc<AppState>, device_id: String, connection_id: Uuid, frame: Value) {
    tokio::spawn(request_with_timeout(
        state,
        device_id,
        connection_id,
        frame,
        Duration::from_secs(30),
    ));
}

async fn request_with_timeout(
    state: Arc<AppState>,
    device_id: String,
    connection_id: Uuid,
    frame: Value,
    response_timeout: Duration,
) {
    let Some(request_id) = frame["requestId"].as_str() else {
        return;
    };
    let result = async {
        let target = frame["targetDeviceId"].as_str().unwrap_or("");
        let caller = {
            let conn = state.store.conn.lock().await;
            let caller = caller(&conn, &device_id, connection_id)?;
            if target == device_id {
                return Err(Failure::forbidden());
            }
            if !conn
                .query_row(
                    "SELECT 1 FROM relay_devices WHERE id=?1 AND owner_user_id=?2",
                    params![target, caller.user_id],
                    |_| Ok(()),
                )
                .optional()
                .ok()
                .flatten()
                .is_some()
            {
                return Err(Failure {
                    status: StatusCode::NOT_FOUND,
                    code: "device_not_found",
                    message: "device was not found",
                });
            }
            caller
        };
        let payload = request_payload(&frame["payload"])?;
        let path = payload["path"].as_str().unwrap_or_default();
        if caller.shared_control
            && !shared_source_allowed(
                payload["method"].as_str().unwrap_or_default(),
                path.split('?').next().unwrap_or(path),
            )
        {
            return Err(Failure::source_shared());
        }
        forward_device_payload_with_timeout(
            state.clone(),
            target.to_string(),
            payload,
            Some(caller),
            response_timeout,
        )
        .await
        .map_err(Failure::from)
    }
    .await;
    let payload = result.unwrap_or_else(Failure::payload);
    send(
        &state,
        &device_id,
        connection_id,
        json!({"type": "peer.response", "requestId": request_id, "payload": payload}),
    )
    .await;
}

pub(super) async fn directory(
    state: &AppState,
    device_id: &str,
    connection_id: Uuid,
    frame: Value,
) {
    let Some(request_id) = frame["requestId"].as_str() else {
        return;
    };
    let devices = {
        let conn = state.store.conn.lock().await;
        (|| {
            let caller = caller(&conn, device_id, connection_id)?;
            let mut stmt = conn
                .prepare("SELECT id,name FROM relay_devices WHERE owner_user_id=?1 ORDER BY created_at,id")
                .map_err(|_| Failure::forbidden())?;
            let rows = stmt
                .query_map(params![caller.user_id], |row| {
                    let id = row.get::<_, String>(0)?;
                    let shared = device_shared_for_control(&conn, &id);
                    Ok((id, row.get::<_, String>(1)?, shared))
                })
                .map_err(|_| Failure::forbidden())?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| Failure::forbidden())
        })()
    };
    let response = match devices {
        Ok(devices) => {
            let sockets = state.sockets.read().await;
            let devices: Vec<Value> = devices
                .into_iter()
                .map(|(id, name, shared_control)| {
                    json!({
                        "deviceId": id,
                        "name": name,
                        "online": sockets.contains_key(&id),
                        "self": id == device_id,
                        "sharedControl": shared_control
                    })
                })
                .collect();
            json!({"type": "peer.directory.result", "requestId": request_id, "devices": devices})
        }
        Err(error) => {
            json!({"type": "peer.response", "requestId": request_id, "payload": error.payload()})
        }
    };
    send(state, device_id, connection_id, response).await;
}

async fn send(state: &AppState, device_id: &str, connection_id: Uuid, frame: Value) {
    let sockets = state.sockets.read().await;
    if let Some(socket) = sockets
        .get(device_id)
        .filter(|socket| socket.connection_id == connection_id)
    {
        let _ = socket.tx.try_send(frame.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::{
        connect_async_with_config,
        tungstenite::{protocol::WebSocketConfig, Message as ClientMessage},
        MaybeTlsStream, WebSocketStream,
    };

    type Tunnel = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

    struct Fixture {
        state: Arc<AppState>,
        data_dir: PathBuf,
        server: Option<tokio::task::JoinHandle<()>>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(server) = &self.server {
                server.abort();
            }
            let _ = std::fs::remove_dir_all(&self.data_dir);
        }
    }

    impl Fixture {
        async fn new(name: &str) -> Self {
            let (state, data_dir) = crate::tests::test_app_state(name);
            {
                let conn = state.store.conn.lock().await;
                for owner in ["owner", "other"] {
                    conn.execute(
                        "INSERT INTO relay_users
                         (id,email,username,role,enabled,created_at,password_salt,password_hash)
                         VALUES (?1,?2,?1,'user',1,'now','salt','hash')",
                        params![owner, format!("{owner}@example.test")],
                    )
                    .unwrap();
                }
                for (id, owner, name) in [
                    ("a", "owner", "Device A"),
                    ("b", "owner", "Device B"),
                    ("c", "owner", "Device C"),
                    ("offline", "owner", "Offline"),
                    ("foreign", "other", "Foreign"),
                ] {
                    conn.execute(
                        "INSERT INTO relay_devices
                         (id,owner_user_id,name,token_hash,token_preview,created_at)
                         VALUES (?1,?2,?3,?4,'test','now')",
                        params![id, owner, name, hash_device_token(&format!("token-{id}"))],
                    )
                    .unwrap();
                }
            }
            Self {
                state,
                data_dir,
                server: None,
            }
        }

        async fn socket(&self, id: &str) -> (Uuid, tokio::sync::mpsc::Receiver<String>) {
            let connection_id = Uuid::new_v4();
            let (tx, rx) = tokio::sync::mpsc::channel(64);
            self.state.sockets.write().await.insert(
                id.to_string(),
                DeviceSocket {
                    tx,
                    connection_id,
                    connected_at: now_rfc3339(),
                    last_heartbeat_at: now_rfc3339(),
                },
            );
            (connection_id, rx)
        }

        async fn serve(&mut self) -> SocketAddr {
            let app = Router::new()
                .route("/supervisor/tunnel", get(supervisor_tunnel))
                .with_state(self.state.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            self.server = Some(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            }));
            address
        }

        async fn complete(&self, frame: &Value, payload: Value) {
            self.state
                .pending
                .lock()
                .unwrap()
                .remove(frame["requestId"].as_str().unwrap())
                .unwrap()
                .tx
                .send(payload)
                .unwrap();
        }

        fn request(
            &self,
            source: &str,
            connection_id: Uuid,
            frame: Value,
            timeout: Duration,
        ) -> tokio::task::JoinHandle<()> {
            tokio::spawn(request_with_timeout(
                self.state.clone(),
                source.to_string(),
                connection_id,
                frame,
                timeout,
            ))
        }
    }

    fn payload() -> Value {
        json!({
            "method": "POST", "path": "/api/peer/cli",
            "headers": {"content-type": "application/octet-stream", "x-rcd-key": "key"},
            "body": "c2VhbGVk", "bodyEncoding": "base64"
        })
    }

    fn frame(request_id: &str, target: &str) -> Value {
        json!({
            "type": "peer.request", "requestId": request_id,
            "targetDeviceId": target, "payload": payload()
        })
    }

    fn assert_error(frame: &Value, status: u16, code: &str) {
        assert_eq!(frame["type"], "peer.response");
        assert_eq!(frame["payload"]["statusCode"], status);
        assert!(frame["payload"]["headers"].get("x-rcd-encrypted").is_none());
        let body: Value = serde_json::from_str(frame["payload"]["body"].as_str().unwrap()).unwrap();
        assert_eq!(body["code"], code);
        assert!(body["message"].is_string());
    }

    async fn receive(rx: &mut tokio::sync::mpsc::Receiver<String>) -> Value {
        let text = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        serde_json::from_str(&text).unwrap()
    }

    async fn connect(address: SocketAddr, id: &str) -> Tunnel {
        let mut config = WebSocketConfig::default();
        config.max_message_size = Some(128 * 1024 * 1024);
        config.max_frame_size = Some(128 * 1024 * 1024);
        let (mut tunnel, _) = connect_async_with_config(
            format!("ws://{address}/supervisor/tunnel?deviceToken=token-{id}"),
            Some(config),
            false,
        )
        .await
        .unwrap();
        let greeting = read(&mut tunnel).await;
        assert_eq!(greeting["type"], "relay.connected");
        assert_eq!(greeting["deviceId"], id);
        assert_eq!(
            greeting["deviceName"],
            format!("Device {}", id.to_uppercase())
        );
        let revocations = read(&mut tunnel).await;
        assert_eq!(revocations["type"], "relay.revocations");
        tunnel
    }

    async fn write(tunnel: &mut Tunnel, frame: Value) {
        tunnel
            .send(ClientMessage::Text(frame.to_string().into()))
            .await
            .unwrap();
    }

    async fn read(tunnel: &mut Tunnel) -> Value {
        let message = tokio::time::timeout(Duration::from_secs(10), tunnel.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    }

    #[test]
    fn peer_payload_enforces_prefix_methods_encryption_and_body_limit() {
        for path in [
            "/api/cli",
            "/api/peer",
            "/api/peer?x=/api/peer/cli",
            "/api/peer-other/cli",
            "https://relay/api/peer/cli",
            "/api/peer/../cli",
            "/api/peer/./cli",
            "/api/peer//cli",
            "/api/peer/%2e%2e/cli",
            "/api/peer/..\\cli",
        ] {
            let mut input = payload();
            input["path"] = json!(path);
            assert_eq!(
                request_payload(&input).unwrap_err().code,
                "peer_forbidden",
                "{path}"
            );
        }
        for method in ["DELETE", "PATCH", "HEAD", "get"] {
            let mut input = payload();
            input["method"] = json!(method);
            assert_eq!(
                request_payload(&input).unwrap_err().status,
                StatusCode::FORBIDDEN
            );
        }
        for method in ["GET", "POST", "PUT"] {
            let mut input = payload();
            input["method"] = json!(method);
            assert!(request_payload(&input).is_ok());
        }
        let mut input = payload();
        input["headers"] = json!({});
        assert!(request_payload(&input).is_err());
        input["method"] = json!("GET");
        input["path"] = json!("/api/peer/transport/key?challenge=test");
        assert!(request_payload(&input).is_ok());
        input["method"] = json!("POST");
        assert!(request_payload(&input).is_err());
        input["method"] = json!("GET");
        input["path"] = json!("/api/peer/transport/key/extra");
        assert!(request_payload(&input).is_err());
        let mut input = payload();
        input["headers"]["x-rcd-key"] = json!("");
        assert!(request_payload(&input).is_err());
        let mut input = payload();
        input["body"] = json!("é".repeat(MAX_BODY / 2));
        assert!(request_payload(&input).is_ok());
        input["body"] = json!("a".repeat(MAX_BODY + 1));
        let error = request_payload(&input).unwrap_err();
        assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(error.code, "payload_too_large");
        input["body"] = json!({"plaintext": true});
        assert!(request_payload(&input).is_err());
    }

    #[tokio::test]
    async fn peer_tunnels_route_replies_filter_headers_and_scope_directory() {
        let mut fixture = Fixture::new("peer-routing").await;
        let address = fixture.serve().await;
        let mut a = connect(address, "a").await;
        let mut b = connect(address, "b").await;
        let mut c = connect(address, "c").await;
        write(&mut a, json!({"type":"peer.directory", "requestId":"directory", "deviceId":"foreign", "userId":"other"})).await;
        let directory = read(&mut a).await;
        assert_eq!(directory["type"], "peer.directory.result");
        assert_eq!(directory["requestId"], "directory");
        assert_eq!(
            directory["devices"],
            json!([
                {"deviceId":"a", "name":"Device A", "online":true, "self":true, "sharedControl":false},
                {"deviceId":"b", "name":"Device B", "online":true, "self":false, "sharedControl":false},
                {"deviceId":"c", "name":"Device C", "online":true, "self":false, "sharedControl":false},
                {"deviceId":"offline", "name":"Offline", "online":false, "self":false, "sharedControl":false}
            ])
        );
        let mut first = frame("same-id", "b");
        first["deviceId"] = json!("foreign");
        first["peer"] = json!({"deviceId":"foreign", "userId":"other"});
        first["payload"]["peer"] = first["peer"].clone();
        first["payload"]["headers"] = json!({
            "Content-Type":"application/octet-stream", "Accept":"application/json",
            "X-RCD-Key":"key", "x-rcd-request":"a.0", "x-rcd-enc":"enc", "x-rcd-sealed":"1",
            "x-rcd-resource":"threads/forged",
            "X-RCD-Peer-Device":"a", "x-rcd-peer-signature":"peer-signature",
            "X-RCD-Session":"forged-session", "x-rcd-signature":"forged-signature",
            "authorization":"forged", "cookie":"forged", "x-peer-device-id":"foreign"
        });
        write(&mut a, first).await;
        write(&mut c, frame("same-id", "b")).await;
        let first = read(&mut b).await;
        let second = read(&mut b).await;
        let requests: HashMap<String, Value> = [first, second]
            .into_iter()
            .map(|request| {
                (
                    request["peer"]["deviceId"].as_str().unwrap().to_string(),
                    request,
                )
            })
            .collect();
        assert_eq!(requests.len(), 2);
        let forwarded = &requests["a"];
        assert_eq!(forwarded["type"], "relay.request");
        assert_eq!(forwarded["deviceId"], "b");
        assert_ne!(forwarded["requestId"], "same-id");
        assert_ne!(forwarded["requestId"], requests["c"]["requestId"]);
        assert_eq!(
            forwarded["peer"],
            json!({"deviceId":"a", "deviceName":"Device A", "userId":"owner", "sharedControl":false})
        );
        assert_eq!(
            forwarded["payload"]["headers"],
            json!({
                "content-type":"application/octet-stream", "accept":"application/json",
                "x-rcd-key":"key", "x-rcd-request":"a.0", "x-rcd-enc":"enc", "x-rcd-sealed":"1",
                "x-rcd-peer-device":"a", "x-rcd-peer-signature":"peer-signature"
            })
        );
        assert_eq!(forwarded["payload"]["bodyEncoding"], "base64");
        assert_eq!(forwarded["payload"]["body"], "c2VhbGVk");
        assert!(forwarded["payload"].get("peer").is_none());
        write(&mut a, json!({"type":"relay.response", "requestId":forwarded["requestId"], "payload":{"statusCode":200, "body":"forged"}})).await;
        a.send(ClientMessage::Ping(Vec::new().into()))
            .await
            .unwrap();
        assert!(matches!(
            a.next().await.unwrap().unwrap(),
            ClientMessage::Pong(_)
        ));
        assert_eq!(fixture.state.pending.lock().unwrap().len(), 2);
        for (source, tunnel) in [("c", &mut c), ("a", &mut a)] {
            let payload = json!({"statusCode":206, "headers":{"x-rcd-encrypted":"1", "x-test-source":source}, "body":format!("sealed-{source}"), "bodyEncoding":"base64"});
            write(&mut b, json!({"type":"relay.response", "requestId":requests[source]["requestId"], "payload":payload})).await;
            assert_eq!(
                read(tunnel).await,
                json!({"type":"peer.response", "requestId":"same-id", "payload":payload})
            );
        }
        assert!(fixture.state.pending.lock().unwrap().is_empty());
        let b_connection = fixture
            .state
            .sockets
            .read()
            .await
            .get("b")
            .unwrap()
            .connection_id;
        b.close(None).await.unwrap();
        remove_supervisor_connection(&fixture.state, "b", b_connection).await;
        write(
            &mut a,
            json!({"type":"peer.directory", "requestId":"after-disconnect"}),
        )
        .await;
        let directory = read(&mut a).await;
        assert_eq!(directory["devices"][1]["online"], false);
    }

    #[tokio::test]
    async fn peer_authorization_hides_other_owners_and_checks_enabled_owner() {
        let fixture = Fixture::new("peer-authorization").await;
        let (connection, mut rx) = fixture.socket("a").await;
        for (target, status, code) in [
            ("foreign", 404, "device_not_found"),
            ("missing", 404, "device_not_found"),
            ("offline", 503, "device_offline"),
            ("a", 403, "peer_forbidden"),
        ] {
            request_with_timeout(
                fixture.state.clone(),
                "a".into(),
                connection,
                frame(target, target),
                Duration::from_secs(1),
            )
            .await;
            let response = receive(&mut rx).await;
            assert_eq!(response["requestId"], target);
            assert_error(&response, status, code);
            assert!(fixture.state.pending.lock().unwrap().is_empty());
        }
        fixture
            .state
            .store
            .conn
            .lock()
            .await
            .execute("UPDATE relay_users SET enabled=0 WHERE id='owner'", [])
            .unwrap();
        request_with_timeout(
            fixture.state.clone(),
            "a".into(),
            connection,
            frame("disabled", "b"),
            Duration::from_secs(1),
        )
        .await;
        assert_error(&receive(&mut rx).await, 403, "peer_forbidden");
        directory(
            &fixture.state,
            "a",
            connection,
            json!({"requestId":"disabled-directory"}),
        )
        .await;
        assert_error(&receive(&mut rx).await, 403, "peer_forbidden");
    }

    fn share(conn: &Connection, id: &str, device: &str, target: &str, access: &str) {
        conn.execute(
            "INSERT INTO relay_shares
             (id,owner_user_id,target_user_id,device_id,thread_id,thread_access,created_at)
             VALUES (?1,'owner',?2,?3,'thread',?4,'now')",
            params![id, target, device, access],
        )
        .unwrap();
    }

    fn grant(conn: &Connection, id: &str, scope: &str, access: &str, create: bool) {
        conn.execute(
            "INSERT INTO relay_access_grants
             (id,owner_user_id,target_user_id,device_id,scope,thread_id,workspace_id,
              thread_access,workspace_access,can_create_threads,created_at)
             VALUES (?1,'owner','other','a',?2,'thread','workspace',?3,'write',?4,'now')",
            params![id, scope, access, create],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn device_shared_for_control_counts_only_live_controlling_access() {
        let fixture = Fixture::new("peer-shared-control").await;
        let conn = fixture.state.store.conn.lock().await;
        let shared = |conn: &Connection| device_shared_for_control(conn, "a");
        assert!(!shared(&conn));
        // Read-only access, including workspace write, does not drive agents.
        share(&conn, "read-share", "a", "other", "read");
        for scope in ["thread", "workspace", "device"] {
            grant(&conn, &format!("read-{scope}"), scope, "read", false);
        }
        assert!(!shared(&conn));
        // A share to the owner and other devices' shares are irrelevant.
        share(&conn, "own-share", "a", "owner", "control");
        share(&conn, "other-device", "b", "other", "control");
        assert!(!shared(&conn));
        assert!(!device_shared_for_control(&conn, "c"));
        assert!(device_shared_for_control(&conn, "b"));
        share(&conn, "control-share", "a", "other", "control");
        assert!(shared(&conn));
        for (column, value) in [
            ("revoked_at", "'now'"),
            ("expires_at", "'2000-01-01T00:00:00Z'"),
        ] {
            conn.execute(
                "UPDATE relay_shares SET revoked_at=NULL,expires_at=NULL WHERE id='control-share'",
                [],
            )
            .unwrap();
            assert!(shared(&conn));
            conn.execute(
                &format!("UPDATE relay_shares SET {column}={value} WHERE id='control-share'"),
                [],
            )
            .unwrap();
            assert!(!shared(&conn), "{column}");
        }
        conn.execute(
            "UPDATE relay_shares SET expires_at='2999-01-01T00:00:00Z',revoked_at=NULL WHERE id='control-share'",
            [],
        )
        .unwrap();
        assert!(shared(&conn));
        conn.execute("DELETE FROM relay_shares WHERE id='control-share'", [])
            .unwrap();
        for scope in ["thread", "workspace", "device"] {
            grant(&conn, "control", scope, "control", false);
            assert!(shared(&conn), "{scope} control");
            conn.execute("DELETE FROM relay_access_grants WHERE id='control'", [])
                .unwrap();
        }
        grant(&conn, "creator", "device", "read", true);
        assert!(shared(&conn));
        conn.execute(
            "UPDATE relay_access_grants SET expires_at='2000-01-01T00:00:00Z' WHERE id='creator'",
            [],
        )
        .unwrap();
        assert!(!shared(&conn));
        conn.execute(
            "UPDATE relay_access_grants SET expires_at=NULL,revoked_at='now' WHERE id='creator'",
            [],
        )
        .unwrap();
        assert!(!shared(&conn));
    }

    #[tokio::test]
    async fn peer_requests_from_a_device_shared_for_control_are_refused() {
        let fixture = Fixture::new("peer-shared-source").await;
        share(
            &*fixture.state.store.conn.lock().await,
            "share",
            "a",
            "other",
            "control",
        );
        let (a, mut a_rx) = fixture.socket("a").await;
        let (b, mut b_rx) = fixture.socket("b").await;
        let (_, mut c_rx) = fixture.socket("c").await;
        for path in [
            "/api/peer/cli",
            "/api/peer/files/read?workspaceId=w&path=x",
            "/api/peer/files/uploads",
            "/api/peer/transport/stream/x?chunk=1",
            "/api/peer/connections/id/export/extra",
            "/api/peer/connections/id/other",
        ] {
            let mut request = frame(path, "b");
            request["payload"]["path"] = json!(path);
            request_with_timeout(
                fixture.state.clone(),
                "a".into(),
                a,
                request,
                Duration::from_secs(1),
            )
            .await;
            let response = receive(&mut a_rx).await;
            assert_error(&response, 403, "peer_source_shared");
            let body: Value =
                serde_json::from_str(response["payload"]["body"].as_str().unwrap()).unwrap();
            assert!(body["message"]
                .as_str()
                .unwrap()
                .contains("can control its agents"));
            assert!(b_rx.try_recv().is_err(), "{path}");
        }
        // Requests that cannot act still pass, and carry the flag to the target.
        for (method, path) in [
            ("GET", "/api/peer/transport/key?challenge=test"),
            ("POST", "/api/peer/transport/certificate"),
            ("POST", "/api/peer/connections/id/export"),
        ] {
            let mut request = frame(path, "b");
            request["payload"]["method"] = json!(method);
            request["payload"]["path"] = json!(path);
            let task = fixture.request("a", a, request, Duration::from_secs(10));
            let forwarded = receive(&mut b_rx).await;
            assert_eq!(forwarded["peer"]["sharedControl"], true, "{path}");
            fixture
                .complete(&forwarded, json!({"statusCode":200}))
                .await;
            task.await.unwrap();
            assert_eq!(receive(&mut a_rx).await["payload"]["statusCode"], 200);
        }
        // The rule only covers the shared device as a source.
        let task = fixture.request("b", b, frame("to-shared", "a"), Duration::from_secs(10));
        let forwarded = receive(&mut a_rx).await;
        assert_eq!(forwarded["type"], "relay.request");
        assert_eq!(forwarded["peer"]["sharedControl"], false);
        fixture
            .complete(&forwarded, json!({"statusCode":200}))
            .await;
        task.await.unwrap();
        assert_eq!(receive(&mut b_rx).await["requestId"], "to-shared");
        directory(&fixture.state, "b", b, json!({"requestId":"list"})).await;
        let listed = receive(&mut b_rx).await;
        let flags: HashMap<String, bool> = listed["devices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d["deviceId"].as_str().unwrap().to_string(),
                    d["sharedControl"].as_bool().unwrap(),
                )
            })
            .collect();
        assert!(flags["a"]);
        assert!(!flags["b"] && !flags["c"]);
        let devices = crate::list_user_devices(&fixture.state, "owner").await;
        let listed_a = devices.iter().find(|d| d["id"] == "a").unwrap();
        assert_eq!(listed_a["sharedControl"], true);
        assert!(devices
            .iter()
            .filter(|d| d["id"] != "a")
            .all(|d| d["sharedControl"] == false));
        // Revoking control restores access.
        fixture
            .state
            .store
            .conn
            .lock()
            .await
            .execute("UPDATE relay_shares SET thread_access='read'", [])
            .unwrap();
        let task = fixture.request("a", a, frame("restored", "b"), Duration::from_secs(10));
        let forwarded = receive(&mut b_rx).await;
        assert_eq!(forwarded["peer"]["sharedControl"], false);
        fixture
            .complete(&forwarded, json!({"statusCode":200}))
            .await;
        task.await.unwrap();
        assert_eq!(receive(&mut a_rx).await["requestId"], "restored");
        assert!(c_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn peer_timeout_and_target_disconnect_release_pending() {
        let fixture = Fixture::new("peer-timeout").await;
        let (source, mut source_rx) = fixture.socket("a").await;
        let (target, mut target_rx) = fixture.socket("b").await;
        let timeout = fixture.request(
            "a",
            source,
            frame("timeout", "b"),
            Duration::from_millis(20),
        );
        receive(&mut target_rx).await;
        timeout.await.unwrap();
        assert_error(&receive(&mut source_rx).await, 504, "timeout");
        assert!(fixture.state.pending.lock().unwrap().is_empty());
        let disconnected = fixture.request(
            "a",
            source,
            frame("disconnected", "b"),
            Duration::from_secs(10),
        );
        receive(&mut target_rx).await;
        remove_supervisor_connection(&fixture.state, "b", target).await;
        disconnected.await.unwrap();
        assert_error(&receive(&mut source_rx).await, 503, "device_offline");
        assert!(fixture.state.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn peer_responses_are_discarded_after_source_disconnect_or_replacement() {
        let fixture = Fixture::new("peer-source-disconnect").await;
        let (_, mut target_rx) = fixture.socket("b").await;
        for replace in [false, true] {
            let (source, mut old_rx) = fixture.socket("a").await;
            let request = fixture.request("a", source, frame("old", "b"), Duration::from_secs(10));
            let forwarded = receive(&mut target_rx).await;
            remove_supervisor_connection(&fixture.state, "a", source).await;
            let mut new_rx = if replace {
                Some(fixture.socket("a").await.1)
            } else {
                None
            };
            fixture
                .complete(&forwarded, json!({"statusCode":200, "body":"late"}))
                .await;
            request.await.unwrap();
            assert!(old_rx.try_recv().is_err());
            if let Some(rx) = &mut new_rx {
                assert!(rx.try_recv().is_err());
            }
            assert!(fixture.state.pending.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn peer_in_flight_limit_is_per_source_device() {
        let fixture = Fixture::new("peer-concurrency").await;
        let (a, mut a_rx) = fixture.socket("a").await;
        let (b, mut b_rx) = fixture.socket("b").await;
        let (c, mut c_rx) = fixture.socket("c").await;
        let mut requests = Vec::new();
        for index in 0..MAX_IN_FLIGHT {
            requests.push(fixture.request(
                "a",
                a,
                frame(&format!("a-{index}"), "b"),
                Duration::from_secs(10),
            ));
        }
        for _ in 0..MAX_IN_FLIGHT {
            receive(&mut b_rx).await;
        }
        assert_eq!(fixture.state.pending.lock().unwrap().len(), MAX_IN_FLIGHT);
        request_with_timeout(
            fixture.state.clone(),
            "a".into(),
            a,
            frame("busy", "b"),
            Duration::from_millis(20),
        )
        .await;
        assert_error(&receive(&mut a_rx).await, 429, "busy");
        let other = fixture.request("c", c, frame("other", "b"), Duration::from_secs(10));
        let forwarded = receive(&mut b_rx).await;
        assert_eq!(forwarded["peer"]["deviceId"], "c");
        fixture
            .complete(&forwarded, json!({"statusCode":200}))
            .await;
        other.await.unwrap();
        assert_eq!(receive(&mut c_rx).await["requestId"], "other");
        remove_supervisor_connection(&fixture.state, "b", b).await;
        for request in requests {
            request.await.unwrap();
        }
        for _ in 0..MAX_IN_FLIGHT {
            assert_error(&receive(&mut a_rx).await, 503, "device_offline");
        }
        assert!(fixture.state.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn peer_global_pending_limit_and_full_target_queue_are_bounded() {
        let fixture = Fixture::new("peer-global-limit").await;
        let (a, mut a_rx) = fixture.socket("a").await;
        let (b, _b_rx) = fixture.socket("b").await;
        let mut receivers = Vec::new();
        {
            let mut pending = fixture.state.pending.lock().unwrap();
            for index in 0..256 {
                let (tx, rx) = tokio::sync::oneshot::channel();
                receivers.push(rx);
                pending.insert(
                    index.to_string(),
                    PendingDeviceRequest {
                        device_id: "b".into(),
                        connection_id: b,
                        peer_device_id: None,
                        tx,
                    },
                );
            }
        }
        request_with_timeout(
            fixture.state.clone(),
            "a".into(),
            a,
            frame("global-busy", "b"),
            Duration::from_millis(20),
        )
        .await;
        assert_error(&receive(&mut a_rx).await, 429, "busy");
        assert_eq!(fixture.state.pending.lock().unwrap().len(), 256);
        fixture.state.pending.lock().unwrap().clear();
        {
            let sockets = fixture.state.sockets.read().await;
            for _ in 0..64 {
                sockets["b"].tx.try_send("occupied".into()).unwrap();
            }
        }
        request_with_timeout(
            fixture.state.clone(),
            "a".into(),
            a,
            frame("full", "b"),
            Duration::from_millis(20),
        )
        .await;
        // Connected but behind is busy, not offline.
        assert_error(&receive(&mut a_rx).await, 429, "busy");
        assert!(fixture.state.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn peer_shared_forwarder_preserves_browser_behavior_without_peer_identity() {
        let fixture = Fixture::new("peer-browser-compatibility").await;
        let (_, mut rx) = fixture.socket("b").await;
        let state = fixture.state.clone();
        let browser = tokio::spawn(async move {
            forward_device_with_timeout(
                state,
                "b".into(),
                "POST".into(),
                "/api/threads".into(),
                Some("AP8=".into()),
                Some("base64".into()),
                json!({"x-rcd-resource":"threads/browser"}),
                Duration::from_secs(10),
            )
            .await
        });
        let request = receive(&mut rx).await;
        assert!(request.get("peer").is_none());
        assert_eq!(
            request["payload"]["headers"],
            json!({"x-rcd-resource":"threads/browser"})
        );
        assert_eq!(request["payload"]["bodyEncoding"], "base64");
        fixture.complete(&request, json!({
            "statusCode":201, "body":"AP8=", "bodyEncoding":"base64",
            "headers":{"content-type":"application/octet-stream", "x-rcd-encrypted":"1", "set-cookie":"filtered"}
        })).await;
        let response = browser.await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response.headers()["content-type"],
            "application/octet-stream"
        );
        assert_eq!(response.headers()["x-rcd-encrypted"], "1");
        assert!(response.headers().get("set-cookie").is_none());
        assert_eq!(
            to_bytes(response.into_body(), 10).await.unwrap().as_ref(),
            &[0, 255]
        );
        let offline = forward_device_with_timeout(
            fixture.state.clone(),
            "offline".into(),
            "GET".into(),
            "/healthz".into(),
            None,
            None,
            json!({}),
            Duration::from_millis(20),
        )
        .await;
        assert_eq!(offline.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: Value =
            serde_json::from_slice(&to_bytes(offline.into_body(), 1024).await.unwrap()).unwrap();
        assert_eq!(body["code"], "service_unavailable");
    }

    #[tokio::test]
    async fn peer_tunnel_accepts_frames_above_16_mib_and_keeps_request_body_limit() {
        let mut fixture = Fixture::new("peer-large-frame").await;
        let address = fixture.serve().await;
        let mut a = connect(address, "a").await;
        let mut b = connect(address, "b").await;
        let mut oversized = frame("oversized", "b");
        oversized["payload"]["body"] = json!("a".repeat(20 * 1024 * 1024));
        write(&mut a, oversized).await;
        assert_error(&read(&mut a).await, 413, "payload_too_large");
        let mut handshake = frame("key", "b");
        handshake["payload"] = json!({"method":"GET", "path":"/api/peer/transport/key?challenge=test", "headers":{}, "body":null});
        write(&mut a, handshake).await;
        let forwarded = read(&mut b).await;
        let large = "a".repeat(20 * 1024 * 1024);
        write(&mut b, json!({"type":"relay.response", "requestId":forwarded["requestId"], "payload":{"statusCode":200, "headers":{}, "body":large, "bodyEncoding":"base64"}})).await;
        let response = read(&mut a).await;
        assert_eq!(response["requestId"], "key");
        assert_eq!(response["payload"]["statusCode"], 200);
        assert_eq!(
            response["payload"]["body"].as_str().unwrap().len(),
            large.len()
        );
        assert!(fixture.state.pending.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn upstream_catalog_tunnel_scopes_owners_and_keeps_offline_metadata() {
        let mut fixture = Fixture::new("upstream-catalog-tunnel").await;
        let address = fixture.serve().await;
        let mut a = connect(address, "a").await;
        let mut b = connect(address, "b").await;
        let (foreign_id, mut foreign_rx) = fixture.socket("foreign").await;
        let id = Uuid::new_v4().to_string();
        let revision = Uuid::new_v4().to_string();
        let metadata =
            json!({"id":id,"name":"API connection","protocol":"gemini","revision":revision});
        write(
            &mut a,
            json!({"type":"relay.heartbeat","upstreamConnections":[metadata.clone()]}),
        )
        .await;
        // Directory on the same socket is ordered after its metadata update.
        write(
            &mut a,
            json!({"type":"upstream.directory","requestId":"own"}),
        )
        .await;
        let own = read(&mut a).await;
        assert_eq!(own["type"], "upstream.directory.result");
        assert_eq!(own["connections"][0]["id"], id);
        write(
            &mut b,
            json!({"type":"upstream.directory","requestId":"B","ownerUserId":"other"}),
        )
        .await;
        let shared = read(&mut b).await;
        assert_eq!(shared["connections"][0]["deviceId"], "a");
        assert_eq!(shared["connections"][0]["online"], true);
        assert!(shared["connections"][0].get("baseUrl").is_none());
        crate::upstream_catalog::directory(
            &fixture.state,
            "foreign",
            foreign_id,
            json!({"type":"upstream.directory","requestId":"foreign","ownerUserId":"owner"}),
        )
        .await;
        let foreign: Value = serde_json::from_str(&foreign_rx.recv().await.unwrap()).unwrap();
        assert_eq!(foreign["connections"], json!([]));
        let mut invalid = metadata.clone();
        invalid["apiKey"] = json!("must-not-be-persisted");
        write(
            &mut a,
            json!({"type":"relay.heartbeat","upstreamConnections":[invalid]}),
        )
        .await;
        write(
            &mut a,
            json!({"type":"upstream.directory","requestId":"after-rejected"}),
        )
        .await;
        assert_eq!(read(&mut a).await["connections"][0]["revision"], revision);
        a.close(None).await.unwrap();
        // Model an established offline state without timing-sensitive polling.
        fixture.state.sockets.write().await.remove("a");
        write(
            &mut b,
            json!({"type":"upstream.directory","requestId":"offline"}),
        )
        .await;
        let offline = read(&mut b).await;
        assert_eq!(offline["connections"][0]["online"], false);
        assert_eq!(offline["connections"][0]["id"], id);
        let c = fixture.state.store.conn.lock().await;
        let stored: String = c
            .query_row(
                "SELECT name FROM relay_upstream_catalog WHERE device_id='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, "API connection");
    }
}
