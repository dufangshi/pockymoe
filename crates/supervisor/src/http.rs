use std::path::{Component, Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::body::Bytes;
use axum::extract::multipart::Multipart;
use axum::extract::{DefaultBodyLimit, FromRequest, Path, Query, Request, State, WebSocketUpgrade};
use axum::http::{header, Method, StatusCode};
use axum::http::{HeaderMap, HeaderValue, Uri};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use pockymoe_protocol::{
    now_rfc3339, ApiError, AuthSessionDto, CreateThreadInput, CreateWorkspaceInput,
    ForkThreadInput, HarnessResourceSettingsDto, ImportThreadInput, PlatformCapabilitiesDto,
    Provider, ReleaseThreadInput, ReleaseThreadResultDto, RuntimeConfigDto, SendThreadPromptInput,
    ThreadWorkspaceTreeNodeDto, UpdateWorkspaceSettingsInput, VersionDto, APP_NAME, APP_VERSION,
};
use pockymoe_runtime::files::WorkspaceDownload;
use pockymoe_runtime::{Supervisor, UploadedPromptAttachment};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tower_http::compression::predicate::{Predicate, SizeAbove};
use tower_http::compression::{CompressionLayer, CompressionLevel};
use tower_http::cors::{AllowOrigin, CorsLayer};

pub(crate) type AppState = Arc<Supervisor>;

const MAX_PROMPT_ATTACHMENTS: usize = 10;
const MAX_PROMPT_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;
const MAX_WORKSPACE_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptAttachmentManifestEntry {
    kind: String,
    original_name: String,
    placeholder: String,
}

#[derive(Deserialize)]
struct UpdatePluginInput {
    enabled: bool,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportTranscriptQuery {
    mode: Option<String>,
    limit: Option<usize>,
    turn_ids: Option<String>,
    profile: Option<String>,
    include_token_and_price: Option<bool>,
    include_command_output: Option<bool>,
    include_absolute_paths: Option<bool>,
}

pub fn router(state: AppState) -> Router {
    let hub =
        crate::treer::Hub::new(state.clone(), state.config.port).expect("initialize Treer hub");
    router_with_treer(state, hub)
}
pub(crate) fn router_with_treer(state: AppState, hub: Option<Arc<crate::treer::Hub>>) -> Router {
    state.start_interaction_worker();
    crate::peer_send::start_outbox_worker(&state);
    let router = Router::new()
        .merge(crate::treer::routes())
        .route(
            "/api/transport/bind",
            post(crate::owner_trust::bind_endpoint),
        )
        .route(
            "/api/transport/browser",
            post(crate::owner_trust::browser_endpoint),
        )
        .route(
            "/api/transport/enrollment",
            get(crate::owner_trust::enrollment_endpoint),
        )
        .route(
            "/api/transport/device-certificate",
            put(crate::owner_trust::certificate_endpoint),
        )
        .route(
            "/api/transport/revocations",
            put(crate::owner_trust::revocations_endpoint),
        )
        .route(
            "/api/transport/trust",
            get(crate::owner_trust::trust_endpoint),
        )
        .route("/api/transport/grants", get(crate::grants::list_endpoint))
        .route(
            "/api/transport/grants/{grantId}",
            put(crate::grants::put_endpoint).delete(crate::grants::delete_endpoint),
        )
        .route("/api/cli", post(crate::interaction::command))
        .route(
            "/api/threads/{thread}/plugins/{plugin}/actions/{action}",
            post(crate::plugins::action),
        )
        // Other devices of the same owner, through the relay (docs/cross-device-peer.zh.md).
        .route(
            "/api/peer/transport/certificate",
            post(crate::owner_trust::peer_certificate_endpoint),
        )
        .route("/api/peer/cli", post(crate::peer_api::cli))
        .route(
            "/api/peer/files/uploads",
            post(crate::peer_files::upload_begin),
        )
        .route(
            "/api/peer/files/uploads/{id}",
            axum::routing::put(crate::peer_files::upload_chunk),
        )
        .route(
            "/api/peer/files/uploads/{id}/commit",
            post(crate::peer_files::upload_commit),
        )
        .route("/api/peer/files/list", post(crate::peer_files::list))
        .route("/api/peer/files/stat", post(crate::peer_files::stat))
        .route("/api/peer/files/read", get(crate::peer_files::read))
        .route(
            "/api/config/peer-access",
            get(crate::peer_api::access).patch(crate::peer_api::set_access),
        )
        .route("/healthz", get(healthz))
        .route("/readyz", get(healthz))
        .route("/api/version", get(version))
        .route("/api/device/metrics", get(device_metrics))
        .route("/api/device/processes", get(device_processes))
        .route(
            "/api/harness-resources/settings",
            get(harness_resource_settings).put(put_harness_resource_settings),
        )
        .route(
            "/api/port-mappings",
            get(crate::ports::list).post(crate::ports::create),
        )
        .route("/api/port-mappings/{id}", delete(crate::ports::remove))
        .route(
            "/api/port-mappings/{id}/lease",
            post(crate::ports::lease_endpoint),
        )
        .route(
            "/api/management/harnesses",
            get(crate::management::harnesses),
        )
        .route("/api/management/jobs", get(crate::management::jobs))
        .route(
            "/api/management/upstreams",
            get(crate::upstreams::list).post(crate::upstreams::save),
        )
        .route(
            "/api/management/upstreams/{id}",
            post(crate::upstreams::action).delete(crate::upstreams::delete),
        )
        .route(
            "/api/management/upstreams/import",
            post(crate::upstreams::import_config),
        )
        .route(
            "/api/management/upstreams/live",
            post(crate::upstreams::live),
        )
        .route(
            "/api/management/upstreams/models",
            post(crate::upstreams::models),
        )
        .route(
            "/api/management/templates",
            post(crate::upstreams::template),
        )
        .route(
            "/api/management/harnesses/{id}",
            post(crate::management::harness_action),
        )
        .route(
            "/api/management/supervisor",
            get(crate::management::supervisor_status),
        )
        .route(
            "/api/management/supervisor/check",
            post(crate::management::supervisor_check),
        )
        .route(
            "/api/management/supervisor/restart",
            post(crate::management::supervisor_restart),
        )
        .route(
            "/api/management/supervisor/update",
            post(crate::management::supervisor_update),
        )
        .route("/api/config/runtime", get(runtime_config))
        .route(
            "/api/agent-runtimes/{provider}/subscription-usage",
            get(subscription_usage),
        )
        .route(
            "/api/config/model-pricing",
            get(model_pricing).patch(patch_model_pricing),
        )
        .route(
            "/api/config/workspace-settings",
            get(workspace_settings).patch(patch_workspace_settings),
        )
        .route("/api/auth/session", get(auth_session))
        .route("/api/auth/login", post(auth_login))
        .route("/api/auth/logout", post(auth_logout))
        .route(
            "/api/auth/passcode",
            put(auth_set_passcode).delete(auth_remove_passcode),
        )
        .route("/api/agent-runtimes", get(agent_runtimes))
        .route("/api/agent-runtimes/{provider}/status", get(agent_status))
        .route("/api/agent-runtimes/{provider}/models", get(agent_models))
        .route("/api/agent-runtimes/{provider}/harness", get(agent_harness))
        .route("/api/agent-runtimes/{provider}/agents", get(agent_agents))
        .route(
            "/api/agent-runtimes/{provider}/capabilities",
            get(agent_caps),
        )
        .route(
            "/api/agent-runtimes/{provider}/install",
            post(agent_install),
        )
        .route(
            "/api/agent-runtimes/{provider}/restart",
            post(agent_restart),
        )
        .route(
            "/api/workspaces",
            get(list_workspaces).post(create_workspace),
        )
        .route(
            "/api/workspaces/{id}",
            get(get_workspace)
                .patch(patch_workspace)
                .delete(delete_workspace),
        )
        .route("/api/workspaces/{id}/favorite", post(favorite_workspace))
        .route("/api/workspaces/{id}/open", post(open_workspace))
        .route(
            "/api/workspaces/{id}/files/capabilities",
            get(crate::file_documents::capabilities),
        )
        .route(
            "/api/workspaces/{id}/files/document",
            get(crate::file_documents::document),
        )
        .route(
            "/api/workspaces/{id}/files/save",
            post(crate::file_documents::save)
                .layer(axum::extract::DefaultBodyLimit::max(400 * 1024)),
        )
        .route(
            "/api/workspaces/{id}/files/operations/{op}",
            get(crate::file_documents::operation),
        )
        .route("/api/workspaces/{id}/files/tree", get(workspace_tree))
        .route("/api/workspaces/{id}/files/preview", get(workspace_preview))
        .route("/api/workspaces/{id}/files/raw", get(workspace_raw))
        .route(
            "/api/workspaces/{id}/files",
            axum::routing::put(workspace_write)
                .post(workspace_create_file)
                .delete(workspace_delete_file),
        )
        .route("/api/threads", get(list_threads))
        .route("/api/threads/start", post(start_thread))
        .route("/api/threads/import", post(import_thread))
        .route("/api/threads/import-candidates", get(import_candidates))
        .route(
            "/api/threads/{id}",
            get(get_thread).patch(rename_thread).delete(delete_thread),
        )
        .route(
            "/api/threads/{id}/automations",
            get(crate::automations::list).post(crate::automations::create),
        )
        .route(
            "/api/threads/{id}/automations/preview",
            post(crate::automations::preview),
        )
        .route(
            "/api/threads/{id}/automations/{automationId}",
            get(crate::automations::show),
        )
        .route(
            "/api/threads/{id}/automations/{automationId}/runs",
            get(crate::automations::runs),
        )
        .route(
            "/api/threads/{id}/automations/{automationId}/{operation}",
            post(crate::automations::control),
        )
        .route(
            "/api/threads/{id}/commands",
            post(crate::automations::command_run),
        )
        .route(
            "/api/threads/{id}/commands/{commandId}",
            get(crate::automations::command_show),
        )
        .route("/api/threads/{id}/search", get(thread_search))
        .route("/api/search", get(device_search))
        .route("/api/workspaces/{id}/search", get(workspace_search))
        .route(
            "/api/threads/{id}/turns/{turnId}/detail",
            get(thread_turn_detail),
        )
        .route(
            "/api/threads/{id}/items/{itemId}/detail",
            get(thread_item_detail),
        )
        .route("/api/threads/{id}/assets/image", get(thread_image))
        .route(
            "/api/threads/{id}/linked-files/{action}",
            get(crate::linked_files::read),
        )
        .route("/api/threads/{id}/settings", patch(thread_settings))
        .route("/api/threads/{id}/prompt", post(thread_prompt))
        .route("/api/threads/{id}/interrupt", post(thread_interrupt))
        .route("/api/threads/{id}/release", post(thread_release))
        .route("/api/threads/{id}/resume", post(thread_resume))
        .route("/api/threads/{id}/disconnect", post(thread_disconnect))
        .route("/api/threads/{id}/models", get(thread_models))
        .route("/api/threads/{id}/group", get(thread_group))
        .route("/api/threads/{id}/watches", get(thread_watches))
        .route("/api/threads/{id}/subagents", get(thread_subagents))
        .route(
            "/api/threads/{id}/subagents/{agentId}",
            get(thread_subagent_detail),
        )
        .route("/api/threads/{id}/fork-turns", get(thread_fork_turns))
        .route("/api/threads/{id}/capabilities", get(thread_capabilities))
        .route("/api/threads/{id}/harness", post(thread_harness_action))
        .route("/api/threads/{id}/fork", post(thread_fork))
        .route("/api/threads/{id}/compact", post(thread_compact))
        .route(
            "/api/threads/{id}/goal",
            get(thread_goal)
                .patch(thread_goal)
                .delete(thread_goal_clear),
        )
        .route("/api/threads/{id}/skills", get(thread_skills))
        .route("/api/threads/{id}/mcp-servers", get(thread_mcp))
        .route("/api/threads/{id}/hooks", get(thread_hooks))
        .route(
            "/api/threads/{id}/requests/{requestId}/respond",
            post(thread_respond),
        )
        .route(
            "/api/threads/{id}/pending-steers/{pendingSteerId}",
            delete(cancel_pending_steer),
        )
        .route(
            "/api/threads/{id}/pending-steers/{pendingSteerId}/steer",
            post(steer_pending_prompt),
        )
        .route("/api/threads/{id}/export-turns", get(export_turns))
        .route("/api/threads/{id}/publications", post(create_publication))
        .route(
            "/api/publications/{token}",
            get(read_publication).delete(revoke_publication),
        )
        .route("/api/threads/{id}/exports/html", get(export_html))
        .route(
            "/api/threads/{id}/shell",
            get(thread_shell).post(create_shell),
        )
        .route("/api/shells/{id}", patch(update_shell))
        .route("/api/shells/{id}/terminate", post(terminate_shell))
        .route(
            "/api/workspaces/{id}/files/download",
            get(workspace_download),
        )
        .route("/api/workspaces/{id}/files/upload", post(workspace_upload))
        .route("/api/workspaces/{id}/files/move", patch(workspace_move))
        .route("/api/plugins", get(list_plugins))
        .route("/api/plugins/import", post(import_plugin_unsupported))
        .route(
            "/api/plugins/{id}/settings",
            get(crate::plugins::settings).put(crate::plugins::save_settings),
        )
        .route(
            "/api/plugins/{id}",
            get(get_plugin).patch(update_plugin).delete(delete_plugin),
        )
        .route("/ws", get(ws_upgrade))
        .fallback(spa_fallback)
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_auth,
        ))
        .with_state(state)
        .layer(middleware::from_fn_with_state(hub, crate::treer::gate))
        .layer(compression_layer());
    match webview_cors_layer() {
        Some(layer) => router.layer(layer),
        None => router,
    }
}

