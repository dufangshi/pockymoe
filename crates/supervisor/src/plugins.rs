use axum::{
    extract::{Path, State},
    http::header,
    Json,
};
use pockymoe_runtime::plugins::{PluginSettings, ThreadPluginOverride};

use crate::http::{map_err, ApiErr, AppState};

pub async fn settings(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, ApiErr> {
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(state.plugin_settings(&id).map_err(map_err)?),
    ))
}

pub async fn save_settings(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<PluginSettings>,
) -> Result<impl axum::response::IntoResponse, ApiErr> {
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(state.save_plugin_settings(&id, input).map_err(map_err)?),
    ))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionInput {
    #[serde(default)]
    input: serde_json::Value,
    client_request_id: String,
    #[serde(default = "action_timeout")]
    timeout_seconds: u64,
}
fn action_timeout() -> u64 {
    120
}

pub async fn action(
    State(state): State<AppState>,
    Path((thread, plugin, action)): Path<(String, String, String)>,
    Json(input): Json<ActionInput>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    Ok(Json(
        state
            .submit_plugin_action(
                &thread,
                &plugin,
                &action,
                input.input,
                input.timeout_seconds,
                &input.client_request_id,
            )
            .map_err(map_err)?,
    ))
}

pub async fn thread_list(
    State(state): State<AppState>,
    Path(thread): Path<String>,
) -> Result<impl axum::response::IntoResponse, ApiErr> {
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(state.list_thread_plugins(&thread).map_err(map_err)?),
    ))
}

pub async fn thread_settings(
    State(state): State<AppState>,
    Path((thread, plugin)): Path<(String, String)>,
) -> Result<impl axum::response::IntoResponse, ApiErr> {
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(
            state
                .thread_plugin_settings(&thread, &plugin)
                .map_err(map_err)?,
        ),
    ))
}

pub async fn save_thread_settings(
    State(state): State<AppState>,
    Path((thread, plugin)): Path<(String, String)>,
    Json(input): Json<ThreadPluginOverride>,
) -> Result<impl axum::response::IntoResponse, ApiErr> {
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(
            state
                .save_thread_plugin_settings(&thread, &plugin, input)
                .map_err(map_err)?,
        ),
    ))
}
