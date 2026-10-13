use super::*;
use crate::actor::{with_load_intent, LoadedSession, ReleaseScope, TurnRunning};
use pockymoe_protocol::{HarnessBusyReason, HarnessLoadReason};

async fn runtime(dir: &std::path::Path, args: &str) -> AcpRuntime {
    let script = dir.join("release_agent.py");
    std::fs::write(
        &script,
        include_str!("../../tests/fixtures/release_agent.py"),
    )
    .unwrap();
    let python = which::which("python3").unwrap();
    let command = format!(
        "{} {args}",
        shell_words::join([python.to_str().unwrap(), script.to_str().unwrap()])
    );
    let runtime = AcpRuntime::catalog(Some(command), 5_000);
    runtime.start().await.unwrap();
    runtime
}

async fn start(runtime: &AcpRuntime, dir: &std::path::Path, thread: &str) -> String {
    with_load_intent(
        thread,
        HarnessLoadReason::View,
        runtime.start_session(StartSessionInput {
            cwd: dir.to_string_lossy().into_owned(),
            agent_id: Some("custom".into()),
            model: "default".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            sandbox_mode: Some("danger-full-access".into()),
        }),
    )
    .await
    .unwrap()
    .provider_session_id
}

fn turn(session: &str, turn: &str) -> StartTurnInput {
    StartTurnInput {
        provider_session_id: session.into(),
        prompt: "hello".into(),
        model: None,
        reasoning_effort: None,
        sandbox_mode: None,
        collaboration_mode: None,
        approval_mode: None,
        performance_mode: None,
        thread_id: "thread-1".into(),
        turn_id: turn.into(),
        hidden: false,
        images: Vec::new(),
        title: None,
        context_delivered: true,
    }
}

/// Gone, or a zombie waiting for init to reap it.
fn exited(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|stat| {
            stat.rsplit_once(')')
                .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
        })
        .unwrap_or(true)
}

