//! Exercise checkpoint writes through the real Supervisor and host guardian.
#![cfg(target_os = "linux")]
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
struct Lab {
    child: Child,
    path: PathBuf,
}
impl Drop for Lab {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
#[tokio::test]
async fn checkpoint_restore_and_undo_write_files_through_live_guardian() {
    let path = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("pockymoe-checkpoint-{}", uuid::Uuid::new_v4()));
    let repo = path.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        assert!(Command::new("git")
            .current_dir(&repo)
            .args(args)
            .status()
            .unwrap()
            .success())
    };
    git(&["init", "-qb", "main"]);
    std::fs::write(repo.join("file with spaces.txt"), "initial\n").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Lab",
        "-c",
        "user.email=lab@example.invalid",
        "commit",
        "-qm",
        "initial",
    ]);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let child = Command::new(env!("CARGO_BIN_EXE_pockymoe"))
        .arg("supervisor")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", &path)
        .env("POCKYMOE_MODE", "local")
        .env("HOST", "127.0.0.1")
        .env("PORT", port.to_string())
        .env("POCKYMOE_DATABASE_PATH", path.join("db.sqlite"))
        .env("POCKYMOE_WORKSPACE_ROOT", &path)
        .env("POCKYMOE_ENABLED_AGENT_PROVIDERS", "codex")
        .env("POCKYMOE_E2E_FAKE_RUNTIME", "true")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _lab = Lab {
        child,
        path: path.clone(),
    };
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    for n in 0..200 {
        if client.get(format!("{base}/healthz")).send().await.is_ok() {
            break;
        }
        assert!(n < 199, "Supervisor did not start");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        for attempt in 0..100 {
            let response = client.post(&url).json(&body).send().await.unwrap();
            let status = response.status();
            let text = response.text().await.unwrap();
            // Settlement publishes idle before its exclusive claim is dropped.
            if status == reqwest::StatusCode::CONFLICT && text.contains("busy") && attempt < 99 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            assert!(status.is_success(), "{status}: {text}");
            return serde_json::from_str(&text).unwrap();
        }
        unreachable!()
    }
    let w = post(
        &client,
        format!("{base}/api/workspaces"),
        json!({"absPath":repo}),
    )
    .await;
    let t=post(&client,format!("{base}/api/threads/start"),json!({"workspaceId":w["id"],"provider":"codex","model":"ios-e2e-stream","approvalMode":"yolo"})).await;
    let id = t["id"].as_str().unwrap();
    post(
        &client,
        format!("{base}/api/e2e/events"),
        json!({"type":"start","threadId":id}),
    )
    .await;
    std::fs::write(repo.join("file with spaces.txt"), "changed\n").unwrap();
    std::fs::write(repo.join("new.txt"), "new\n").unwrap();
    post(
        &client,
        format!("{base}/api/e2e/events"),
        json!({"type":"complete","threadId":id,"text":"done"}),
    )
    .await;
    let cp: Value = client
        .get(format!("{base}/api/threads/{id}/checkpoints"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let restore = post(
        &client,
        format!("{base}/api/threads/{id}/checkpoints/restore"),
        json!({"turnId":cp["turns"][0]["turnId"],"phase":"start"}),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(repo.join("file with spaces.txt")).unwrap(),
        "initial\n"
    );
    assert!(!repo.join("new.txt").exists());
    post(
        &client,
        format!("{base}/api/threads/{id}/checkpoints/undo"),
        json!({"undoId":restore["undoId"]}),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(repo.join("file with spaces.txt")).unwrap(),
        "changed\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("new.txt")).unwrap(),
        "new\n"
    );
    assert!(
        std::fs::read_dir(path.join("db.sqlite.environments/host-processes"))
            .unwrap()
            .count()
            > 0,
        "must exercise live guardian"
    );
}
