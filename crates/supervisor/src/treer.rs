//! Hub wiring for Treer's local attach protocol and AIS. Credentials never enter SQLite.
use axum::{
    body::to_bytes,
    extract::{
        ws::{Message, WebSocket},
        ConnectInfo, Extension, Request, State, WebSocketUpgrade,
    },
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures_util::StreamExt;
use pockymoe_protocol::{
    now_rfc3339, treer::*, CreateThreadInput, CreateWorkspaceInput, ThreadSendInput,
};
use pockymoe_runtime::Supervisor;
use pockymoe_treer::{Delivery, Prompt, Source};
use rand::RngCore;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use subtle::ConstantTimeEq;

pub(crate) struct Hub {
    pub state: Arc<Supervisor>,
    port: u16,
    token: String,
    pub hub_id: String,
    browser_operations: tokio::sync::Mutex<()>,
    registry_operations: tokio::sync::Mutex<()>,
    hub_identity: Mutex<Option<Binding>>,
    live: Mutex<HashMap<String, Binding>>,
    lifecycle: tokio::sync::Mutex<()>,
    creating: Mutex<HashSet<String>>,
    browser_commands: Mutex<browser::Commands>,
    message_executable: Mutex<std::path::PathBuf>,
    message_operations: tokio::sync::Mutex<()>,
    message_workers: Mutex<HashSet<(String, String)>>,
}
#[derive(Clone)]
struct Binding {
    thread: String,
    agent: String,
    instance: String,
    identity: AttachTreerIdentity,
    cancelled: tokio_util::sync::CancellationToken,
}
#[derive(Clone)]
pub(crate) struct Scope {
    pub hub: Arc<Hub>,
    pub thread: String,
    agent: String,
    instance: String,
    controller: bool,
}
impl Scope {
    pub fn valid(&self) -> bool {
        self.hub
            .live
            .lock()
            .unwrap()
            .get(&self.agent)
            .is_some_and(|b| {
                b.instance == self.instance
                    && b.thread == self.thread
                    && !b.cancelled.is_cancelled()
            })
    }
    pub fn allows(&self, id: &str) -> bool {
        self.valid()
            && self.hub.state.get_thread(id).is_ok_and(|t| {
                let thread = &t.id;
                let bound = self
                    .hub
                    .state
                    .db
                    .with(|c| {
                        Ok(c.query_row(
                            "SELECT EXISTS(SELECT 1 FROM treer_bindings WHERE thread_id=?1)",
                            [thread],
                            |r| r.get::<_, bool>(0),
                        )?)
                    })
                    .unwrap_or(false);
                pockymoe_treer::thread_in_scope(
                    if self.controller {
                        Source::Controller
                    } else {
                        Source::ProxyUi
                    },
                    &self.thread,
                    &t.id,
                    t.root_thread_id.as_deref(),
                    bound,
                    self.hub.creating.lock().unwrap().contains(thread),
                )
            })
    }
    pub fn event(&self, value: &Value) -> bool {
        self.valid() && pockymoe_treer::event_allowed(value, |id| self.allows(id))
    }
}
mod app_page;
mod browser;
mod registry;
pub(crate) mod messages;

fn failure(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error":code,"code":code}))).into_response()
}
fn map_failure(error: anyhow::Error) -> Response {
    let text = error.to_string();
    let status = if text.starts_with("conflict:") {
        StatusCode::CONFLICT
    } else if text.contains("not found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::BAD_REQUEST
    };
    // Runtime errors can contain upstream details; AIS exposes only stable codes.
    failure(
        status,
        if status == StatusCode::CONFLICT {
            "conflict"
        } else if status == StatusCode::NOT_FOUND {
            "not_found"
        } else {
            "invalid_request"
        },
    )
}

fn attach_failure(error: &anyhow::Error) -> AttachReply {
    // Only fixed, recognized diagnostics may cross the attach boundary. Runtime
    // errors can otherwise contain credentials, local paths or subprocess output.
    let (code, message) = match error.to_string().as_str() {
        "unsupported_protocol" => ("unsupported_protocol", "Unsupported attach protocol"),
        "codex is not enabled" => ("provider_unavailable", "codex is not enabled on this hub."),
        "claude is not enabled" => ("provider_unavailable", "claude is not enabled on this hub."),
        "opencode is not enabled" => (
            "provider_unavailable",
            "opencode is not enabled on this hub.",
        ),
        "acp is not enabled" => ("provider_unavailable", "acp is not enabled on this hub."),
        "Install the base agent first: codex" => {
            ("base_agent_missing", "Install the base agent first: codex")
        }
        "Install the base agent first: claude" => {
            ("base_agent_missing", "Install the base agent first: claude")
        }
        "Install the base agent first: grok" => {
            ("base_agent_missing", "Install the base agent first: grok")
        }
        "Install the base agent first: opencode" => (
            "base_agent_missing",
            "Install the base agent first: opencode",
        ),
        "thread not found" => (
            "thread_not_found",
            "The requested thread no longer exists on this hub.",
        ),
        "thread_already_live" => (
            "thread_already_live",
            "This thread is already attached to a running Treer Agent.",
        ),
        _ => ("binding_refused", "Attach binding refused"),
    };
    AttachReply::Error {
        code: code.into(),
        message: message.into(),
    }
}
impl Hub {
    pub(crate) fn new(state: Arc<Supervisor>, port: u16) -> anyhow::Result<Option<Arc<Self>>> {
        Self::new_with_identity(state, port, registry::app_identity())
    }
    fn new_with_identity(
        state: Arc<Supervisor>,
        port: u16,
        hub_identity: Option<Binding>,
    ) -> anyhow::Result<Option<Arc<Self>>> {
        if !state.config.treer_enabled {
            return Ok(None);
        }
        anyhow::ensure!(
            state.config.host_mode == pockymoe_runtime::host_mode::HostMode::Hosted,
            "Treer requires hosted mode"
        );
        let dir = state
            .config
            .database_url
            .parent()
            .unwrap_or(std::path::Path::new("."));
        std::fs::create_dir_all(dir)?;
        let path = dir.join(TREER_ATTACH_TOKEN_FILE);
        // Never follow a symlink or accept a world-readable existing token.
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            anyhow::ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "invalid attach token file"
            );
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                use std::io::Write;
                let mut bytes = [0; 32];
                rand::rngs::OsRng.fill_bytes(&mut bytes);
                file.write_all(hex::encode(bytes).as_bytes())?;
                file.sync_all()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            anyhow::ensure!(
                std::fs::metadata(&path)?.permissions().mode() & 0o777 == 0o600,
                "attach token permissions must be 0600"
            );
        }
        let token = std::fs::read_to_string(path)?.trim().to_owned();
        anyhow::ensure!(
            token.len() == 64 && hex::decode(&token).is_ok(),
            "invalid attach token"
        );
        registry::ensure_schema(&state)?;
        state.db.with(|c| Ok(c.execute_batch(messages::SCHEMA)?))?;
        let pending_bindings = state.db.with(|c| {
            let mut stmt = c.prepare("SELECT t.id FROM threads t LEFT JOIN treer_registry r ON r.thread_id=t.id WHERE r.state IS NULL OR r.state!='hidden'")?;
            let ids = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })?;
        for id in pending_bindings {
            state.pause_integration_thread(&id);
        }
        let hub_id = state.db.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO treer_hub_identity(id,hub_id) VALUES(1,?1)",
                [uuid::Uuid::new_v4().to_string()],
            )?;
            Ok(c.query_row(
                "SELECT hub_id FROM treer_hub_identity WHERE id=1",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })?;
        let hub = Arc::new(Self {
            hub_id,
            registry_operations: Default::default(),
            hub_identity: Mutex::new(hub_identity),
            browser_operations: Default::default(),
            state,
            port,
            token,
            live: Default::default(),
            lifecycle: Default::default(),
            creating: Default::default(),
            browser_commands: Default::default(),
            message_executable: Mutex::new("treer".into()),
            message_operations: Default::default(),
            message_workers: Default::default(),
        });
        hub.state
            .set_thread_host(Arc::new(registry::Registry::new(&hub)))?;
        let weak = Arc::downgrade(&hub);
        tokio::spawn(async move {
            loop {
                let Some(hub) = weak.upgrade() else { break };
                hub.reconcile_registry().await;
                drop(hub);
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        });
        hub.state
            .set_host_messaging(Arc::new(messages::Bridge(Arc::downgrade(&hub))));
        let weak = Arc::downgrade(&hub);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let Some(hub) = weak.upgrade() else { break };
                hub.start_message_workers();
                hub.deliver_notifications().await;
            }
        });
        Ok(Some(hub))
    }
    fn token_matches(&self, supplied: &str) -> bool {
        bool::from(self.token.as_bytes().ct_eq(supplied.as_bytes()))
    }
    fn scope(self: &Arc<Self>, request: &Request) -> Option<Scope> {
        // Only TCP loopback may claim tunnel provenance. In-process relay requests
        // have no ConnectInfo and cannot gain access by adding headers.
        if !request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .is_some_and(|p| p.0.ip().is_loopback())
        {
            return None;
        }
        let h = request.headers();
        let one = |key| {
            if h.get_all(key).iter().count() != 1 {
                None
            } else {
                h.get(key).and_then(|v| v.to_str().ok())
            }
        };
        let src = pockymoe_treer::source(
            one("x-treer-interface-source")?,
            one("host")?,
            self.port,
            h.contains_key("origin"),
            h.keys().any(|k| k.as_str().starts_with("sec-fetch-")),
        )?;
        let agent = one("x-treer-agent-id")?;
        let instance = one("x-treer-interface-instance")?;
        let live = self.live.lock().unwrap();
        let b = live.get(agent)?;
        if b.instance != instance || b.cancelled.is_cancelled() {
            return None;
        }
        Some(Scope {
            hub: self.clone(),
            thread: b.thread.clone(),
            agent: agent.into(),
            instance: instance.into(),
            controller: src == Source::Controller,
        })
    }
    async fn bind(&self, hello: AttachHello) -> anyhow::Result<(Binding, bool)> {
        pockymoe_treer::validate_hello(&hello)?;
        let _lock = self.lifecycle.lock().await;
        anyhow::ensure!(
            !self.live.lock().unwrap().contains_key(&hello.agent_id),
            "agent_already_live"
        );
        let saved: Option<String> = self.state.db.with(|c| {
            Ok(c.query_row(
                "SELECT thread_id FROM treer_bindings WHERE agent_id=?1",
                [&hello.agent_id],
                |r| r.get(0),
            )
            .optional()?)
        })?;
        anyhow::ensure!(
            hello
                .thread_id
                .as_ref()
                .zip(saved.as_ref())
                .is_none_or(|(requested, saved)| requested == saved),
            "agent_thread_mismatch"
        );
        let target = hello.thread_id.clone().or(saved);
        let created = target.is_none();
        let thread = if let Some(id) = target {
            self.state.get_thread(&id)?;
            let existing: Option<String> = self.state.db.with(|c| {
                Ok(c.query_row(
                    "SELECT agent_id FROM treer_bindings WHERE thread_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .optional()?)
            })?;
            anyhow::ensure!(
                existing
                    .as_ref()
                    .is_none_or(|agent| agent == &hello.agent_id),
                "agent_thread_mismatch"
            );
            anyhow::ensure!(
                !self.live.lock().unwrap().values().any(|b| b.thread == id),
                "thread_already_live"
            );
            // Existing processes may have an old identity; release before rebinding.
            self.state.pause_integration_thread(&id);
            self.state.release_thread_harness(&id, true, None).await?;
            self.state.set_thread_cli_identity(
                &id,
                pockymoe_treer::identity_env(&hello.agent_id, &hello.treer),
            )?;
            id
        } else {
            let create = hello
                .create
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("create_required"))?;
            let cwd = std::path::PathBuf::from(&create.cwd);
            let canonical = cwd.canonicalize().unwrap_or(cwd);
            let workspace = self
                .state
                .list_workspaces()?
                .into_iter()
                .find(|w| std::path::Path::new(&w.abs_path) == canonical)
                .map(Ok)
                .unwrap_or_else(|| {
                    self.state.create_workspace(CreateWorkspaceInput {
                        abs_path: Some(create.cwd.clone()),
                        label: None,
                        git_url: None,
                    })
                })?;
            self.state
                .create_attached_thread(
                    CreateThreadInput {
                        workspace_id: workspace.id,
                        title: create.title.clone(),
                        provider: Some(serde_json::from_value(json!(create.provider))?),
                        agent_id: create.agent.clone(),
                        model: create.model.clone().unwrap_or_default(),
                        reasoning_effort: create.reasoning_effort.clone(),
                        approval_mode: "yolo".into(),
                        parent_thread_id: None,
                    },
                    pockymoe_treer::identity_env(&hello.agent_id, &hello.treer),
                    hello.agent_id.clone(),
                )
                .await?
                .id
        };
        let now = now_rfc3339();
        let persisted=self.state.db.with(|c| { let tx=c.unchecked_transaction()?;
            anyhow::ensure!(tx.query_row("SELECT agent_id FROM treer_bindings WHERE thread_id=?1", [&thread], |r| r.get::<_,String>(0)).optional()?.is_none_or(|agent| agent == hello.agent_id), "agent_thread_mismatch");
            tx.execute("INSERT INTO treer_bindings(thread_id,agent_id,created_at,last_instance_id,last_seen_at) VALUES(?1,?2,?3,?4,?3) ON CONFLICT(thread_id) DO UPDATE SET agent_id=excluded.agent_id,last_instance_id=excluded.last_instance_id,last_seen_at=excluded.last_seen_at",params![thread,hello.agent_id,now,hello.instance_id])?; tx.commit()?; Ok(()) });
        if let Err(error) = persisted {
            self.state.pause_integration_thread(&thread);
            self.state.clear_thread_cli_identity(&thread);
            let _ = self.state.release_thread_harness(&thread, true, None).await;
            return Err(error);
        }
        let binding = Binding {
            thread: thread.clone(),
            agent: hello.agent_id,
            instance: hello.instance_id,
            identity: hello.treer,
            cancelled: Default::default(),
        };
        self.live
            .lock()
            .unwrap()
            .insert(binding.agent.clone(), binding.clone());
        self.state.resume_integration_thread(&thread);
        self.state.db.with(|c| { c.execute("INSERT INTO treer_registry(thread_id,state) VALUES(?1,'bound') ON CONFLICT(thread_id) DO UPDATE SET state='bound'", [&thread])?; Ok(()) })?;
        Ok((binding, created))
    }
    async fn detach(&self, binding: &Binding) {
        {
            let mut live = self.live.lock().unwrap();
            let Some(current) = live.get_mut(&binding.agent) else {
                return;
            };
            if current.instance != binding.instance {
                return;
            }
            current.cancelled.cancel();
            current.identity = AttachTreerIdentity {
                agent_server_url: None,
                workload_credential: None,
                workspace_id: None,
                server_id: None,
            };
        }
        self.state.pause_integration_thread(&binding.thread);
        self.state.clear_thread_cli_identity(&binding.thread);
        // Retain a cancelled reservation until A1 finishes: a new attach cannot
        // race the old socket cleanup and have its fresh harness released.
        let _lock = self.lifecycle.lock().await;
        let _ = self.state.db.with(|c| {
            c.execute(
                "UPDATE treer_bindings SET last_seen_at=?2 WHERE thread_id=?1",
                params![binding.thread, now_rfc3339()],
            )?;
            Ok(())
        });
        // A1 may find no loaded process even while an in-flight turn still owns
        // a cancellation token (for example after a harness crash).
        if self
            .state
            .get_thread(&binding.thread)
            .is_ok_and(|t| t.active_turn_id.is_some())
        {
            let _ = self.state.interrupt(&binding.thread).await;
        }
        if self
            .state
            .release_thread_harness(&binding.thread, true, None)
            .await
            .is_err()
        {
            let _ = self.state.interrupt(&binding.thread).await;
            if self
                .state
                .release_thread_harness(&binding.thread, true, None)
                .await
                .is_err()
            {
                tracing::warn!(thread_id=%binding.thread,"Treer detach harness release deferred");
            }
        }
        self.live.lock().unwrap().remove(&binding.agent);
    }
}
pub(crate) fn routes() -> Router<Arc<Supervisor>> {
    Router::new()
        .route(TREER_ATTACH_PATH, get(attach))
        .route("/v1/{*path}", axum::routing::any(ais))
}
pub(crate) async fn gate(
    State(hub): State<Option<Arc<Hub>>>,
    mut request: Request,
    next: Next,
) -> Response {
    let marked = request
        .headers()
        .keys()
        .any(|h| h.as_str().starts_with("x-treer-") || h.as_str().starts_with("x_treer_"));
    let Some(hub) = hub else {
        return if request.uri().path() == TREER_ATTACH_PATH
            || request.uri().path().starts_with("/v1/")
        {
            failure(StatusCode::NOT_FOUND, "treer_disabled")
        } else if marked {
            failure(StatusCode::FORBIDDEN, "treer_disabled")
        } else {
            next.run(request).await
        };
    };
    if let Some(response) = app_page::route(&hub, &request) {
        return response;
    }
    if request.uri().path() == TREER_ATTACH_PATH {
        request.extensions_mut().insert(hub);
        return next.run(request).await;
    }
    if !marked {
        return if request.uri().path().starts_with("/v1/") {
            failure(StatusCode::FORBIDDEN, "binding_required")
        } else {
            next.run(request).await
        };
    }
    let Some(scope) = hub.scope(&request) else {
        return failure(StatusCode::FORBIDDEN, "invalid_interface_provenance");
    };
    let path = request.uri().path();
    if scope.controller {
        if !path.starts_with("/v1/") {
            return failure(StatusCode::FORBIDDEN, "controller_route_forbidden");
        }
    } else {
        if path == "/" || path == "/index.html" {
            // Relative Location survives the Treer path prefix.
            return (
                StatusCode::TEMPORARY_REDIRECT,
                [(header::LOCATION, format!("threads/{}", scope.thread))],
            )
                .into_response();
        }
        if path == "/api/auth/session" && request.method() == axum::http::Method::GET {
            return Json(crate::auth::local_session(&hub.state.config)).into_response();
        }
        if request.method() == axum::http::Method::GET
            && matches!(path, "/api/threads" | "/api/workspaces")
        {
            let result = if path == "/api/threads" {
                hub.state.list_threads(None, true).map(|rows| {
                    json!(rows
                        .into_iter()
                        .filter(|t| scope.allows(&t.id))
                        .collect::<Vec<_>>())
                })
            } else {
                hub.state.list_threads(None, true).and_then(|threads| {
                    let ids: HashSet<_> = threads
                        .into_iter()
                        .filter(|t| scope.allows(&t.id))
                        .map(|t| t.workspace_id)
                        .collect();
                    Ok(json!(hub
                        .state
                        .list_workspaces()?
                        .into_iter()
                        .filter(|w| ids.contains(&w.id))
                        .collect::<Vec<_>>()))
                })
            };
            return result
                .map(|v| Json(v).into_response())
                .unwrap_or_else(map_failure);
        }
        let scoped_global = if request.method() == axum::http::Method::GET {
            let workspace = path
                .strip_prefix("/api/workspaces/")
                .and_then(|tail| tail.split('/').next());
            let allowed_workspace = workspace.is_some_and(|id| {
                hub.state.list_threads(None, true).is_ok_and(|threads| {
                    threads
                        .iter()
                        .any(|t| t.workspace_id == id && scope.allows(&t.id))
                })
            });
            (allowed_workspace
                && workspace.is_some_and(|id| {
                    path == format!("/api/workspaces/{id}")
                        || path.starts_with(&format!("/api/workspaces/{id}/files/"))
                }))
                || path
                    .strip_prefix("/api/agent-runtimes/")
                    .is_some_and(|tail| {
                        let Some((provider, action)) = tail.split_once('/') else {
                            return false;
                        };
                        // Creation needs model/capability reads for every enabled harness.
                        hub.state
                            .config
                            .enabled_providers
                            .iter()
                            .any(|p| p.as_str() == provider)
                            && matches!(action, "status" | "models" | "agents" | "capabilities")
                    })
        } else {
            false
        };
        if !scoped_global
            && !pockymoe_treer::gate(request.method().as_str(), path, |id| scope.allows(id))
        {
            return failure(StatusCode::FORBIDDEN, "thread_scope_forbidden");
        }
    }
    request.extensions_mut().insert(scope);
    next.run(request).await
}
async fn attach(
    Extension(hub): Extension<Arc<Hub>>,
    ws: WebSocketUpgrade,
    request: Request,
) -> Response {
    let headers = request.headers();
    if !request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|p| p.0.ip().is_loopback())
        || headers.contains_key("origin")
        || headers.keys().any(|k| k.as_str().starts_with("sec-fetch-"))
    {
        return failure(StatusCode::FORBIDDEN, "loopback_required");
    }
    let supplied = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !hub.token_matches(supplied) {
        return failure(StatusCode::UNAUTHORIZED, "invalid_attach_token");
    }
    ws.max_message_size(128 * 1024)
        .max_frame_size(128 * 1024)
        .on_upgrade(move |socket| attach_loop(hub, socket))
}
async fn attach_loop(hub: Arc<Hub>, mut socket: WebSocket) {
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), socket.next()).await;
    let hello = match first {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<AttachHello>(&text).ok(),
        _ => None,
    };
    let Some(hello) = hello else {
        let _ = socket
            .send(Message::Text(
                json!({"type":"error","code":"invalid_hello","message":"Invalid attach hello"})
                    .to_string()
                    .into(),
            ))
            .await;
        return;
    };
    let (binding, created) = match hub.bind(hello).await {
        Ok(b) => b,
        Err(e) => {
            let reply = attach_failure(&e);
            let _ = socket
                .send(Message::Text(serde_json::to_string(&reply).unwrap().into()))
                .await;
            return;
        }
    };
    let reconcile = hub.clone();
    tokio::spawn(async move {
        reconcile.reconcile_registry().await;
    });
    let reply = AttachReply::Bound {
        thread_id: binding.thread.clone(),
        created,
        interface_port: hub.port,
        ui_path: "/".into(),
        capabilities: pockymoe_treer::CAPABILITIES
            .iter()
            .map(|s| s.to_string())
            .collect(),
    };
    if socket
        .send(Message::Text(serde_json::to_string(&reply).unwrap().into()))
        .await
        .is_ok()
    {
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(15));
        let mut last = tokio::time::Instant::now();
        loop {
            tokio::select! {
                _ = binding.cancelled.cancelled() => break,
                msg=socket.next()=>match msg {None|Some(Err(_))|Some(Ok(Message::Close(_)))=>break,Some(Ok(Message::Ping(v)))=>{last=tokio::time::Instant::now();if socket.send(Message::Pong(v)).await.is_err(){break;}},Some(Ok(_))=>{last=tokio::time::Instant::now();}},
                _=heartbeat.tick()=>{if last.elapsed()>std::time::Duration::from_secs(60){break;} if socket.send(Message::Ping(Vec::new().into())).await.is_err(){break;} let _=hub.state.db.with(|c|{c.execute("UPDATE treer_bindings SET last_seen_at=?2 WHERE thread_id=?1",params![binding.thread,now_rfc3339()])?;Ok(())});}
            }
        }
    }
    hub.detach(&binding).await;
    hub.reconcile_registry().await;
}
async fn ais(Extension(scope): Extension<Scope>, request: Request) -> Response {
    if !scope.valid() {
        return failure(StatusCode::FORBIDDEN, "binding_required");
    }
    let hub = &scope.hub;
    let state = &hub.state;
    let thread = &scope.thread;
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let query = request.uri().query().unwrap_or_default();
    if method == axum::http::Method::GET || method == axum::http::Method::HEAD {
        match path.as_str() {
            "/v1/manifest" => {
                return Json(pockymoe_treer::Manifest {
                    protocol: TREER_AGENT_INTERFACE_PROTOCOL,
                    instance_id: scope.instance.clone(),
                    capabilities: pockymoe_treer::CAPABILITIES.to_vec(),
                    ui_path: "/",
                })
                .into_response()
            }
            "/v1/health" => {
                return Json(json!({"instance_id":scope.instance,"status":"ok"})).into_response()
            }
            "/v1/status" => {
                let queue_length = match hub.pending_count(thread) {
                    Ok(count) => count,
                    Err(e) => return map_failure(e),
                };
                return match state.interaction_status(thread).await {
                    Ok(s) => Json(pockymoe_treer::Status {
                        agent_id: scope.agent.clone(),
                        interface_instance_id: scope.instance.clone(),
                        status: pockymoe_treer::status(
                            s["activeTurnId"].is_string() || s["status"] == "running",
                            s["waitingForInput"] == true,
                        ),
                        busy: s["activeTurnId"].is_string() || s["status"] == "running",
                        queue_length,
                        error: s["lastError"].as_str().map(str::to_owned),
                    })
                    .into_response(),
                    Err(e) => map_failure(e),
                };
            }
            "/v1/transcript" => {
                let q = url::form_urlencoded::parse(query.as_bytes()).collect::<HashMap<_, _>>();
                let page = q
                    .get("page")
                    .or_else(|| q.get("cursor"))
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let limit = q
                    .get("limit")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 1000);
                return match hub.transcript(&scope, page, limit).await {
                    Ok(value) => Json(value).into_response(),
                    Err(e) => map_failure(e),
                };
            }
            "/v1/queue" => {
                return match hub.submissions(thread, true) {
                    Ok(rows) => Json(json!({"submissions":rows})).into_response(),
                    Err(e) => map_failure(e),
                }
            }
            _ => {
                if let Some(id) = path.strip_prefix("/v1/submissions/") {
                    return match hub.submission(thread, id) {
                        Ok(Some(s)) => Json(json!({"submission":s})).into_response(),
                        Ok(None) => failure(StatusCode::NOT_FOUND, "not_found"),
                        Err(e) => map_failure(e),
                    };
                }
            }
        }
    }
    if method == axum::http::Method::DELETE {
        if let Some(id) = path.strip_prefix("/v1/queue/") {
            return match hub.cancel(thread, id).await {
                Ok(s) => Json(json!({"submission":s})).into_response(),
                Err(e) => map_failure(e),
            };
        }
    }
    if method == axum::http::Method::POST {
        let body = match to_bytes(request.into_body(), 128 * 1024).await {
            Ok(b) => b,
            Err(_) => return failure(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large"),
        };
        if path == "/v1/abort" {
            return match state.interrupt(thread).await {
                Ok(_) => (StatusCode::ACCEPTED, Json(json!({"accepted":true}))).into_response(),
                Err(e) => map_failure(e),
            };
        }
        if path == "/v1/prompts" {
            let prompt = match serde_json::from_slice::<Prompt>(&body) {
                Ok(p) => p,
                Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid_prompt"),
            };
            if !scope.controller && (prompt.sender.is_some() || prompt.notify.is_some()) {
                return failure(StatusCode::FORBIDDEN, "controller_required");
            }
            return match hub.submit(thread, prompt).await {
                Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
                Err(e) => map_failure(e),
            };
        }
    }
    failure(StatusCode::NOT_FOUND, "not_found")
}

