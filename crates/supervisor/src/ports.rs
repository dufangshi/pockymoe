//! Explicitly approved, device-local HTTP ports. Traffic uses separate outbound
//! WebSockets, so page downloads never occupy the chat/control message queue.
use anyhow::{bail, Result};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use futures_util::{SinkExt, StreamExt};
use pockymoe_runtime::Supervisor;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Mapping {
    pub id: String,
    pub port: u16,
    pub label: String,
    pub created_at: String,
}

const KEY: &str = "devicePortMappings";
type Active = Mutex<HashMap<String, Vec<tokio::task::AbortHandle>>>;
fn active() -> &'static Active {
    static ACTIVE: OnceLock<Active> = OnceLock::new();
    ACTIVE.get_or_init(Default::default)
}

pub(crate) fn snapshot(state: &Supervisor) -> Vec<Mapping> {
    let mappings: Vec<Mapping> = state
        .db
        .get_kv(KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if !crate::owner_trust::strict(state) {
        return mappings;
    }
    state.db.with(|c| {
        Ok(mappings.into_iter().filter(|m| c.query_row("SELECT EXISTS(SELECT 1 FROM owner_port_mappings m JOIN owner_credentials c USING(credential_id) WHERE mapping_id=?1)", [&m.id], |r| r.get::<_,bool>(0)).unwrap_or(false)).collect())
    }).unwrap_or_default()
}

pub(crate) async fn list(State(state): State<Arc<Supervisor>>) -> Json<Value> {
    Json(json!({"mappings": snapshot(&state)}))
}

#[derive(Deserialize)]
pub(crate) struct Create {
    port: u16,
    #[serde(default)]
    label: String,
}

pub(crate) async fn create(
    State(state): State<Arc<Supervisor>>,
    owner: Option<Extension<crate::owner_trust::OwnerPrincipal>>,
    Json(input): Json<Create>,
) -> Response {
    match create_authorized_mapping(
        &state,
        input.port,
        &input.label,
        owner.as_ref().map(|p| &p.0),
    ) {
        Ok(mapping) => Json(json!(mapping)).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"message":error.to_string()})),
        )
            .into_response(),
    }
}

fn changed(state: &Supervisor) {
    state.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
        event_type: "device.ports.changed".into(),
        thread_id: String::new(),
        timestamp: pockymoe_protocol::now_rfc3339(),
        payload: json!({"reason":"port_mappings_changed"}),
    });
}

pub(crate) fn create_mapping(state: &Supervisor, port: u16, label: &str) -> Result<Mapping> {
    create_authorized_mapping(state, port, label, None)
}
fn create_authorized_mapping(
    state: &Supervisor,
    port: u16,
    label: &str,
    owner: Option<&crate::owner_trust::OwnerPrincipal>,
) -> Result<Mapping> {
    if port == 0 || port == state.config.port || label.len() > 120 {
        bail!("Choose an HTTP service port from 1–65535, excluding the Supervisor port. Labels may contain up to 120 bytes.");
    }
    let mapping = state.db.with(|conn| {
        // Serialize provenance and mapping creation with local pairing/reset writes.
        let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        let strict: bool = tx.query_row("SELECT strict FROM device_owner_trust WHERE singleton=1", [], |r| r.get(0))?;
        let valid_owner = match owner {
            Some(p) => tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM owner_sessions s JOIN owner_credentials c
                 USING(credential_id) WHERE session_id=?1 AND credential_id=?2 AND expires_at>?3)",
                rusqlite::params![p.session_id, p.credential_id, crate::owner_trust::now()],
                |r| r.get::<_,bool>(0),
            )?,
            None => false,
        };
        if (strict || owner.is_some()) && !valid_owner {
            bail!("An owner session is required to create a port mapping");
        }
        let saved: String = tx.query_row(
            "SELECT COALESCE((SELECT value FROM kv WHERE key=?1),'[]')", [KEY], |r| r.get(0),
        )?;
        let mut mappings: Vec<Mapping> = serde_json::from_str(&saved)?;
        let mapping = if let Some(existing) = mappings.iter().find(|m| m.port == port) {
            existing.clone()
        } else {
            if mappings.len() >= 32 { bail!("At most 32 port mappings can be enabled on a device"); }
            let mapping = Mapping {
                id: Uuid::new_v4().simple().to_string(), port,
                label: label.trim().to_string(), created_at: pockymoe_protocol::now_rfc3339(),
            };
            mappings.push(mapping.clone());
            mapping
        };
        if let Some(p) = owner {
            tx.execute(
                "INSERT INTO owner_port_mappings(mapping_id,credential_id) VALUES(?1,?2)
                 ON CONFLICT(mapping_id) DO UPDATE SET credential_id=excluded.credential_id",
                [&mapping.id, &p.credential_id],
            )?;
        }
        tx.execute(
            "INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [KEY, &serde_json::to_string(&mappings)?],
        )?;
        tx.commit()?;
        Ok(mapping)
    })?;
    changed(state);
    Ok(mapping)
}

