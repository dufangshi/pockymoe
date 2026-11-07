use crate::http::{map_err, ApiErr};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use pockymoe_runtime::Supervisor;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GitQuery {
    working_copy_id: Option<String>,
    area: Option<String>,
    base: Option<String>,
    path: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}
pub(crate) async fn context(
    State(state): State<Arc<Supervisor>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiErr> {
    tokio::task::spawn_blocking(move || state.worktree_context(&id))
        .await
        .map_err(|e| map_err(e.into()))?
        .map(Json)
        .map_err(map_err)
}
pub(crate) async fn read_git(
    State(state): State<Arc<Supervisor>>,
    Path((id, action)): Path<(String, String)>,
    Query(query): Query<GitQuery>,
) -> Result<Json<Value>, ApiErr> {
    tokio::task::spawn_blocking(move || match action.as_str() {
        "status" => state.worktree_git_status(&id, query.working_copy_id.as_deref()),
        "diff" => state.worktree_git_diff(
            &id,
            query.working_copy_id.as_deref(),
            query.area.as_deref().unwrap_or("worktree"),
            query.base.as_deref(),
            query.path.as_deref(),
        ),
        "commits" => state.worktree_git_commits(
            &id,
            query.working_copy_id.as_deref(),
            query.limit.unwrap_or(30),
            query.offset.unwrap_or(0),
        ),
        _ => anyhow::bail!("unknown Git action"),
    })
    .await
    .map_err(|e| map_err(e.into()))?
    .map(Json)
    .map_err(map_err)
}
pub(crate) async fn cli(state: &Arc<Supervisor>, input: Value) -> anyhow::Result<Value> {
    let thread = input["threadId"].as_str().unwrap_or("").to_owned();
    if input["operation"] == "worktreeUse" {
        return state
            .worktree_use(&thread, input["workingCopyId"].as_str().unwrap_or(""))
            .await;
    }
    let state = state.clone();
    tokio::task::spawn_blocking(move||{
        let id=input["workingCopyId"].as_str().unwrap_or("");
        let cwd=input["cwd"].as_str().unwrap_or("");
        Ok(match input["operation"].as_str().unwrap_or("") {
            "worktreeCreate"=>state.worktree_create(&thread,&input)?,
            "worktreeList"=>state.worktree_context(&thread)?,
            "worktreeCurrent"=>state.worktree_current(&thread,cwd)?,
            "worktreeShow"=>json!({"workingCopy":state.worktree_copy(&thread,id)?}),
            "worktreeAdopt"=>json!({"workingCopy":state.worktree_adopt(&thread,input["path"].as_str().unwrap_or(""))?}),
            "worktreeRemove"=>state.worktree_remove(&thread,id)?,
            _=>anyhow::bail!("unknown worktree operation"),
        })
    }).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn worktree_cli_binds_operations_to_the_authenticated_thread() {
        let fixture = crate::isolation_tests::setup().await;
        let state = &fixture.state;
        let open = crate::isolation_tests::thread(state, &fixture.open).await;
        let isolated = crate::isolation_tests::thread(state, &fixture.isolated).await;
        let input = json!({"operation":"worktreeList","threadId":open,"fromThreadId":isolated});
        assert!(
            crate::interaction::run(state, input, Some(isolated.clone()))
                .await
                .unwrap_err()
                .to_string()
                .contains("authenticated thread")
        );
        let input = json!({"operation":"worktreeList","threadId":open,"fromThreadId":isolated});
        assert!(crate::interaction::run(state, input, Some(open.clone()))
            .await
            .unwrap_err()
            .to_string()
            .contains("authenticated thread"));
        let input = json!({"operation":"worktreeList","threadId":open,"fromThreadId":open});
        let context = crate::interaction::run(state, input, Some(open.clone()))
            .await
            .unwrap();
        assert!(context["workingCopies"].is_array());
        assert!(crate::interaction::run_from_peer(
            state,
            json!({"operation":"worktreeList","threadId":open})
        )
        .await
        .is_err());
    }
    #[tokio::test]
    async fn worktree_http_reads_and_controlled_command_use_the_selected_checkout() {
        use axum::{
            body::{to_bytes, Body},
            http::Request,
        };
        use tower::ServiceExt;
        let fixture = crate::isolation_tests::setup().await;
        let state = &fixture.state;
        let repo = state.get_workspace(&fixture.open).unwrap().abs_path;
        for args in [
            vec!["init", "-q"],
            vec!["add", "notes.txt"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "initial",
            ],
        ] {
            let output = std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let thread = crate::isolation_tests::thread(state, &fixture.open).await;
        let created=crate::interaction::run(state,json!({"operation":"worktreeCreate","threadId":thread,"name":"http-check","cwd":repo,"clientRequestId":"one"}),Some(thread.clone())).await.unwrap();
        let copy = &created["workingCopy"];
        let id = copy["id"].as_str().unwrap();
        let path = copy["path"].as_str().unwrap();
        std::fs::write(
            std::path::Path::new(path).join("notes.txt"),
            "selected checkout\n",
        )
        .unwrap();
        std::fs::write(
            std::path::Path::new(path).join("new file.txt"),
            "new content\n",
        )
        .unwrap();
        let router = crate::http::router(state.clone());
        for (suffix, expected) in [
            ("working-context".to_owned(), "workingCopies"),
            (format!("git/status?workingCopyId={id}"), "files"),
            (
                format!("git/diff?workingCopyId={id}&area=worktree&path=new%20file.txt"),
                "patch",
            ),
            (format!("git/commits?workingCopyId={id}&limit=1"), "commits"),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/threads/{thread}/{suffix}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let data: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert!(data.get(expected).is_some(), "{data}");
            if expected == "patch" {
                assert!(data["patch"].as_str().unwrap().contains("+new content"));
            }
            if expected == "files" {
                assert_eq!(data["files"].as_array().unwrap().len(), 2);
            }
        }
        let executed=crate::interaction::run(state,json!({"operation":"commandRun","threadId":thread,"input":{"workingCopyId":id,"argv":["git","rev-parse","--show-toplevel"]}}),Some(thread.clone())).await.unwrap();
        assert_eq!(executed["exitCode"], 0);
        assert_eq!(executed["stdout"].as_str().unwrap().trim(), path);
        assert_eq!(
            std::fs::read_to_string(std::path::Path::new(&repo).join("notes.txt")).unwrap(),
            "secret"
        );
    }
}