/// Text and JSON bodies are compressed for clients that send Accept-Encoding.
/// WebSocket upgrades, SSE, images, archives and ranges stay identity. Relay
/// requests are dispatched in-process without Accept-Encoding and compressed
/// inside the encrypted envelope instead (secure_transport.rs).
pub(crate) fn compression_layer() -> CompressionLayer<impl Predicate> {
    CompressionLayer::new()
        .no_deflate()
        .no_zstd()
        // Brotli's default quality 11 costs ~100 ms per MB; 4 compresses JSON
        // about as well as gzip -6 in a fraction of the time.
        .quality(CompressionLevel::Precise(4))
        .compress_when(SizeAbove::new(1024).and(
            |status: StatusCode,
             _: axum::http::Version,
             headers: &HeaderMap,
             _: &axum::http::Extensions| {
                status != StatusCode::SWITCHING_PROTOCOLS
                    && headers
                        .get(header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .is_some_and(compressible_content_type)
            },
        ))
}

pub(crate) fn compressible_content_type(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    (essence.starts_with("text/") && essence != "text/event-stream")
        || matches!(
            essence.as_str(),
            "application/json"
                | "application/javascript"
                | "application/xml"
                | "application/manifest+json"
                | "image/svg+xml"
        )
        || essence.ends_with("+json")
        || essence.ends_with("+xml")
}

fn webview_cors_layer() -> Option<CorsLayer> {
    if std::env::var("POCKYMOE_ENABLE_WEBVIEW_CORS").as_deref() != Ok("true") {
        return None;
    }
    let configured = std::env::var("POCKYMOE_WEBVIEW_CORS_ORIGINS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .filter_map(|value| value.trim().parse::<HeaderValue>().ok())
                .collect::<Vec<_>>()
        })
        .filter(|origins| !origins.is_empty());
    let origins = configured.unwrap_or_else(|| {
        [
            "null",
            "capacitor://localhost",
            "ionic://localhost",
            "http://localhost",
            "https://localhost",
            "https://appassets.androidplatform.net",
        ]
        .into_iter()
        .map(HeaderValue::from_static)
        .collect()
    });
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PATCH,
                Method::PUT,
                Method::DELETE,
            ])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
            .max_age(std::time::Duration::from_secs(600)),
    )
}

fn configured_web_dist() -> Option<PathBuf> {
    std::env::var("POCKYMOE_WEB_DIST_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            let candidate = PathBuf::from("apps/supervisor-web/dist");
            candidate.join("index.html").is_file().then_some(candidate)
        })
        // `pockymoe start` from an installed release serves that release's UI.
        .or_else(crate::distribution::installed_web_dist)
}

fn safe_static_path(dist: &FsPath, uri_path: &str) -> Option<PathBuf> {
    let relative = uri_path.trim_start_matches('/');
    let requested = FsPath::new(relative);
    if requested
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return None;
    }
    Some(dist.join(requested))
}

pub(crate) fn static_content_type(path: &FsPath) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
    {
        "css" => "text/css; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "ico" => "image/x-icon",
        "jpeg" | "jpg" => "image/jpeg",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "webmanifest" => "application/json; charset=utf-8",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

async fn spa_fallback(State(state): State<AppState>, request: Request) -> Response {
    if request.uri().path() == "/api"
        || request.uri().path().starts_with("/api/")
        || request.uri().path() == "/ws"
    {
        return err(StatusCode::NOT_FOUND, "not_found", "Route not found").into_response();
    }
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    if state.config.mode == pockymoe_protocol::Mode::Relay {
        return err(
            StatusCode::NOT_FOUND,
            "not_found",
            "Web UI is served by the Relay, not by the device supervisor",
        )
        .into_response();
    }
    let Some(dist) = configured_web_dist() else {
        return err(
            StatusCode::NOT_FOUND,
            "not_found",
            "Web UI is not installed",
        )
        .into_response();
    };
    let Some(candidate) = safe_static_path(&dist, request.uri().path()) else {
        return err(StatusCode::BAD_REQUEST, "bad_request", "Invalid asset path").into_response();
    };
    let path = if candidate.is_file() {
        candidate
    } else {
        dist.join("index.html")
    };
    let Ok(mut bytes) = tokio::fs::read(&path).await else {
        return err(
            StatusCode::NOT_FOUND,
            "not_found",
            "Web UI is not installed",
        )
        .into_response();
    };
    if path.file_name().and_then(|value| value.to_str()) == Some("index.html") {
        bytes = spa_index_with_base(&bytes, request.uri().path());
    }
    let cache_control = if path.file_name().and_then(|value| value.to_str()) == Some("index.html") {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    Response::builder()
        .status(StatusCode::OK)
        // The installed Web UI is the app itself: its scripts must run, so it keeps
        // real content types and no sandbox (unlike raw workspace documents).
        .header(header::CONTENT_TYPE, static_content_type(&path))
        .header("referrer-policy", "no-referrer")
        .header(header::CACHE_CONTROL, cache_control)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(if request.method() == Method::HEAD {
            Body::empty()
        } else {
            Body::from(bytes)
        })
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// The proxy removes its mount prefix. A relative base ascends only the route
// segments seen here, leaving that external prefix intact in the browser.
fn spa_index_with_base(bytes: &[u8], request_path: &str) -> Vec<u8> {
    let depth = request_path
        .bytes()
        .filter(|byte| *byte == b'/')
        .count()
        .saturating_sub(1);
    let base = if depth == 0 {
        "./".to_owned()
    } else {
        "../".repeat(depth)
    };
    String::from_utf8_lossy(bytes)
        .replace(
            "<base href=\"./\" data-pockymoe-base />",
            &format!("<base href=\"{base}\" data-pockymoe-base />"),
        )
        .into_bytes()
}

#[cfg(test)]
mod web_base_tests {
    use super::spa_index_with_base;

    #[test]
    fn spa_base_ascends_only_the_stripped_route() {
        let index =
            b"<base href=\"./\" data-pockymoe-base /><script src=\"./assets/app.js\"></script>";
        for (route, base) in [
            ("/", "./"),
            ("/index.html", "./"),
            ("/workspaces", "./"),
            ("/threads/t", "../"),
            ("/threads/t/", "../../"),
            ("/devices/d/threads/t", "../../../"),
        ] {
            let html = String::from_utf8(spa_index_with_base(index, route)).unwrap();
            assert!(
                html.contains(&format!("href=\"{base}\"")),
                "{route}: {html}"
            );
            assert!(html.contains("src=\"./assets/app.js\""));
        }
        assert_eq!(
            spa_index_with_base(b"<base href=\"/injected/\">", "/threads/t"),
            b"<base href=\"/injected/\">"
        );
    }
}

pub(crate) struct ApiErr(StatusCode, ApiError);

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

pub(crate) fn err(status: StatusCode, code: &str, message: impl Into<String>) -> ApiErr {
    ApiErr(status, ApiError::new(code, message))
}

pub(crate) fn map_err(e: anyhow::Error) -> ApiErr {
    let message = e.to_string();
    if e.is::<pockymoe_runtime::host_mode::FeatureDisabled>() {
        err(
            StatusCode::FORBIDDEN,
            pockymoe_runtime::host_mode::FEATURE_DISABLED_CODE,
            message,
        )
    } else if message.contains("database disk image is malformed")
        || message.contains("SQLITE_CORRUPT")
    {
        err(StatusCode::SERVICE_UNAVAILABLE, "harness_storage_corrupt",
            "The agent's local database is damaged. Reconnecting or retrying the prompt cannot repair it. Back up and recover the agent database before resuming this thread.")
    } else if message.contains("already has an active writer") {
        err(StatusCode::CONFLICT, "harness_session_in_use",
            "This Codex thread is open in another app or CLI session. Close that session, then reconnect here. Importing history does not enable simultaneous editing from separate Codex processes.")
    } else if message.starts_with("forbidden: ") {
        err(
            StatusCode::FORBIDDEN,
            "forbidden",
            message.strip_prefix("forbidden: ").unwrap_or(&message),
        )
    } else if message.contains("not found") {
        err(StatusCode::NOT_FOUND, "not_found", message)
    } else if message.contains("Resume / Connect") || message.starts_with("conflict: ") {
        err(
            StatusCode::CONFLICT,
            "conflict",
            message.strip_prefix("conflict: ").unwrap_or(&message),
        )
    } else if message.contains("not installed") || message.contains("not enabled") {
        err(StatusCode::BAD_REQUEST, "harness_unavailable", message)
    } else {
        err(StatusCode::BAD_REQUEST, "bad_request", message)
    }
}

async fn healthz(State(state): State<AppState>) -> Json<Value> {
    Json(
        json!({"hostMode":state.config.host_mode,"hostCapabilities":state.host_capabilities,"relayStatus":crate::owner_trust::relay_status(&state),"status":"ok", "timestamp":now_rfc3339(), "activeTurnCount":state.active_turn_count(), "runningVersion":APP_VERSION, "processId":std::process::id(), "relayConnected":state.relay_connected.load(std::sync::atomic::Ordering::SeqCst)}),
    )
}

async fn version() -> Json<VersionDto> {
    Json(VersionDto {
        name: APP_NAME.into(),
        version: APP_VERSION.into(),
    })
}

async fn device_metrics(State(state): State<AppState>) -> Response {
    match state.device_monitor.snapshot().await {
        Ok(metrics) => (
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            Json(metrics),
        )
            .into_response(),
        Err(error) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metrics_unavailable",
            error.to_string(),
        )
        .into_response(),
    }
}

async fn device_processes(State(state): State<AppState>) -> Response {
    match state.harness_processes().await {
        Ok(processes) => (
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            Json(processes),
        )
            .into_response(),
        Err(error) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "processes_unavailable",
            error.to_string(),
        )
        .into_response(),
    }
}

fn settings_storage_error(error: anyhow::Error) -> ApiErr {
    err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "settings_unavailable",
        error.to_string(),
    )
}

