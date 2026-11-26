//! Host credentials are owner-only; agent tools are restricted to their own thread.
use crate::http::{map_err, ApiErr};
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use pockymoe_runtime::Supervisor;
use serde_json::{json, Value};
use std::sync::Arc;
pub(crate) fn routes() -> Router<Arc<Supervisor>> {
    Router::new()
        .route("/api/source-control/settings", get(settings).put(save))
        .route("/api/source-control/scan", post(scan))
        .route("/api/source-control/clone", post(clone))
        .route("/api/source-control/jobs/{id}", get(job))
        .route("/api/threads/{id}/pull-requests", get(links).post(create))
        .route("/api/threads/{id}/source-control/{action}", post(action))
}
async fn settings(State(s): State<Arc<Supervisor>>) -> Result<Json<Value>, ApiErr> {
    s.source_control_settings().map(Json).map_err(map_err)
}
async fn save(
    State(s): State<Arc<Supervisor>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    let _guard = s.upstream_gate.lock().await;
    s.save_source_control_settings(&input)
        .map(Json)
        .map_err(map_err)
}
async fn links(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiErr> {
    s.source_links(&id)
        .map(|links| Json(json!({"links":links})))
        .map_err(map_err)
}
async fn create(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    s.source_control_action(&id, "create", input)
        .await
        .map(Json)
        .map_err(map_err)
}
async fn action(
    State(s): State<Arc<Supervisor>>,
    Path((id, action)): Path<(String, String)>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    s.source_control_action(&id, &action, input)
        .await
        .map(Json)
        .map_err(map_err)
}

async fn scan(State(s): State<Arc<Supervisor>>) -> Result<Json<Value>, ApiErr> {
    s.rescan_source_control().await.map(Json).map_err(map_err)
}
async fn clone(
    State(s): State<Arc<Supervisor>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    let url = input["url"]
        .as_str()
        .ok_or_else(|| map_err(anyhow::anyhow!("Git URL required")))?
        .to_owned();
    pockymoe_runtime::source_control::Repository::parse(&url).map_err(map_err)?;
    let label = input["label"].as_str().map(str::to_owned);
    let id = uuid::Uuid::new_v4().to_string();
    let key = format!("source-job:{id}");
    s.db.set_kv(&key, &json!({"status":"running"}).to_string())
        .map_err(map_err)?;
    tokio::spawn(async move {
        let result = match s.clone_source_workspace(&url, label).await {
            Ok(result) => json!({"status":"completed","result":result}),
            Err(error) => json!({"status":"failed","error":error.to_string()}),
        };
        let _ = s.db.set_kv(&key, &result.to_string());
    });
    Ok(Json(json!({"jobId":id})))
}
async fn job(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiErr> {
    let raw =
        s.db.get_kv(&format!("source-job:{id}"))
            .map_err(map_err)?
            .ok_or_else(|| map_err(anyhow::anyhow!("Clone job not found")))?;
    serde_json::from_str(&raw)
        .map(Json)
        .map_err(|e| map_err(e.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn source_control_cli_confines_credentials_to_own_thread_and_never_returns_tokens() {
        let fixture = crate::isolation_tests::setup().await;
        let s = &fixture.state;
        let own = crate::isolation_tests::thread(s, &fixture.open).await;
        let other = crate::isolation_tests::thread(s, &fixture.isolated).await;
        let input =
            json!({"operation":"sourceControl","action":"links","threadId":own,"fromThreadId":own});
        assert!(
            crate::interaction::run(s, input.clone(), Some(other.clone()))
                .await
                .is_err()
        );
        assert!(crate::interaction::run_from_peer(s, input.clone())
            .await
            .is_err());
        assert!(crate::interaction::run(s, input, Some(own.clone()))
            .await
            .unwrap()["links"]
            .is_array());
        let result=s.save_source_control_settings(&json!({"host":"github.com","kind":"github","token":"fixture-token-never-returned"})).unwrap();
        assert!(!result.to_string().contains("fixture-token-never-returned"));
        assert_eq!(result["hosts"][0]["hasToken"], true);
        let url = "https://github.com/dufangshi/pockymoe/pull/165";
        s.source_link(&own, url, false).unwrap();
        s.source_link(&own, url, false).unwrap();
        assert_eq!(s.source_links(&own).unwrap().len(), 1);
        assert!(s.source_links(&other).unwrap().is_empty());
        s.source_link(&own, url, true).unwrap();
        assert!(s.source_links(&own).unwrap().is_empty());
        assert!(s.source_auto_link(&own, "echo text", url).is_ok());
        assert!(s.source_links(&own).unwrap().is_empty());
        s.source_auto_link(&own, "gh pr create", url).unwrap();
        assert_eq!(s.source_links(&own).unwrap().len(), 1);
    }
    #[tokio::test]
    async fn source_control_watch_survives_reopen_and_stops_on_manual_archive_delete() {
        let fixture = crate::isolation_tests::setup().await;
        let directory = tempfile::tempdir().unwrap();
        let mut config = fixture.state.config.clone();
        config.database_url = directory.path().join("restart.sqlite");
        config.workspace_root = directory.path().join("workspaces");
        let open = || {
            Supervisor::new(
                config.clone(),
                pockymoe_runtime::Database::open(&config.database_url).unwrap(),
                vec![Arc::new(pockymoe_runtime::fake::FakeRuntime::new(
                    pockymoe_protocol::Provider::Codex,
                ))],
            )
        };
        let s = open();
        let workspace = s
            .create_workspace(pockymoe_protocol::CreateWorkspaceInput {
                abs_path: Some(directory.path().to_string_lossy().into_owned()),
                git_url: None,
                label: None,
            })
            .unwrap()
            .id;
        let thread = crate::isolation_tests::thread(&s, &workspace).await;
        let repository =
            pockymoe_runtime::source_control::Repository::parse("https://github.com/fixture/repo")
                .unwrap();
        let key = format!("pr-watch:{thread}");
        let saved = json!({"repository":repository,"number":42,"state":{"active":true,"failures":3,"commentWakes":4,"stoppedReason":null,"previous":{"state":"open","checks":"pending","comments":[],"conflicts":false,"queued":true}}});
        s.db.set_kv(&key, &saved.to_string()).unwrap();
        drop(s);
        let s = open();
        assert_eq!(
            s.source_watch_status(&thread).unwrap()["state"]["failures"],
            3
        );
        assert_eq!(
            s.source_watch_status(&thread).unwrap()["state"]["commentWakes"],
            4
        );
        s.source_unwatch(&thread).unwrap();
        assert_eq!(
            s.source_watch_status(&thread).unwrap()["state"]["active"],
            false
        );
        s.db.set_kv(&key, &saved.to_string()).unwrap();
        s.db.with(|c| {
            c.execute(
                "UPDATE threads SET closed_at='2026-11-26T00:00:00Z' WHERE id=?1",
                [&thread],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            s.poll_source_watch(&thread).await.unwrap()["state"]["active"],
            false
        );
        s.db.set_kv(&key, &saved.to_string()).unwrap();
        s.delete_thread(&thread).unwrap();
        assert_eq!(
            s.poll_source_watch(&thread).await.unwrap()["state"]["active"],
            false
        );
        let child = crate::isolation_tests::thread(&s, &workspace).await;
        let parent = crate::isolation_tests::thread(&s, &workspace).await;
        s.db.with(|c| {
            c.execute(
                "UPDATE threads SET parent_thread_id=?2 WHERE id=?1",
                rusqlite::params![child, parent],
            )?;
            Ok(())
        })
        .unwrap();
        let error = s.source_watch(&child, &repository, 42).await.unwrap_err();
        assert!(error.to_string().contains("Sub-agents"));
    }
}

#[cfg(test)]
mod live_forgejo_tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires throwaway local Forgejo on port 19123; fake token file supplied explicitly"]
    async fn source_control_local_forgejo_pr_lifecycle() {
        use std::process::Command;
        let token_path =
            std::env::var("GIT_PR_FIXTURE_TOKEN_FILE").expect("fixture token file required");
        let token = std::fs::read_to_string(token_path).unwrap();
        let fixture = crate::isolation_tests::setup().await;
        let s = &fixture.state;
        let thread = crate::isolation_tests::thread(s, &fixture.open).await;
        let repo = s.get_workspace(&fixture.open).unwrap().abs_path;
        let origin = "http://127.0.0.1:19123/fixture/parity.git";
        let status = Command::new("git")
            .current_dir(&repo)
            .args(["init", "-q", "-b", "fixture"])
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .current_dir(&repo)
            .args(["remote", "add", "origin", origin])
            .status()
            .unwrap();
        assert!(status.success());
        s.save_source_control_settings(
            &json!({"host":"127.0.0.1:19123","kind":"forgejo","token":token.trim()}),
        )
        .unwrap();
        let result = s
            .source_control_action(&thread, "show", json!({"number":1}))
            .await
            .unwrap();
        let sha = result["data"]["head"]["sha"].as_str().unwrap();
        let work = s
            .source_control_action(&thread, "workOn", json!({"number":1,"name":"fixture-pr"}))
            .await
            .unwrap();
        let child = work["thread"]["id"].as_str().unwrap();
        assert_ne!(child, thread);
        assert_eq!(s.source_links(child).unwrap().len(), 1);
        assert_eq!(
            s.get_thread(child).unwrap().worktree_path.as_deref(),
            work["workingCopy"]["path"].as_str()
        );

        let bad = s
            .source_control_action(
                &thread,
                "merge",
                json!({"number":1,"expectedHead":"stale","method":"squash"}),
            )
            .await
            .unwrap_err();
        assert!(bad.to_string().contains("head moved"));
        s.source_control_action(
            &thread,
            "comment",
            json!({"number":1,"body":"fixture comment"}),
        )
        .await
        .unwrap();
        s.source_control_action(&thread, "watch", json!({"number":1}))
            .await
            .unwrap();
        assert_eq!(
            s.source_watch_status(&thread).unwrap()["state"]["active"],
            true
        );
        let reviewer_path =
            std::env::var("GIT_PR_REVIEWER_TOKEN_FILE").expect("reviewer token file required");
        let reviewer = std::fs::read_to_string(reviewer_path).unwrap();
        s.save_source_control_settings(
            &json!({"host":"127.0.0.1:19123","kind":"forgejo","token":reviewer.trim()}),
        )
        .unwrap();
        s.source_control_action(
            &thread,
            "comment",
            json!({"number":1,"body":"reviewer wake fixture"}),
        )
        .await
        .unwrap();
        s.save_source_control_settings(
            &json!({"host":"127.0.0.1:19123","kind":"forgejo","token":token.trim()}),
        )
        .unwrap();
        let wake = s.poll_source_watch(&thread).await.unwrap();
        assert_eq!(wake["reasons"], json!(["New comments or reviews"]));
        let queued:i64=s.db.with(|c|Ok(c.query_row("SELECT COUNT(*) FROM thread_pending_steers WHERE thread_id=?1 AND submitted_prompt LIKE 'PR watch:%'",[&thread],|r|r.get(0))?)).unwrap();
        assert_eq!(queued, 1);
        assert!(s.poll_source_watch(&thread).await.unwrap()["reasons"]
            .as_array()
            .unwrap()
            .is_empty());
        s.source_control_action(
            &thread,
            "merge",
            json!({"number":1,"expectedHead":sha,"method":"squash"}),
        )
        .await
        .unwrap();
        let terminal = s.poll_source_watch(&thread).await.unwrap();
        assert_eq!(terminal["state"]["active"], false);
        assert_eq!(terminal["reasons"], json!(["PR merged"]));
        let repository = pockymoe_runtime::source_control::Repository::parse(origin).unwrap();
        // A new watch of an ended PR is refused rather than silently polling it.
        assert!(s.source_watch(&thread, &repository, 1).await.is_err());
        let comments = s
            .source_control_action(&thread, "comments", json!({"number":1}))
            .await
            .unwrap();
        assert!(comments["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["body"] == "fixture comment"));
    }
}
