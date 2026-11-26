//! Outside-agent MCP. JSON response Streamable HTTP; no unsolicited SSE stream.
mod audit;
mod cancellation;
mod events;
mod oauth;
#[cfg(test)]
mod regression_tests;
mod remote;
mod store;
#[cfg(test)]
mod tests;
mod tools;

use anyhow::{ensure, Context, Result};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use pockymoe_runtime::Supervisor;
use serde_json::{json, Value};
use std::sync::Arc;
use tools::Grant;

/// The canonical origin OAuth clients use for this Supervisor.
pub(crate) fn issuer(state: &Supervisor) -> String {
    oauth::issuer(state)
}

pub(crate) fn start(state: &Arc<Supervisor>) {
    events::start(state);
}

/// Protocol versions this server speaks, newest first.
const VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

pub(crate) fn routes(state: &Arc<Supervisor>) -> Router<Arc<Supervisor>> {
    Router::new()
        .route("/mcp", post(http).get(no_stream).delete(terminate))
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth::protected),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(oauth::protected),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth::metadata),
        )
        .route("/oauth/mcp/register", post(oauth::register))
        .route("/oauth/mcp/authorize", get(oauth::authorize))
        .route("/oauth/mcp/token", post(oauth::token))
        .route("/oauth/mcp/revoke", post(oauth::revoke))
        .route(
            "/api/mcp/consent/{id}",
            get(oauth::consent_get).post(oauth::consent_post),
        )
        .route("/api/mcp/clients", get(oauth::settings))
        .route(
            "/api/mcp/clients/{id}",
            axum::routing::delete(oauth::remove),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            guard_host,
        ))
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
}
/// DNS rebinding: a page on a hostile name resolved to this machine sends its
/// own name as Host. IP literals, localhost and the issuer cannot be such a name.
fn host_valid(s: &Supervisor, h: &HeaderMap) -> bool {
    let Some(host) = h.get("host") else {
        return true;
    };
    let Ok(host) = host.to_str() else {
        return false;
    };
    let Ok(url) = url::Url::parse(&format!("http://{host}")) else {
        return false;
    };
    let issuer = url::Url::parse(&oauth::issuer(s)).ok();
    match url.host() {
        Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => true,
        Some(url::Host::Domain(name)) => {
            name.eq_ignore_ascii_case("localhost")
                || issuer
                    .as_ref()
                    .and_then(|i| i.host_str())
                    .is_some_and(|i| i.eq_ignore_ascii_case(name))
        }
        None => false,
    }
}
async fn guard_host(
    State(s): State<Arc<Supervisor>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = request.uri().path();
    let guarded = path == "/mcp"
        || path.starts_with("/oauth/mcp/")
        || path.starts_with("/.well-known/oauth-")
        || path.starts_with("/api/mcp/consent/");
    if guarded && !host_valid(&s, request.headers()) {
        return (
            StatusCode::MISDIRECTED_REQUEST,
            Json(json!({"error":"invalid_host","error_description":"Use the address in POCKYMOE_MCP_ISSUER, localhost or an IP address."})),
        )
            .into_response();
    }
    next.run(request).await
}
const INSTRUCTIONS: &str = "Pockymoe runs coding agents in threads on the owner's devices. List workspaces, launch a thread with a prompt, then call pockymoe_thread_wait (repeat it while the status is still running) and pockymoe_thread_read. The grant decides which tools are listed.";
/// The client's version when this server speaks it, else the newest.
fn negotiate(request: &Value) -> &'static str {
    let asked = request["params"]["protocolVersion"].as_str().unwrap_or("");
    VERSIONS
        .into_iter()
        .find(|v| *v == asked)
        .unwrap_or(VERSIONS[0])
}
fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
pub(crate) async fn rpc(state: &Arc<Supervisor>, grant: &Grant, request: Value) -> Option<Value> {
    let id = request.get("id").cloned();
    if request["jsonrpc"] != "2.0"
        || !request["method"].is_string()
        || id
            .as_ref()
            .is_some_and(|v| !v.is_string() && !v.is_number())
    {
        return Some(rpc_error(
            id.unwrap_or(Value::Null),
            -32600,
            "Invalid Request",
        ));
    }
    let method = request["method"].as_str().unwrap();
    let id = id?;
    let result: Result<Value> = match method {
        "initialize" => Ok(
            json!({"protocolVersion":negotiate(&request),"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"pockymoe","title":"Pockymoe","version":pockymoe_protocol::APP_VERSION},"instructions":INSTRUCTIONS}),
        ),
        "ping" => Ok(json!({})),
        "tools/list" => {
            // Only what this grant can call, so a model is not offered tools
            // that can only fail.
            let mut c = tools::catalog();
            c["tools"].as_array_mut().unwrap().retain(|t| {
                let name = t["name"].as_str().unwrap();
                grant.check(name, None, None).is_ok()
                    && (grant.device.is_none() || remote::supported(name, &grant.access))
            });
            Ok(c)
        }
        "tools/call" => {
            let name = request["params"]["name"].as_str().unwrap_or("");
            if !tools::known(name) {
                return Some(rpc_error(id, -32602, &format!("Unknown tool: {name}")));
            }
            let args = request["params"]
                .get("arguments")
                .cloned()
                .unwrap_or(json!({}));
            let mut audit = match audit::CallAudit::begin(state, &grant.client, name, &args) {
                Ok(a) => a,
                Err(_) => return Some(rpc_error(id, -32603, "Audit unavailable")),
            };
            let result = tools::call(state, grant, name, &args).await;
            let audit = audit.finish(result.is_ok());
            match (result, audit) {
                (Ok(v), Ok(())) => Ok(
                    json!({"content":[{"type":"text","text":v.to_string()}],"structuredContent":v,"isError":false}),
                ),
                (Err(e), _) | (_, Err(e)) => {
                    Ok(json!({"content":[{"type":"text","text":e.to_string()}],"isError":true}))
                }
            }
        }
        _ => return Some(rpc_error(id, -32601, "Method not found")),
    };
    Some(match result {
        Ok(v) => json!({"jsonrpc":"2.0","id":id,"result":v}),
        Err(e) => rpc_error(id, -32603, &format!("{e}")),
    })
}
pub(crate) async fn local(
    state: &Arc<Supervisor>,
    input: &Value,
    caller: Option<&str>,
) -> Result<Value> {
    ensure!(
        caller.is_none(),
        "outside MCP requires the owner CLI credential"
    );
    let access = input["access"].as_str().unwrap_or("supervised");
    ensure!(
        matches!(access, "read" | "supervised" | "full"),
        "invalid access"
    );
    let workspaces = match &input["workspaces"] {
        Value::Null => None,
        Value::Array(ids) => Some(
            ids.iter()
                .map(|id| {
                    let id = id.as_str().context("workspace IDs must be strings")?;
                    // A remote device checks the workspace when it is used.
                    if input["deviceId"].is_null() {
                        state.get_workspace(id)?;
                    }
                    Ok(id.to_owned())
                })
                .collect::<Result<Vec<_>>>()?,
        ),
        _ => anyhow::bail!("workspaces must be a list"),
    };
    let grant = Grant {
        client: "local-stdio".into(),
        access: access.into(),
        workspaces,
        device: input["deviceId"].as_str().map(str::to_owned),
    };
    let request = input["request"].clone();
    let key = |id: &Value| {
        format!(
            "stdio:{}:{}:{}",
            state.config.database_url.display(),
            input["sessionId"],
            id
        )
    };
    if request["method"] == "notifications/cancelled" {
        cancellation::cancel(&key(&request["params"]["requestId"]));
        return Ok(Value::Null);
    }
    if request["method"] == "tools/call" {
        let pending = cancellation::Pending::begin(key(&request["id"]))
            .ok_or_else(|| anyhow::anyhow!("duplicate request id"))?;
        let id = request["id"].clone();
        return Ok(
            tokio::select! {reply=rpc(state,&grant,request)=>reply.unwrap_or(Value::Null),_=pending.token.cancelled()=>rpc_error(id,-32800,"Request cancelled")},
        );
    }
    Ok(rpc(state, &grant, request).await.unwrap_or(Value::Null))
}
fn unauthorized(s: &Supervisor, presented: bool) -> Response {
    let mut r = (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error":"invalid_token"})),
    )
        .into_response();
    if let Ok(v) = format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"{}",
        oauth::issuer(s),
        // RFC 6750 §3.1: a presented token that no longer works is invalid.
        if presented {
            ", error=\"invalid_token\", error_description=\"The access token is not valid here: it expired, its grant was revoked, or it was issued for another address\""
        } else {
            ""
        }
    )
    .parse()
    {
        r.headers_mut().insert("www-authenticate", v);
    }
    r
}
fn origin_valid(s: &Supervisor, h: &HeaderMap) -> bool {
    h.get("origin")
        .is_none_or(|v| v.to_str().ok().is_some_and(|o| o == oauth::issuer(s)))
}
fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(rpc_error(Value::Null, -32600, message)),
    )
        .into_response()
}
async fn http(
    State(s): State<Arc<Supervisor>>,
    h: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    if !origin_valid(&s, &h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok((grant_id, grant)) = oauth::authenticate(&s, &h) else {
        return unauthorized(&s, h.contains_key("authorization"));
    };
    if !request.is_object() {
        return bad_request("Send one JSON-RPC message per request; batches are not supported");
    }
    let init = request["method"] == "initialize";
    let session = h.get("mcp-session-id").and_then(|v| v.to_str().ok());
    if !init {
        let negotiated = store::transaction(&s, |db| {
            let v = store::get(db, "session", session.unwrap_or(""))?;
            ensure!(v["grant"] == grant_id, "wrong session");
            Ok(v["version"].as_str().map(str::to_owned))
        });
        let Ok(negotiated) = negotiated else {
            return StatusCode::NOT_FOUND.into_response();
        };
        // Without the header the session's negotiated version applies.
        let version = h
            .get("mcp-protocol-version")
            .map(|v| v.to_str().unwrap_or(""))
            .or(negotiated.as_deref());
        if version.is_some_and(|v| !VERSIONS.contains(&v)) {
            return bad_request("Unsupported MCP-Protocol-Version");
        }
    }
    let call_key = |id: &Value| {
        format!(
            "{}:{}:{}:{}",
            s.config.database_url.display(),
            grant_id,
            session.unwrap_or(""),
            id
        )
    };
    if request["method"] == "notifications/cancelled" {
        cancellation::cancel(&call_key(&request["params"]["requestId"]));
        return StatusCode::ACCEPTED.into_response();
    }
    let pending = if request["method"] == "tools/call" {
        let Some(pending) = cancellation::Pending::begin(call_key(&request["id"])) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        Some(pending)
    } else {
        None
    };
    let session_id = if init {
        let id = store::secret();
        if store::transaction(&s, |db| {
            store::put(
                db,
                "session",
                &id,
                &json!({"grant":grant_id,"version":negotiate(&request)}),
                86400,
            )
        })
        .is_err()
        {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Some(id)
    } else {
        None
    };
    let id = request["id"].clone();
    let reply = if let Some(pending) = &pending {
        tokio::select! { reply=rpc(&s,&grant,request)=>reply, _=pending.token.cancelled()=>Some(rpc_error(id,-32800,"Request cancelled")) }
    } else {
        rpc(&s, &grant, request).await
    };
    let mut response = match reply {
        Some(v) => Json(v).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    };
    if let Some(id) = session_id {
        response
            .headers_mut()
            .insert("mcp-session-id", id.parse().unwrap());
    }
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}
async fn no_stream(State(s): State<Arc<Supervisor>>, h: HeaderMap) -> Response {
    if !origin_valid(&s, &h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if oauth::authenticate(&s, &h).is_err() {
        return unauthorized(&s, h.contains_key("authorization"));
    }
    StatusCode::METHOD_NOT_ALLOWED.into_response()
}
async fn terminate(State(s): State<Arc<Supervisor>>, h: HeaderMap) -> Response {
    if !origin_valid(&s, &h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok((grant_id, _)) = oauth::authenticate(&s, &h) else {
        return unauthorized(&s, h.contains_key("authorization"));
    };
    let result = store::transaction(&s, |db| {
        let id = h
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let v = store::get(db, "session", id)?;
        ensure!(v["grant"] == grant_id, "wrong session");
        store::delete(db, "session", id)
    });
    if result.is_ok() {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
    .into_response()
}