async fn harness_resource_settings(
    State(state): State<AppState>,
) -> Result<Json<HarnessResourceSettingsDto>, ApiErr> {
    Ok(Json(
        state
            .harness_resource_settings()
            .map_err(settings_storage_error)?,
    ))
}

/// The body replaces the policy, so every key must be present: a missing
/// one must not silently turn a rule off.
async fn put_harness_resource_settings(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<HarnessResourceSettingsDto>, ApiErr> {
    let invalid = |message: String| err(StatusCode::BAD_REQUEST, "invalid_settings", message);
    let value: Value = serde_json::from_slice(&body).map_err(|e| invalid(e.to_string()))?;
    for key in [
        "idleReleaseMinutes",
        "maxLoadedThreads",
        "maxHarnessMemoryBytes",
    ] {
        if value.get(key).is_none() {
            return Err(invalid(format!(
                "{key} is required (null turns the rule off)"
            )));
        }
    }
    let settings: HarnessResourceSettingsDto =
        serde_json::from_value(value).map_err(|e| invalid(e.to_string()))?;
    state
        .update_harness_resource_settings(settings)
        .map(Json)
        .map_err(|e| {
            if e.is::<pockymoe_runtime::service::InvalidSettings>() {
                invalid(e.to_string())
            } else {
                settings_storage_error(e)
            }
        })
}

async fn runtime_config(
    State(state): State<AppState>,
    scope: Option<axum::Extension<crate::treer::Scope>>,
) -> Json<Value> {
    let mut config = serde_json::to_value(RuntimeConfigDto {
        app_name: state.config.app_name.clone(),
        app_version: state.config.app_version.clone(),
        mode: state.config.mode,
        host: state.config.host.clone(),
        port: state.config.port,
        workspace_root: state.config.workspace_root.to_string_lossy().into(),
        environment: state.config.environment.clone(),
        platform: Some(std::env::consts::OS.into()),
        architecture: Some(std::env::consts::ARCH.into()),
        capabilities: Some(PlatformCapabilitiesDto {
            terminal: cfg!(unix),
            tmux: which_tmux(),
            managed_signals: cfg!(unix),
            windows_task_scheduler: cfg!(windows),
        }),
    })
    .expect("runtime config is serializable");
    config["hostMode"] = json!(state.config.host_mode);
    config["hostCapabilities"] = json!(state.host_capabilities);
    if let Some(scope) = scope {
        config["hostScope"] = json!({
            "kind": "treerAgent",
            "boundThreadId": scope.thread,
            "hubId": scope.hub.hub_id,
            "allowed": {
                "boundThreads": true,
                "childThreads": true,
                "topLevelThreads": true,
                "workspaceCreation": false,
                "management": false,
                "workspaceFiles": "read"
            }
        });
    }
    Json(config)
}

fn which_tmux() -> bool {
    #[cfg(windows)]
    return false;
    #[cfg(not(windows))]
    std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn workspace_settings(State(state): State<AppState>) -> Json<Value> {
    Json(serde_json::to_value(state.workspace_settings()).unwrap_or(json!({})))
}

async fn patch_workspace_settings(
    State(state): State<AppState>,
    Json(body): Json<UpdateWorkspaceSettingsInput>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(state.update_workspace_settings(body).map_err(map_err)?)
            .unwrap_or(json!({})),
    ))
}

async fn auth_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Json<AuthSessionDto> {
    Json(crate::auth::verify_request(&state, &headers, &uri))
}

#[derive(Deserialize)]
struct LoginInput {
    username: Option<String>,
    password: Option<String>,
}

/// Recent failed sign-ins, across all clients: ten within a minute pause sign-in.
fn sign_in_failures() -> &'static std::sync::Mutex<Vec<std::time::Instant>> {
    static FAILURES: std::sync::OnceLock<std::sync::Mutex<Vec<std::time::Instant>>> =
        std::sync::OnceLock::new();
    FAILURES.get_or_init(Default::default)
}

async fn auth_login(
    State(state): State<AppState>,
    Json(body): Json<LoginInput>,
) -> Result<Response, ApiErr> {
    let passcode = crate::auth::local_passcode(&state).is_some();
    if passcode {
        let mut failures = sign_in_failures().lock().unwrap();
        failures.retain(|at| at.elapsed() < std::time::Duration::from_secs(60));
        if failures.len() >= 10 {
            return Err(err(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Too many wrong passcodes. Wait a minute and try again.",
            ));
        }
    }
    let signed_in = if passcode {
        crate::auth::login_passcode(&state, body.password.as_deref().unwrap_or_default())
    } else {
        crate::auth::login(
            &state.config,
            body.username.as_deref().unwrap_or_default(),
            body.password.as_deref().unwrap_or_default(),
        )
    };
    let Some((token, session)) = signed_in else {
        if passcode {
            sign_in_failures()
                .lock()
                .unwrap()
                .push(std::time::Instant::now());
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            return Err(err(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Wrong passcode.",
            ));
        }
        return Err(err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid username or password.",
        ));
    };
    Ok(signed_in_response(token, session))
}

fn signed_in_response(token: String, session: AuthSessionDto) -> Response {
    let mut response = Json(json!({
        "token": if token.is_empty() { Value::Null } else { json!(token) },
        "session": session,
    }))
    .into_response();
    if !token.is_empty() {
        response.headers_mut().insert(
            header::SET_COOKIE,
            crate::auth::session_cookie(&token)
                .parse()
                .expect("valid session cookie"),
        );
    }
    response
}

#[derive(Deserialize)]
struct PasscodeInput {
    passcode: String,
}

/// Set or change the local-mode passcode; this browser stays signed in.
async fn auth_set_passcode(
    State(state): State<AppState>,
    Json(body): Json<PasscodeInput>,
) -> Result<Response, ApiErr> {
    let (token, session) = crate::auth::set_passcode(&state, Some(&body.passcode))
        .map_err(|error| {
            err(
                StatusCode::BAD_REQUEST,
                "invalid_passcode",
                &error.to_string(),
            )
        })?
        .ok_or_else(|| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Could not sign in",
            )
        })?;
    Ok(signed_in_response(token, session))
}

async fn auth_remove_passcode(State(state): State<AppState>) -> Result<Response, ApiErr> {
    crate::auth::set_passcode(&state, None).map_err(|error| {
        err(
            StatusCode::BAD_REQUEST,
            "invalid_passcode",
            &error.to_string(),
        )
    })?;
    let mut response = Json(crate::auth::local_session(&state.config)).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        crate::auth::clear_session_cookie()
            .parse()
            .expect("valid cleared session cookie"),
    );
    Ok(response)
}

async fn auth_logout(State(state): State<AppState>) -> Response {
    let mut response = Json(crate::auth::unauthenticated_session(&state)).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        crate::auth::clear_session_cookie()
            .parse()
            .expect("valid cleared session cookie"),
    );
    response
}

async fn agent_runtimes(State(state): State<AppState>) -> Json<Vec<Value>> {
    Json(
        state
            .backends()
            .into_iter()
            .filter_map(|b| serde_json::to_value(b).ok())
            .collect(),
    )
}

fn parse_provider(raw: &str) -> Result<Provider, ApiErr> {
    match raw {
        "codex" => Ok(Provider::Codex),
        "claude" => Ok(Provider::Claude),
        "opencode" => Ok(Provider::Opencode),
        "acp" => Ok(Provider::Acp),
        _ => Err(err(StatusCode::NOT_FOUND, "not_found", "unknown provider")),
    }
}