pub(crate) fn close_unauthorized(state: &Supervisor) {
    let allowed = snapshot(state);
    active().lock().unwrap().retain(|id, handles| {
        if allowed.iter().any(|m| &m.id == id) {
            true
        } else {
            for handle in handles {
                handle.abort();
            }
            false
        }
    });
}

pub(crate) fn refresh_owner_permissions(state: &Supervisor) {
    close_unauthorized(state);
    changed(state);
}

const DSH_CONSOLE: &str = "DSH console ";

/// A thread's native DSH console behind an owner-only preview. Console ports
/// are ephemeral, so mappings of consoles that stopped listening are dropped
/// first and they never pile up against the mapping limit.
pub(crate) async fn dsh_console_mapping(
    state: &Supervisor,
    thread_id: &str,
    port: u16,
    owner: Option<&crate::owner_trust::OwnerPrincipal>,
) -> Result<Mapping> {
    for mapping in snapshot(state) {
        if !mapping.label.starts_with(DSH_CONSOLE) || mapping.port == port {
            continue;
        }
        let listening = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, mapping.port)),
        )
        .await
        .is_ok_and(|connected| connected.is_ok());
        if !listening {
            remove_mapping(state, &mapping.id)?;
        }
    }
    let short: String = thread_id.chars().take(8).collect();
    create_authorized_mapping(state, port, &format!("{DSH_CONSOLE}{short}"), owner)
}

