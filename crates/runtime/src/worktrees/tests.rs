use super::*;
use crate::{fake::FakeRuntime, local_sessions::LocalSessionHomes, Database, RuntimeConfig};
use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Mode, Provider};
use std::sync::Arc;

struct Fixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    state: Arc<Supervisor>,
    workspace: String,
    thread: String,
}

fn run_git(root: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .args([
            "-c",
            "core.hooksPath=",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=Worktree Test",
            "-c",
            "user.email=worktree@example.invalid",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_CONFIG_COUNT",
    ] {
        command.env_remove(key);
    }
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("project with spaces");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let repo = std::fs::canonicalize(repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "status.renames", "true"]);
        std::fs::write(repo.join("tracked.txt"), "initial\n").unwrap();
        std::fs::write(repo.join("old name.txt"), "rename me\n").unwrap();
        std::fs::write(repo.join("src/file.rs"), "fn initial() {}\n").unwrap();
        std::fs::write(repo.join(".gitignore"), "ignored.env\n").unwrap();
        run_git(
            &repo,
            &[
                "add",
                "tracked.txt",
                "old name.txt",
                "src/file.rs",
                ".gitignore",
            ],
        );
        run_git(&repo, &["commit", "-qm", "initial"]);

        let config = RuntimeConfig {
            treer_enabled: false,
            host_mode: Default::default(),
            mode: Mode::Local,
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
        let state = Arc::new(
            Supervisor::new(
                config,
                db,
                vec![Arc::new(FakeRuntime::new(Provider::Codex))],
            )
            .with_local_session_homes(LocalSessionHomes {
                codex_home: dir.path().join("codex"),
                grok_home: dir.path().join("grok"),
                claude_home: dir.path().join("claude"),
            }),
        );
        let workspace = state
            .create_workspace(CreateWorkspaceInput {
                abs_path: Some(repo.to_string_lossy().into_owned()),
                git_url: None,
                label: None,
            })
            .unwrap()
            .id;
        let thread = new_thread(&state, &workspace).await;
        Self {
            _dir: dir,
            repo,
            state,
            workspace,
            thread,
        }
    }

    fn create(&self, name: &str) -> WorkingCopy {
        let result = self
            .state
            .worktree_create(&self.thread, &json!({"name":name,"cwd":self.repo}))
            .unwrap();
        serde_json::from_value(result["workingCopy"].clone()).unwrap()
    }

    fn write_commit(&self, content: &str, subject: &str) -> String {
        std::fs::write(self.repo.join("tracked.txt"), content).unwrap();
        run_git(&self.repo, &["add", "tracked.txt"]);
        run_git(&self.repo, &["commit", "-qm", subject]);
        run_git(&self.repo, &["rev-parse", "HEAD"])
    }
}

