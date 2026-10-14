use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use pockymoe_runtime::{management, Supervisor};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn harnesses(State(state): State<Arc<Supervisor>>) -> Json<Value> {
    Json(management::inventory(&state).await)
}
pub async fn jobs(State(state): State<Arc<Supervisor>>) -> Json<Value> {
    Json(json!(*state.management_jobs.lock().unwrap()))
}

#[derive(Deserialize)]
pub struct HarnessAction {
    action: String,
    #[serde(default = "base")]
    component: String,
}
fn base() -> String {
    "base".into()
}

pub async fn harness_action(
    State(state): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Json(body): Json<HarnessAction>,
) -> Response {
    if !matches!(body.action.as_str(), "restart" | "update" | "install")
        || !matches!(body.component.as_str(), "base" | "adapter")
        || !pockymoe_runtime::acp::builtin_agents(state.config.acp_command.as_deref())
            .iter()
            .any(|d| d.id == id)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"code":"bad_request","message":"Unknown harness action"})),
        )
            .into_response();
    }
    let Ok(maintenance) = state.maintenance_gate.clone().try_read_owned() else {
        return (
            StatusCode::CONFLICT,
            Json(json!({"code":"conflict","message":"Supervisor update in progress"})),
        )
            .into_response();
    };
    let Ok(guard) = state.harness_gate(&id).try_write_owned() else {
        return (StatusCode::CONFLICT, Json(json!({"code":"conflict","message":"This harness is busy. Wait for its turns to finish or stop them before maintenance."}))).into_response();
    };
    state.management_jobs.lock().unwrap().insert(
        id.clone(),
        json!({"state":"running","action":body.action,"component":body.component}),
    );
    tokio::spawn(async move {
        let _guard = guard;
        let _maintenance = maintenance;
        let result = if body.action == "restart" {
            state.restart_harness(&id).await.map(|_| ())
        } else if body.action == "install" && body.component == "base" {
            management::install_harness(&state, &id).await
        } else {
            management::update_harness(&state, &id, &body.component).await
        };
        let value = match result {
            Ok(()) => {
                json!({"state":"completed","action":body.action,"component":body.component,"connectionVerified":body.action=="update" && body.component=="adapter"})
            }
            Err(error) => {
                json!({"state":"failed","action":body.action,"component":body.component,"error":format!("{error:#}")})
            }
        };
        state.management_jobs.lock().unwrap().insert(id, value);
    });
    (StatusCode::ACCEPTED, Json(json!({"state":"running"}))).into_response()
}

async fn updater(state: &Supervisor, action: &str) -> anyhow::Result<Value> {
    if action == "status" {
        if let Some(status) = state.supervisor_update_status.lock().unwrap().clone() {
            return Ok(status);
        }
    }
    crate::distribution::updater(state, action).await
}
pub async fn supervisor_status(State(state): State<Arc<Supervisor>>) -> Json<Value> {
    let mut value = updater(&state, "status").await.unwrap_or_else(|error| json!({"runningVersion":env!("CARGO_PKG_VERSION"),"canUpdate":false,"canRestart":false,"reason":error.to_string()}));
    value["startedAt"] = json!(state.started_at);
    value["uptimeSeconds"] = json!(state.started_instant.elapsed().as_secs());
    value["processId"] = json!(std::process::id());
    Json(value)
}