pub(crate) async fn remove(
    State(state): State<Arc<Supervisor>>,
    Path(id): Path<String>,
) -> Response {
    match remove_mapping(&state, &id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub(crate) fn remove_mapping(state: &Supervisor, id: &str) -> Result<()> {
    state.db.with(|conn| {
        let saved: String = conn.query_row("SELECT COALESCE((SELECT value FROM kv WHERE key=?1),'[]')", [KEY], |r| r.get(0))?;
        let mut mappings: Vec<Mapping> = serde_json::from_str(&saved)?;
        mappings.retain(|m| m.id != id);
        conn.execute("DELETE FROM owner_port_mappings WHERE mapping_id=?1", [id])?;
        conn.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [KEY, &serde_json::to_string(&mappings)?])?;
        Ok(())
    })?;
    if let Some(handles) = active().lock().unwrap().remove(id) {
        for handle in handles {
            handle.abort();
        }
    }
    changed(state);
    Ok(())
}

pub(crate) fn open(state: Arc<Supervisor>, message: &Value) {
    let Some(id) = message["mappingId"].as_str() else {
        return;
    };
    let Some(mapping) = snapshot(&state).into_iter().find(|m| m.id == id) else {
        return;
    };
    let Some(ticket) = message["ticket"]
        .as_str()
        .filter(|s| Uuid::parse_str(s).is_ok())
        .map(str::to_owned)
    else {
        return;
    };
    let id = mapping.id.clone();
    let mut handles = active().lock().unwrap();
    if !snapshot(&state)
        .iter()
        .any(|m| m.id == id && m.port == mapping.port)
    {
        return;
    }
    let handles = handles.entry(id).or_default();
    handles.retain(|h| !h.is_finished());
    if handles.len() >= 32 {
        return;
    }
    let task = tokio::spawn(async move {
        if let Err(error) = connect(&state, mapping.port, &ticket).await {
            tracing::debug!(%error, port=mapping.port, "port preview stream ended");
        }
    });
    handles.push(task.abort_handle());
}

async fn connect(state: &Supervisor, port: u16, ticket: &str) -> Result<()> {
    let upstream = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        match tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await {
            Ok(socket) => Ok(socket),
            Err(_) => tokio::net::TcpStream::connect((std::net::Ipv6Addr::LOCALHOST, port)).await,
        }
    })
    .await??;
    let mut url = url::Url::parse(state.config.relay_server_url.as_deref().unwrap_or(""))?;
    let scheme = if matches!(url.scheme(), "https" | "wss") {
        "wss"
    } else {
        "ws"
    };
    url.set_scheme(scheme)
        .map_err(|_| anyhow::anyhow!("Invalid relay URL"))?;
    url.set_path(&format!("/supervisor/port-stream/{ticket}"));
    url.set_query(None);
    let mut request = url.as_str().into_client_request()?;
    request.headers_mut().insert(
        "authorization",
        format!(
            "Bearer {}",
            state.config.relay_agent_token.as_deref().unwrap_or("")
        )
        .parse()?,
    );
    let (socket, _) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio_tungstenite::connect_async(request),
    )
    .await??;
    let (mut sink, mut source) = socket.split();
    let (mut read, mut write) = upstream.into_split();
    {
        let send = async {
            let mut buffer = vec![0; 64 * 1024];
            let mut ping = tokio::time::interval(std::time::Duration::from_secs(20));
            ping.tick().await;
            loop {
                tokio::select! {
                    count = read.read(&mut buffer) => {
                        let count = count?;
                        if count == 0 { sink.close().await?; break; }
                        sink.send(Message::Binary(buffer[..count].to_vec().into())).await?;
                    }
                    _ = ping.tick() => sink.send(Message::Ping(Vec::new().into())).await?,
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
        tokio::pin!(send, receive);
        tokio::select! {
            result=&mut send => {
                result?;
                // Finish the WS close handshake rather than dropping a TCP socket
                // with unread control frames, which can reset a short HTTP reply.
                let _ = tokio::time::timeout(std::time::Duration::from_secs(2), &mut receive).await;
            },
            result=&mut receive => result?,
        }
    }
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), sink.close()).await;
    Ok(())
}

#[cfg(test)]
mod owner_mapping_tests {
    use super::*;
    #[tokio::test]
    async fn mappings_require_owner_and_survive_session_expiry_but_not_credential_removal_or_reset()
    {
        let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
        let legacy = create_mapping(&state, 3001, "legacy").unwrap();
        assert!(snapshot(&state).iter().any(|m| m.id == legacy.id));
        let owner = crate::owner_trust::tests::live_owner(&state);
        assert!(snapshot(&state).is_empty());
        assert!(create_mapping(&state, 3002, "local unsigned").is_err());
        let mapping = create_authorized_mapping(&state, 3002, "owner", Some(&owner)).unwrap();
        assert_eq!(snapshot(&state).len(), 1);
        state
            .db
            .with(|c| {
                c.execute("UPDATE owner_sessions SET expires_at=0", [])?;
                Ok(())
            })
            .unwrap();
        assert_eq!(snapshot(&state)[0].id, mapping.id);
        state
            .db
            .with(|c| {
                c.execute(
                    "DELETE FROM owner_credentials WHERE credential_id=?1",
                    [&owner.credential_id],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(snapshot(&state).is_empty());
        crate::owner_trust::reset_owner_trust(&state.config.database_url).unwrap();
        assert!(!snapshot(&state).iter().any(|m| m.id == mapping.id));
        assert!(snapshot(&state).iter().any(|m| m.id == legacy.id));
    }
}