async fn new_thread(state: &Supervisor, workspace: &str) -> String {
    state
        .create_thread(CreateThreadInput {
            workspace_id: workspace.into(),
            title: None,
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: None,
        })
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn creation_retries_keep_one_checkout_and_reject_changed_request_input() {
    let f = Fixture::new().await;
    let request = json!({"name":"fix","cwd":f.repo,"clientRequestId":"checkout-once"});
    let first = f.state.worktree_create(&f.thread, &request).unwrap();
    let id = first["workingCopy"]["id"].as_str().unwrap();
    let path = first["workingCopy"]["path"].as_str().unwrap();
    assert_eq!(first["created"], true);
    assert_eq!(first["sessionCwdUpdated"], false);
    assert!(!Path::new(path).starts_with(&f.repo));
    assert!(Path::new(path).join("tracked.txt").exists());
    assert!(f
        .state
        .get_thread(&f.thread)
        .unwrap()
        .worktree_path
        .is_none());

    let retry = f.state.worktree_create(&f.thread, &request).unwrap();
    assert_eq!(retry["created"], false);
    assert_eq!(retry["workingCopy"]["id"], id);
    assert_eq!(
        run_git(&f.repo, &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        2
    );
    let mut changed = request;
    changed["base"] = json!("main");
    assert!(f
        .state
        .worktree_create(&f.thread, &changed)
        .unwrap_err()
        .to_string()
        .contains("different input"));
}

#[tokio::test]
async fn source_checkout_head_is_used_without_copying_uncommitted_changes() {
    let f = Fixture::new().await;
    let source = f.create("source");
    let source_root = Path::new(&source.path);
    std::fs::write(source_root.join("tracked.txt"), "source commit\n").unwrap();
    run_git(source_root, &["add", "tracked.txt"]);
    run_git(source_root, &["commit", "-qm", "source only"]);
    let source_head = run_git(source_root, &["rev-parse", "HEAD"]);
    std::fs::write(
        source_root.join("tracked.txt"),
        "uncommitted secret setup\n",
    )
    .unwrap();
    let result = f
        .state
        .worktree_create(&f.thread, &json!({"name":"derived","cwd":source.path}))
        .unwrap();
    let root = Path::new(result["workingCopy"]["path"].as_str().unwrap());
    assert_eq!(run_git(root, &["rev-parse", "HEAD"]), source_head);
    assert_eq!(
        std::fs::read_to_string(root.join("tracked.txt")).unwrap(),
        "source commit\n"
    );
    assert_eq!(
        std::fs::read_to_string(source_root.join("tracked.txt")).unwrap(),
        "uncommitted secret setup\n"
    );
}

#[tokio::test]
async fn adopting_a_sibling_identifies_actual_cwd_without_acquiring_cleanup_rights() {
    let f = Fixture::new().await;
    let sibling = f.repo.parent().unwrap().join("externally created checkout");
    run_git(
        &f.repo,
        &[
            "worktree",
            "add",
            "--detach",
            sibling.to_str().unwrap(),
            "HEAD",
        ],
    );
    let adopted = f
        .state
        .worktree_adopt(&f.thread, sibling.to_str().unwrap())
        .unwrap();
    assert!(!adopted.managed);
    let current = f
        .state
        .worktree_current(&f.thread, sibling.join("src").to_str().unwrap())
        .unwrap();
    assert_eq!(current["currentWorkingCopy"]["id"], adopted.id);
    assert_ne!(current["primaryWorkingCopyId"], adopted.id);
    assert_eq!(current["configuredCwd"], f.repo.to_str().unwrap());
    assert!(f
        .state
        .worktree_remove(&f.thread, &adopted.id)
        .unwrap_err()
        .to_string()
        .contains("creating thread"));
    assert!(sibling.join("tracked.txt").exists());

    let unrelated = f.repo.parent().unwrap().join("unrelated");
    std::fs::create_dir_all(&unrelated).unwrap();
    run_git(&unrelated, &["init", "-q"]);
    assert!(f
        .state
        .worktree_adopt(&f.thread, unrelated.to_str().unwrap())
        .unwrap_err()
        .to_string()
        .contains("repository"));
}

#[tokio::test]
async fn git_status_preserves_rename_paths_and_untracked_files() {
    let f = Fixture::new().await;
    run_git(&f.repo, &["mv", "old name.txt", "renamed name.txt"]);
    std::fs::write(f.repo.join("untracked file.txt"), "new file\n").unwrap();
    let status = f.state.worktree_git_status(&f.thread, None).unwrap();
    let files = status["files"].as_array().unwrap();
    let rename = files
        .iter()
        .find(|file| file["path"] == "renamed name.txt")
        .unwrap();
    assert_eq!(rename["indexStatus"], "R");
    assert_eq!(rename["originalPath"], "old name.txt");
    let untracked = files
        .iter()
        .find(|file| file["path"] == "untracked file.txt")
        .unwrap();
    assert_eq!(untracked["indexStatus"], "?");
    assert_eq!(untracked["worktreeStatus"], "?");
}

#[tokio::test]
async fn staged_and_worktree_diff_read_their_own_git_states() {
    let f = Fixture::new().await;
    std::fs::write(f.repo.join("tracked.txt"), "staged content\n").unwrap();
    run_git(&f.repo, &["add", "tracked.txt"]);
    std::fs::write(f.repo.join("tracked.txt"), "unstaged content\n").unwrap();
    let staged = f
        .state
        .worktree_git_diff(&f.thread, None, "index", None, Some("tracked.txt"))
        .unwrap();
    let staged = staged["patch"].as_str().unwrap();
    assert!(staged.contains("+staged content"));
    assert!(!staged.contains("unstaged content"));
    let working = f
        .state
        .worktree_git_diff(&f.thread, None, "worktree", None, Some("tracked.txt"))
        .unwrap();
    let working = working["patch"].as_str().unwrap();
    assert!(working.contains("-staged content"));
    assert!(working.contains("+unstaged content"));
    assert!(f
        .state
        .worktree_git_diff(&f.thread, None, "worktree", None, Some("../outside.txt"))
        .is_err());
    assert!(f
        .state
        .worktree_git_diff(&f.thread, None, "worktree", None, f.repo.to_str())
        .is_err());
}

#[tokio::test]
async fn untracked_previews_bound_utf8_output_and_do_not_follow_symlinks() {
    let f = Fixture::new().await;
    std::fs::write(f.repo.join("large.txt"), "é\n".repeat(OUTPUT_LIMIT / 3)).unwrap();
    let result = f
        .state
        .worktree_git_diff(&f.thread, None, "worktree", None, Some("large.txt"))
        .unwrap();
    assert_eq!(result["truncated"], true);
    assert!(result["patch"].as_str().unwrap().len() <= OUTPUT_LIMIT);
    #[cfg(unix)]
    {
        let secret = f.repo.parent().unwrap().join("outside-secret");
        std::fs::write(&secret, "private contents must stay outside the patch").unwrap();
        std::os::unix::fs::symlink(&secret, f.repo.join("link")).unwrap();
        let result = f
            .state
            .worktree_git_diff(&f.thread, None, "worktree", None, Some("link"))
            .unwrap();
        assert!(result["patch"].as_str().unwrap().contains("outside-secret"));
        assert!(!result["patch"]
            .as_str()
            .unwrap()
            .contains("private contents"));
    }
}

#[tokio::test]
async fn tool_locations_persist_before_background_discovery_updates_the_context() {
    let f = Fixture::new().await;
    f.state.db.with(|c| {
        c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('location-turn',?1,'inProgress',1)", [&f.thread])?;
        Ok(())
    }).unwrap();
    f.state.spawn_live_item_persister();
    let mut events = f.state.bus.subscribe();
    f.state.bus.emit(ThreadEventEnvelope {
        event_type: ThreadEventType::ThreadItemStarted,
        thread_id: f.thread.clone(),
        timestamp: now_rfc3339(),
        payload: json!({"turnId":"location-turn","item":{"id":"location-tool","kind":"toolCall","text":"read","workingLocations":[{"path":f.repo.join("tracked.txt"),"source":"file","agentId":"child"}]}}),
    });
    f.state
        .db
        .with(|c| {
            let saved: String = c.query_row(
                "SELECT item_json FROM thread_history_items WHERE item_id='location-tool'",
                [],
                |r| r.get(0),
            )?;
            assert!(saved.contains("workingLocations"));
            let observed: i64 =
                c.query_row("SELECT COUNT(*) FROM working_copy_observations", [], |r| {
                    r.get(0)
                })?;
            assert_eq!(observed, 0, "Git probing must not block event persistence");
            Ok(())
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.payload["reason"] == "workingCopyChanged" {
                break;
            }
        }
    })
    .await
    .unwrap();
    let context = f.state.worktree_context(&f.thread).unwrap();
    assert_eq!(context["observations"][0]["toolCallId"], "location-tool");
    assert_eq!(context["observations"][0]["agentId"], "child");
    assert_eq!(context["configuredCwd"], f.repo.to_str().unwrap());
}

#[tokio::test]
async fn commit_pages_preserve_order_and_report_the_last_page() {
    let f = Fixture::new().await;
    let initial = run_git(&f.repo, &["rev-parse", "HEAD"]);
    let second = f.write_commit("second\n", "second subject");
    let third = f.write_commit("third\n", "third subject");
    for (offset, oid, subject, more) in [
        (0, third, "third subject", true),
        (1, second, "second subject", true),
        (2, initial, "initial", false),
    ] {
        let page = f
            .state
            .worktree_git_commits(&f.thread, None, 1, offset)
            .unwrap();
        assert_eq!(page["commits"].as_array().unwrap().len(), 1);
        assert_eq!(page["commits"][0]["oid"], oid);
        assert_eq!(page["commits"][0]["subject"], subject);
        assert_eq!(page["hasMore"], more);
    }
    let empty = f.state.worktree_git_commits(&f.thread, None, 1, 3).unwrap();
    assert!(empty["commits"].as_array().unwrap().is_empty());
    assert_eq!(empty["hasMore"], false);
}

#[tokio::test]
async fn removal_preserves_dirty_and_running_checkouts_then_keeps_the_branch() {
    let f = Fixture::new().await;
    let copy = f.create("cleanup");
    let root = Path::new(&copy.path);
    std::fs::write(root.join("untracked.txt"), "keep me\n").unwrap();
    let dirty = f.state.worktree_remove(&f.thread, &copy.id).unwrap_err();
    assert!(
        dirty.to_string().contains("uncommitted or untracked"),
        "{dirty:#}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("untracked.txt")).unwrap(),
        "keep me\n"
    );
    std::fs::remove_file(root.join("untracked.txt")).unwrap();
    std::fs::write(root.join("ignored.env"), "fixture local config\n").unwrap();
    let ignored = f.state.worktree_remove(&f.thread, &copy.id).unwrap_err();
    assert!(
        ignored.to_string().contains("uncommitted or untracked"),
        "{ignored:#}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("ignored.env")).unwrap(),
        "fixture local config\n"
    );
    std::fs::remove_file(root.join("ignored.env")).unwrap();
    f.state
        .db
        .with(|c| {
            c.execute(
                "UPDATE threads SET status='running' WHERE id=?1",
                [&f.thread],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(f
        .state
        .worktree_remove(&f.thread, &copy.id)
        .unwrap_err()
        .to_string()
        .contains("running"));
    assert!(root.exists());
    f.state
        .db
        .with(|c| {
            c.execute("UPDATE threads SET status='idle' WHERE id=?1", [&f.thread])?;
            Ok(())
        })
        .unwrap();
    f.state.db.with(|c| { c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('unfinished-cleanup',?1,'recovering',1)", [&f.thread])?; Ok(()) }).unwrap();
    let unfinished = f.state.worktree_remove(&f.thread, &copy.id).unwrap_err();
    assert!(unfinished.to_string().contains("running"), "{unfinished:#}");
    assert!(root.exists());
    f.state
        .db
        .with(|c| {
            c.execute("DELETE FROM thread_turns WHERE id='unfinished-cleanup'", [])?;
            Ok(())
        })
        .unwrap();
    let removed = f.state.worktree_remove(&f.thread, &copy.id).unwrap();
    assert_eq!(removed["branchDeleted"], false);
    assert!(!root.exists());
    assert_eq!(
        run_git(
            &f.repo,
            &["rev-parse", &format!("refs/heads/{}", copy.branch.unwrap())]
        ),
        copy.head.unwrap()
    );
}

#[tokio::test]
async fn removal_refuses_a_working_copy_associated_with_another_thread() {
    let f = Fixture::new().await;
    let copy = f.create("shared");
    let other = new_thread(&f.state, &f.workspace).await;
    f.state.worktree_adopt(&other, &copy.path).unwrap();
    assert!(f
        .state
        .worktree_remove(&f.thread, &copy.id)
        .unwrap_err()
        .to_string()
        .contains("other threads"));
    assert!(Path::new(&copy.path).join("tracked.txt").exists());
    assert!(f
        .state
        .worktree_remove(&other, &copy.id)
        .unwrap_err()
        .to_string()
        .contains("creating thread"));
}

#[tokio::test]
async fn closing_a_delegate_cannot_delete_an_adopted_checkout_but_can_remove_its_own() {
    let f = Fixture::new().await;
    let child = f
        .state
        .create_thread(CreateThreadInput {
            workspace_id: f.workspace.clone(),
            title: None,
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: Some(f.thread.clone()),
        })
        .await
        .unwrap()
        .id;
    let external = f.repo.parent().unwrap().join("existing user checkout");
    run_git(
        &f.repo,
        &[
            "worktree",
            "add",
            "--detach",
            external.to_str().unwrap(),
            "HEAD",
        ],
    );
    let adopted = f
        .state
        .worktree_adopt(&child, external.to_str().unwrap())
        .unwrap();
    f.state.worktree_use(&child, &adopted.id).await.unwrap();
    let refused = f
        .state
        .close_agent_thread(Some(&f.thread), &child, true)
        .unwrap_err();
    assert!(
        refused.to_string().contains("did not create"),
        "{refused:#}"
    );
    assert!(external.join("tracked.txt").exists());
    assert!(f.state.get_thread(&child).unwrap().closed_at.is_none());

    let created = f
        .state
        .worktree_create(&child, &json!({"name":"owned","cwd":f.repo}))
        .unwrap();
    let id = created["workingCopy"]["id"].as_str().unwrap();
    let root = Path::new(created["workingCopy"]["path"].as_str().unwrap());
    let branch = created["workingCopy"]["branch"].as_str().unwrap();
    f.state.worktree_use(&child, id).await.unwrap();
    let closed = f
        .state
        .close_agent_thread(Some(&f.thread), &child, true)
        .unwrap();
    assert_eq!(closed["worktreeRemoved"], true);
    assert!(!root.exists());
    assert!(external.join("tracked.txt").exists());
    assert!(f.state.get_thread(&child).unwrap().closed_at.is_some());
    assert!(f.state.get_thread(&child).unwrap().worktree_path.is_none());
    assert!(f.state.worktree_copy(&child, id).is_err());
    assert!(!run_git(&f.repo, &["rev-parse", &format!("refs/heads/{branch}")]).is_empty());
}

#[tokio::test]
async fn use_rejects_running_and_queued_work_without_waiting_for_its_own_turn() {
    let f = Fixture::new().await;
    let copy = f.create("switch");
    f.state
        .db
        .with(|c| {
            c.execute(
                "UPDATE threads SET status='running' WHERE id=?1",
                [&f.thread],
            )?;
            Ok(())
        })
        .unwrap();
    let running = tokio::time::timeout(
        Duration::from_secs(3),
        f.state.worktree_use(&f.thread, &copy.id),
    )
    .await
    .expect("use must return a conflict, not await its calling turn")
    .unwrap_err();
    assert!(running.to_string().contains("running"));
    f.state.db.with(|c| {
        c.execute("UPDATE threads SET status='idle' WHERE id=?1", [&f.thread])?;
        c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('unfinished-switch',?1,'inProgress',1)", [&f.thread])?;
        Ok(())
    }).unwrap();
    let unfinished = tokio::time::timeout(
        Duration::from_secs(3),
        f.state.worktree_use(&f.thread, &copy.id),
    )
    .await
    .expect("use must reject a nonterminal turn even with an idle status cache")
    .unwrap_err();
    assert!(unfinished.to_string().contains("running"), "{unfinished:#}");
    f.state.db.with(|c| {
        c.execute("DELETE FROM thread_turns WHERE id='unfinished-switch'", [])?;
        c.execute("INSERT INTO thread_pending_steers(id,thread_id,turn_id,display_prompt,submitted_prompt,delivery,created_at,updated_at)
            VALUES('pending-switch',?1,'','queued','queued','queue','now','now')", [&f.thread])?;
        Ok(())
    }).unwrap();
    let queued = tokio::time::timeout(
        Duration::from_secs(3),
        f.state.worktree_use(&f.thread, &copy.id),
    )
    .await
    .expect("use must not wait for queued work")
    .unwrap_err();
    assert!(queued.to_string().contains("queued"), "{queued:#}");
    assert!(f
        .state
        .get_thread(&f.thread)
        .unwrap()
        .worktree_path
        .is_none());
    f.state
        .db
        .with(|c| {
            c.execute(
                "DELETE FROM thread_pending_steers WHERE thread_id=?1",
                [&f.thread],
            )?;
            Ok(())
        })
        .unwrap();
    let switched = f.state.worktree_use(&f.thread, &copy.id).await.unwrap();
    assert_eq!(switched["sessionCwdUpdated"], false);
    assert_eq!(switched["appliesOnNextSession"], true);
    assert_eq!(
        f.state
            .get_thread(&f.thread)
            .unwrap()
            .worktree_path
            .as_deref(),
        Some(copy.path.as_str())
    );
    let configured = f.state.worktree_remove(&f.thread, &copy.id).unwrap_err();
    assert!(
        configured.to_string().contains("configured"),
        "{configured:#}"
    );
}

#[tokio::test]
async fn upgrade_retains_legacy_worktree_ownership_after_git_panel_discovery() {
    let f = Fixture::new().await;
    let root = f.repo.parent().unwrap().join("legacy-checkout");
    run_git(
        &f.repo,
        &[
            "worktree",
            "add",
            "-b",
            "legacy",
            root.to_str().unwrap(),
            "HEAD",
        ],
    );
    f.state
        .db
        .with(|c| {
            c.execute(
                "UPDATE threads SET worktree_path=?2 WHERE id=?1",
                params![f.thread, root.to_str()],
            )?;
            migrate(c)?;
            Ok(())
        })
        .unwrap();
    let context = f.state.worktree_context(&f.thread).unwrap();
    let copy = context["workingCopies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == context["primaryWorkingCopyId"])
        .unwrap();
    assert_eq!(copy["managed"], true);
    f.state
        .remove_configured_worktree(&f.thread, &root)
        .unwrap();
    assert!(!root.exists());
    assert!(f.repo.exists());
}

#[tokio::test]
async fn parallel_tool_and_subagent_observations_do_not_rebind_the_thread() {
    let f = Fixture::new().await;
    let first = f.create("first");
    let second = f.create("second");
    f.state
        .observe_worktree_location(&f.thread, &first.path, "commandCwd", "tool-a", "agent-a")
        .unwrap();
    f.state
        .observe_worktree_location(&f.thread, &second.path, "filePath", "tool-b", "agent-b")
        .unwrap();
    let context = f.state.worktree_context(&f.thread).unwrap();
    let observations = context["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 2);
    assert!(observations
        .iter()
        .any(|item| item["workingCopyId"] == first.id && item["agentId"] == "agent-a"));
    assert!(observations
        .iter()
        .any(|item| item["workingCopyId"] == second.id && item["agentId"] == "agent-b"));
    assert_eq!(context["configuredCwd"], f.repo.to_str().unwrap());
    assert_ne!(context["primaryWorkingCopyId"], first.id);
    assert_ne!(context["primaryWorkingCopyId"], second.id);
}
