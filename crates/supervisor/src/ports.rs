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
const LOCAL_KEY: &str = "deviceLocalPortMappings";
/// Origin comes from server-side extensions, never a forwarded HTTP header.
#[derive(Clone, Copy)]
pub(crate) enum MappingAuthority<'a> {
    Local,
    Tunnel(Option<&'a crate::owner_trust::BrowserPrincipal>),
}
impl<'a> MappingAuthority<'a> {
    pub(crate) fn for_request(
        forwarded: Option<&crate::auth::TrustedRelayForward>,
        owner: Option<&'a crate::owner_trust::BrowserPrincipal>,
    ) -> Self {
        if forwarded.is_some() {
            Self::Tunnel(owner)
        } else {
            Self::Local
        }
    }
}
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
    state
        .db
        .with(|c| {
            let local = local_mappings(c)?;
            let mut allowed = Vec::new();
            for mapping in mappings {
                if local.contains(&mapping.id)
                    || c.query_row(
                        "SELECT EXISTS(SELECT 1 FROM owner_port_mappings m
                         JOIN browser_registrations b USING(browser_id) JOIN device_owner_trust t ON t.account_key_id=b.account_key_id WHERE mapping_id=?1 AND b.role='owner')",
                        [&mapping.id],
                        |r| r.get::<_, bool>(0),
                    )?
                {
                    allowed.push(mapping);
                }
            }
            Ok(allowed)
        })
        .unwrap_or_default()
}