async fn agent_status(
    Path(provider): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let provider = parse_provider(&provider)?;
    let backend = state
        .backends()
        .into_iter()
        .find(|b| b.provider == provider)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "provider missing"))?;
    Ok(Json(serde_json::to_value(backend).unwrap_or(json!({}))))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentQuery {
    agent_id: Option<String>,
    cwd: Option<String>,
}

async fn agent_models(
    Path(provider): Path<String>,
    Query(query): Query<AgentQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let provider = parse_provider(&provider)?;
    let models = state
        .list_models(provider, query.agent_id.as_deref(), query.cwd.as_deref())
        .await
        .map_err(map_err)?;
    Ok(Json(serde_json::to_value(models).unwrap_or(json!([]))))
}

async fn agent_harness(
    Path(provider): Path<String>,
    Query(query): Query<AgentQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let provider = parse_provider(&provider)?;
    let catalog = state
        .harness_catalog(provider, query.agent_id.as_deref(), query.cwd.as_deref())
        .await
        .map_err(map_err)?;
    Ok(Json(catalog))
}

async fn agent_agents(
    Path(provider): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let provider = parse_provider(&provider)?;
    let agents = state.list_agents(provider).await.map_err(map_err)?;
    Ok(Json(serde_json::to_value(agents).unwrap_or(json!([]))))
}

#[derive(Deserialize)]
struct CapabilitiesQuery {
    load: Option<bool>,
}

async fn thread_capabilities(
    Path(id): Path<String>,
    Query(query): Query<CapabilitiesQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let caps = state
        .thread_capabilities(&id, query.load.unwrap_or(true))
        .await
        .map_err(map_err)?;
    Ok(Json(serde_json::to_value(caps).unwrap_or(json!({}))))
}

async fn thread_harness_action(
    Path(id): Path<String>,
    State(state): State<AppState>,
    owner: Option<axum::Extension<crate::owner_trust::BrowserPrincipal>>,
    forwarded: Option<axum::Extension<crate::auth::TrustedRelayForward>>,
    Json(action): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    let console = action["kind"] == "console";
    let mut result = state
        .thread_harness_action(&id, action)
        .await
        .map_err(map_err)?;
    // Relay browsers reach the console's loopback port through a preview.
    let port = result["console"]["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok());
    if let (true, true, Some(port)) = (
        console,
        state.host_capabilities.previews && state.config.relay_server_url.is_some(),
        port,
    ) {
        let mapping = crate::ports::dsh_console_mapping(
            &state,
            &id,
            port,
            crate::ports::MappingAuthority::for_request(
                forwarded.as_ref().map(|f| &f.0),
                owner.as_ref().map(|p| &p.0),
            ),
        )
        .await
        .map_err(map_err)?;
        let authority = crate::ports::MappingAuthority::for_request(
            forwarded.as_ref().map(|f| &f.0),
            owner.as_ref().map(|p| &p.0),
        );
        if !matches!(authority, crate::ports::MappingAuthority::Tunnel(None)) {
            let lease =
                crate::ports::create_lease(&state, &mapping.id, authority).map_err(map_err)?;
            result["console"]["leaseId"] = json!(lease.lease_id);
            result["console"]["expiresAt"] = json!(lease.expires_at);
        }
        result["console"]["mappingId"] = json!(mapping.id);
    }
    Ok(Json(result))
}

async fn agent_caps(
    Path(provider): Path<String>,
    Query(query): Query<AgentQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let provider = parse_provider(&provider)?;
    let caps = state
        .capabilities(provider, query.agent_id.as_deref())
        .await
        .map_err(map_err)?;
    Ok(Json(serde_json::to_value(caps).unwrap_or(json!({}))))
}

async fn agent_install(
    Path(provider): Path<String>,
    Query(query): Query<AgentQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let provider = parse_provider(&provider)?;
    let dto = state
        .install(provider, query.agent_id.as_deref())
        .await
        .map_err(map_err)?;
    Ok(Json(serde_json::to_value(dto).unwrap_or(json!({}))))
}

async fn agent_restart(
    Query(query): Query<AgentQuery>,
    Path(provider): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let provider = parse_provider(&provider)?;
    let id = query
        .agent_id
        .as_deref()
        .unwrap_or(if provider == Provider::Acp {
            "codex"
        } else {
            provider.as_str()
        });
    let _maintenance = state.maintenance_gate.try_read().map_err(|_| {
        err(
            StatusCode::CONFLICT,
            "conflict",
            "Supervisor update in progress",
        )
    })?;
    let _guard = state
        .harness_gate(id)
        .try_write_owned()
        .map_err(|_| err(StatusCode::CONFLICT, "conflict", "Harness is busy"))?;
    state.restart_harness(id).await.map_err(map_err)?;
    let backend = state
        .backends()
        .into_iter()
        .find(|b| b.provider == provider)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "provider missing"))?;
    Ok(Json(serde_json::to_value(backend).unwrap_or(json!({}))))
}

async fn list_workspaces(State(state): State<AppState>) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(state.list_workspaces().map_err(map_err)?).unwrap(),
    ))
}

async fn create_workspace(
    State(state): State<AppState>,
    Json(body): Json<CreateWorkspaceInput>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(state.create_workspace(body).map_err(map_err)?).unwrap(),
    ))
}

async fn get_workspace(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(state.get_workspace(&id).map_err(map_err)?).unwrap(),
    ))
}

#[derive(Deserialize)]
struct LabelBody {
    label: String,
}

async fn patch_workspace(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<LabelBody>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(state.update_workspace(&id, &body.label).map_err(map_err)?).unwrap(),
    ))
}

async fn delete_workspace(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    state.delete_workspace(&id).map_err(map_err)?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FavoriteBody {
    is_favorite: Option<bool>,
}

async fn favorite_workspace(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<FavoriteBody>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(
            state
                .set_favorite(&id, body.is_favorite.unwrap_or(true))
                .map_err(map_err)?,
        )
        .unwrap(),
    ))
}

async fn open_workspace(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(state.open_workspace(&id).map_err(map_err)?).unwrap(),
    ))
}

#[derive(Deserialize)]
struct PathQuery {
    path: Option<String>,
}

async fn workspace_tree(
    Path(id): Path<String>,
    Query(query): Query<PathQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let rel = query.path.as_deref().unwrap_or(".");
    let nodes = state.workspace_tree(&id, rel).map_err(map_err)?;
    let ws = state.get_workspace(&id).map_err(map_err)?;
    let root = ThreadWorkspaceTreeNodeDto {
        name: std::path::Path::new(rel)
            .file_name()
            .or_else(|| std::path::Path::new(&ws.abs_path).file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".into()),
        path: rel.replace('\\', "/"),
        kind: "directory".into(),
        size: None,
        has_children: Some(!nodes.is_empty()),
        children_loaded: Some(true),
        children: Some(nodes),
    };
    Ok(Json(serde_json::to_value(root).unwrap()))
}

async fn workspace_preview(
    Path(id): Path<String>,
    Query(query): Query<PathQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let path = query
        .path
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_request", "path is required"))?;
    Ok(Json(
        serde_json::to_value(state.workspace_preview(&id, &path).map_err(map_err)?).unwrap(),
    ))
}

async fn workspace_raw(
    Path(id): Path<String>,
    Query(query): Query<PathQuery>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiErr> {
    let path = query
        .path
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_request", "path is required"))?;
    let workspace = state.get_workspace(&id).map_err(map_err)?;
    let path = pockymoe_runtime::files::assert_within(
        std::path::Path::new(&workspace.abs_path),
        std::path::Path::new(&path),
    )
    .map_err(map_err)?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| map_err(e.into()))?;
    let metadata = file.metadata().await.map_err(|e| map_err(e.into()))?;
    if !metadata.is_file() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Path must point to a file",
        ));
    }
    // Workspace files change, so they revalidate on every use; chat markdown
    // images then cost a 304 instead of the whole file on each reload.
    let etag = file_etag(&metadata);
    if if_none_match(&headers, &etag) {
        return Ok(not_modified(&etag, "private, no-cache"));
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::ETAG, etag)
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(
            header::CONTENT_TYPE,
            if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("js" | "mjs" | "cjs")
            ) {
                "text/plain; charset=utf-8"
            } else {
                static_content_type(&path)
            },
        )
        // Raw workspace documents are untrusted, even when opened directly in a tab.
        .header(
            "content-security-policy",
            "sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data: blob:",
        )
        .header("referrer-policy", "no-referrer")
        .header(header::CACHE_CONTROL, "private, no-cache")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(stream_file(file, ()))
        .map_err(|error| map_err(error.into()))
}

