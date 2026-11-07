//! Safe editor API: never falls back to the legacy unconditional PUT.
use crate::http::{err, map_err, ApiErr, AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use pockymoe_runtime::file_documents::SaveDocument;
use serde::Deserialize;
use serde_json::{json, Value};
#[derive(Clone)]
pub(crate) struct FileActor(pub String);
fn actor(state: &AppState, forwarded: Option<Extension<FileActor>>) -> String {
    forwarded.map(|a| a.0 .0).unwrap_or_else(|| {
        format!(
            "local:{}",
            state.config.admin_username.as_deref().unwrap_or("owner")
        )
    })
}
fn response(status: StatusCode, value: Value) -> Response {
    (
        status,
        [("cache-control", "private, no-store")],
        Json(value),
    )
        .into_response()
}
pub(crate) async fn capabilities(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Response, ApiErr> {
    Ok(response(
        StatusCode::OK,
        state.file_capabilities(&id).map_err(map_err)?,
    ))
}
#[derive(Deserialize)]
pub(crate) struct DocumentQuery {
    path: String,
}
pub(crate) async fn document(
    Path(id): Path<String>,
    Query(q): Query<DocumentQuery>,
    State(state): State<AppState>,
) -> Result<Response, ApiErr> {
    let doc = state.file_document(&id, &q.path).map_err(|e| {
        if e.to_string() == "unsupportedPlatform" {
            err(
                StatusCode::NOT_IMPLEMENTED,
                "unsupportedPlatform",
                "Safe document reads are unavailable on this platform.",
            )
        } else {
            map_err(e)
        }
    })?;
    Ok(response(StatusCode::OK, serde_json::to_value(doc).unwrap()))
}
pub(crate) async fn save(
    Path(id): Path<String>,
    State(state): State<AppState>,
    caller: Option<Extension<FileActor>>,
    trusted: Option<Extension<crate::auth::TrustedRelayForward>>,
    Json(input): Json<SaveDocument>,
) -> Result<Response, ApiErr> {
    if trusted.is_some() && caller.is_none() {
        return Err(err(
            StatusCode::FORBIDDEN,
            "actorUnavailable",
            "Update the relay before using safe saves.",
        ));
    }
    let actor = actor(&state, caller);
    let operation = state.track_file_save(&actor, &id, &input);
    // File I/O and fsync are bounded but blocking; do not stall the socket reactor.
    let result = pockymoe_runtime::execution::spawn_blocking(move || {
        let _operation = operation;
        state.file_save(&actor, &id, input)
    })
    .await
    .map_err(|e| map_err(e.into()))?
    .map_err(|error| {
        let code = error.to_string();
        let status = match code.as_str() {
            "workspaceChanged" | "operationIdReuse" => StatusCode::CONFLICT,
            "operationExpired" => StatusCode::GONE,
            "saveJournalFull" => StatusCode::INSUFFICIENT_STORAGE,
            "unsupportedPlatform" => StatusCode::NOT_IMPLEMENTED,
            _ => StatusCode::BAD_REQUEST,
        };
        err(status, &code, &code)
    })?;
    let status = match result["status"].as_str() {
        Some("conflict") => StatusCode::CONFLICT,
        Some("failedBeforeWrite") => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::OK,
    };
    if status.is_success() {
        Ok(response(status, result))
    } else {
        Ok(response(
            status,
            json!({"code":result["code"],"message":result["message"].as_str().unwrap_or("File changed on disk. Your draft is preserved."),"details":result}),
        ))
    }
}
pub(crate) async fn operation(
    Path((id, op)): Path<(String, String)>,
    State(state): State<AppState>,
    caller: Option<Extension<FileActor>>,
    trusted: Option<Extension<crate::auth::TrustedRelayForward>>,
) -> Result<Response, ApiErr> {
    if trusted.is_some() && caller.is_none() {
        return Err(err(
            StatusCode::FORBIDDEN,
            "actorUnavailable",
            "Update the relay before querying save receipts.",
        ));
    }
    let result = state
        .file_operation(&actor(&state, caller), &id, &op)
        .map_err(|_| {
            err(
                StatusCode::GONE,
                "operationExpired",
                "This save receipt is unavailable or expired. Verify disk before saving again.",
            )
        })?;
    Ok(response(StatusCode::OK, result))
}
