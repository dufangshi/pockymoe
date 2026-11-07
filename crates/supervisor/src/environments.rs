//! Owner-managed environments and the deliberately narrow in-environment gateway.
use crate::http::{map_err, ApiErr};
use anyhow::{ensure, Context, Result};
use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use pockymoe_protocol::*;
use pockymoe_runtime::{
    execution::{self, ExecutionContext},
    Supervisor,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

fn require(condition: bool, message: &str) -> Result<()> {
    ensure!(condition, "{message}");
    Ok(())
}

pub(crate) fn routes() -> Router<Arc<Supervisor>> {
    Router::new()
        .route("/api/environments", get(list).post(create))
        .route("/api/environments/connections/{id}", put(connection))
        .route(
            "/api/environments/connections/{id}/test",
            post(test_connection),
        )
        .route("/api/environments/budget", put(budget))
        .route("/api/environments/{id}/{action}", post(action))
        .route("/api/threads/{id}/environment", get(thread).put(bind))
}
async fn list(State(state): State<Arc<Supervisor>>) -> Result<Json<EnvironmentsDto>, ApiErr> {
    Ok(Json(
        state.environments.reconcile_all().await.map_err(map_err)?,
    ))
}
async fn connection(
    State(state): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Json(input): Json<UpdateEnvironmentConnectionRequest>,
) -> Result<Json<EnvironmentConnectionDto>, ApiErr> {
    Ok(Json(
        state
            .environments
            .save_connection(&id, &input)
            .await
            .map_err(map_err)?,
    ))
}
async fn test_connection(
    State(state): State<Arc<Supervisor>>,
    Path(id): Path<String>,
) -> Result<Json<TestEnvironmentConnectionResponse>, ApiErr> {
    Ok(Json(
        state
            .environments
            .test_connection(&id)
            .await
            .map_err(map_err)?,
    ))
}
async fn budget(
    State(state): State<Arc<Supervisor>>,
    Json(input): Json<EnvironmentBudgetDto>,
) -> Result<Json<EnvironmentBudgetDto>, ApiErr> {
    Ok(Json(
        state
            .environments
            .save_budget(&input)
            .await
            .map_err(map_err)?,
    ))
}
async fn create(
    State(state): State<Arc<Supervisor>>,
    Json(input): Json<CreateEnvironmentRequest>,
) -> Result<Json<EnvironmentDto>, ApiErr> {
    require(
        !state.config.treer_enabled,
        "unsupported_host_environment: Treer requires an isolated execution adapter",
    )
    .map_err(map_err)?;
    let thread = state.get_thread(&input.thread_id).map_err(map_err)?;
    let workspace = state.get_workspace(&thread.workspace_id).map_err(map_err)?;
    let cwd = state
        .session_cwd(&thread)
        .unwrap_or_else(|| workspace.abs_path.clone());
    Ok(Json(
        state
            .environments
            .create(
                &input,
                std::path::Path::new(&workspace.abs_path),
                std::path::Path::new(&cwd),
            )
            .await
            .map_err(map_err)?,
    ))
}
async fn action(
    State(state): State<Arc<Supervisor>>,
    Path((id, action)): Path<(String, String)>,
) -> Result<Json<EnvironmentDto>, ApiErr> {
    let action: EnvironmentAction = action
        .parse()
        .map_err(|_| map_err(anyhow::anyhow!("invalid environment action")))?;
    let env = state.environments.get(&id).map_err(map_err)?;
    let mut release = Vec::new();
    if action != EnvironmentAction::Start {
        for thread in state.list_threads(None, true).map_err(map_err)? {
            if state
                .environments
                .for_thread(&thread.id)
                .map_err(map_err)?
                .is_some_and(|bound| bound.id == env.id)
            {
                release.push(thread.id);
            }
        }
        execution::revoke_capabilities(&env.id);
    }
    // Lifecycle fencing and container shutdown must not depend on an agent
    // acknowledging cancellation. Cleanup afterwards also runs on provider error.
    let result = state.environments.action(&id, action).await;
    for thread in release {
        crate::shells::hub().terminate_thread(&thread);
        if !matches!(
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                state.release_thread_harness(&thread, true, None),
            )
            .await,
            Ok(Ok(_))
        ) {
            tracing::warn!(thread_id = %thread, "environment harness cleanup remains pending after lifecycle action");
        }
    }
    let env = result.map_err(map_err)?;
    if env.state == "running" {
        ensure_gateway(state.clone(), &env).map_err(map_err)?;
    }
    Ok(Json(env))
}
async fn thread(
    State(state): State<Arc<Supervisor>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiErr> {
    state.get_thread(&id).map_err(map_err)?;
    Ok(Json(
        json!({"environment":state.environments.for_thread(&id).map_err(map_err)?,"supportedHarnesses":["codex","claude","custom"],"limitations":["API gateway authentication only; native subscription credentials, SSH and credentialed MCP are not projected","Disk quota for original bind storage is unavailable","Environment capability expires after one hour; reconnect the harness to renew"]}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BindInput {
    environment_id: Option<String>,
    #[serde(default)]
    reset_session: bool,
}
async fn bind(
    State(state): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Json(input): Json<BindInput>,
) -> Result<Json<Value>, ApiErr> {
    require(
        input.environment_id.is_none() || !state.config.treer_enabled,
        "unsupported_host_environment: Treer requires an isolated execution adapter",
    )
    .map_err(map_err)?;
    let thread = state.get_thread(&id).map_err(map_err)?;
    require(
        thread.status == "idle" || thread.status == "failed" || thread.status == "interrupted",
        "conflict: stop the active turn before changing environment",
    )
    .map_err(map_err)?;
    require(
        thread
            .parent_thread_id
            .as_deref()
            .map(|p| state.environments.for_thread(p))
            .transpose()
            .map_err(map_err)?
            .flatten()
            .is_none(),
        "inherited_environment_required: child threads cannot change or remove their environment",
    )
    .map_err(map_err)?;
    if let Some(environment) = &input.environment_id {
        let env = state.environments.get(environment).map_err(map_err)?;
        require(
            env.thread_id == id,
            "forbidden: environment belongs to another thread",
        )
        .map_err(map_err)?;
    }
    if thread.provider_session_id.is_some() {
        require(
            input.reset_session,
            "conflict: resetSession is required to release the current native session",
        )
        .map_err(map_err)?;
        state
            .release_thread_harness(&id, false, None)
            .await
            .map_err(map_err)?;
        state
            .db
            .with(|c| {
                c.execute(
                    "UPDATE threads SET provider_session_id=NULL WHERE id=?1",
                    [&id],
                )?;
                Ok(())
            })
            .map_err(map_err)?;
    }
    if let Some(old) = state.environments.for_thread(&id).map_err(map_err)? {
        execution::revoke_capabilities(&old.id);
    }
    state
        .environments
        .bind_thread(&id, input.environment_id.as_deref())
        .map_err(map_err)?;
    crate::ports::revoke_thread_mappings(&state, &id)
        .await
        .map_err(map_err)?;
    if let Some(context) = state.execution_context(&id).map_err(map_err)? {
        let env = context
            .manager
            .get(&context.environment_id)
            .map_err(map_err)?;
        if env.state == "running" {
            ensure_gateway(state.clone(), &env).map_err(map_err)?;
        }
    }
    self::thread(State(state), Path(id)).await
}

#[cfg(unix)]
pub(crate) fn secure_socket(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let directory = std::fs::metadata(path.parent().context("socket directory missing")?)?;
    let socket = std::fs::metadata(path)?;
    if socket.uid() != directory.uid() || socket.gid() != directory.gid() {
        std::os::unix::fs::chown(path, Some(directory.uid()), Some(directory.gid()))?;
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn ensure_gateway(state: Arc<Supervisor>, env: &EnvironmentDto) -> Result<()> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static SERVERS: OnceLock<
        Mutex<HashMap<(std::path::PathBuf, String, u64), tokio::task::AbortHandle>>,
    > = OnceLock::new();
    let servers = SERVERS.get_or_init(Default::default);
    let mut servers = servers.lock().unwrap();
    let key = (
        state.config.database_url.clone(),
        env.id.clone(),
        env.generation,
    );
    if servers.get(&key).is_some_and(|h| !h.is_finished()) {
        return Ok(());
    }
    let path = state
        .environments
        .bridge_directory(&env.id, env.generation)?
        .join("gateway.sock");
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    use std::os::fd::AsRawFd;
    let parent = std::fs::File::open(path.parent().context("bridge directory missing")?)?;
    // sockaddr_un is short even when the user's data directory is long.
    let listener = tokio::net::UnixListener::bind(format!(
        "/proc/self/fd/{}/gateway.sock",
        parent.as_raw_fd()
    ))?;
    secure_socket(&path)?;
    let gateway = Gateway {
        state,
        environment_id: env.id.clone(),
        generation: env.generation,
        model_slots: Arc::new(tokio::sync::Semaphore::new(4)),
    };
    crate::environments_egress::start(gateway.clone())?;
    let router = Router::new()
        .route("/api/cli", post(gateway_cli))
        .route("/model/{harness}/{*path}", axum::routing::any(model))
        .with_state(gateway);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    servers.retain(|(_, id, generation), handle| {
        if id == &env.id && *generation != env.generation {
            handle.abort();
            false
        } else {
            !handle.is_finished()
        }
    });
    servers.insert(key, handle.abort_handle());
    Ok(())
}
#[cfg(not(unix))]
pub(crate) fn ensure_gateway(_: Arc<Supervisor>, _: &EnvironmentDto) -> Result<()> {
    anyhow::bail!("unsupported_environment_platform")
}

#[derive(Clone)]
pub(crate) struct Gateway {
    pub(crate) state: Arc<Supervisor>,
    pub(crate) environment_id: String,
    pub(crate) generation: u64,
    model_slots: Arc<tokio::sync::Semaphore>,
}
fn authorize(g: &Gateway, headers: &HeaderMap) -> Result<ExecutionContext> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()))
        .context("environment capability required")?;
    let thread = execution::validate_capability(token, &g.environment_id, g.generation)?;
    let context = g
        .state
        .execution_context(&thread)?
        .context("thread environment is not bound")?;
    ensure!(
        context.environment_id == g.environment_id && context.generation == g.generation,
        "stale_environment_generation"
    );
    context.checked()?;
    Ok(context)
}
async fn gateway_cli(
    State(g): State<Gateway>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    let context = authorize(&g, &headers).map_err(map_err)?;
    require(
        input["fromThreadId"]
            .as_str()
            .is_none_or(|id| id == context.thread_id),
        "forbidden: forged thread identity",
    )
    .map_err(map_err)?;
    let op = input["operation"].as_str().unwrap_or("");
    require(
        matches!(
            op,
            "info"
                | "skill"
                | "guide"
                | "list"
                | "show"
                | "status"
                | "send"
                | "resolve"
                | "tree"
                | "wait"
                | "wake"
                | "close"
                | "inboxWait"
                | "inbox"
                | "inboxRead"
                | "inboxAck"
                | "taskAdd"
                | "taskList"
                | "taskShow"
                | "taskClaim"
                | "taskDone"
                | "taskRelease"
                | "transcript"
                | "create"
                | "previewCreate"
                | "previewList"
                | "previewCheck"
                | "previewStop"
                | "commandRun"
                | "commandShow"
                | "worktreeCreate"
                | "worktreeList"
                | "worktreeCurrent"
                | "worktreeShow"
                | "worktreeUse"
                | "worktreeRemove"
                | "fsList"
                | "fsGet"
        ),
        "unsupported_environment_operation",
    )
    .map_err(map_err)?;
    if op == "commandRun" {
        require(
            input["threadId"].as_str() == Some(context.thread_id.as_str()),
            "forbidden: commands must use the authenticated thread",
        )
        .map_err(map_err)?;
    }
    if op == "create" {
        require(
            input["role"].is_null(),
            "unsupported_environment_role: host home role templates are not projected",
        )
        .map_err(map_err)?;
        let caller = g.state.get_thread(&context.thread_id).map_err(map_err)?;
        require(
            input["workspaceId"]
                .as_str()
                .is_none_or(|w| w == caller.workspace_id),
            "forbidden: child must use the original workspace",
        )
        .map_err(map_err)?;
    }
    if matches!(op, "fsList" | "fsGet") {
        let thread = g.state.get_thread(&context.thread_id).map_err(map_err)?;
        require(
            input["workspaceId"].as_str() == Some(&thread.workspace_id),
            "forbidden: filesystem workspace mismatch",
        )
        .map_err(map_err)?;
        require(
            input["deviceId"]
                .as_str()
                .is_none_or(|id| id == g.state.db.host_id),
            "unsupported_environment_remote_files",
        )
        .map_err(map_err)?;
        context.checked().map_err(map_err)?;
        let root = &context.working_directory;
        let path = input["path"].as_str().unwrap_or(".");
        let value = if op == "fsList" {
            context.request(&json!({"op":"tree","root":root,"path":path}))
        } else {
            let to = input["out"].as_str().map(str::to_owned).unwrap_or_else(|| {
                std::path::Path::new(path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into()
            });
            context.request(&json!({"op":"copyFile","root":root,"path":path,"to":to}))
        }
        .map_err(map_err)?;
        return Ok(Json(value));
    }
    let caller = context.thread_id.clone();
    let response = execution::scope(
        Some(context),
        crate::interaction::run(&g.state, input, Some(caller)),
    )
    .await
    .map_err(map_err)?;
    Ok(Json(response))
}
async fn model(
    State(g): State<Gateway>,
    Path((harness, path)): Path<(String, String)>,
    request: Request,
) -> Result<Response, ApiErr> {
    let context = authorize(&g, request.headers()).map_err(map_err)?;
    let permit = g
        .model_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| map_err(anyhow::anyhow!("environment_model_concurrency_limit")))?;
    require(
        matches!(harness.as_str(), "codex" | "claude"),
        "unsupported_environment_model_provider",
    )
    .map_err(map_err)?;
    let thread = g.state.get_thread(&context.thread_id).map_err(map_err)?;
    let selected = thread
        .agent_id
        .as_deref()
        .unwrap_or(if thread.provider == Provider::Acp {
            "codex"
        } else {
            thread.provider.as_str()
        });
    // Custom ACP commands use the explicitly configured Codex Responses profile;
    // a capability cannot select another harness's credential or billing profile.
    let credential_provider = match selected {
        "claude" => "claude",
        "codex" | "custom" => "codex",
        _ => "unsupported",
    };
    require(
        harness == credential_provider,
        "environment_model_provider_not_authorized",
    )
    .map_err(map_err)?;
    require(
        request.method() == axum::http::Method::POST
            && match harness.as_str() {
                "codex" => matches!(path.as_str(), "responses" | "v1/responses"),
                "claude" => matches!(path.as_str(), "messages" | "v1/messages"),
                _ => false,
            },
        "unsupported_environment_model_endpoint",
    )
    .map_err(map_err)?;
    let profile =
        pockymoe_runtime::upstreams::active_profile(&context.upstream_directory, &harness)
            .map_err(map_err)?
            .context("API upstream required")
            .map_err(map_err)?;
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        axum::body::to_bytes(request.into_body(), 32 * 1024 * 1024),
    )
    .await
    .map_err(|_| map_err(anyhow::anyhow!("environment_model_request_timeout")))?
    .map_err(|_| map_err(anyhow::anyhow!("environment_model_request_invalid")))?;
    let payload: Value = serde_json::from_slice(&body).map_err(|e| map_err(e.into()))?;
    let allowed = thread.model.as_deref().unwrap_or(&profile.model);
    require(
        payload["model"].as_str() == Some(allowed),
        "environment_model_not_authorized",
    )
    .map_err(map_err)?;
    let base = profile.base_url.trim_end_matches('/');
    let suffix = path
        .strip_prefix("v1/")
        .filter(|_| base.ends_with("/v1"))
        .unwrap_or(&path);
    let url = format!("{base}/{suffix}");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .no_proxy()
        .build()
        .map_err(|e| map_err(e.into()))?;
    let mut outbound = client
        .post(url)
        .header("content-type", "application/json")
        .body(body);
    if harness == "claude" {
        outbound = outbound.header("anthropic-version", "2023-06-01");
    }
    outbound = if profile.auth_type == "bearer" || harness == "codex" {
        outbound.bearer_auth(&profile.api_key)
    } else {
        outbound.header("x-api-key", &profile.api_key)
    };
    let upstream = tokio::select! {
        result = tokio::time::timeout(std::time::Duration::from_secs(120), outbound.send()) => {
            result.map_err(|_| map_err(anyhow::anyhow!("environment_model_upstream_timeout")))?
                .map_err(|_| map_err(anyhow::anyhow!("environment_model_upstream_unavailable")))?
        },
        _ = async { loop {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if context.checked().is_err() { break }
        }} => return Err(map_err(anyhow::anyhow!("environment_model_revoked"))),
    };
    let status = upstream.status();
    let content_type = upstream.headers().get("content-type").cloned();
    let secret = profile.api_key.into_bytes();
    let stream = futures_util::stream::try_unfold(
        (upstream, Vec::<u8>::new(), context, secret, false, permit),
        |(mut stream, mut pending, context, secret, done, permit)| async move {
            if done {
                return Ok::<_, std::io::Error>(None);
            }
            if context.checked().is_err() {
                return Err(std::io::Error::other("environment revoked"));
            }
            let next = tokio::select! {
                chunk = stream.chunk() => chunk,
                _ = async { loop { tokio::time::sleep(std::time::Duration::from_millis(250)).await; if context.checked().is_err() { break } } } => return Err(std::io::Error::other("environment revoked")),
            };
            let ended = match next {
                Ok(Some(bytes)) => {
                    pending.extend_from_slice(&bytes);
                    false
                }
                Err(_) => return Err(std::io::Error::other("upstream stream interrupted")),
                Ok(None) => true,
            };
            // Keep a suffix so credentials echoed across arbitrary stream boundaries
            // are redacted too. Upstream response headers are never forwarded wholesale.
            redact_secret(&mut pending, &secret);
            let count = if ended {
                pending.len()
            } else {
                pending.len().saturating_sub(secret.len().saturating_sub(1))
            };
            let bytes = pending.drain(..count).collect::<Vec<_>>();
            Ok(Some((
                bytes,
                (stream, pending, context, secret, ended, permit),
            )))
        },
    );
    let mut response = Response::builder().status(status);
    if let Some(value) = content_type {
        response = response.header("content-type", value);
    }
    Ok(response
        .header("cache-control", "no-store")
        .body(Body::from_stream(stream))
        .unwrap())
}

pub(crate) fn start(state: Arc<Supervisor>) {
    tokio::spawn(async move {
        loop {
            if let Ok(snapshot) = state.environments.reconcile_all().await {
                for env in snapshot
                    .environments
                    .iter()
                    .filter(|e| e.state == "interrupted")
                {
                    execution::revoke_capabilities(&env.id);
                    if let Ok(threads) = state.list_threads(None, true) {
                        for thread in threads {
                            if state
                                .environments
                                .for_thread(&thread.id)
                                .ok()
                                .flatten()
                                .is_some_and(|bound| bound.id == env.id)
                            {
                                crate::shells::hub().terminate_thread(&thread.id);
                                let _ = tokio::time::timeout(
                                    std::time::Duration::from_secs(5),
                                    state.release_thread_harness(&thread.id, true, None),
                                )
                                .await;
                            }
                        }
                    }
                }
                for env in snapshot
                    .environments
                    .into_iter()
                    .filter(|e| e.state == "running")
                {
                    let _ = ensure_gateway(state.clone(), &env);
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
}

/// Only authenticated request routing selects an execution environment. The
/// query's thread is checked against the workspace before it can select a backend.
pub(crate) async fn execution_scope(
    State(state): State<Arc<Supervisor>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let segments = path.split('/').collect::<Vec<_>>();
    let selected = (|| -> Result<Option<ExecutionContext>> {
        if segments.get(2) == Some(&"threads") {
            if let Some(id) = segments.get(3) {
                return state.execution_context(id);
            }
        }
        if segments.get(2) == Some(&"workspaces")
            && segments.get(4).is_some_and(|s| {
                matches!(
                    *s,
                    "files" | "tree" | "preview" | "raw" | "download" | "file-capabilities"
                )
            })
        {
            let workspace = segments[3];
            let supplied =
                url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
                    .find(|(k, _)| k == "threadId")
                    .map(|(_, v)| v.into_owned());
            if let Some(id) = supplied {
                ensure!(
                    state.get_thread(&id)?.workspace_id == workspace,
                    "forbidden: thread workspace mismatch"
                );
                return state.execution_context(&id);
            }
            let ws = state.get_workspace(workspace)?;
            // An unqualified workspace request cannot silently inspect the host
            // when this working storage has any isolated binding.
            let threads = state.list_threads(Some(workspace), true)?;
            for thread in threads {
                if state.environments.for_thread(&thread.id)?.is_some() {
                    anyhow::bail!(
                        "environment_thread_required: add threadId for isolated workspace files"
                    )
                }
            }
            let _ = ws;
        }
        Ok(None)
    })();
    match selected {
        Ok(context) => execution::scope(context, next.run(request)).await,
        Err(error) => (
            StatusCode::CONFLICT,
            Json(json!({"code":"environment_context_required","message":error.to_string()})),
        )
            .into_response(),
    }
}

fn redact_secret(bytes: &mut Vec<u8>, secret: &[u8]) {
    if secret.is_empty() {
        return;
    }
    let mut from = 0;
    let mut result = Vec::new();
    while let Some(offset) = bytes[from..]
        .windows(secret.len())
        .position(|v| v == secret)
    {
        let at = from + offset;
        result.extend_from_slice(&bytes[from..at]);
        result.extend_from_slice(b"[redacted]");
        from = at + secret.len();
    }
    if from > 0 {
        result.extend_from_slice(&bytes[from..]);
        *bytes = result;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redaction_handles_repeats_and_a_key_equal_to_the_marker() {
        let secret = uuid::Uuid::new_v4().to_string().into_bytes();
        let mut bytes = [secret.as_slice(), b" middle ", secret.as_slice()].concat();
        redact_secret(&mut bytes, &secret);
        assert!(!bytes.windows(secret.len()).any(|v| v == secret));
        let mut marker = b"[redacted][redacted]".to_vec();
        redact_secret(&mut marker, b"[redacted]");
        assert_eq!(marker, b"[redacted][redacted]");
    }
}
