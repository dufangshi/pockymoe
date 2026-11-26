//! Outside-agent MCP. JSON response Streamable HTTP; no unsolicited SSE stream.
mod audit;
mod cancellation;
mod events;
mod oauth;
mod remote;
mod store;
#[cfg(test)]
mod tests;
mod tools;

use anyhow::{ensure, Result};
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

pub(crate) fn start(state: &Arc<Supervisor>) {
    events::start(state);
}

pub(crate) fn routes() -> Router<Arc<Supervisor>> {
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
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
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
        "initialize" => {
            let version = request["params"]["protocolVersion"].as_str().unwrap_or("");
            Ok(
                json!({"protocolVersion":if matches!(version,"2025-06-18"|"2025-11-25"){version}else{"2025-11-25"},"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"pockymoe","version":pockymoe_protocol::APP_VERSION}}),
            )
        }
        "ping" => Ok(json!({})),
        "tools/list" => {
            let mut c = tools::catalog();
            if grant.device.is_some() {
                c["tools"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|t| remote::supported(t["name"].as_str().unwrap(), &grant.access));
            }
            Ok(c)
        }
        "tools/call" => {
            let name = request["params"]["name"].as_str().unwrap_or("");
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
    let grant = Grant {
        client: "local-stdio".into(),
        access: access.into(),
        workspaces: None,
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
fn unauthorized(s: &Supervisor) -> Response {
    let mut r = (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error":"invalid_token"})),
    )
        .into_response();
    if let Ok(v) = format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"",
        oauth::issuer(s)
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
async fn http(
    State(s): State<Arc<Supervisor>>,
    h: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    if !origin_valid(&s, &h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok((grant_id, grant)) = oauth::authenticate(&s, &h) else {
        return unauthorized(&s);
    };
    let init = request["method"] == "initialize";
    let session = h.get("mcp-session-id").and_then(|v| v.to_str().ok());
    if !init {
        let valid = store::transaction(&s, |db| {
            let v = store::get(db, "session", session.unwrap_or(""))?;
            ensure!(v["grant"] == grant_id, "wrong session");
            Ok(())
        });
        if valid.is_err() {
            return StatusCode::NOT_FOUND.into_response();
        }
        if !h
            .get("mcp-protocol-version")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| matches!(v, "2025-06-18" | "2025-11-25"))
        {
            return StatusCode::BAD_REQUEST.into_response();
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
            store::put(db, "session", &id, &json!({"grant":grant_id}), 86400)
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
        return unauthorized(&s);
    }
    StatusCode::METHOD_NOT_ALLOWED.into_response()
}
async fn terminate(State(s): State<Arc<Supervisor>>, h: HeaderMap) -> Response {
    if !origin_valid(&s, &h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok((grant_id, _)) = oauth::authenticate(&s, &h) else {
        return unauthorized(&s);
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
