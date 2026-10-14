use std::sync::Arc;

use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Provider, SendThreadPromptInput};
use pockymoe_runtime::actor::SharedRuntime;
use pockymoe_runtime::config::RuntimeConfig;
use pockymoe_runtime::db::Database;
use pockymoe_runtime::fake::FakeRuntime;
use pockymoe_runtime::Supervisor;
use tempfile::tempdir;

fn test_config(dir: &std::path::Path) -> RuntimeConfig {
    RuntimeConfig {
        treer_enabled: false,
        host_mode: Default::default(),
        mode: pockymoe_protocol::Mode::Local,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root: dir.join("workspaces"),
        database_url: dir.join("test.sqlite"),
        app_name: "test".into(),
        app_version: "0.12.0".into(),
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
    }
}

fn prompt_input(prompt: &str) -> SendThreadPromptInput {
    SendThreadPromptInput {
        prompt: prompt.into(),
        client_request_id: None,
        model: None,
        reasoning_effort: None,
        collaboration_mode: None,
        images: vec![],
    }
}

fn insert_stale_turn(supervisor: &Supervisor, thread_id: &str, turn_id: &str) {
    let now = pockymoe_protocol::now_rfc3339();
    supervisor
        .db
        .with(|conn| {
            conn.execute(
                "INSERT INTO thread_turns(
                   id, thread_id, status, error, model, reasoning_effort,
                   started_at, completed_at, ordinal
                 ) VALUES (?1,?2,'inProgress',NULL,NULL,NULL,?3,NULL,1)",
                (turn_id, thread_id, &now),
            )?;
            conn.execute(
                "UPDATE threads SET status='running', updated_at=?1 WHERE id=?2",
                (&now, thread_id),
            )?;
            Ok(())
        })
        .unwrap();
}

async fn seeded_thread(
    provider: Provider,
) -> (
    tempfile::TempDir,
    Supervisor,
    pockymoe_protocol::WorkspaceDto,
    pockymoe_protocol::ThreadDto,
) {
    let dir = tempdir().unwrap();
    let ws_path = dir.path().join("proj");
    std::fs::create_dir_all(&ws_path).unwrap();
    std::fs::write(ws_path.join("README.md"), "# hi\n").unwrap();
    let mut config = test_config(dir.path());
    config.enabled_providers = vec![provider];
    let db = Database::open(&config.database_url).unwrap();
    let runtime: SharedRuntime = Arc::new(FakeRuntime::new(provider));
    let supervisor = Supervisor::new(config, db, vec![runtime]);
    let workspace = supervisor
        .create_workspace(CreateWorkspaceInput {
            abs_path: Some(ws_path.to_string_lossy().into()),
            git_url: None,
            label: Some("proj".into()),
        })
        .unwrap();
    let thread = supervisor
        .create_thread(CreateThreadInput {
            workspace_id: workspace.id.clone(),
            title: Some("t1".into()),
            provider: Some(provider),
            agent_id: None,
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: None,
        })
        .await
        .unwrap();
    (dir, supervisor, workspace, thread)
}

