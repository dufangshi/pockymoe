//! A new thread in its own managed worktree (the new-thread forms' route).
use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Provider};
use pockymoe_runtime::{fake::FakeRuntime, Database, NewThreadWorktree, RuntimeConfig, Supervisor};
use std::{path::Path, process::Command, sync::Arc};

fn setup() -> (tempfile::TempDir, Arc<Supervisor>) {
    let dir = tempfile::tempdir().unwrap();
    let config = RuntimeConfig {
        treer_enabled: false,
        host_mode: Default::default(),
        mode: pockymoe_protocol::Mode::Local,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root: dir.path().into(),
        database_url: dir.path().join("db.sqlite"),
        app_name: "test".into(),
        app_version: "test".into(),
        environment: "test".into(),
        auth_required: false,
        admin_username: None,
        admin_password: None,
        session_secret: None,
        relay_server_url: None,
        web_origin: None,
        relay_agent_token: None,
        enabled_providers: vec![Provider::Codex],
        acp_command: None,
        acp_startup_timeout_ms: 1000,
        fake_runtime: true,
        supervisor_restart_strategy: Default::default(),
    };
    let db = Database::open(&config.database_url).unwrap();
    let state = Arc::new(Supervisor::new(
        config,
        db,
        vec![Arc::new(FakeRuntime::new(Provider::Codex))],
    ));
    state.spawn_live_item_persister();
    (dir, state)
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=t@t", "-c", "user.name=t"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn repo_workspace(s: &Supervisor, root: &Path) -> (String, std::path::PathBuf) {
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join(".pockymoe")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "second"]);
    std::fs::write(
        repo.join(".pockymoe/worktree.json"),
        r#"{"commands":["printf ready > setup-ran.txt; echo set up"]}"#,
    )
    .unwrap();
    let ws = s
        .create_workspace(CreateWorkspaceInput {
            abs_path: Some(repo.to_string_lossy().into()),
            git_url: None,
            label: Some("repo".into()),
        })
        .unwrap();
    (ws.id, repo)
}

fn input(ws: &str, title: Option<&str>) -> CreateThreadInput {
    CreateThreadInput {
        workspace_id: ws.into(),
        title: title.map(str::to_owned),
        provider: Some(Provider::Codex),
        agent_id: Some("codex".into()),
        model: "ios-e2e-stream".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: None,
    }
}

#[tokio::test]
async fn a_new_thread_gets_its_own_worktree_with_setup() {
    let (dir, s) = setup();
    let (ws, repo) = repo_workspace(&s, dir.path());
    let first_commit = git(&repo, &["rev-parse", "HEAD~1"]);

    // Named after the title, on agent/NAME, from HEAD, with the setup's outcome.
    let created = s
        .start_thread_in_worktree(
            input(&ws, Some("Fix the login")),
            NewThreadWorktree::default(),
        )
        .await
        .unwrap();
    let path = dir.path().join("repo.worktrees/fix-the-login");
    assert_eq!(created["worktree"]["path"], path.to_string_lossy().as_ref());
    assert_eq!(created["worktree"]["branch"], "agent/fix-the-login");
    assert_eq!(
        created["thread"]["worktreePath"],
        path.to_string_lossy().as_ref()
    );
    assert_eq!(created["setup"]["completed"], true);
    assert_eq!(
        std::fs::read_to_string(path.join("setup-ran.txt")).unwrap(),
        "ready"
    );
    assert_eq!(
        git(&path, &["rev-parse", "HEAD"]),
        git(&repo, &["rev-parse", "HEAD"])
    );

    // A named branch from a base revision, retried safely with a request id.
    let request = NewThreadWorktree {
        name: Some("review".into()),
        branch: Some("feature/review".into()),
        base: Some(first_commit.clone()),
        client_request_id: Some("form-1".into()),
    };
    let one = s
        .start_thread_in_worktree(input(&ws, None), request.clone())
        .await
        .unwrap();
    let again = s
        .start_thread_in_worktree(input(&ws, None), request.clone())
        .await
        .unwrap();
    assert_eq!(one["created"], true);
    assert_eq!(again["created"], false);
    assert_eq!(one["thread"]["id"], again["thread"]["id"]);
    let review = dir.path().join("repo.worktrees/review");
    assert_eq!(git(&review, &["rev-parse", "HEAD"]), first_commit);
    assert_eq!(
        git(&review, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "feature/review"
    );
    let reused = s
        .start_thread_in_worktree(
            input(&ws, None),
            NewThreadWorktree {
                base: None,
                ..request.clone()
            },
        )
        .await
        .unwrap_err();
    assert!(reused.to_string().contains("request ID reused"), "{reused}");

    // An existing branch is checked out as is, so a base is refused, and the
    // request id stays free for a corrected retry.
    let refused = s
        .start_thread_in_worktree(
            input(&ws, None),
            NewThreadWorktree {
                name: Some("again".into()),
                branch: Some("feature/review".into()),
                base: Some("HEAD".into()),
                client_request_id: Some("form-2".into()),
            },
        )
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("already exists"), "{refused}");
    assert!(!dir.path().join("repo.worktrees/again").exists());
    let missing = s
        .start_thread_in_worktree(
            input(&ws, None),
            NewThreadWorktree {
                name: Some("again".into()),
                base: Some("no-such-revision".into()),
                client_request_id: Some("form-2".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("not a commit"), "{missing}");
    let fixed = s
        .start_thread_in_worktree(
            input(&ws, None),
            NewThreadWorktree {
                name: Some("again".into()),
                client_request_id: Some("form-2".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(fixed["worktree"]["branch"], "agent/again");

    // A bad name and a delegate's parent are refused before anything is made.
    let bad = s
        .start_thread_in_worktree(
            input(&ws, None),
            NewThreadWorktree {
                name: Some("Bad Name".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(bad.to_string().contains("name must match"), "{bad}");
    let parent = one["thread"]["id"].as_str().unwrap().to_owned();
    let delegated = s
        .start_thread_in_worktree(
            CreateThreadInput {
                parent_thread_id: Some(parent),
                ..input(&ws, None)
            },
            NewThreadWorktree::default(),
        )
        .await
        .unwrap_err();
    assert!(delegated.to_string().contains("no parent"), "{delegated}");
}

#[tokio::test]
async fn a_failed_setup_keeps_the_thread_and_says_why() {
    let (dir, s) = setup();
    let (ws, repo) = repo_workspace(&s, dir.path());
    std::fs::write(
        repo.join(".pockymoe/worktree.json"),
        r#"{"commands":["echo half; exit 4"]}"#,
    )
    .unwrap();
    let created = s
        .start_thread_in_worktree(
            input(&ws, Some("broken setup")),
            NewThreadWorktree::default(),
        )
        .await
        .unwrap();
    assert_eq!(created["setup"]["completed"], false);
    assert!(created["setup"]["error"]
        .as_str()
        .unwrap()
        .contains("failed"));
    let id = created["thread"]["id"].as_str().unwrap();
    assert!(s.get_thread(id).unwrap().worktree_path.is_some());
}

#[tokio::test]
async fn a_workspace_without_git_is_refused() {
    let (dir, s) = setup();
    let plain = dir.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let ws = s
        .create_workspace(CreateWorkspaceInput {
            abs_path: Some(plain.to_string_lossy().into()),
            git_url: None,
            label: Some("plain".into()),
        })
        .unwrap();
    let error = s
        .start_thread_in_worktree(input(&ws.id, None), NewThreadWorktree::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("git repository"), "{error}");
}
