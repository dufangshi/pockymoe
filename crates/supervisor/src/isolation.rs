//! Owner settings for agent isolation (docs/agent-isolation.md). Routes live under
//! `/api/config`, which only the device owner reaches.
use crate::http::{map_err, ApiErr};
use axum::{
    extract::{Path, State},
    Json,
};
use pockymoe_runtime::Supervisor;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// `device` is what other devices see: also isolated while peer access is off.
fn view(state: &Supervisor) -> Value {
    let isolation = state.agent_isolation();
    json!({
        "device": isolation.device || !state.peer_access_enabled(),
        "workspaces": isolation.workspaces,
    })
}

pub(crate) async fn get(State(state): State<Arc<Supervisor>>) -> Json<Value> {
    Json(view(&state))
}

#[derive(Deserialize)]
pub(crate) struct DeviceInput {
    device: bool,
}

/// Ending isolation lets other devices in, which needs peer access on.
pub(crate) async fn set(
    State(state): State<Arc<Supervisor>>,
    Json(input): Json<DeviceInput>,
) -> Result<Json<Value>, ApiErr> {
    state.set_device_isolation(input.device).map_err(map_err)?;
    if !input.device {
        state.set_peer_access(true).map_err(map_err)?;
    }
    Ok(Json(view(&state)))
}

#[derive(Deserialize)]
pub(crate) struct WorkspaceInput {
    isolated: bool,
}

pub(crate) async fn set_workspace(
    State(state): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Json(input): Json<WorkspaceInput>,
) -> Result<Json<Value>, ApiErr> {
    state
        .set_workspace_isolation(&id, input.isolated)
        .map_err(map_err)?;
    Ok(Json(
        serde_json::to_value(state.get_workspace(&id).map_err(map_err)?)
            .map_err(|e| map_err(e.into()))?,
    ))
}