#[tokio::test]
async fn reconnect_returns_before_queued_work_and_keeps_it_owned() {
    let (_dir, state, _workspace, thread) = seeded_thread(Provider::Codex).await;
    insert_stale_turn(&state, &thread.id, "unconfirmed");
    state
        .db
        .with(|c| {
            c.execute(
                "UPDATE threads SET status='recovering' WHERE id=?1",
                [&thread.id],
            )?;
            c.execute(
                "UPDATE thread_turns SET status='recovering' WHERE thread_id=?1",
                [&thread.id],
            )?;
            Ok(())
        })
        .unwrap();
    let state = Arc::new(state);
    let mut input = prompt_input("Inspect this repository in depth");
    input.client_request_id = Some("reconnect-queued-request".into());
    state.accept_prompt(&thread.id, &input).await.unwrap();
    // Drop the initiating request after it has launched reconnect, as a relay
    // disconnect would do. The subsequent retry must share the saved work.
    {
        use std::future::Future;
        let mut request = Box::pin(state.resume_thread(&thread.id));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(request.as_mut().poll(&mut context).is_pending());
    }
    let detail = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        state.resume_thread(&thread.id),
    )
    .await
    .expect("reconnect must not wait for the 25-second queued turn")
    .unwrap();
    assert_ne!(detail.thread.status, "recovering");
    assert!(detail.thread.last_error.is_none());
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state.get_thread(&thread.id).unwrap().status != "running" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Retrying both the message receipt and reconnect cannot duplicate execution.
    state.accept_prompt(&thread.id, &input).await.unwrap();
    state.resume_thread(&thread.id).await.unwrap();
    let detail = state.get_thread_detail(&thread.id, None).await.unwrap();
    assert_eq!(
        state
            .db
            .with(|c| Ok(c.query_row(
                "SELECT count(*) FROM thread_turns WHERE thread_id=?1",
                [&thread.id],
                |r| r.get::<_, i64>(0)
            )?))
            .unwrap(),
        2
    );
    assert!(detail.pending_steers.is_empty());
    assert_eq!(
        detail.thread.provider_session_id,
        thread.provider_session_id
    );
    state.interrupt(&thread.id).await.unwrap();
    assert_eq!(state.get_thread(&thread.id).unwrap().status, "interrupted");
}