fn local_mappings(conn: &rusqlite::Connection) -> Result<Vec<String>> {
    let saved: String = conn.query_row(
        "SELECT COALESCE((SELECT value FROM kv WHERE key=?1),'[]')",
        [LOCAL_KEY],
        |r| r.get(0),
    )?;
    Ok(serde_json::from_str(&saved)?)
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
    owner: Option<Extension<crate::owner_trust::BrowserPrincipal>>,
    forwarded: Option<Extension<crate::auth::TrustedRelayForward>>,
    Json(input): Json<Create>,
) -> Response {
    match create_authorized_mapping(
        &state,
        input.port,
        &input.label,
        MappingAuthority::for_request(
            forwarded.as_ref().map(|f| &f.0),
            owner.as_ref().map(|p| &p.0),
        ),
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

/// Called by the authenticated local CLI, which the tunnel cannot access.
pub(crate) fn create_local_mapping(state: &Supervisor, port: u16, label: &str) -> Result<Mapping> {
    create_authorized_mapping(state, port, label, MappingAuthority::Local)
}
fn create_authorized_mapping(
    state: &Supervisor,
    port: u16,
    label: &str,
    authority: MappingAuthority<'_>,
) -> Result<Mapping> {
    if port == 0 || port == state.config.port || label.len() > 120 {
        bail!("Choose an HTTP service port from 1–65535, excluding the Supervisor port. Labels may contain up to 120 bytes.");
    }
    let mapping = state.db.with(|conn| {
        // Serialize provenance and mapping creation with local binding/reset writes.
        let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        let strict: bool = tx.query_row("SELECT strict FROM device_owner_trust WHERE singleton=1", [], |r| r.get(0))?;
        let valid_owner = match authority {
            MappingAuthority::Tunnel(Some(p)) => tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM browser_registrations b JOIN device_owner_trust t ON t.account_key_id=b.account_key_id WHERE browser_id=?1 AND b.account_key_id=?2 AND role='owner')",
                rusqlite::params![p.browser_id, p.account_key_id],
                |r| r.get::<_,bool>(0),
            )?,
            _ => false,
        };
        let requires_owner = match authority {
            MappingAuthority::Local => false,
            MappingAuthority::Tunnel(owner) => strict || owner.is_some(),
        };
        if requires_owner && !valid_owner {
            bail!("An owner browser is required to create a port mapping");
        }
        let saved: String = tx.query_row(
            "SELECT COALESCE((SELECT value FROM kv WHERE key=?1),'[]')", [KEY], |r| r.get(0),
        )?;
        let mut mappings: Vec<Mapping> = serde_json::from_str(&saved)?;
        let mut local = local_mappings(&tx)?;
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
        if matches!(authority, MappingAuthority::Local) {
            if !local.contains(&mapping.id) { local.push(mapping.id.clone()); }
            tx.execute("DELETE FROM owner_port_mappings WHERE mapping_id=?1", [&mapping.id])?;
            tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [LOCAL_KEY, &serde_json::to_string(&local)?])?;
        } else if let MappingAuthority::Tunnel(Some(p)) = authority {
            // An owner request reusing a locally approved port cannot demote its provenance.
            if !local.contains(&mapping.id) {
                tx.execute(
                    "INSERT INTO owner_port_mappings(mapping_id,browser_id) VALUES(?1,?2)
                     ON CONFLICT(mapping_id) DO UPDATE SET browser_id=excluded.browser_id",
                    [&mapping.id, &p.browser_id],
                )?;
            }
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

pub(crate) fn remove_browser_mappings(c: &rusqlite::Connection, browser: &str) -> Result<()> {
    let ids = c
        .prepare("SELECT mapping_id FROM owner_port_mappings WHERE browser_id=?1")?
        .query_map([browser], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let saved: String = c.query_row(
        "SELECT COALESCE((SELECT value FROM kv WHERE key=?1),'[]')",
        [KEY],
        |r| r.get(0),
    )?;
    let mut mappings: Vec<Mapping> = serde_json::from_str(&saved)?;
    mappings.retain(|m| !ids.contains(&m.id));
    c.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [KEY, &serde_json::to_string(&mappings)?])?;
    c.execute(
        "DELETE FROM owner_port_mappings WHERE browser_id=?1",
        [browser],
    )?;
    Ok(())
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
    authority: MappingAuthority<'_>,
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
    create_authorized_mapping(state, port, &format!("{DSH_CONSOLE}{short}"), authority)
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
        let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        let saved: String = tx.query_row("SELECT COALESCE((SELECT value FROM kv WHERE key=?1),'[]')", [KEY], |r| r.get(0))?;
        let mut mappings: Vec<Mapping> = serde_json::from_str(&saved)?;
        mappings.retain(|m| m.id != id);
        tx.execute("DELETE FROM owner_port_mappings WHERE mapping_id=?1", [id])?;
        let mut local = local_mappings(&tx)?;
        local.retain(|local_id| local_id != id);
        tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [LOCAL_KEY, &serde_json::to_string(&local)?])?;
        tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [KEY, &serde_json::to_string(&mappings)?])?;
        tx.commit()?;
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
    use crate::owner_trust::tests::{decoded, encrypted, live_owner};
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn strict_local_http_mappings_survive_credential_removal_and_reset_but_tunnel_requires_owner(
    ) {
        let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
        let owner = live_owner(&state);
        let router = crate::http::router(state.clone());
        let input = json!({"port":3003,"label":"Local service"});
        let request = |token: Option<&str>| {
            let mut builder = Request::builder()
                .method("POST")
                .uri("/api/port-mappings")
                .header("content-type", "application/json");
            if let Some(token) = token {
                builder = builder.header("authorization", format!("Bearer {token}"));
            }
            builder.body(Body::from(input.to_string())).unwrap()
        };
        assert_eq!(
            router
                .clone()
                .oneshot(request(None))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let token = crate::auth::login(&state.config, "admin", "secret123")
            .unwrap()
            .0;
        let response = router.oneshot(request(Some(&token))).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let local: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        let local_id = local["id"].as_str().unwrap();
        let plain = crate::tunnel::forward_local(
            &state,
            json!({"method":"POST","path":"/api/port-mappings","body":input.to_string()}),
            None,
        )
        .await;
        assert_eq!(plain["statusCode"], 403);
        assert_eq!(
            serde_json::from_str::<Value>(plain["body"].as_str().unwrap()).unwrap()["code"],
            "device_strict_mode"
        );
        let (r, key, aad) = encrypted(
            &state,
            "POST",
            "/api/port-mappings",
            json!({"port":3004}),
            None,
        );
        let (status, body) = decoded(
            crate::tunnel::forward_local(&state, r, None).await,
            &key,
            &aad,
        );
        assert_eq!(status, 401);
        assert_eq!(body["code"], "browser_registration_required");
        assert_eq!(snapshot(&state).len(), 1);

        // Signed tunnel requests can reuse local approvals, but cannot demote them.
        let (r, key, aad) = encrypted(&state, "POST", "/api/port-mappings", input, Some(&owner));
        let (status, reused) = decoded(
            crate::tunnel::forward_local(&state, r, None).await,
            &key,
            &aad,
        );
        assert_eq!(status, 200);
        assert_eq!(reused["id"], local_id);
        let (r, key, aad) = encrypted(
            &state,
            "POST",
            "/api/port-mappings",
            json!({"port":3004}),
            Some(&owner),
        );
        assert_eq!(
            decoded(
                crate::tunnel::forward_local(&state, r, None).await,
                &key,
                &aad
            )
            .0,
            200
        );
        assert_eq!(snapshot(&state).len(), 2);

        crate::owner_trust::apply_revocations(
            &state,
            &[crate::owner_trust::tests::browser_revocation(&owner)],
        )
        .unwrap();
        assert!(crate::owner_trust::strict(&state));
        assert_eq!(snapshot(&state).len(), 1);
        assert_eq!(snapshot(&state)[0].id, local_id);
        crate::owner_trust::reset_account_trust(&state.config.database_url).unwrap();
        assert_eq!(snapshot(&state).len(), 1);
        assert_eq!(snapshot(&state)[0].id, local_id);
        let persisted = rusqlite::Connection::open_with_flags(
            &state.config.database_url,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let local_provenance: String = persisted
            .query_row("SELECT value FROM kv WHERE key=?1", [LOCAL_KEY], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&local_provenance).unwrap(),
            vec![local_id]
        );
        remove_mapping(&state, local_id).unwrap();
        assert!(snapshot(&state).is_empty());
        assert!(state.db.with(local_mappings).unwrap().is_empty());
    }

    #[tokio::test]
    async fn strict_local_dsh_and_reapproved_owner_mappings_keep_local_provenance() {
        let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
        let owner = live_owner(&state);
        let remote = create_authorized_mapping(
            &state,
            3005,
            "Remote service",
            MappingAuthority::Tunnel(Some(&owner)),
        )
        .unwrap();
        let reapproved = create_local_mapping(&state, 3005, "Local service").unwrap();
        assert_eq!(remote.id, reapproved.id);
        let console = dsh_console_mapping(&state, "local-thread", 3006, MappingAuthority::Local)
            .await
            .unwrap();
        crate::owner_trust::apply_revocations(
            &state,
            &[crate::owner_trust::tests::browser_revocation(&owner)],
        )
        .unwrap();
        let ids: Vec<String> = snapshot(&state).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![remote.id, console.id]);
        crate::owner_trust::reset_account_trust(&state.config.database_url).unwrap();
        assert_eq!(
            snapshot(&state)
                .into_iter()
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[tokio::test]
    async fn mappings_require_owner_and_are_removed_on_browser_revocation_or_reset() {
        let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
        let legacy =
            create_authorized_mapping(&state, 3001, "legacy", MappingAuthority::Tunnel(None))
                .unwrap();
        assert!(snapshot(&state).iter().any(|m| m.id == legacy.id));
        let owner = crate::owner_trust::tests::live_owner(&state);
        assert!(snapshot(&state).is_empty());
        assert!(create_authorized_mapping(
            &state,
            3002,
            "tunnel unsigned",
            MappingAuthority::Tunnel(None)
        )
        .is_err());
        let mapping = create_authorized_mapping(
            &state,
            3002,
            "owner",
            MappingAuthority::Tunnel(Some(&owner)),
        )
        .unwrap();
        assert_eq!(snapshot(&state).len(), 1);
        assert_eq!(snapshot(&state)[0].id, mapping.id);
        crate::owner_trust::apply_revocations(
            &state,
            &[crate::owner_trust::tests::browser_revocation(&owner)],
        )
        .unwrap();
        assert!(snapshot(&state).is_empty());
        crate::owner_trust::reset_account_trust(&state.config.database_url).unwrap();
        assert!(!snapshot(&state).iter().any(|m| m.id == mapping.id));
        assert!(snapshot(&state).iter().any(|m| m.id == legacy.id));
    }
}
