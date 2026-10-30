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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallInput {
    source: String,
    #[serde(default, rename = "ref")]
    git_ref: Option<String>,
}

/// Owner-only (see `access::access_allows`): this clones and later runs code.
pub async fn install(
    State(state): State<AppState>,
    Json(input): Json<InstallInput>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    Ok(Json(serde_json::json!(state
        .install_external_plugin(&input.source, input.git_ref.as_deref())
        .await
        .map_err(map_err)?)))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    Ok(Json(serde_json::json!(state
        .update_external_plugin(&id)
        .await
        .map_err(map_err)?)))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceActionInput {
    #[serde(default)]
    input: serde_json::Value,
}

/// Run an installed plugin's action at device scope and return its output.
pub async fn device_action(
    State(state): State<AppState>,
    Path((id, action)): Path<(String, String)>,
    Json(input): Json<DeviceActionInput>,
) -> Result<impl axum::response::IntoResponse, ApiErr> {
    let output = state
        .run_external_plugin_action(&id, &action, input.input, "manual")
        .await
        .map_err(map_err)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({ "output": output })),
    ))
}

pub async fn runs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, ApiErr> {
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(state.external_plugin_runs(&id).map_err(map_err)?),
    ))
}

/// Owner-credential CLI operations for installed plugins and model completions.
pub(crate) async fn cli(
    state: &std::sync::Arc<pockymoe_runtime::Supervisor>,
    input: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    use anyhow::Context;
    use serde_json::json;
    let id = || input["pluginId"].as_str().context("pluginId required");
    match input["operation"].as_str().unwrap_or("") {
        "pluginList" => Ok(json!({ "plugins": state.list_plugins() })),
        "pluginInstall" => {
            let source = input["source"].as_str().context("source required")?;
            Ok(json!(
                state
                    .install_external_plugin(source, input["ref"].as_str())
                    .await?
            ))
        }
        "pluginUpdate" => Ok(json!(state.update_external_plugin(id()?).await?)),
        "pluginRemove" => {
            state.remove_external_plugin(id()?, input["purgeData"] == true)?;
            Ok(json!({ "removed": id()? }))
        }
        "pluginEnable" => {
            let enabled = input["enabled"].as_bool().context("enabled required")?;
            state.get_plugin(id()?)?;
            state.set_plugin_enabled(id()?, enabled)?;
            Ok(json!(state.get_plugin(id()?)?))
        }
        "pluginRun" => {
            let action = input["action"].as_str().context("action required")?;
            let output = state
                .run_external_plugin_action(id()?, action, input["input"].clone(), "manual")
                .await?;
            Ok(json!({ "output": output }))
        }
        "pluginRuns" => state.external_plugin_runs(id()?),
        "mcpList" => Ok(json!({
            "servers": pockymoe_runtime::acp::mcp_servers::list_redacted()?
        })),
        "mcpSet" => {
            let server: pockymoe_runtime::acp::mcp_servers::McpServer =
                serde_json::from_value(input["server"].clone()).context("invalid server")?;
            Ok(json!(pockymoe_runtime::acp::mcp_servers::upsert(server)?))
        }
        "mcpRemove" => {
            let name = input["name"].as_str().context("name required")?;
            pockymoe_runtime::acp::mcp_servers::remove(name)?;
            Ok(json!({ "removed": name }))
        }
        // A plugin's own working directory for the threads it keeps.
        "workspaceEnsure" => {
            let path = input["absPath"].as_str().context("absPath required")?;
            anyhow::ensure!(
                std::path::Path::new(path).is_absolute(),
                "absPath must be absolute"
            );
            std::fs::create_dir_all(path)?;
            let canonical = std::fs::canonicalize(path)?;
            if let Some(found) = state
                .list_workspaces()?
                .into_iter()
                .find(|w| std::path::Path::new(&w.abs_path) == canonical)
            {
                return Ok(json!(found));
            }
            Ok(json!(state.create_workspace(
                pockymoe_protocol::CreateWorkspaceInput {
                    abs_path: Some(canonical.to_string_lossy().into_owned()),
                    git_url: None,
                    label: input["label"].as_str().map(str::to_owned),
                }
            )?))
        }
        "modelComplete" => {
            let connection = input["connectionId"]
                .as_str()
                .context("connectionId required")?;
            let user = input["user"].as_str().context("user prompt required")?;
            let system = input["system"].as_str().unwrap_or("");
            anyhow::ensure!(
                system.len() + user.len() <= 262144,
                "prompt exceeds 256 KiB"
            );
            let max_tokens = input["maxTokens"].as_u64().unwrap_or(1024).clamp(1, 16384) as usize;
            let saved = pockymoe_runtime::upstreams::connections::get(
                &pockymoe_runtime::upstreams::directory(&state.config.database_url),
                connection,
            )?;
            let text = pockymoe_runtime::upstreams::connections::complete(
                &saved,
                input["model"].as_str().unwrap_or(""),
                system,
                user,
                max_tokens,
            )
            .await?;
            Ok(json!({ "text": text }))
        }
        _ => anyhow::bail!("unknown plugin operation"),
    }
}
