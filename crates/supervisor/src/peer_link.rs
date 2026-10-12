//! Requests from this device to other devices of the same relay owner. Each one
//! travels through this device's relay tunnel and, except for the key handshake,
//! is end-to-end encrypted to the target. Contract: docs/cross-device-peer.zh.md.
use crate::{bounded_channel, secure_transport::client::Client};
use anyhow::{anyhow, Result};
use pockymoe_runtime::Supervisor;
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::sync::oneshot;
use uuid::Uuid;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(35);
const MAX_PENDING: usize = 32;

/// This device as the relay knows it, learned from `relay.connected`.
#[derive(Clone, Debug)]
pub(crate) struct RelayIdentity {
    pub device_id: String,
    pub device_name: String,
    pub port_preview_base_url: Option<String>,
    pub public_base_url: Option<String>,
}

/// A decrypted reply from another device's `/api/peer/...` handler.
#[derive(Debug)]
pub(crate) struct PeerResponse {
    pub status: u16,
    pub headers: Map<String, Value>,
    pub body: Vec<u8>,
}

/// Failures before the target's handler answered. Only `retryable()` ones may be
/// kept in the outbox; everything else is reported to the caller.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PeerError {
    #[error("relay tunnel is not connected")]
    RelayUnavailable,
    #[error("device {0} is offline")]
    Offline(String),
    #[error("device {0} did not respond in time")]
    Timeout(String),
    #[error("device identity changed for {0}; check `pockymoe relay-fingerprint` on that device, then run `pockymoe device trust {0} --reset`")]
    IdentityChanged(String),
    #[error("peer_target_unverified: cannot verify device {0}; pair the target device with the same passkey")]
    TargetUnverified(String),
    #[error("{code}: {message}")]
    Remote {
        status: u16,
        code: String,
        message: String,
    },
}

