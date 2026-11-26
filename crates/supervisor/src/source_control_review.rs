//! The device-wide Pull requests page and PR review. Owner-only: every path is
//! under `/api/source-control/`, which no share, grant or peer reaches.
use crate::http::{map_err, ApiErr};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use pockymoe_runtime::{source_control::review::ReviewCaller, Supervisor};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub(crate) fn routes() -> Router<Arc<Supervisor>> {
    Router::new()
        .route("/api/source-control/pull-requests", get(list))
        .route("/api/source-control/pull-requests/{action}", post(action))
        .route(
            "/api/source-control/pull-request-views",
            get(views).put(save_views),
        )
}

#[derive(Deserialize)]
struct ListQuery {
    state: Option<String>,
}

async fn list(
    State(s): State<Arc<Supervisor>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ApiErr> {
    s.pull_request_page(query.state.as_deref().unwrap_or("open"))
        .await
        .map(Json)
        .map_err(map_err)
}

async fn action(
    State(s): State<Arc<Supervisor>>,
    Path(action): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    let url = input["url"]
        .as_str()
        .ok_or_else(|| map_err(anyhow::anyhow!("PR url required")))?
        .to_owned();
    s.pull_request_review(&url, &action, &input, ReviewCaller::Owner)
        .await
        .map(Json)
        .map_err(map_err)
}

async fn views(State(s): State<Arc<Supervisor>>) -> Result<Json<Value>, ApiErr> {
    s.pull_request_views().map(Json).map_err(map_err)
}

async fn save_views(
    State(s): State<Arc<Supervisor>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    s.save_pull_request_views(&input).map(Json).map_err(map_err)
}