#[derive(Deserialize)]
struct WriteFileBody {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateFileBody {
    path: String,
}

async fn workspace_create_file(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<CreateFileBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let path = body.path;
    let returned_path = path.clone();
    tokio::task::spawn_blocking(move || state.workspace_create_file(&id, &path))
        .await
        .map_err(|e| map_err(e.into()))?
        .map_err(
            |e| match e.downcast_ref::<std::io::Error>().map(std::io::Error::kind) {
                Some(std::io::ErrorKind::AlreadyExists) => err(
                    StatusCode::CONFLICT,
                    "fileAlreadyExists",
                    "A file or folder already exists at this path.",
                ),
                Some(std::io::ErrorKind::PermissionDenied) => err(
                    StatusCode::FORBIDDEN,
                    "permissionDenied",
                    "This directory is not writable.",
                ),
                _ => map_err(e),
            },
        )?;
    Ok((
        StatusCode::CREATED,
        [("cache-control", "private, no-store")],
        Json(json!({"path": returned_path})),
    ))
}

async fn workspace_write(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<WriteFileBody>,
) -> Result<Json<Value>, ApiErr> {
    state
        .workspace_write(&id, &body.path, &body.content)
        .map_err(map_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn workspace_delete_file(
    Path(id): Path<String>,
    Query(query): Query<PathQuery>,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiErr> {
    let path = query
        .path
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_request", "path is required"))?;
    let _gate = pockymoe_runtime::file_documents::mutation_guard();
    let ws = state.get_workspace(&id).map_err(map_err)?;
    let abs = pockymoe_runtime::files::assert_mutation_within(
        std::path::Path::new(&ws.abs_path),
        std::path::Path::new(&path),
    )
    .map_err(map_err)?;
    if abs
        == std::path::Path::new(&ws.abs_path)
            .canonicalize()
            .map_err(|e| map_err(e.into()))?
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "The workspace root cannot be deleted",
        ));
    }
    if abs.is_dir() {
        std::fs::remove_dir_all(abs).map_err(|e| map_err(e.into()))?;
    } else {
        std::fs::remove_file(abs).map_err(|e| map_err(e.into()))?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadListQuery {
    workspace_id: Option<String>,
    /// Off by default: a workspace listing shows only the threads a person started.
    /// Pass true to include agent-spawned descendants in the flat list.
    #[serde(default)]
    include_agent_threads: bool,
}

async fn list_threads(
    Query(query): Query<ThreadListQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(
            state
                .list_threads(query.workspace_id.as_deref(), query.include_agent_threads)
                .map_err(map_err)?,
        )
        .unwrap(),
    ))
}

async fn start_thread(
    State(state): State<AppState>,
    scope: Option<axum::Extension<crate::treer::Scope>>,
    Json(body): Json<CreateThreadInput>,
) -> Result<Json<Value>, ApiErr> {
    let thread = if let Some(scope) = scope {
        scope
            .hub
            .create_browser_thread(&scope, body)
            .await
            .map_err(|e| err(e.status, e.code, e.message))?
    } else {
        state.create_thread(body).await.map_err(map_err)?
    };
    Ok(Json(serde_json::to_value(thread).unwrap()))
}

async fn import_thread(
    State(state): State<AppState>,
    Json(body): Json<ImportThreadInput>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(state.import_thread(body).await.map_err(map_err)?).unwrap(),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportCandidatesQuery {
    provider: Option<String>,
    agent_id: Option<String>,
}

async fn import_candidates(
    Query(query): Query<ImportCandidatesQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let provider = query.provider.as_deref().and_then(Provider::from_name);
    Ok(Json(
        serde_json::to_value(
            state
                .list_import_candidates(provider, query.agent_id.as_deref())
                .await
                .map_err(map_err)?,
        )
        .unwrap(),
    ))
}

#[derive(Deserialize)]
struct DetailQuery {
    limit: Option<u32>,
    #[serde(rename = "beforeTurnId")]
    before_turn_id: Option<String>,
    view: Option<String>,
    compact: Option<String>,
    /// `narration=1` keeps every reply in summary turns (docs/architecture.md).
    narration: Option<String>,
}

/// `compact=1` opts into `history::compact_turn` (docs/architecture.md, History payloads).
#[derive(Deserialize)]
struct CompactQuery {
    compact: Option<String>,
}

fn compact_requested(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "true"))
}

async fn get_thread(
    Path(id): Path<String>,
    Query(query): Query<DetailQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    if query.limit.is_some_and(|limit| limit == 0 || limit > 100) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "limit must be between 1 and 100",
        ));
    }
    if query
        .before_turn_id
        .as_deref()
        .is_some_and(|turn_id| turn_id.trim().is_empty())
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "beforeTurnId must not be empty",
        ));
    }
    if query
        .view
        .as_deref()
        .is_some_and(|view| !matches!(view, "summary" | "full" | "delivery"))
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "view must be summary, full or delivery",
        ));
    }
    if query.view.as_deref() == Some("delivery") {
        return Ok(Json(state.thread_delivery(&id).await.map_err(map_err)?));
    }
    let summary_only = query.view.as_deref() == Some("summary");
    let mut detail = state
        .get_thread_detail_page_with(
            &id,
            query.limit.or(Some(if summary_only { 3 } else { 10 })),
            query.before_turn_id.as_deref(),
            summary_only,
            compact_requested(query.narration.as_deref()),
        )
        .await
        .map_err(map_err)?;
    if compact_requested(query.compact.as_deref()) {
        detail
            .turns
            .iter_mut()
            .for_each(pockymoe_runtime::history::compact_turn);
    }
    let mut value = serde_json::to_value(detail).unwrap();
    pockymoe_runtime::history::defer_tool_details(&mut value);
    Ok(Json(value))
}

#[derive(Deserialize)]
struct ThreadSearchQuery {
    q: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct ConversationSearchQuery {
    q: String,
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn device_search(
    Query(query): Query<ConversationSearchQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    conversation_search(state, query, None).await
}

async fn workspace_search(
    Path(id): Path<String>,
    Query(query): Query<ConversationSearchQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    conversation_search(state, query, Some(id)).await
}

async fn conversation_search(
    state: AppState,
    query: ConversationSearchQuery,
    workspace_id: Option<String>,
) -> Result<Json<Value>, ApiErr> {
    let value = tokio::task::spawn_blocking(move || {
        state.search_conversations(
            &query.q,
            workspace_id.as_deref(),
            query.limit.unwrap_or(50),
            query.offset.unwrap_or(0),
        )
    })
    .await
    .map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            &e.to_string(),
        )
    })?
    .map_err(map_err)?;
    Ok(Json(value))
}

async fn thread_search(
    Path(id): Path<String>,
    Query(query): Query<ThreadSearchQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    if query.q.trim().is_empty()
        || query.q.chars().count() > 200
        || query.limit.is_some_and(|limit| !(1..=100).contains(&limit))
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Search needs 1 to 200 characters and a limit between 1 and 100",
        ));
    }
    let value = tokio::task::spawn_blocking(move || {
        state.search_thread_messages(&id, &query.q, query.limit.unwrap_or(50))
    })
    .await
    .map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            &e.to_string(),
        )
    })?
    .map_err(map_err)?;
    Ok(Json(value))
}

async fn thread_turn_detail(
    Path((id, turn_id)): Path<(String, String)>,
    Query(query): Query<CompactQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let mut turn = state
        .get_thread_turn_detail(&id, &turn_id)
        .await
        .map_err(map_err)?;
    if compact_requested(query.compact.as_deref()) {
        pockymoe_runtime::history::compact_turn(&mut turn);
    }
    let mut value = serde_json::to_value(turn).unwrap();
    pockymoe_runtime::history::defer_tool_details(&mut value);
    Ok(Json(value))
}

async fn thread_item_detail(
    Path((id, item_id)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        state
            .get_history_item_detail(&id, &item_id)
            .map_err(map_err)?,
    ))
}

async fn thread_subagents(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        state.native_subagents(&id, None).await.map_err(map_err)?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentHistoryQuery {
    before: Option<i64>,
    item_id: Option<String>,
}

async fn thread_subagent_detail(
    Path((id, agent_id)): Path<(String, String)>,
    Query(query): Query<SubagentHistoryQuery>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        state
            .native_subagent_history(&id, Some(&agent_id), query.before, query.item_id.as_deref())
            .await
            .map_err(map_err)?,
    ))
}

#[derive(Deserialize)]
struct ImageQuery {
    path: String,
    /// `thumbnail` asks for a downscaled preview; anything else is the original.
    variant: Option<String>,
}

/// Thumbnails decode whole photos; bound how many are built at once.
static THUMBNAIL_BUILDS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

async fn thread_image(
    Path(id): Path<String>,
    Query(query): Query<ImageQuery>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiErr> {
    let mut image = state.open_thread_image(&id, &query.path).map_err(map_err)?;
    let thumbnail = query.variant.as_deref() == Some("thumbnail");
    let etag = pockymoe_runtime::thread_images::etag(
        &image,
        if thumbnail { "thumbnail" } else { "original" },
    );
    // Upload names carry a random suffix and are never rewritten.
    let cache_control = if image.immutable_name {
        "private, max-age=31536000, immutable"
    } else {
        "private, no-cache"
    };
    if if_none_match(&headers, &etag) {
        return Ok(not_modified(&etag, cache_control));
    }
    let dir = state.thread_thumbnail_cache_dir();
    let _build = if thumbnail && !pockymoe_runtime::thread_images::is_cached(&image, &dir) {
        THUMBNAIL_BUILDS.acquire().await.ok()
    } else {
        None
    };
    let (bytes, mime) = tokio::task::spawn_blocking(move || {
        if thumbnail {
            pockymoe_runtime::thread_images::thumbnail_or_original(&mut image, &dir)
        } else {
            image.read_all().map(|bytes| (bytes, image.mime))
        }
    })
    .await
    .map_err(|error| map_err(error.into()))?
    .map_err(map_err)?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, cache_control),
            (header::ETAG, etag.as_str()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'",
            ),
        ],
        bytes,
    )
        .into_response())
}

/// Weak comparison, as RFC 9110 requires for If-None-Match.
fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    let opaque = |tag: &str| tag.trim().trim_start_matches("W/").to_owned();
    let wanted = opaque(etag);
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|tag| tag.trim() == "*" || opaque(tag) == wanted)
}

fn file_etag(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("W/\"{:x}-{modified:x}\"", metadata.len())
}

fn not_modified(etag: &str, cache_control: &'static str) -> Response {
    Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, cache_control)
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::NOT_MODIFIED.into_response())
}

#[derive(Deserialize)]
struct RenameBody {
    title: String,
}

async fn rename_thread(
    Path(id): Path<String>,
    State(state): State<AppState>,
    scope: Option<axum::Extension<crate::treer::Scope>>,
    Json(body): Json<RenameBody>,
) -> Result<Json<Value>, ApiErr> {
    let thread = if let Some(scope) = scope {
        scope
            .hub
            .rename_browser_thread(&scope, &id, &body.title)
            .await
            .map_err(|e| err(e.status, e.code, e.message))?
    } else {
        state.rename_thread(&id, &body.title).map_err(map_err)?
    };
    Ok(Json(serde_json::to_value(thread).unwrap()))
}

async fn delete_thread(
    Path(id): Path<String>,
    State(state): State<AppState>,
    scope: Option<axum::Extension<crate::treer::Scope>>,
) -> Result<StatusCode, ApiErr> {
    if let Some(scope) = scope {
        scope
            .hub
            .delete_browser_thread(&scope, &id)
            .await
            .map_err(|e| err(e.status, e.code, e.message))?;
    } else {
        state.delete_thread_releasing(&id).await.map_err(map_err)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsBody {
    model: Option<String>,
    reasoning_effort: Option<String>,
    fast_mode: Option<bool>,
    collaboration_mode: Option<String>,
    sandbox_mode: Option<String>,
}

async fn thread_settings(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<SettingsBody>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(
            state
                .update_settings(
                    &id,
                    body.model,
                    body.reasoning_effort,
                    body.fast_mode,
                    body.collaboration_mode,
                    body.sandbox_mode,
                )
                .await
                .map_err(map_err)?,
        )
        .unwrap(),
    ))
}

