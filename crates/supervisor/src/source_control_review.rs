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

#[cfg(test)]
mod tests {
    use serde_json::json;

    /// Agents reach review state only through their own thread, and their
    /// drafts never mix with the owner's (no host call is needed for drafts).
    #[tokio::test]
    async fn review_drafts_are_per_author_and_confined_to_the_calling_thread() {
        let fixture = crate::isolation_tests::setup().await;
        let s = &fixture.state;
        let repo = s.get_workspace(&fixture.open).unwrap().abs_path;
        for args in [
            vec!["init", "-q"],
            vec![
                "remote",
                "add",
                "origin",
                "https://github.com/fixture/parser.git",
            ],
        ] {
            assert!(std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let own = crate::isolation_tests::thread(s, &fixture.open).await;
        let other = crate::isolation_tests::thread(s, &fixture.isolated).await;
        let draft = json!({"operation":"sourceControl","action":"review","reviewAction":"draftSave","number":7,"kind":"line","path":"src/a.rs","line":3,"body":"Agent note","threadId":own,"fromThreadId":own});
        let saved = crate::interaction::run(s, draft.clone(), Some(own.clone()))
            .await
            .unwrap();
        assert_eq!(saved["drafts"].as_array().unwrap().len(), 1);
        // Another thread's credential cannot act on this thread's PR review.
        assert!(
            crate::interaction::run(s, draft.clone(), Some(other.clone()))
                .await
                .is_err()
        );
        assert!(crate::interaction::run_from_peer(s, draft).await.is_err());
        let url = "https://github.com/fixture/parser/pull/7";
        let owner = s
            .pull_request_review(
                url,
                "drafts",
                &json!({}),
                pockymoe_runtime::source_control::review::ReviewCaller::Owner,
            )
            .await
            .unwrap();
        assert_eq!(
            owner["drafts"],
            json!([]),
            "the owner does not see the agent's draft"
        );
        let agent = s
            .pull_request_review(
                url,
                "drafts",
                &json!({}),
                pockymoe_runtime::source_control::review::ReviewCaller::Agent(&own),
            )
            .await
            .unwrap();
        assert_eq!(agent["drafts"][0]["body"], "Agent note");
        // Linking and starting work stay owner actions with their own commands.
        let link = json!({"operation":"sourceControl","action":"review","reviewAction":"workOn","number":7,"threadId":own,"fromThreadId":own});
        assert!(crate::interaction::run(s, link, Some(own.clone()))
            .await
            .is_err());
    }
}
