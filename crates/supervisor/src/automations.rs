use crate::http::{map_err, ApiErr};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use pockymoe_protocol::{AutomationDefinition, CommandRunInput};
use pockymoe_runtime::Supervisor;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInput {
    pub definition: AutomationDefinition,
    pub client_request_id: Option<String>,
}
#[derive(Deserialize)]
pub struct RunsQuery {
    pub limit: Option<u64>,
}
pub async fn list(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(s.automation_list(&id).map_err(map_err)?))
}
pub async fn create(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Json(v): Json<CreateInput>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        s.automation_create(&id, v.definition, v.client_request_id.as_deref())
            .map_err(map_err)?,
    ))
}
pub async fn preview(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Json(v): Json<AutomationDefinition>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(s.automation_preview(&id, v).map_err(map_err)?))
}
pub async fn show(
    State(s): State<Arc<Supervisor>>,
    Path((thread, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(s.automation_show(&thread, &id).map_err(map_err)?))
}
pub async fn control(
    State(s): State<Arc<Supervisor>>,
    Path((thread, id, op)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        s.automation_control(&thread, &id, &op).map_err(map_err)?,
    ))
}
pub async fn runs(
    State(s): State<Arc<Supervisor>>,
    Path((thread, id)): Path<(String, String)>,
    Query(q): Query<RunsQuery>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(
        s.automation_runs(&thread, &id, q.limit.unwrap_or(20))
            .map_err(map_err)?,
    ))
}
pub async fn command_run(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Json(v): Json<CommandRunInput>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(s.command_run(&id, v).await.map_err(map_err)?))
}
pub async fn command_show(
    State(s): State<Arc<Supervisor>>,
    Path((thread, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiErr> {
    Ok(Json(s.command_show(&thread, &id).map_err(map_err)?))
}

pub async fn cli(
    s: &Supervisor,
    v: &Value,
    requester: pockymoe_runtime::interaction::AgentRequester<'_>,
) -> anyhow::Result<Value> {
    let thread = v["threadId"].as_str().unwrap_or("");
    let id = v["automationId"].as_str().unwrap_or("");
    // Both the thread an automation acts in and the thread it watches.
    s.ensure_agent_reaches_thread(requester, thread)?;
    let trigger = &v["definition"]["trigger"];
    for source in ["sourceThreadId", "rootThreadId"] {
        if let Some(source) = trigger[source].as_str() {
            s.ensure_agent_reaches_thread(requester, source)?;
        }
    }
    Ok(match v["operation"].as_str().unwrap_or("") {
        "automationCreate" => s.automation_create(
            thread,
            serde_json::from_value(v["definition"].clone())?,
            v["clientRequestId"].as_str(),
        )?,
        "automationPreview" => {
            s.automation_preview(thread, serde_json::from_value(v["definition"].clone())?)?
        }
        "automationList" => s.automation_list(thread)?,
        "automationShow" => s.automation_show(thread, id)?,
        "automationRuns" => s.automation_runs(thread, id, v["limit"].as_u64().unwrap_or(20))?,
        "automationPause" => s.automation_control(thread, id, "pause")?,
        "automationResume" => s.automation_control(thread, id, "resume")?,
        "automationCancel" => s.automation_control(thread, id, "cancel")?,
        "commandRun" => {
            s.command_run(thread, serde_json::from_value(v["input"].clone())?)
                .await?
        }
        "commandShow" => s.command_show(thread, v["commandId"].as_str().unwrap_or(""))?,
        _ => json!(null),
    })
}