async fn thread_prompt(
    Path(id): Path<String>,
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiErr> {
    let content_type = request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let input = if content_type.contains("multipart") {
        let mut multipart = Multipart::from_request(request, &state)
            .await
            .map_err(|e| err(StatusCode::BAD_REQUEST, "bad_request", e.to_string()))?;
        let mut prompt = String::new();
        let mut client_request_id = None;
        let mut model = None;
        let mut reasoning_effort = None;
        let mut collaboration_mode = None;
        let mut attachment_manifest = None;
        let mut files = Vec::new();
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|e| err(StatusCode::BAD_REQUEST, "bad_request", e.to_string()))?
        {
            let name = field.name().unwrap_or("").to_string();
            let file_name = field.file_name().map(str::to_string);
            let mime = field
                .content_type()
                .unwrap_or("application/octet-stream")
                .to_string();
            let bytes = field
                .bytes()
                .await
                .map_err(|e| err(StatusCode::BAD_REQUEST, "bad_request", e.to_string()))?;
            match name.as_str() {
                "prompt" => prompt = String::from_utf8_lossy(&bytes).into_owned(),
                "clientRequestId" => {
                    client_request_id = Some(String::from_utf8_lossy(&bytes).into_owned())
                }
                "model" => model = Some(String::from_utf8_lossy(&bytes).into_owned()),
                "reasoningEffort" | "reasoning_effort" => {
                    reasoning_effort = Some(String::from_utf8_lossy(&bytes).into_owned())
                }
                "collaborationMode" => {
                    collaboration_mode = Some(String::from_utf8_lossy(&bytes).into_owned())
                }
                "attachmentManifest" => {
                    attachment_manifest = Some(String::from_utf8_lossy(&bytes).into_owned())
                }
                _ if file_name.is_some() => {
                    if files.len() >= MAX_PROMPT_ATTACHMENTS {
                        return Err(err(
                            StatusCode::BAD_REQUEST,
                            "bad_request",
                            format!(
                                "A prompt can include at most {MAX_PROMPT_ATTACHMENTS} attachments."
                            ),
                        ));
                    }
                    if bytes.len() > MAX_PROMPT_ATTACHMENT_BYTES {
                        return Err(err(
                            StatusCode::BAD_REQUEST,
                            "bad_request",
                            "Each attachment must be 25 MB or smaller.",
                        ));
                    }
                    files.push((
                        file_name.unwrap_or_else(|| name.clone()),
                        mime,
                        bytes.to_vec(),
                    ));
                }
                _ => {}
            }
        }
        let manifest = if files.is_empty() {
            Vec::new()
        } else {
            let raw = attachment_manifest.ok_or_else(|| {
                err(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "attachmentManifest is required when files are uploaded.",
                )
            })?;
            let parsed =
                serde_json::from_str::<Vec<PromptAttachmentManifestEntry>>(&raw).map_err(|_| {
                    err(
                        StatusCode::BAD_REQUEST,
                        "bad_request",
                        "attachmentManifest must be valid JSON.",
                    )
                })?;
            if parsed.len() != files.len() {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "attachmentManifest must describe every uploaded attachment.",
                ));
            }
            parsed
        };
        let attachments = files
            .into_iter()
            .zip(manifest)
            .map(|((file_name, mime_type, bytes), manifest)| {
                let expected_prefix = match manifest.kind.as_str() {
                    "photo" if mime_type.starts_with("image/") => "[PHOTO ",
                    "file" => "[FILE ",
                    "photo" => {
                        return Err(err(
                            StatusCode::BAD_REQUEST,
                            "bad_request",
                            "Photo attachments must use an image MIME type.",
                        ))
                    }
                    _ => {
                        return Err(err(
                            StatusCode::BAD_REQUEST,
                            "bad_request",
                            "Attachment kind must be photo or file.",
                        ))
                    }
                };
                if !manifest.placeholder.starts_with(expected_prefix)
                    || !manifest.placeholder.ends_with(']')
                {
                    return Err(err(
                        StatusCode::BAD_REQUEST,
                        "bad_request",
                        "Attachment placeholder does not match its kind.",
                    ));
                }
                Ok(UploadedPromptAttachment {
                    kind: manifest.kind,
                    original_name: if manifest.original_name.trim().is_empty() {
                        file_name
                    } else {
                        manifest.original_name
                    },
                    placeholder: manifest.placeholder,
                    bytes,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let prompt = state
            .prepare_prompt_attachments(&id, &prompt, attachments)
            .map_err(map_err)?;
        SendThreadPromptInput {
            prompt,
            client_request_id,
            model,
            reasoning_effort,
            collaboration_mode,
            images: Vec::new(),
        }
    } else {
        let bytes = axum::body::to_bytes(request.into_body(), 32 * 1024 * 1024)
            .await
            .map_err(|e| err(StatusCode::BAD_REQUEST, "bad_request", e.to_string()))?;
        serde_json::from_slice::<SendThreadPromptInput>(&bytes)
            .map_err(|e| err(StatusCode::BAD_REQUEST, "bad_request", e.to_string()))?
    };
    state.accept_prompt(&id, &input).await.map_err(map_err)?;
    state.dispatch_inbox(id.clone());
    Ok(Json(
        serde_json::to_value(state.get_thread(&id).map_err(map_err)?).unwrap(),
    ))
}

async fn thread_interrupt(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    state.interrupt(&id).await.map_err(map_err)?;
    Ok(Json(
        serde_json::to_value(state.get_thread(&id).map_err(map_err)?).unwrap(),
    ))
}

async fn thread_release(
    Path(id): Path<String>,
    State(state): State<AppState>,
    principal: Option<axum::Extension<crate::owner_trust::BrowserPrincipal>>,
    treer_scope: Option<axum::Extension<crate::treer::Scope>>,
    body: Bytes,
) -> Result<Json<ReleaseThreadResultDto>, ApiErr> {
    let input = if body.iter().all(u8::is_ascii_whitespace) {
        ReleaseThreadInput::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| err(StatusCode::BAD_REQUEST, "bad_request", e.to_string()))?
    };
    // A shared caller may end only threads it could release one by one.
    let shared = principal.map(|p| p.0).filter(|p| !p.is_owner());
    let may_release = |thread: &str| {
        treer_scope.as_ref().is_none_or(|s| s.allows(thread))
            && shared.as_ref().is_none_or(|p| {
                crate::grants::authorize_path(
                    &state,
                    p,
                    "POST",
                    &format!("/api/threads/{thread}/release"),
                )
                .is_ok()
            })
    };
    state
        .release_thread_harness(
            &id,
            input.interrupt,
            (shared.is_some() || treer_scope.is_some())
                .then_some(&may_release as &(dyn Fn(&str) -> bool + Send + Sync)),
        )
        .await
        .map(Json)
        .map_err(|e| {
            if e.is::<pockymoe_runtime::actor::TurnRunning>() {
                err(StatusCode::CONFLICT, "turn_running", e.to_string())
            } else {
                map_err(e)
            }
        })
}

async fn thread_resume(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(state.resume_thread(&id).await.map_err(map_err)?).unwrap(),
    ))
}

async fn thread_disconnect(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(state.get_thread_detail(&id, None).await.map_err(map_err)?).unwrap(),
    ))
}

async fn thread_models(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let thread = state.get_thread(&id).map_err(map_err)?;
    let workspace = state.get_workspace(&thread.workspace_id).map_err(map_err)?;
    let models = state
        .list_models(
            thread.provider,
            thread.agent_id.as_deref(),
            Some(&workspace.abs_path),
        )
        .await
        .map_err(map_err)?;
    Ok(Json(serde_json::to_value(models).unwrap()))
}

async fn thread_watches(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(state.thread_watches(&id).await.map_err(map_err)?))
}

async fn thread_group(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    state.get_thread(&id).map_err(map_err)?;
    let ids = state.db.with(|conn| {
        let mut query = conn.prepare("WITH RECURSIVE family(id,depth) AS (SELECT id,0 FROM threads WHERE id=?1 UNION ALL SELECT t.id,f.depth+1 FROM threads t JOIN family f ON t.parent_thread_id=f.id WHERE f.depth<3) SELECT id FROM family")?;
        let rows = query.query_map([&id], |r|r.get::<_,String>(0))?.collect::<std::result::Result<std::collections::HashSet<_>,_>>()?;
        Ok(rows)
    }).map_err(map_err)?;
    let mut threads = state.list_threads(None, true).map_err(map_err)?;
    threads.retain(|t| ids.contains(&t.id));
    for thread in &mut threads {
        if thread.id == id {
            thread.parent_thread_id = None;
            thread.root_thread_id = None;
        } else {
            thread.root_thread_id = Some(id.clone());
        }
    }
    Ok(Json(serde_json::to_value(threads).unwrap()))
}

async fn thread_fork(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<ForkThreadInput>,
) -> Result<Json<Value>, ApiErr> {
    let (thread, source_turn_id, source_turn_index) = state
        .fork_thread_at(&id, &body.mode, body.turn_id.as_deref())
        .await
        .map_err(map_err)?;
    let detail = state
        .get_thread_detail(&thread.id, None)
        .await
        .map_err(map_err)?;
    Ok(Json(json!({
        "thread": detail,
        "sourceThreadId": id,
        "sourceTurnId": source_turn_id,
        "sourceTurnIndex": source_turn_index
    })))
}

async fn thread_fork_turns(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(state.list_fork_turn_options(&id).map_err(map_err)?).unwrap(),
    ))
}

async fn thread_compact(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(state.compact_thread(&id).await.map_err(map_err)?).unwrap(),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoalBody {
    objective: Option<String>,
    status: Option<String>,
    #[serde(default = "missing_goal_token_budget")]
    token_budget: Value,
}

fn missing_goal_token_budget() -> Value {
    json!({ "missing": true })
}

fn parse_goal_token_budget(value: Value) -> Result<Option<Option<u64>>, ApiErr> {
    match value {
        Value::Object(object) if object.get("missing") == Some(&Value::Bool(true)) => Ok(None),
        Value::Null => Ok(Some(None)),
        Value::Number(number) => number
            .as_u64()
            .filter(|budget| *budget > 0)
            .map(|budget| Some(Some(budget)))
            .ok_or_else(|| {
                err(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "tokenBudget must be a positive integer or null.",
                )
            }),
        _ => Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "tokenBudget must be a positive integer or null.",
        )),
    }
}

