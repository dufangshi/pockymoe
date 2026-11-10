//! Agents call models through the owner's saved connections
//! (docs/upstream-connections.md, "Agent access"). The agent authenticates with
//! its own CLI credential; the upstream key never leaves the Supervisor.
use crate::auth::CliCaller;
use anyhow::{bail, ensure, Context, Result};
use axum::{
    body::{Body, Bytes},
    extract::{Path, RawQuery, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use pockymoe_runtime::{
    interaction::AgentRequester,
    upstreams::{
        self,
        agent_proxy::{self, KeyScan},
        connections::{self as saved, Connection},
    },
    Supervisor,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

/// A request body is a prompt with its attachments, not a file upload.
pub(crate) const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;
/// Long enough for a slow reasoning model to finish one answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(900);

fn dir(state: &Supervisor) -> std::path::PathBuf {
    upstreams::directory(&state.config.database_url)
}

/// Threads of this device and the owner may call models; other devices and
/// isolated environments may not.
fn ensure_may_call(state: &Supervisor, requester: AgentRequester<'_>) -> Result<()> {
    match requester {
        AgentRequester::Owner => Ok(()),
        AgentRequester::Thread(thread) => {
            ensure!(
                state.environments.for_thread(thread)?.is_none(),
                "forbidden: saved connections are not available inside an isolated environment"
            );
            Ok(())
        }
        _ => bail!("forbidden: saved connections are only available to this device's own agents"),
    }
}

/// The connection named by id or exact name, if this caller may use it.
async fn connection(
    state: &Supervisor,
    requester: AgentRequester<'_>,
    wanted: &str,
) -> Result<Connection> {
    ensure_may_call(state, requester)?;
    let all = {
        let _gate = state.upstream_gate.lock().await;
        saved::migrate(&dir(state))?;
        saved::with_agent_access(&dir(state))?
    };
    let owner = requester == AgentRequester::Owner;
    let mut matches = all
        .into_iter()
        .filter(|(c, _)| c.id == wanted || c.name.eq_ignore_ascii_case(wanted));
    let (found, access) = matches
        .next()
        .with_context(|| format!("not_found: no saved connection named {wanted}"))?;
    ensure!(
        matches.next().is_none(),
        "conflict: several connections are named {wanted}; use the id"
    );
    ensure!(
        owner || access,
        "forbidden: the owner has not let agents call this connection (Settings > Upstreams, or `pockymoe model allow`)"
    );
    Ok(found)
}

/// `modelList`, `modelAccess` and `modelComplete` of the local CLI.
pub(crate) async fn cli(
    state: &Arc<Supervisor>,
    input: &Value,
    requester: AgentRequester<'_>,
) -> Result<Value> {
    match input["operation"].as_str().unwrap_or("") {
        "modelList" => {
            ensure_may_call(state, requester)?;
            let root = state
                .interaction
                .context
                .read()
                .unwrap()
                .as_ref()
                .map(|context| context.url.clone())
                .context("the local CLI is not configured")?;
            let owner = requester == AgentRequester::Owner;
            let all = {
                let _gate = state.upstream_gate.lock().await;
                saved::migrate(&dir(state))?;
                saved::with_agent_access(&dir(state))?
            };
            let closed = all.iter().filter(|(_, access)| !access).count();
            let connections: Vec<Value> = all
                .iter()
                .filter(|(_, access)| owner || *access)
                .map(|(c, access)| agent_proxy::agent_view(c, *access, &root))
                .collect();
            let mut result = json!({
                "connections": connections,
                "apiKeyEnv": "POCKYMOE_TOKEN",
                "guide": "pockymoe guide models",
            });
            if !owner && closed > 0 {
                result["notOpenToAgents"] = json!(closed);
            }
            Ok(result)
        }
        "modelAccess" => {
            ensure!(
                requester == AgentRequester::Owner,
                "Only the computer's owner decides which connections agents can call"
            );
            let wanted = input["connectionId"]
                .as_str()
                .context("connection required")?;
            let enabled = input["enabled"].as_bool().context("enabled required")?;
            let found = connection(state, requester, wanted).await?;
            let _gate = state.upstream_gate.lock().await;
            let summary = saved::set_agent_access(&dir(state), &found.id, enabled)?;
            Ok(json!({"id": found.id, "name": found.name, "agentAccess": summary["agentAccess"]}))
        }
        "modelComplete" => {
            let wanted = input["connectionId"]
                .as_str()
                .context("connectionId required")?;
            let user = input["user"].as_str().context("user prompt required")?;
            let system = input["system"].as_str().unwrap_or("");
            ensure!(
                system.len() + user.len() <= 262144,
                "prompt exceeds 256 KiB"
            );
            let max_tokens = input["maxTokens"].as_u64().unwrap_or(1024).clamp(1, 16384) as usize;
            let found = connection(state, requester, wanted).await?;
            let images: Vec<saved::CompletionImage> = match input.get("images") {
                None | Some(Value::Null) => vec![],
                Some(value) => serde_json::from_value(value.clone()).context("invalid images")?,
            };
            let model = input["model"].as_str().unwrap_or("");
            let text =
                saved::complete_with_images(&found, model, system, user, &images, max_tokens)
                    .await?;
            log(requester, &found, "complete", StatusCode::OK);
            Ok(json!({"text": text, "model": if model.is_empty() { &found.model } else { model }}))
        }
        _ => bail!("unknown model operation"),
    }
}

fn log(requester: AgentRequester<'_>, c: &Connection, route: &str, status: StatusCode) {
    let caller = match requester {
        AgentRequester::Thread(thread) => thread,
        _ => "owner",
    };
    tracing::info!(caller, connection = %c.id, route, status = status.as_u16(), "model request through a saved connection");
}

/// Errors in the shape the SDKs print: `{"error":{"message":…}}`.
fn refuse(error: &anyhow::Error) -> Response {
    let message = error.to_string();
    let (status, code) = if message.starts_with("forbidden:") {
        (StatusCode::FORBIDDEN, "forbidden")
    } else if message.starts_with("not_found:") || message == "Unsupported model route" {
        (StatusCode::NOT_FOUND, "not_found")
    } else if message.starts_with("conflict:") {
        (StatusCode::CONFLICT, "conflict")
    } else {
        (StatusCode::BAD_GATEWAY, "upstream_unavailable")
    };
    (
        status,
        Json(json!({"error": {"message": message, "type": "pockymoe_model_proxy", "code": code}})),
    )
        .into_response()
}

/// `ANY /api/agent-models/{connection}/{*path}`: one request to the upstream,
/// its answer streamed back as it arrives.
pub(crate) async fn forward(
    State(state): State<Arc<Supervisor>>,
    Extension(caller): Extension<CliCaller>,
    Path((wanted, path)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let requester = crate::interaction::requester(caller.0.as_deref(), false);
    let prepared = async {
        let found = connection(&state, requester, &wanted).await?;
        let target = agent_proxy::target(&found, method.as_str(), &path, query.as_deref())?;
        Ok::<_, anyhow::Error>((found, target))
    }
    .await;
    let (found, target) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return refuse(&error),
    };
    let sent: Vec<(String, String)> = headers
        .iter()
        .filter_map(|(name, value)| {
            Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
        })
        .collect();
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(_) => return refuse(&anyhow::anyhow!("Cannot initialize upstream client")),
    };
    let mut request = client.request(method.clone(), target.url.clone());
    for (name, value) in agent_proxy::upstream_headers(&found, target.protocol, &sent) {
        request = request.header(name, value);
    }
    if method != Method::GET {
        request = request.body(body);
    }
    let upstream = match request.send().await {
        Ok(upstream) => upstream,
        // The error's text could name the upstream URL; the agent gets a plain reason.
        Err(error) => {
            let reason = if error.is_timeout() {
                "timed out"
            } else {
                "could not be reached"
            };
            return refuse(&anyhow::anyhow!("The connection's upstream {reason}"));
        }
    };
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    log(requester, &found, path.trim_matches('/'), status);
    let mut response = Response::builder().status(status);
    for (name, value) in upstream.headers() {
        if agent_proxy::returns_response_header(name.as_str()) {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_str().as_bytes()),
                HeaderValue::from_bytes(value.as_bytes()),
            ) {
                response = response.header(name, value);
            }
        }
    }
    // Events reach the agent as they are produced, not when a buffer fills.
    response = response
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-accel-buffering", "no");
    let scan = KeyScan::new(&found);
    let stream = futures_util::stream::unfold(Some((upstream, scan)), |state| async move {
        let (mut upstream, mut scan) = state?;
        match upstream.chunk().await {
            Ok(Some(chunk)) if scan.leaks(&chunk) => Some((
                Err(std::io::Error::other(
                    "the upstream response contained the connection's credential",
                )),
                None,
            )),
            Ok(Some(chunk)) => Some((Ok(chunk), Some((upstream, scan)))),
            Ok(None) => None,
            Err(_) => Some((
                Err(std::io::Error::other("the upstream response ended early")),
                None,
            )),
        }
    });
    response
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| refuse(&anyhow::anyhow!("Invalid upstream response")))
}
