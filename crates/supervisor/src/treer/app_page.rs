//! Metadata-only landing page for the Managed App tunnel. The identity token
//! selects this restricted response; it never grants access to thread routes.
use super::*;
use axum::{http::Method, response::Html};

const OPEN: &str = "Use a Pockymoe Agent to open conversations";
const STOP: &str = "Stopping this App takes every Pockymoe Agent on this machine offline; they reconnect when it starts again";

pub(super) fn route(hub: &Hub, request: &Request) -> Option<Response> {
    let headers = request.headers();
    if !request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|peer| peer.0.ip().is_loopback())
        || headers.get_all("x-treer-identity-token").iter().count() != 1
        || headers["x-treer-identity-token"].is_empty()
        || headers.keys().any(|key| {
            (key.as_str().starts_with("x-treer-") && key.as_str() != "x-treer-identity-token")
                || key.as_str().starts_with("x_treer_")
        })
    {
        return None;
    }
    // Deliberately do not parse, echo, or verify the token: this page exposes no
    // privileged data, and every route is terminated here, including WebSockets.
    let read = matches!(*request.method(), Method::GET | Method::HEAD);
    let path = request.uri().path();
    let machine: String = hub
        .state
        .db
        .with(|c| {
            Ok(c.query_row(
                "SELECT hostname FROM hosts WHERE id=?1",
                [&hub.state.db.host_id],
                |row| row.get(0),
            )?)
        })
        .unwrap_or_else(|_| "unknown".into());
    let bound: u64 = hub
        .state
        .db
        .with(|c| Ok(c.query_row("SELECT COUNT(*) FROM treer_bindings", [], |row| row.get(0))?))
        .unwrap_or(0);
    let version = &hub.state.config.app_version;
    let mut response = if read && matches!(path, "/" | "/index.html") {
        Html(format!(
            "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
             <title>Pockymoe hub</title><style>\
             :root{{color-scheme:light dark;font:16px system-ui}}\
             body{{max-width:42rem;margin:12vh auto;padding:0 1.5rem;line-height:1.6}}\
             dt{{font-weight:600}}dd{{margin:0 0 1rem}}\
             </style><main><h1>Pockymoe hub</h1><dl><dt>App</dt><dd>pockymoe</dd>\
             <dt>Machine</dt><dd>{}</dd><dt>Running version</dt><dd>{}</dd>\
             <dt>Bound Agents</dt><dd>{bound}</dd></dl><p>{OPEN}.</p><p>{STOP}.</p></main></html>",
            escape(&machine),
            escape(version),
        ))
        .into_response()
    } else {
        let status = if read && matches!(path, "/api/status" | "/v1/status" | "/healthz") {
            StatusCode::OK
        } else {
            StatusCode::FORBIDDEN
        };
        (status, Json(json!({
            "status": "running", "appName": "pockymoe", "machine": machine,
            "runningVersion": version, "boundAgents": bound,
            "message": OPEN, "stopWarning": STOP,
            "code": if status == StatusCode::OK { "hub_app_status" } else { "hub_app_read_only" }
        }))).into_response()
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        "default-src 'none'; style-src 'unsafe-inline'"
            .parse()
            .unwrap(),
    );
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    Some(response)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