async fn thread_goal(
    Path(id): Path<String>,
    State(state): State<AppState>,
    body: Option<Json<GoalBody>>,
) -> Result<Json<Value>, ApiErr> {
    let (objective, status, token_budget) = body
        .map(|Json(body)| {
            Ok((
                body.objective,
                body.status,
                parse_goal_token_budget(body.token_budget)?,
            ))
        })
        .transpose()?
        .unwrap_or((None, None, None));
    Ok(Json(
        state
            .submit_thread_goal(&id, objective, status, token_budget)
            .await
            .map_err(map_err)?,
    ))
}

async fn thread_goal_clear(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        state
            .thread_goal(&id, None, None, None, true)
            .await
            .map_err(map_err)?,
    ))
}

async fn thread_skills() -> Json<Value> {
    Json(json!({ "cwd": ".", "skills": [], "errors": [] }))
}

async fn thread_mcp() -> Json<Value> {
    Json(json!({ "servers": [] }))
}

async fn thread_hooks() -> Json<Value> {
    Json(
        json!({ "cwd": ".", "hooks": [], "warnings": [], "errors": [], "globalHooksPath": "", "projectHooksPath": "" }),
    )
}

#[derive(Deserialize)]
struct RespondBody {
    #[serde(default)]
    allow: Option<bool>,
    #[serde(default)]
    answers: Option<Value>,
}

async fn thread_respond(
    Path((id, request_id)): Path<(String, String)>,
    State(state): State<AppState>,
    body: Option<Json<RespondBody>>,
) -> Result<Json<Value>, ApiErr> {
    let (allow, answer) = match body {
        Some(Json(body)) => (
            body.allow.unwrap_or(true),
            body.answers.map(|answers| answers.to_string()),
        ),
        None => (true, None),
    };
    Ok(Json(
        serde_json::to_value(
            state
                .respond_request(&id, &request_id, allow, answer.as_deref())
                .await
                .map_err(map_err)?,
        )
        .unwrap(),
    ))
}

async fn cancel_pending_steer(
    Path((id, pending_steer_id)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(
            state
                .cancel_pending_steer(&id, &pending_steer_id)
                .await
                .map_err(map_err)?,
        )
        .unwrap(),
    ))
}

async fn steer_pending_prompt(
    Path((id, pending_steer_id)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        serde_json::to_value(
            state
                .steer_pending_prompt(&id, &pending_steer_id)
                .await
                .map_err(map_err)?,
        )
        .unwrap(),
    ))
}

async fn create_publication(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(input): Json<pockymoe_runtime::publications::CreatePublication>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        state
            .create_publication(&id, input)
            .await
            .map_err(map_err)?,
    ))
}

async fn read_publication(
    Path(token): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(state.read_publication(&token).await.map_err(map_err)?))
}

async fn revoke_publication(
    Path(token): Path<String>,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiErr> {
    state.revoke_publication(&token).map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn export_turns(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let detail = state.get_thread_detail(&id, None).await.map_err(map_err)?;
    let turns: Vec<Value> = detail
        .turns
        .iter()
        .enumerate()
        .map(|(idx, turn)| {
            json!({
                "turnId": turn.id,
                "turnNumber": idx + 1,
                "startedAt": turn.started_at,
                "status": turn.status,
                "userPromptPreview": turn.items.iter().find(|i| i.kind == "userMessage").map(|i| i.text.clone()).unwrap_or_default()
            })
        })
        .collect();
    Ok(Json(
        json!({ "turns": turns, "totalTurnCount": detail.total_turn_count }),
    ))
}

async fn export_html(
    Path(id): Path<String>,
    Query(query): Query<ExportTranscriptQuery>,
    State(state): State<AppState>,
) -> Result<Response, ApiErr> {
    let detail = state.get_thread_detail(&id, None).await.map_err(map_err)?;
    render_transcript_export(&detail, &query)
}

fn render_transcript_export(
    detail: &pockymoe_protocol::ThreadDetailDto,
    query: &ExportTranscriptQuery,
) -> Result<Response, ApiErr> {
    let mode = query.mode.as_deref().unwrap_or("latest");
    if !matches!(mode, "latest" | "selected") {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Export mode must be latest or selected.",
        ));
    }
    let profile = query.profile.as_deref().unwrap_or("review");
    if !matches!(profile, "review" | "technical") {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Export profile must be review or technical.",
        ));
    }
    let turn_ids = query
        .turn_ids
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let turns = crate::export::select_turns(&detail.turns, mode, query.limit, &turn_ids)
        .map_err(|error| err(StatusCode::BAD_REQUEST, "bad_request", error.to_string()))?;
    let options = crate::export::TranscriptExportOptions {
        profile: profile.into(),
        include_token_and_price: query.include_token_and_price.unwrap_or(true),
        include_command_output: query.include_command_output.unwrap_or(false),
        include_absolute_paths: query.include_absolute_paths.unwrap_or(false),
    };
    let bytes = crate::export::html_transcript(detail, &turns, &options)
        .map_err(map_err)?
        .into_bytes();
    let content_type = "text/html; charset=utf-8";
    let extension = "html";
    let stem = safe_export_stem(&detail.thread.title);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"pockymoe-{stem}.{extension}\""),
        )
        .header(header::CACHE_CONTROL, "private, no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(bytes))
        .map_err(|error| map_err(error.into()))
}

fn safe_export_stem(title: &str) -> String {
    let stem = title
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if stem.is_empty() {
        "thread".into()
    } else {
        stem.chars().take(72).collect()
    }
}

async fn create_shell(
    Path(id): Path<String>,
    State(state): State<AppState>,
    body: Option<Json<ShellCreateBody>>,
) -> Result<Json<Value>, ApiErr> {
    let thread = state.get_thread(&id).map_err(map_err)?;
    let workspace = state.get_workspace(&thread.workspace_id).map_err(map_err)?;
    let body = body.map(|Json(body)| body).unwrap_or_default();
    let cols = body.cols.unwrap_or(80);
    let rows = body.rows.unwrap_or(24);
    if cols == 0 || rows == 0 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Shell rows and columns must be positive.",
        ));
    }
    let (shell_id, shell) = crate::shells::hub()
        .create(
            &thread.id,
            &workspace.id,
            &workspace.abs_path,
            cols,
            rows,
            body.label,
        )
        .map_err(map_err)?;
    Ok(Json(json!({
        "threadId": thread.id,
        "workspaceId": workspace.id,
        "workspacePathStatus": "present",
        "state": "running",
        "shell": shell,
        "shells": [shell],
        "activeShellId": shell_id
    })))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShellCreateBody {
    cols: Option<u16>,
    rows: Option<u16>,
    label: Option<String>,
}

#[derive(Deserialize)]
struct ShellUpdateBody {
    label: Option<String>,
}

async fn update_shell(
    Path(id): Path<String>,
    Json(body): Json<ShellUpdateBody>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        crate::shells::hub()
            .update_label(&id, body.label)
            .map_err(map_err)?,
    ))
}

async fn terminate_shell(Path(id): Path<String>) -> Result<Json<Value>, ApiErr> {
    Ok(Json(crate::shells::hub().terminate(&id).map_err(map_err)?))
}

async fn workspace_download(
    Path(id): Path<String>,
    Query(query): Query<PathQuery>,
    State(state): State<AppState>,
) -> Result<Response, ApiErr> {
    let requested_path = query
        .path
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_request", "path is required"))?;
    let download =
        tokio::task::spawn_blocking(move || state.workspace_download(&id, &requested_path))
            .await
            .map_err(|error| map_err(error.into()))?
            .map_err(map_err)?;
    match download {
        WorkspaceDownload::File { path, file } => {
            let filename = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("workspace-file");
            download_response(
                filename,
                static_content_type(&path),
                file.metadata().map_err(|e| map_err(e.into()))?.len(),
                stream_file(tokio::fs::File::from_std(file), ()),
            )
        }
        WorkspaceDownload::DirectoryArchive { filename, archive } => {
            let content_length = archive
                .as_file()
                .metadata()
                .map_err(|error| map_err(error.into()))?
                .len();
            let file =
                tokio::fs::File::from_std(archive.reopen().map_err(|error| map_err(error.into()))?);
            download_response(
                &filename,
                "application/zip",
                content_length,
                stream_file(file, archive),
            )
        }
    }
}

pub(crate) fn stream_file<G: Send + 'static>(file: tokio::fs::File, guard: G) -> Body {
    Body::from_stream(futures_util::stream::try_unfold(
        (file, guard),
        |(mut file, guard)| async move {
            let mut bytes = vec![0u8; 64 * 1024];
            let read = file.read(&mut bytes).await?;
            if read == 0 {
                Ok::<_, std::io::Error>(None)
            } else {
                bytes.truncate(read);
                Ok(Some((Bytes::from(bytes), (file, guard))))
            }
        },
    ))
}
fn download_response(
    filename: &str,
    content_type: &str,
    content_length: u64,
    body: Body,
) -> Result<Response, ApiErr> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, content_length)
        .header(
            header::CONTENT_DISPOSITION,
            attachment_content_disposition(filename),
        )
        .header(header::CACHE_CONTROL, "private, no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(body)
        .map_err(|error| map_err(error.into()))
}