pub async fn supervisor_check(State(state): State<Arc<Supervisor>>) -> Response {
    update_response(&state, "check").await
}
pub async fn supervisor_restart(State(state): State<Arc<Supervisor>>) -> Response {
    supervisor_action(state, true).await
}
pub async fn supervisor_update(State(state): State<Arc<Supervisor>>) -> Response {
    supervisor_action(state, false).await
}
async fn supervisor_action(state: Arc<Supervisor>, restart: bool) -> Response {
    let Ok(update_lock) = state.update_lock.clone().try_lock_owned() else {
        return (
            StatusCode::CONFLICT,
            Json(
                json!({"code":"conflict","message":"A Supervisor update is already in progress."}),
            ),
        )
            .into_response();
    };
    let mut value = match updater(&state, if restart { "status" } else { "check" }).await {
        Ok(value) => value,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"code":"update_unavailable","message":error.to_string()})),
            )
                .into_response()
        }
    };
    if value["job"]["phase"].as_str().is_some_and(|phase| {
        matches!(
            phase,
            "scheduled" | "preparing" | "installing" | "restarting" | "verifying"
        )
    }) {
        return (
            StatusCode::CONFLICT,
            Json(
                json!({"code":"conflict","message":"A Supervisor update is already in progress."}),
            ),
        )
            .into_response();
    }
    if (restart && value["canRestart"] != true)
        || (!restart
            && (value["canUpdate"] != true
                || (value["nativeMigration"] != true
                    && value["latestVersion"] == value["runningVersion"]
                    && value["latestVersion"] == value["installedVersion"])))
    {
        return Json(value).into_response();
    }
    value["job"] = json!({"phase":"preparing","action":if restart {"restart"} else {"update"},"targetVersion":if restart {&value["runningVersion"]} else {&value["latestVersion"]}});
    *state.supervisor_update_status.lock().unwrap() = Some(value.clone());
    let response = value.clone();
    // Own the orchestration independently of the browser/relay HTTP request.
    tokio::spawn(async move {
        let _update_lock = update_lock;
        let result = async {
            let guard = state.prepare_update_restart().await?;
            let launch = updater(&state, if restart { "restart" } else { "launch" }).await;
            match launch {
                Ok(launched) => {
                    if launched["canUpdate"] != true {
                        anyhow::bail!(
                            "{}",
                            launched["reason"]
                                .as_str()
                                .unwrap_or("Unable to launch update worker")
                        );
                    }
                    *state.supervisor_update_status.lock().unwrap() = None;
                    if launched["job"]["restartStrategy"] == "host-managed"
                        && launched["job"]["phase"] == "restarting"
                    {
                        // Both paused-turn intent and the verification handoff
                        // have been flushed before relinquishing this process.
                        // Give the accepted HTTP response a chance to flush,
                        // especially for a restart with no installation work.
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        std::process::exit(pockymoe_runtime::HOST_MANAGED_RESTART_EXIT_CODE);
                    }
                    if launched["job"]["phase"].as_str().is_some_and(|phase| {
                        matches!(
                            phase,
                            "scheduled" | "preparing" | "installing" | "restarting" | "verifying"
                        )
                    }) {
                        let mut errors = 0;
                        loop {
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                            match updater(&state, "status").await {
                                Ok(status) => {
                                    errors = 0;
                                    if status["job"]["phase"].as_str().is_some_and(|phase| {
                                        matches!(
                                            phase,
                                            "completed"
                                                | "failed"
                                                | "rolled-back"
                                                | "rollback-failed"
                                        )
                                    }) {
                                        break;
                                    }
                                }
                                Err(error) => {
                                    errors += 1;
                                    if errors >= 10 {
                                        return Err(error);
                                    }
                                }
                            }
                        }
                    }
                    drop(guard);
                    Ok(())
                }
                Err(error) => {
                    drop(guard);
                    Err(error)
                }
            }
        }
        .await;
        if let Err(error) = result {
            value["job"] = json!({"phase":"failed","error":error.to_string()});
            *state.supervisor_update_status.lock().unwrap() = Some(value);
        }
        // The old process reaches here only when launch failed or rolled back.
        state.finish_update_attempt();
    });
    (StatusCode::ACCEPTED, Json(response)).into_response()
}

async fn update_response(state: &Supervisor, action: &str) -> Response {
    match updater(state, action).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"code":"update_unavailable","message":error.to_string()})),
        )
            .into_response(),
    }
}

/// Health/relay verification must complete before an updated process resumes tasks.
/// Otherwise a subsequent rollback could interrupt a continuation whose marker
/// has already been consumed.
pub(crate) async fn recover_after_update(state: Arc<Supervisor>) {
    if crate::distribution::host_managed_pending(&state) {
        let had_paused_tasks = state.defer_update_recovery();
        match crate::distribution::complete_host_managed_startup(&state).await {
            Ok(true) => {
                state.finish_update_attempt();
                if !had_paused_tasks {
                    state.spawn_inbox_recovery();
                }
            }
            Ok(false) => tracing::error!(
                "host-managed restart verification failed; recovery journal retained"
            ),
            Err(error) => {
                tracing::error!(%error, "host-managed restart verification failed; recovery journal retained")
            }
        }
        return;
    }
    loop {
        let active = updater(&state, "status").await.ok().is_some_and(|status| {
            status["job"]["phase"].as_str().is_some_and(|phase| {
                matches!(
                    phase,
                    "scheduled" | "preparing" | "installing" | "restarting" | "verifying"
                )
            })
        });
        if !active {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    state.finish_update_attempt();
}