#[tokio::test]
async fn update_intent_survives_crash_before_cancellation_and_failed_reconnect() {
    let (_dir, state, _workspace, thread) = seeded_thread(Provider::Codex).await;
    insert_stale_turn(&state, &thread.id, "journaled-before-crash");
    state.db.with(|c| {
        c.execute("INSERT INTO thread_pending_steers(id,thread_id,turn_id,display_prompt,submitted_prompt,delivery,created_at,updated_at) VALUES ('maintenance-intent',?1,'journaled-before-crash','continue maintenance','continue maintenance','update-resume','2026-09-11','2026-09-11')",[&thread.id])?;
        Ok(())
    }).unwrap();
    let config = state.config.clone();
    drop(state);
    // Missing backend represents a reconnect preflight failure. Its intent must
    // survive and must not be silently turned into success or discarded.
    let state = Arc::new(Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![],
    ));
    state.finish_update_attempt();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if state
                .get_thread(&thread.id)
                .unwrap()
                .last_error
                .unwrap_or_default()
                .contains("Automatic recovery")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        state
            .db
            .with(|c| Ok(c.query_row(
                "SELECT count(*) FROM thread_pending_steers WHERE id='maintenance-intent'",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .unwrap(),
        1
    );
    // Wait for the failed background attempt to release database ownership.
    while Arc::strong_count(&state) > 1 {
        tokio::task::yield_now().await;
    }
    drop(state);
    let state = Arc::new(Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![Arc::new(FakeRuntime::new(Provider::Codex))],
    ));
    assert!(state.defer_update_recovery());
    state.finish_update_attempt();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while state.get_thread(&thread.id).unwrap().status != "idle" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let detail = state.get_thread_detail(&thread.id, None).await.unwrap();
    assert_eq!(detail.turns.len(), 2);
    assert_eq!(detail.turns[0].status, "interrupted");
    assert_eq!(detail.turns[1].status, "completed");
    assert_eq!(
        detail.thread.provider_session_id,
        thread.provider_session_id
    );
    assert_eq!(
        state
            .db
            .with(|c| Ok(c
                .query_row("SELECT count(*) FROM thread_pending_steers", [], |r| r
                    .get::<_, i64>(0))?))
            .unwrap(),
        0
    );
}
async fn verify_update_recovery(restart: bool, strategy: pockymoe_runtime::RestartStrategy) {
    let (_dir, mut supervisor, _workspace, thread) = seeded_thread(Provider::Codex).await;
    supervisor.config.supervisor_restart_strategy = strategy;
    let supervisor = Arc::new(supervisor);
    supervisor.spawn_live_item_persister();
    let mut events = supervisor.bus.subscribe();
    let running = {
        let state = supervisor.clone();
        let id = thread.id.clone();
        tokio::spawn(async move {
            state
                .prompt(
                    &id,
                    prompt_input("Inspect this repository before making changes."),
                )
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while events.recv().await.unwrap().event_type != "thread.turn.started" {}
    })
    .await
    .unwrap();
    let session = supervisor
        .get_thread(&thread.id)
        .unwrap()
        .provider_session_id;
    supervisor
        .prompt(&thread.id, prompt_input("hello queued after recovery"))
        .await
        .unwrap();
    let guard = supervisor.prepare_update_restart().await.unwrap();
    // A separate connection sees committed recovery intent before the caller
    // can exit; neither the live process nor its in-memory state is required.
    let journal = rusqlite::Connection::open(&supervisor.config.database_url).unwrap();
    let persisted: i64 = journal.query_row(
        "SELECT count(*) FROM thread_pending_steers WHERE delivery='update-resume' AND thread_id=?1",
        [&thread.id], |r| r.get(0)).unwrap();
    assert_eq!(persisted, 1);
    drop(journal);
    running.await.unwrap().unwrap();
    assert_eq!(
        supervisor.get_thread(&thread.id).unwrap().status,
        "interrupted"
    );
    assert!(supervisor
        .prompt(&thread.id, prompt_input("must not start while updating"))
        .await
        .unwrap_err()
        .to_string()
        .contains("updating"));
    let paused = supervisor
        .get_thread_detail(&thread.id, None)
        .await
        .unwrap();
    assert_eq!(
        paused.pending_steers.len(),
        1,
        "internal resume must stay hidden"
    );
    assert_eq!(paused.turns.len(), 1);
    drop(guard);
    let state = if restart {
        let config = supervisor.config.clone();
        drop(supervisor);
        let db = Database::open(&config.database_url).unwrap();
        Arc::new(Supervisor::new(
            config,
            db,
            vec![Arc::new(FakeRuntime::new(Provider::Codex))],
        ))
    } else {
        supervisor
    };
    state.spawn_live_item_persister();
    assert_eq!(state.config.supervisor_restart_strategy, strategy);
    assert!(state.defer_update_recovery());
    assert!(state
        .prompt(
            &thread.id,
            prompt_input("wait for update health verification")
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("updating"));
    state.finish_update_attempt();
    state.spawn_update_recovery(); // Repeated startup notification must not duplicate a turn.
    tokio::time::timeout(std::time::Duration::from_secs(35), async {
        loop {
            let detail = state.get_thread_detail(&thread.id, None).await.unwrap();
            if detail.turns.len() == 3 && detail.thread.status == "idle" {
                assert_eq!(detail.turns[0].status, "interrupted");
                assert_eq!(detail.turns[1].status, "completed");
                assert!(detail.turns[1]
                    .items
                    .iter()
                    .find(|item| item.kind == "userMessage")
                    .unwrap()
                    .text
                    .as_str()
                    .contains("Supervisor was restarted for device maintenance"));
                assert_eq!(
                    detail.turns[2]
                        .items
                        .iter()
                        .find(|item| item.kind == "userMessage")
                        .unwrap()
                        .text
                        .as_str(),
                    "hello queued after recovery"
                );
                assert!(detail.pending_steers.is_empty());
                assert_eq!(detail.thread.provider_session_id, session);
                assert_eq!(
                    detail.thread.sandbox_mode.as_deref(),
                    Some("danger-full-access")
                );
                assert!(detail.thread.last_error.is_none());
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let remaining: i64 = state
        .db
        .with(|conn| {
            Ok(
                conn.query_row("SELECT count(*) FROM thread_pending_steers", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn supervisor_update_recovers_session_and_queue_after_process_restart() {
    verify_update_recovery(true, pockymoe_runtime::RestartStrategy::ExternalUpdater).await;
}

#[tokio::test]
async fn supervisor_update_host_managed_journal_is_durable_before_exit_and_recovers_once() {
    verify_update_recovery(true, pockymoe_runtime::RestartStrategy::HostManaged).await;
}

#[tokio::test]
async fn supervisor_update_failure_resumes_old_process_without_duplicate_turns() {
    verify_update_recovery(false, pockymoe_runtime::RestartStrategy::ExternalUpdater).await;
}

#[tokio::test]
async fn supervisor_update_recovery_respects_user_stop_and_ordinary_crashes() {
    let (_dir, supervisor, _workspace, thread) = seeded_thread(Provider::Codex).await;
    insert_stale_turn(&supervisor, &thread.id, "stale-update");
    supervisor.db.with(|conn| {
        conn.execute("INSERT INTO thread_pending_steers(id,thread_id,turn_id,display_prompt,submitted_prompt,delivery,created_at,updated_at) VALUES ('update-resume:stale-update',?1,'stale-update','resume','resume','update-resume','2026-09-07','2026-09-07')", [&thread.id])?;
        Ok(())
    }).unwrap();
    let config = supervisor.config.clone();
    drop(supervisor);
    let db = Database::open(&config.database_url).unwrap();
    let state = Arc::new(Supervisor::new(
        config,
        db,
        vec![Arc::new(FakeRuntime::new(Provider::Codex))],
    ));
    state.interrupt(&thread.id).await.unwrap();
    state.spawn_update_recovery();
    let detail = state.get_thread_detail(&thread.id, None).await.unwrap();
    assert_eq!(detail.turns.len(), 1);
    assert_eq!(detail.thread.status, "interrupted");
    let remaining: i64 = state
        .db
        .with(|conn| {
            Ok(
                conn.query_row("SELECT count(*) FROM thread_pending_steers", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(remaining, 0);
    // Without an update marker, a restart must leave this interrupted turn alone.
    state.spawn_update_recovery();
    assert_eq!(state.get_thread(&thread.id).unwrap().status, "interrupted");
}

#[test]
fn db_owner_child() {
    let Some(path) = std::env::var_os("POCKYMOE_LOCK_TEST_PATH") else {
        return;
    };
    assert!(Database::open(std::path::Path::new(&path)).is_err());
}

#[test]
fn second_process_cannot_open_owned_database_and_lock_releases_on_exit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("owned.sqlite");
    let db = Database::open(&path).unwrap();
    db.set_kv("preserved", "yes").unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "db_owner_child", "--nocapture"])
        .env("POCKYMOE_LOCK_TEST_PATH", &path)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    #[cfg(unix)]
    {
        let alias = dir.path().join("alias.sqlite");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert!(Database::open(&alias).is_err());
    }
    drop(db);
    let reopened = Database::open(&path).unwrap();
    assert_eq!(
        reopened.get_kv("preserved").unwrap().as_deref(),
        Some("yes")
    );
}

#[tokio::test]
async fn acknowledged_input_survives_restart_before_dispatch() {
    let (_dir, state, _ws, thread) = seeded_thread(Provider::Claude).await;
    let config = state.config.clone();
    let mut input = prompt_input("saved before dispatch");
    input.client_request_id = Some("before-crash".into());
    state.accept_prompt(&thread.id, &input).await.unwrap();
    assert!(state
        .get_thread_detail(&thread.id, None)
        .await
        .unwrap()
        .turns
        .is_empty());
    drop(state);
    // Other tests spawn ACP child processes concurrently. On Unix a fork can
    // briefly inherit the lock descriptor until exec closes it. Wait for actual
    // lock release instead of assuming no concurrent fork/exec overlaps the drop.
    let reopened = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            match Database::open(&config.database_url) {
                Ok(db) => break db,
                Err(error) => {
                    assert!(error.to_string().contains("already owned"), "{error:#}");
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
        }
    })
    .await
    .expect("old Supervisor database ownership must be released");
    let restarted = Supervisor::new(
        config.clone(),
        reopened,
        vec![Arc::new(FakeRuntime::new(Provider::Claude))],
    );
    assert_eq!(
        restarted
            .get_thread_detail(&thread.id, None)
            .await
            .unwrap()
            .pending_steers
            .len(),
        1
    );
    restarted.prompt(&thread.id, input.clone()).await.unwrap();
    assert_eq!(
        restarted
            .get_thread_detail(&thread.id, None)
            .await
            .unwrap()
            .turns
            .len(),
        1
    );
    input.prompt = "different payload".into();
    assert!(restarted
        .accept_prompt(&thread.id, &input)
        .await
        .unwrap_err()
        .to_string()
        .contains("already used"));
}