impl Hub {
    fn pending_count(&self, thread: &str) -> anyhow::Result<usize> {
        self.state.db.with(|c| Ok(c.query_row("SELECT count(*) FROM thread_pending_steers WHERE thread_id=?1 AND delivery IN ('continuation','cli-steer')",[thread],|r|r.get(0))?))
    }
    async fn transcript(
        &self,
        scope: &Scope,
        page: usize,
        limit: usize,
    ) -> anyhow::Result<pockymoe_treer::Transcript> {
        let (total, ids)=self.state.db.with(|c| {
            let total=c.query_row("SELECT count(*) FROM thread_turns WHERE thread_id=?1",[&scope.thread],|r|r.get::<_,usize>(0))?;
            let mut stmt=c.prepare("SELECT id FROM thread_turns WHERE thread_id=?1 ORDER BY ordinal,rowid LIMIT ?2 OFFSET ?3")?;
            let ids=stmt.query_map(params![scope.thread,limit as i64,page.min(i64::MAX as usize) as i64],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            Ok((total,ids))
        })?;
        let mut turns = Vec::with_capacity(ids.len());
        for id in ids {
            turns.push(
                self.state
                    .get_thread_turn_detail(&scope.thread, &id)
                    .await?,
            );
        }
        Ok(pockymoe_treer::transcript_page(
            &scope.agent,
            &scope.instance,
            &turns,
            total,
            page,
        ))
    }
    async fn submit(self: &Arc<Self>, thread: &str, prompt: Prompt) -> anyhow::Result<Value> {
        use sha2::{Digest, Sha256};
        prompt.validate()?;
        let _lock = self.lifecycle.lock().await;
        anyhow::ensure!(
            !self.state.integration_thread_paused(thread),
            "conflict: detached binding"
        );
        let encoded = serde_json::to_string(&prompt)?;
        let saved=self.state.db.with(|c| Ok(c.query_row("SELECT request_json,receipt_json FROM treer_submissions WHERE thread_id=?1 AND operation_id=?2",params![thread,prompt.operation_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()?))?;
        if let Some((request, receipt)) = saved {
            anyhow::ensure!(
                request == encoded,
                "conflict: operation_id was used with different input"
            );
            let mut receipt: Value = serde_json::from_str(&receipt)?;
            anyhow::ensure!(
                receipt.get("error").is_none(),
                "conflict: steering delivery failed and remains held"
            );
            receipt["duplicate"] = json!(true);
            return Ok(receipt);
        }
        let key = format!(
            "treer:{}",
            hex::encode(Sha256::digest(prompt.operation_id.as_bytes()))
        );
        // Metadata participates in the existing service's durable retry fingerprint,
        // including the crash window between acceptance and saving the AIS receipt.
        let hash = hex::encode(Sha256::digest(encoded.as_bytes()));
        let running = self.state.get_thread(thread)?.active_turn_id.is_some();
        let delivery = if prompt.delivery == Delivery::Steer && running {
            "steer"
        } else {
            "queue"
        };
        let input: ThreadSendInput = serde_json::from_value(
            json!({"text":prompt.text,"delivery":delivery,"clientRequestId":key,"subject":hash}),
        )?;
        let result = self.state.send_to_thread_locally(thread, input)?;
        let id = result["pendingSteerId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing submission id"))?;
        let position=self.state.db.with(|c| Ok(c.query_row("SELECT count(*) FROM thread_pending_steers WHERE thread_id=?1 AND delivery IN ('continuation','cli-steer')",[thread],|r|r.get::<_,usize>(0))?))?;
        let response = json!({"accepted":true,"operation_id":prompt.operation_id,"submission":{"id":id,"disposition":if result["delivery"]=="steer" {"steered"} else {"queued"},"position":if delivery=="steer" {None} else {Some(position)}}});
        self.state.db.with(|c| {c.execute("INSERT INTO treer_submissions(id,thread_id,operation_id,request_json,receipt_json,created_at,notified) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,thread,prompt.operation_id,encoded,response.to_string(),result["acceptedAt"].as_str().unwrap_or_default(),prompt.notify.as_ref().is_none_or(|n|n.on.is_empty())])?;Ok(())})?;
        if delivery == "steer" {
            if self.state.steer_pending_prompt(thread, id).await.is_err() {
                self.state.db.with(|c| {
                    c.execute(
                        "UPDATE treer_submissions SET receipt_json=?2 WHERE id=?1",
                        params![id, json!({"error":"steering_delivery_failed"}).to_string()],
                    )?;
                    Ok(())
                })?;
                anyhow::bail!("conflict: steering delivery failed and remains held");
            }
        } else {
            self.state.dispatch_inbox(thread.to_owned());
        }
        Ok(response)
    }
    fn submission(&self, thread: &str, id: &str) -> anyhow::Result<Option<Value>> {
        self.state.db.with(|c| {
            let row=c.query_row(
                "SELECT request_json,created_at,cancelled,receipt_json FROM treer_submissions WHERE thread_id=?1 AND id=?2",
                params![thread,id],
                |r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?,r.get::<_,String>(3)?)),
            ).optional()?;
            let Some((raw,created,cancelled,receipt))=row else {return Ok(None)};
            let prompt: Prompt=serde_json::from_str(&raw)?;
            let turn: Option<String>=c.query_row(
                "SELECT value FROM kv WHERE key=?1",[format!("cli:submission:turn:{id}")],|r|r.get(0),
            ).optional()?;
            let pending=c.query_row(
                "SELECT delivery,turn_id FROM thread_pending_steers WHERE thread_id=?1 AND id=?2",
                params![thread,id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)),
            ).optional()?;
            let failed=serde_json::from_str::<Value>(&receipt)?.get("error").is_some();
            let (state,error)=if failed {
                ("failed",Some("Steering delivery failed; input remains held".to_owned()))
            } else if cancelled {
                ("interrupted",Some("Cancelled before delivery".to_owned()))
            } else if let Some(turn)=&turn {
                let status=c.query_row(
                    "SELECT status,error FROM thread_turns WHERE id=?1 AND thread_id=?2",
                    params![turn,thread],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?)),
                ).optional()?;
                match status {
                    Some((status,error))=>(pockymoe_treer::submission_state(&status.into()),error),
                    None=>("interrupted",Some("Turn unavailable".into())),
                }
            } else if let Some((delivery,target))=&pending {
                let stale=delivery=="cli-steer" && !c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM thread_turns WHERE thread_id=?1 AND id=?2 AND status IN ('inProgress','recovering'))",
                    params![thread,target],|r|r.get::<_,bool>(0),
                )?;
                if stale {("interrupted",Some("Targeted turn ended before steering acknowledgement".into()))} else {("queued",None)}
            } else {
                ("interrupted",Some("Submission no longer pending".into()))
            };
            let position=if state=="queued" {
                Some(c.query_row(
                    "SELECT count(*) FROM thread_pending_steers WHERE thread_id=?1 AND delivery IN ('continuation','cli-steer') AND (created_at,id)<=(SELECT created_at,id FROM thread_pending_steers WHERE id=?2)",
                    params![thread,id],|r|r.get::<_,usize>(0),
                )?)
            } else {None};
            let preview=if let Some(turn)=&turn {
                c.query_row(
                    "SELECT json_extract(item_json,'$.text') FROM thread_history_items WHERE thread_id=?1 AND turn_id=?2 AND json_extract(item_json,'$.kind')='agentMessage' ORDER BY created_at DESC LIMIT 1",
                    params![thread,turn],|r|r.get::<_,Option<String>>(0),
                ).optional()?.flatten().map(|s|bounded(&s,4096))
            } else {None};
            Ok(Some(json!({"id":id,"sender":prompt.sender,"created_at":created,"position":position,"text_preview":bounded(&prompt.text,1024),"state":state,"turn_id":turn,"assistant_preview":preview,"error":error})))
        })
    }
    fn submissions(&self, thread: &str, pending_only: bool) -> anyhow::Result<Vec<Value>> {
        let ids = self.state.db.with(|c| {
            let mut stmt = c.prepare(
                "SELECT id FROM treer_submissions WHERE thread_id=?1 ORDER BY created_at,id",
            )?;
            let rows = stmt
                .query_map([thread], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        let mut rows = Vec::new();
        for id in ids {
            if let Some(row) = self.submission(thread, &id)? {
                if !pending_only || row["state"] == "queued" {
                    rows.push(row);
                }
            }
        }
        Ok(rows)
    }
    async fn cancel(&self, thread: &str, id: &str) -> anyhow::Result<Value> {
        let _lock = self.lifecycle.lock().await;
        let row = self
            .submission(thread, id)?
            .ok_or_else(|| anyhow::anyhow!("submission not found"))?;
        if row["state"] == "interrupted" {
            return Ok(row);
        }
        anyhow::ensure!(
            row["state"] == "queued",
            "conflict: only pending submissions may be cancelled"
        );
        self.state.cancel_pending_steer(thread, id).await?;
        self.state.db.with(|c| {
            c.execute(
                "UPDATE treer_submissions SET cancelled=1 WHERE thread_id=?1 AND id=?2",
                params![thread, id],
            )?;
            Ok(())
        })?;
        Ok(self.submission(thread, id)?.unwrap())
    }
    /// Durable pending notifications retry on the next live attach. Never rerun
    /// work, use the bound Agent's in-memory identity, and discard CLI output.
    async fn deliver_notifications(&self) {
        self.deliver_notifications_with(std::ffi::OsStr::new("treer"))
            .await;
    }
    async fn deliver_notifications_with(&self, executable: &std::ffi::OsStr) {
        let bindings = self
            .live
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for b in bindings {
            let records = self.state.db.with(|c| {
                let mut stmt = c.prepare(
                    "SELECT id,request_json FROM treer_submissions WHERE thread_id=?1 AND notified=0 AND notification_next_attempt<=?2"
                )?;
                let rows = stmt.query_map(params![b.thread, chrono::Utc::now().timestamp_millis()], |r| {
                    Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?))
                })?.collect::<Result<Vec<_>,_>>()?;
                Ok(rows)
            });
            let Ok(records) = records else { continue };
            for (id, raw) in records {
                let Ok(prompt) = serde_json::from_str::<Prompt>(&raw) else {
                    continue;
                };
                let Some(notify) = prompt.notify else {
                    continue;
                };
                let Some(to) = notify.to.or(prompt.sender.map(|s| s.agent_id)) else {
                    continue;
                };
                let Ok(Some(submission)) = self.submission(&b.thread, &id) else {
                    continue;
                };
                let outcome = submission["state"].as_str().unwrap_or_default();
                if !["done", "failed", "interrupted"].contains(&outcome) {
                    continue;
                }
                if !notify.on.iter().any(|s| s == outcome) {
                    let _ = self.state.db.with(|c| {
                        c.execute("UPDATE treer_submissions SET notified=1 WHERE id=?1", [&id])?;
                        Ok(())
                    });
                    continue;
                }
                // Serialize with detach: no credential-bearing subprocess survives
                // removal of its binding. Output is never retained or logged.
                let _lock = self.lifecycle.lock().await;
                if !self.live.lock().unwrap().get(&b.agent).is_some_and(|live| {
                    live.instance == b.instance && !live.cancelled.is_cancelled()
                }) {
                    continue;
                }
                use tokio::io::AsyncWriteExt;
                let mut command = tokio::process::Command::new(executable);
                for key in pockymoe_treer::IDENTITY_KEYS {
                    command.env_remove(key);
                }
                command
                    .args([
                        "message",
                        "send",
                        "--to",
                        &to,
                        "--idempotency-key",
                        &format!("prompt-submission/{id}/terminal"),
                    ])
                    .envs(pockymoe_treer::identity_env(&b.agent, &b.identity))
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(true);
                let Ok(mut child) = command.spawn() else {
                    self.defer_notification(&id);
                    continue;
                };
                let body=json!({"type":"treer.prompt.completed","status":outcome,"submission_id":id,"turn_id":submission["turn_id"],"assistant_preview":submission["assistant_preview"],"error":submission["error"]}).to_string()+"\n";
                let result = tokio::select! {
                    _=b.cancelled.cancelled()=>{ continue; },
                    result=tokio::time::timeout(std::time::Duration::from_secs(15),async {if let Some(mut stdin)=child.stdin.take() {stdin.write_all(body.as_bytes()).await?;} child.wait().await })=>result,
                };
                if matches!(result,Ok(Ok(status)) if status.success()) {
                    let _ = self.state.db.with(|c| {
                        c.execute("UPDATE treer_submissions SET notified=1 WHERE id=?1", [&id])?;
                        Ok(())
                    });
                } else {
                    self.defer_notification(&id);
                }
            }
        }
    }
    fn defer_notification(&self, id: &str) {
        let _ = self.state.db.with(|c| {
            let attempts = c.query_row(
                "SELECT notification_attempts FROM treer_submissions WHERE id=?1", [id],
                |row| row.get::<_, u32>(0),
            )?;
            let delay_ms = (1000_i64 << attempts.min(6)).min(60_000);
            c.execute(
                "UPDATE treer_submissions SET notification_attempts=?2,notification_next_attempt=?3 WHERE id=?1 AND notified=0",
                params![id, attempts.saturating_add(1), chrono::Utc::now().timestamp_millis() + delay_ms],
            )?;
            Ok(())
        });
        tracing::warn!(submission_id = id, "Treer completion notification deferred");
    }
}
fn bounded(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

#[cfg(test)]
mod tests;