fn attachment_content_disposition(filename: &str) -> String {
    let fallback = filename
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let fallback = if fallback.is_empty() {
        "workspace-file".to_string()
    } else {
        fallback
    };
    let encoded = url::form_urlencoded::byte_serialize(filename.as_bytes()).collect::<String>();
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

fn upload_file_name(filename: Option<&str>) -> String {
    filename
        .and_then(|name| FsPath::new(name).file_name())
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .unwrap_or("upload.bin")
        .to_string()
}

async fn workspace_upload(
    Path(id): Path<String>,
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, ApiErr> {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !content_type.contains("multipart/form-data") {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "File upload must use multipart/form-data.",
        ));
    }
    let mut multipart = Multipart::from_request(request, &state)
        .await
        .map_err(|error| err(StatusCode::BAD_REQUEST, "bad_request", error.to_string()))?;
    let mut requested_path = None;
    let mut uploaded_file: Option<(String, Vec<u8>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| err(StatusCode::BAD_REQUEST, "bad_request", error.to_string()))?
    {
        let field_name = field.name().unwrap_or_default().to_string();
        let filename = field.file_name().map(str::to_string);
        let bytes = field
            .bytes()
            .await
            .map_err(|error| err(StatusCode::BAD_REQUEST, "bad_request", error.to_string()))?;
        match field_name.as_str() {
            "path" if filename.is_none() => {
                requested_path = Some(String::from_utf8_lossy(&bytes).trim().to_string());
            }
            "file" => {
                if uploaded_file.is_some() {
                    return Err(err(
                        StatusCode::BAD_REQUEST,
                        "bad_request",
                        "Only one file can be uploaded at a time.",
                    ));
                }
                if bytes.len() > MAX_WORKSPACE_UPLOAD_BYTES {
                    return Err(err(
                        StatusCode::BAD_REQUEST,
                        "bad_request",
                        "Workspace uploads must be 50 MB or smaller.",
                    ));
                }
                uploaded_file = Some((upload_file_name(filename.as_deref()), bytes.to_vec()));
            }
            _ if filename.is_some() => {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    format!("Unexpected multipart file field: {field_name}."),
                ));
            }
            _ => {}
        }
    }
    let (filename, bytes) = uploaded_file.ok_or_else(|| {
        err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "A file field is required.",
        )
    })?;
    let path = requested_path
        .filter(|path| !path.is_empty())
        .unwrap_or(filename);
    let (path, size) = state
        .workspace_write_bytes(&id, &path, &bytes)
        .map_err(map_err)?;
    let name = FsPath::new(&path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("upload.bin");
    Ok(Json(json!({
        "kind": "file",
        "file": { "path": path, "name": name, "size": size }
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveBody {
    from_path: String,
    to_path: String,
}

async fn workspace_move(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<MoveBody>,
) -> Result<Json<Value>, ApiErr> {
    let _gate = pockymoe_runtime::file_documents::mutation_guard();
    let ws = state.get_workspace(&id).map_err(map_err)?;
    let from = pockymoe_runtime::files::assert_mutation_within(
        std::path::Path::new(&ws.abs_path),
        std::path::Path::new(&body.from_path),
    )
    .map_err(map_err)?;
    let to = pockymoe_runtime::files::assert_mutation_within(
        std::path::Path::new(&ws.abs_path),
        std::path::Path::new(&body.to_path),
    )
    .map_err(map_err)?;
    let root = std::path::Path::new(&ws.abs_path)
        .canonicalize()
        .map_err(|e| map_err(e.into()))?;
    if from == root || to == root {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "The workspace root cannot be renamed",
        ));
    }
    if from == to {
        return Ok(Json(json!({"ok":true})));
    }
    if to.exists() {
        return Err(err(
            StatusCode::CONFLICT,
            "conflict",
            "A file or folder with that name already exists",
        ));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| map_err(e.into()))?;
    }
    std::fs::rename(from, to).map_err(|e| map_err(e.into()))?;
    Ok(Json(json!({ "ok": true })))
}

async fn thread_shell(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let thread = state.get_thread(&id).map_err(map_err)?;
    let workspace = state.get_workspace(&thread.workspace_id).map_err(map_err)?;
    let shells = crate::shells::hub().list_for_thread(&thread.id);
    let active = shells
        .first()
        .and_then(|s| s.get("id").and_then(Value::as_str))
        .map(str::to_string);
    Ok(Json(json!({
        "threadId": thread.id,
        "workspaceId": workspace.id,
        "workspacePathStatus": if std::path::Path::new(&workspace.abs_path).exists() { "present" } else { "missing" },
        "state": if shells.is_empty() { "not_created" } else { "running" },
        "shell": shells.first().cloned(),
        "shells": shells,
        "activeShellId": active
    })))
}

async fn list_plugins(State(state): State<AppState>) -> Json<Value> {
    Json(json!(state.list_plugins()))
}

async fn get_plugin(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(json!(state.get_plugin(&id).map_err(map_err)?)))
}

async fn import_plugin_unsupported() -> ApiErr {
    err(
        StatusCode::NOT_IMPLEMENTED,
        "unsupported",
        "Imported plugins are not supported by the Rust supervisor yet.",
    )
}

async fn delete_plugin(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    state.get_plugin(&id).map_err(map_err)?;
    Err(err(
        StatusCode::CONFLICT,
        "unsupported",
        "Built-in plugins cannot be uninstalled.",
    ))
}

async fn update_plugin(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(input): Json<UpdatePluginInput>,
) -> Result<Json<Value>, ApiErr> {
    state
        .set_plugin_enabled(&id, input.enabled)
        .map_err(map_err)?;
    Ok(Json(json!(state.get_plugin(&id).map_err(map_err)?)))
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    scope: Option<axum::Extension<crate::treer::Scope>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| {
        crate::socket::websocket_loop_scoped(socket, state, scope.map(|s| s.0))
    })
}

async fn model_pricing(State(state): State<AppState>) -> Json<Value> {
    Json(state.model_pricing())
}
async fn patch_model_pricing(
    State(state): State<AppState>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    state
        .update_model_pricing(&input)
        .map(Json)
        .map_err(map_err)
}

async fn subscription_usage(
    Path(provider): Path<String>,
    Query(query): Query<AgentQuery>,
    State(state): State<AppState>,
) -> Json<Value> {
    if state.config.fake_runtime {
        return Json(json!({"usage":null}));
    }
    let agent = query.agent_id.as_deref().unwrap_or(if provider == "acp" {
        "codex"
    } else {
        &provider
    });
    Json(json!(state.subscription_usage.read(agent).await))
}

#[cfg(test)]
mod compression_tests {
    use super::*;
    use tower::ServiceExt;

    #[test]
    fn only_text_like_bodies_are_compressible() {
        for compressible in [
            "application/json",
            "application/json; charset=utf-8",
            "text/html",
            "text/plain;charset=utf-8",
            "application/javascript",
            "image/svg+xml",
            "application/problem+json",
        ] {
            assert!(compressible_content_type(compressible), "{compressible}");
        }
        for identity in [
            "text/event-stream",
            "image/png",
            "image/jpeg",
            "image/webp",
            "application/octet-stream",
            "application/zip",
            "application/gzip",
            "video/mp4",
            "font/woff2",
            "",
        ] {
            assert!(!compressible_content_type(identity), "{identity}");
        }
    }

    #[tokio::test]
    async fn the_layer_skips_images_upgrades_and_small_bodies() {
        let app = Router::new()
            .route(
                "/png",
                get(|| async { ([(header::CONTENT_TYPE, "image/png")], vec![7u8; 8192]) }),
            )
            .route(
                "/json",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "application/json")],
                        "[1]".repeat(2048),
                    )
                }),
            )
            .route(
                "/tiny",
                get(|| async { ([(header::CONTENT_TYPE, "application/json")], "[1]") }),
            )
            .route(
                "/sse",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/event-stream")],
                        "data: x\n\n".repeat(500),
                    )
                }),
            )
            .route(
                "/upgrade",
                get(|| async {
                    (
                        StatusCode::SWITCHING_PROTOCOLS,
                        [(header::CONTENT_TYPE, "text/plain")],
                        "x".repeat(4096),
                    )
                }),
            )
            .layer(compression_layer());
        for (path, compressed) in [
            ("/png", false),
            ("/json", true),
            ("/tiny", false),
            ("/sse", false),
            ("/upgrade", false),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::ACCEPT_ENCODING, "gzip, br")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.headers().get(header::CONTENT_ENCODING).is_some(),
                compressed,
                "{path}"
            );
        }
    }
}

#[cfg(test)]
mod public_link_tests {
    use super::*;
    use aes_gcm::{
        aead::{Aead, KeyInit, Payload},
        Aes256Gcm, Nonce,
    };
    use base64::Engine;
    use tower::ServiceExt;

    #[tokio::test]
    async fn live_publication_http_returns_only_ciphertext_decryptable_by_fragment_key() {
        let (_dir, mut state) = crate::tunnel::tests::state_with_relay_url("http://localhost:8788");
        // The fixture exercises HTTP serialization with local authentication disabled.
        std::sync::Arc::get_mut(&mut state)
            .unwrap()
            .config
            .auth_required = false;
        let workspace = state
            .create_workspace(pockymoe_protocol::CreateWorkspaceInput {
                abs_path: Some(state.config.workspace_root.to_string_lossy().into()),
                git_url: None,
                label: Some("test".into()),
            })
            .unwrap();
        let thread = state
            .create_thread(pockymoe_protocol::CreateThreadInput {
                workspace_id: workspace.id,
                title: Some("Private publication title".into()),
                provider: Some(pockymoe_protocol::Provider::Codex),
                agent_id: Some("codex".into()),
                model: "ios-e2e-stream".into(),
                reasoning_effort: None,
                approval_mode: "yolo".into(),
                parent_thread_id: None,
            })
            .await
            .unwrap();
        let turn = uuid::Uuid::new_v4().to_string();
        state.db.with(|conn| {
            conn.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES (?1,?2,'completed',1)", (&turn, &thread.id))?;
            Ok(())
        }).unwrap();
        let fixture: Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/public-link-v2.json"))
                .unwrap();
        let publication = state
            .create_publication(
                &thread.id,
                pockymoe_runtime::publications::CreatePublication {
                    turn_ids: vec![turn],
                    theme: "light".into(),
                    encryption_key: fixture["key"].as_str().unwrap().into(),
                },
            )
            .await
            .unwrap();
        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/api/publications/{}",
                        publication["token"].as_str().unwrap()
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["version"], 2);
        assert_eq!(value.as_object().unwrap().len(), 3);
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let iv = b64.decode(value["iv"].as_str().unwrap()).unwrap();
        let ciphertext = b64.decode(value["ciphertext"].as_str().unwrap()).unwrap();
        let key = b64.decode(fixture["key"].as_str().unwrap()).unwrap();
        let plaintext = Aes256Gcm::new_from_slice(&key)
            .unwrap()
            .decrypt(
                Nonce::from_slice(&iv),
                Payload {
                    msg: &ciphertext,
                    aad: b"pockymoe-public-link-v1",
                },
            )
            .unwrap();
        let snapshot: Value = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(snapshot["title"], "Private publication title");
        assert_eq!(snapshot["live"], true);
        assert_eq!(snapshot["theme"], "light");
        assert_eq!(snapshot["turnCount"], 1);
    }
}