impl PeerError {
    pub(crate) fn retryable(&self) -> bool {
        matches!(
            self,
            Self::RelayUnavailable | Self::Offline(_) | Self::Timeout(_)
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ReplyKind {
    Request,
    Directory,
}
struct Pending {
    kind: ReplyKind,
    reply: oneshot::Sender<Result<Value, PeerError>>,
}
struct Connection {
    id: Uuid,
    outgoing: bounded_channel::Sender<Value>,
}
#[derive(Default)]
struct LinkState {
    identity: Option<RelayIdentity>,
    connection: Option<Connection>,
    pending: HashMap<String, Pending>,
}
#[derive(Default)]
struct Link {
    state: Mutex<LinkState>,
    client: Client,
}
static LINKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Link>>>> = OnceLock::new();
fn link(state: &Supervisor) -> Arc<Link> {
    LINKS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap()
        .entry(state.config.database_url.clone())
        .or_default()
        .clone()
}
fn fail_pending(state: &mut LinkState) {
    for (_, pending) in state.pending.drain() {
        let _ = pending.reply.send(Err(PeerError::RelayUnavailable));
    }
}

/// The guard also fails waiters when the tunnel task is cancelled.
pub(crate) struct TunnelConnection {
    link: Arc<Link>,
    id: Uuid,
    outgoing: bounded_channel::Sender<Value>,
}
impl TunnelConnection {
    pub(crate) fn new(state: &Supervisor, outgoing: bounded_channel::Sender<Value>) -> Self {
        Self {
            link: link(state),
            id: Uuid::new_v4(),
            outgoing,
        }
    }
    pub(crate) fn connected(&self, identity: Option<RelayIdentity>) {
        let mut state = self.link.state.lock().unwrap();
        if state.connection.as_ref().is_none_or(|c| c.id != self.id) {
            fail_pending(&mut state);
        }
        if let Some(identity) = identity {
            state.identity = Some(identity);
        }
        state.connection = Some(Connection {
            id: self.id,
            outgoing: self.outgoing.clone(),
        });
    }
    pub(crate) fn receive(&self, message: &Value) {
        let (kind, value) = match message["type"].as_str() {
            Some("peer.response") => (ReplyKind::Request, &message["payload"]),
            Some("peer.directory.result") => (ReplyKind::Directory, &message["devices"]),
            _ => return,
        };
        let Some(id) = message["requestId"].as_str() else {
            return;
        };
        let mut state = self.link.state.lock().unwrap();
        if state.connection.as_ref().is_none_or(|c| c.id != self.id)
            || state.pending.get(id).is_none_or(|p| {
                p.kind != kind
                    && !(p.kind == ReplyKind::Directory
                        && kind == ReplyKind::Request
                        && value["statusCode"]
                            .as_u64()
                            .is_some_and(|status| status >= 400))
            })
        {
            return;
        }
        if let Some(pending) = state.pending.remove(id) {
            let _ = pending.reply.send(Ok(value.clone()));
        }
    }
}
impl Drop for TunnelConnection {
    fn drop(&mut self) {
        let mut state = self.link.state.lock().unwrap();
        if state.connection.as_ref().is_some_and(|c| c.id == self.id) {
            state.connection = None;
            fail_pending(&mut state);
        }
    }
}
struct PendingGuard {
    link: Arc<Link>,
    id: String,
}
impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.link.state.lock().unwrap().pending.remove(&self.id);
    }
}
fn require_access(state: &Supervisor) -> Result<()> {
    if !state.peer_access_enabled() {
        return Err(PeerError::Remote {
            status: 403,
            code: "peer_access_disabled".into(),
            message: "Peer access is disabled on this device.".into(),
        }
        .into());
    }
    Ok(())
}
async fn exchange(
    state: &Supervisor,
    mut frame: Value,
    kind: ReplyKind,
    target: &str,
) -> Result<Value> {
    // The directory only asks the relay, so it stays available before opting in.
    if matches!(kind, ReplyKind::Request) {
        require_access(state)?;
    }
    let link = link(state);
    let id = Uuid::new_v4().to_string();
    frame["requestId"] = json!(id);
    let (reply, receiver) = oneshot::channel();
    let _guard = PendingGuard {
        link: link.clone(),
        id: id.clone(),
    };
    {
        let mut state = link.state.lock().unwrap();
        let connection = state
            .connection
            .as_ref()
            .ok_or(PeerError::RelayUnavailable)?;
        if state.pending.len() >= MAX_PENDING {
            return Err(PeerError::Remote {
                status: 429,
                code: "busy".into(),
                message: "Too many peer requests are in flight.".into(),
            }
            .into());
        }
        let outgoing = connection.outgoing.clone();
        state.pending.insert(id, Pending { kind, reply });
        if outgoing.send(frame).is_err() {
            state.connection = None;
            fail_pending(&mut state);
            return Err(PeerError::RelayUnavailable.into());
        }
    }
    tokio::time::timeout(REQUEST_TIMEOUT, receiver)
        .await
        .map_err(|_| PeerError::Timeout(target.into()))?
        .map_err(|_| PeerError::RelayUnavailable)?
        .map_err(Into::into)
}

/// None until the tunnel has connected once in this process.
pub(crate) fn relay_identity(state: &Supervisor) -> Option<RelayIdentity> {
    link(state).state.lock().unwrap().identity.clone()
}

/// Devices owned by this device's relay owner, including this one (`self: true`):
/// `[{deviceId, name, online, self}]`.
pub(crate) async fn directory(state: &Supervisor) -> Result<Vec<Value>> {
    let response = exchange(
        state,
        json!({"type":"peer.directory"}),
        ReplyKind::Directory,
        "directory",
    )
    .await?;
    if response.is_object() {
        return Err(crate::secure_transport::client::relay_error("directory", &response)?.into());
    }
    response
        .as_array()
        .cloned()
        .ok_or_else(|| anyhow!("invalid peer directory response"))
}
pub(crate) async fn relay_request(
    state: &Supervisor,
    device_id: &str,
    payload: Value,
) -> Result<Value> {
    exchange(
        state,
        json!({"type":"peer.request","targetDeviceId":device_id,"payload":payload}),
        ReplyKind::Request,
        device_id,
    )
    .await
}

/// One encrypted request to `/api/peer/...` on `device_id`. Ok carries any status
/// the target's handler returned; Err is a `PeerError` (or a local failure).
pub(crate) async fn request(
    state: &Supervisor,
    device_id: &str,
    method: &str,
    path_and_query: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> Result<PeerResponse> {
    require_access(state)?;
    link(state)
        .client
        .request(
            state,
            device_id,
            method,
            path_and_query,
            content_type,
            &body,
        )
        .await
}

/// POST JSON and decode the JSON reply. A non-2xx reply becomes `PeerError::Remote`
/// with the target's `{code, message}`.
pub(crate) async fn request_json(
    state: &Supervisor,
    device_id: &str,
    path: &str,
    body: &Value,
) -> Result<Value> {
    let response = request(
        state,
        device_id,
        "POST",
        path,
        Some("application/json"),
        serde_json::to_vec(body)?,
    )
    .await?;
    if !(200..300).contains(&response.status) {
        let value: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
        return Err(PeerError::Remote {
            status: response.status,
            code: value["code"].as_str().unwrap_or("peer_error").into(),
            message: value["message"].as_str().unwrap_or("request failed").into(),
        }
        .into());
    }
    Ok(serde_json::from_slice(&response.body)?)
}

/// Forget a device's pinned identity after the user verified its new fingerprint.
pub(crate) fn reset_pin(state: &Supervisor, device_id: &str) -> Result<()> {
    link(state).client.reset_pin(state, device_id)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::tunnel::tests::state_with_relay_url;

    pub(crate) fn connected(
        state: &Supervisor,
        device_id: &str,
    ) -> (TunnelConnection, bounded_channel::Receiver<Value>) {
        let (outgoing, outbound) = bounded_channel::channel();
        let connection = TunnelConnection::new(state, outgoing);
        connection.connected(Some(RelayIdentity {
            device_id: device_id.into(),
            device_name: format!("Device {device_id}"),
            port_preview_base_url: None,
            public_base_url: None,
        }));
        (connection, outbound)
    }
    async fn frame(outbound: &mut bounded_channel::Receiver<Value>) -> Value {
        tokio::time::timeout(Duration::from_secs(2), outbound.recv())
            .await
            .unwrap()
            .unwrap()
    }
    #[tokio::test]
    async fn peer_pending_correlates_kind_and_id_and_fails_on_disconnect() {
        let (_dir, state) = state_with_relay_url("http://127.0.0.1:1");
        let (_other_dir, other) = state_with_relay_url("http://127.0.0.1:1");
        state.set_peer_access(true).unwrap();
        other.set_peer_access(true).unwrap();
        let (connection, mut outbound) = connected(&state, "local");
        assert!(relay_identity(&other).is_none());
        assert_eq!(relay_identity(&state).unwrap().device_name, "Device local");
        assert!(matches!(
            directory(&other).await.unwrap_err().downcast_ref(),
            Some(PeerError::RelayUnavailable)
        ));
        let first = tokio::spawn({
            let state = state.clone();
            async move { relay_request(&state, "remote", json!({"path":"first"})).await }
        });
        let first_frame = frame(&mut outbound).await;
        let second = tokio::spawn({
            let state = state.clone();
            async move { directory(&state).await }
        });
        let second_frame = frame(&mut outbound).await;
        assert_eq!(first_frame["type"], "peer.request");
        assert_eq!(first_frame["targetDeviceId"], "remote");
        assert_eq!(second_frame["type"], "peer.directory");
        connection.receive(&json!({"type":"peer.directory.result","requestId":first_frame["requestId"],"devices":[]}));
        connection.receive(&json!({"type":"peer.response","requestId":"unknown","payload":{}}));
        assert_eq!(link(&state).state.lock().unwrap().pending.len(), 2);
        let devices =
            json!([{ "deviceId":"local", "name":"Device local", "online":true, "self":true }]);
        connection.receive(&json!({"type":"peer.directory.result","requestId":second_frame["requestId"],"devices":devices}));
        assert_eq!(
            second.await.unwrap().unwrap(),
            devices.as_array().unwrap().clone()
        );
        assert!(!first.is_finished());
        connection.receive(&json!({"type":"peer.response","requestId":first_frame["requestId"],"payload":{"statusCode":204}}));
        assert_eq!(first.await.unwrap().unwrap()["statusCode"], 204);
        let pending = tokio::spawn({
            let state = state.clone();
            async move { directory(&state).await }
        });
        frame(&mut outbound).await;
        drop(connection);
        assert!(matches!(
            pending.await.unwrap().unwrap_err().downcast_ref(),
            Some(PeerError::RelayUnavailable)
        ));
        assert_eq!(relay_identity(&state).unwrap().device_id, "local");
        assert!(link(&state).state.lock().unwrap().pending.is_empty());
    }
    #[tokio::test]
    async fn peer_pending_cancellation_limit_and_reconnect_are_isolated() {
        let (_dir, state) = state_with_relay_url("http://127.0.0.1:1");
        state.set_peer_access(true).unwrap();
        let (old, mut outbound) = connected(&state, "old");
        let cancelled = tokio::spawn({
            let state = state.clone();
            async move { directory(&state).await }
        });
        frame(&mut outbound).await;
        cancelled.abort();
        let _ = cancelled.await;
        assert!(link(&state).state.lock().unwrap().pending.is_empty());
        let mut tasks = Vec::new();
        for _ in 0..MAX_PENDING {
            tasks.push(tokio::spawn({
                let state = state.clone();
                async move { directory(&state).await }
            }));
            frame(&mut outbound).await;
        }
        assert!(
            matches!(directory(&state).await.unwrap_err().downcast_ref(), Some(PeerError::Remote { status:429, code, .. }) if code == "busy")
        );
        let (replacement, mut outbound) = connected(&state, "new");
        for task in tasks {
            assert!(matches!(
                task.await.unwrap().unwrap_err().downcast_ref(),
                Some(PeerError::RelayUnavailable)
            ));
        }
        let current = tokio::spawn({
            let state = state.clone();
            async move { directory(&state).await }
        });
        let frame = frame(&mut outbound).await;
        old.receive(&json!({"type":"peer.directory.result","requestId":frame["requestId"],"devices":["stale"]}));
        drop(old);
        assert_eq!(link(&state).state.lock().unwrap().pending.len(), 1);
        replacement.receive(
            &json!({"type":"peer.directory.result","requestId":frame["requestId"],"devices":[]}),
        );
        assert!(current.await.unwrap().unwrap().is_empty());
    }
    #[tokio::test]
    async fn peer_disabled_requests_do_not_enter_the_tunnel() {
        let (_dir, state) = state_with_relay_url("http://127.0.0.1:1");
        let (_connection, mut outbound) = connected(&state, "local");
        let error = request(&state, "remote", "POST", "/api/peer/cli", None, vec![])
            .await
            .unwrap_err();
        assert!(
            matches!(error.downcast_ref(), Some(PeerError::Remote { status:403, code, .. }) if code == "peer_access_disabled")
        );
        assert!(link(&state).state.lock().unwrap().pending.is_empty());
        assert!(
            tokio::time::timeout(Duration::from_millis(10), outbound.recv())
                .await
                .is_err()
        );
        // The directory only asks the relay, so it works before opting in.
        let listing = tokio::spawn({
            let state = state.clone();
            async move { directory(&state).await }
        });
        assert_eq!(outbound.recv().await.unwrap()["type"], "peer.directory");
        listing.abort();
    }
    #[tokio::test]
    async fn peer_directory_correlates_relay_error_responses() {
        let (_dir, state) = state_with_relay_url("http://127.0.0.1:1");
        state.set_peer_access(true).unwrap();
        let (connection, mut outbound) = connected(&state, "local");
        let pending = tokio::spawn({
            let state = state.clone();
            async move { directory(&state).await }
        });
        let request = frame(&mut outbound).await;
        connection.receive(&json!({"type":"peer.response","requestId":request["requestId"],"payload":{"statusCode":403,"headers":{"content-type":"application/json"},"body":r#"{"code":"peer_forbidden","message":"Owner access is disabled"}"#}}));
        assert!(
            matches!(pending.await.unwrap().unwrap_err().downcast_ref(), Some(PeerError::Remote { status:403, code, message }) if code == "peer_forbidden" && message == "Owner access is disabled")
        );
        assert!(link(&state).state.lock().unwrap().pending.is_empty());
    }
    #[test]
    fn peer_retryable_errors_are_only_connection_offline_and_timeout() {
        assert!(PeerError::RelayUnavailable.retryable());
        assert!(PeerError::Offline("remote".into()).retryable());
        assert!(PeerError::Timeout("remote".into()).retryable());
        assert!(!PeerError::IdentityChanged("remote".into()).retryable());
        assert!(!PeerError::Remote {
            status: 503,
            code: "peer_error".into(),
            message: "failed".into()
        }
        .retryable());
    }
}