async fn wait_exited(pid: u32) -> bool {
    for _ in 0..100 {
        if exited(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

fn tool_pid(dir: &std::path::Path) -> u32 {
    std::fs::read_to_string(dir.join("tool.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

async fn busy(runtime: &AcpRuntime) -> Vec<HarnessBusyReason> {
    only(runtime).await.busy
}

async fn only(runtime: &AcpRuntime) -> LoadedSession {
    let mut loaded = runtime.loaded_sessions().await;
    assert_eq!(loaded.len(), 1);
    loaded.remove(0)
}

#[tokio::test]
async fn release_records_the_load_then_closes_the_session_and_ends_the_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime(dir.path(), "").await;
    let mut events = runtime.lifecycle().unwrap();
    let session = start(&runtime, dir.path(), "thread-1").await;
    match events.try_recv().unwrap() {
        crate::actor::SessionLifecycle::Loaded {
            session_id,
            thread_id,
            reason,
            ..
        } => {
            assert_eq!(session_id, session);
            assert_eq!(thread_id.as_deref(), Some("thread-1"));
            assert_eq!(reason, HarnessLoadReason::View);
        }
        other => panic!("unexpected {other:?}"),
    }
    let loaded = only(&runtime).await;
    assert_eq!(loaded.thread_id.as_deref(), Some("thread-1"));
    assert_eq!(loaded.load_reason, HarnessLoadReason::View);
    assert!(!loaded.prompted && loaded.busy.is_empty());
    let root = loaded.root_pid.unwrap();
    let tool = tool_pid(dir.path());

    runtime
        .start_turn(
            turn(&session, "t1"),
            EventBus::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let prompted = only(&runtime).await;
    assert!(
        prompted.prompted,
        "a prompt turns a view load into an idle-rule load"
    );
    assert!(prompted.last_activity_at >= loaded.last_activity_at);

    let released = runtime
        .release_loaded(&session, ReleaseScope::Process)
        .await
        .unwrap();
    assert_eq!(released.len(), 1);
    let closed = std::fs::read_to_string(dir.path().join("closed.jsonl")).unwrap();
    let closed: Value = serde_json::from_str(closed.lines().next().unwrap()).unwrap();
    assert_eq!(
        closed["sessionId"].as_str(),
        session.split_once("::").map(|(_, raw)| raw)
    );
    assert!(wait_exited(root).await, "the adapter exits");
    assert!(
        wait_exited(tool).await,
        "a tool ignoring SIGTERM is killed with the group"
    );
    assert!(runtime.loaded_sessions().await.is_empty());
    assert!(!runtime.session_loaded(&session));
    assert!(runtime
        .release_loaded(&session, ReleaseScope::Process)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn release_skips_session_close_the_adapter_does_not_advertise() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime(dir.path(), "--no-close").await;
    let session = start(&runtime, dir.path(), "thread-1").await;
    let root = only(&runtime).await.root_pid.unwrap();
    runtime
        .release_loaded(&session, ReleaseScope::Process)
        .await
        .unwrap();
    assert!(!dir.path().join("closed.jsonl").exists());
    assert!(wait_exited(root).await);
    assert!(wait_exited(tool_pid(dir.path())).await);
}

#[tokio::test]
async fn release_reports_each_runtime_busy_reason() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime(dir.path(), "").await;
    let session = start(&runtime, dir.path(), "thread-1").await;
    assert!(busy(&runtime).await.is_empty());
    let process_id = {
        let mut sessions = runtime.inner.sessions.lock().await;
        let live = sessions.get_mut(&session).unwrap();
        live.goal = Some(GoalState {
            objective: "ship".into(),
            status: "active".into(),
            ..Default::default()
        });
        live.process.id.clone()
    };
    assert_eq!(busy(&runtime).await, [HarnessBusyReason::Goal]);
    {
        let mut sessions = runtime.inner.sessions.lock().await;
        let live = sessions.get_mut(&session).unwrap();
        live.goal.as_mut().unwrap().status = "paused".into();
        live.claude_lifecycle
            .record(&json!({"type":"system","subtype":"init"}));
        live.claude_lifecycle
            .record(&json!({"type":"system","subtype":"task_started","task_id":"shell"}));
    }
    assert_eq!(busy(&runtime).await, [HarnessBusyReason::BackgroundTask]);
    {
        let mut sessions = runtime.inner.sessions.lock().await;
        let live = sessions.get_mut(&session).unwrap();
        live.claude_lifecycle.record(&json!({"type":"system","subtype":"task_notification","task_id":"shell","status":"completed"}));
        live.active_subagents.insert(
            "agent".into(),
            serde_json::from_value(json!({"id":"agent","name":"Explore","status":"running"}))
                .unwrap(),
        );
    }
    assert_eq!(busy(&runtime).await, [HarnessBusyReason::Subagent]);
    runtime
        .inner
        .sessions
        .lock()
        .await
        .get_mut(&session)
        .unwrap()
        .active_subagents
        .clear();
    let terminal = runtime
        .inner
        .terminals
        .create(
            &process_id,
            "sleep",
            &["30".into()],
            dir.path().into(),
            &[],
            None,
        )
        .await
        .unwrap();
    assert_eq!(busy(&runtime).await, [HarnessBusyReason::BackgroundTask]);
    runtime.inner.terminals.release(&terminal);
    for _ in 0..50 {
        if runtime.inner.terminals.running(&process_id) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(busy(&runtime).await.is_empty());
    {
        let mut sessions = runtime.inner.sessions.lock().await;
        let live = sessions.get_mut(&session).unwrap();
        live.active = Some(ActiveTurn {
            thread_id: "thread-1".into(),
            turn_id: "t1".into(),
            bus: EventBus::new(),
            background_owned: false,
            cancel: CancellationToken::new(),
            pending_prompts: 1,
            continuations: mpsc::unbounded_channel().0,
        });
    }
    runtime.inner.pending_dtos.lock().await.insert(
        "thread-1".into(),
        vec![ThreadActionRequestDto {
            id: "perm-1".into(),
            kind: "permissionRequest".into(),
            title: "Run".into(),
            description: None,
            turn_id: Some("t1".into()),
            item_id: None,
            created_at: now_rfc3339(),
            questions: Vec::new(),
        }],
    );
    assert_eq!(
        busy(&runtime).await,
        [
            HarnessBusyReason::Turn,
            HarnessBusyReason::PendingRequest,
            HarnessBusyReason::QueuedInput
        ]
    );
    let error = runtime
        .release_loaded(&session, ReleaseScope::Process)
        .await
        .unwrap_err();
    assert!(error.is::<TurnRunning>(), "{error:#}");
    runtime
        .inner
        .sessions
        .lock()
        .await
        .get_mut(&session)
        .unwrap()
        .active = None;
    runtime
        .release_loaded(&session, ReleaseScope::Process)
        .await
        .unwrap();
}

#[tokio::test]
async fn release_of_a_shared_fork_process_waits_for_every_session() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime(dir.path(), "").await;
    let parent = start(&runtime, dir.path(), "parent").await;
    let def = runtime.agent_def(Some("custom")).unwrap();
    let (_, mut fork) = runtime
        .spawn_session(
            &def,
            dir.path().to_str().unwrap(),
            ProductSessionPolicy::default(),
            None,
            None,
        )
        .await
        .unwrap();
    // A Claude/Grok fork is served by its parent's process until reloaded.
    let own = std::mem::replace(
        &mut fork.process,
        runtime.inner.sessions.lock().await[&parent].process.clone(),
    );
    own.shutdown().await.unwrap();
    fork.session_id = "fork-session".into();
    fork.load = LoadRecord::fork();
    runtime
        .inner
        .sessions
        .lock()
        .await
        .insert("custom::fork-session".into(), fork);
    let loaded = runtime.loaded_sessions().await;
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].process_id, loaded[1].process_id);
    let fork = loaded
        .iter()
        .find(|s| s.session_id == "custom::fork-session")
        .unwrap();
    assert_eq!(
        (fork.load_reason, fork.thread_id.as_deref()),
        (HarnessLoadReason::Fork, None)
    );
    let root = fork.root_pid.unwrap();

    runtime
        .inner
        .sessions
        .lock()
        .await
        .get_mut(&parent)
        .unwrap()
        .active = Some(ActiveTurn {
        thread_id: "parent".into(),
        turn_id: "t1".into(),
        bus: EventBus::new(),
        background_owned: false,
        cancel: CancellationToken::new(),
        pending_prompts: 0,
        continuations: mpsc::unbounded_channel().0,
    });
    let error = runtime
        .release_loaded("custom::fork-session", ReleaseScope::Process)
        .await
        .unwrap_err();
    assert!(
        error.is::<TurnRunning>(),
        "a busy parent keeps the shared process"
    );
    runtime
        .inner
        .sessions
        .lock()
        .await
        .get_mut(&parent)
        .unwrap()
        .active = None;
    let released = runtime
        .release_loaded("custom::fork-session", ReleaseScope::Process)
        .await
        .unwrap();
    let mut ids: Vec<_> = released.iter().map(|s| s.session_id.as_str()).collect();
    let mut expected = vec!["custom::fork-session", parent.as_str()];
    ids.sort();
    expected.sort();
    assert_eq!(ids, expected);
    assert!(wait_exited(root).await);
    let closed = std::fs::read_to_string(dir.path().join("closed.jsonl")).unwrap();
    assert_eq!(closed.lines().count(), 2, "each session is closed first");
}

#[tokio::test]
async fn release_of_one_session_keeps_a_process_shared_with_another() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime(dir.path(), "").await;
    let parent = start(&runtime, dir.path(), "parent").await;
    let def = runtime.agent_def(Some("custom")).unwrap();
    let (_, mut fork) = runtime
        .spawn_session(
            &def,
            dir.path().to_str().unwrap(),
            ProductSessionPolicy::default(),
            None,
            None,
        )
        .await
        .unwrap();
    let shared = runtime.inner.sessions.lock().await[&parent].process.clone();
    std::mem::replace(&mut fork.process, shared.clone())
        .shutdown()
        .await
        .unwrap();
    fork.session_id = "fork-session".into();
    runtime
        .inner
        .sessions
        .lock()
        .await
        .insert("custom::fork-session".into(), fork);
    let released = runtime
        .release_loaded("custom::fork-session", ReleaseScope::Session)
        .await
        .unwrap();
    assert_eq!(released.len(), 1);
    assert!(
        shared.connection_open().await,
        "deleting a fork leaves its parent running"
    );
    assert_eq!(runtime.loaded_sessions().await.len(), 1);
    runtime
        .release_loaded(&parent, ReleaseScope::Session)
        .await
        .unwrap();
    assert!(!shared.connection_open().await);
}
