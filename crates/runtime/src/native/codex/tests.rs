use super::*;
fn input() -> StartSessionInput {
    StartSessionInput {
        cwd: ".".into(),
        agent_id: Some("codex".into()),
        model: "test".into(),
        reasoning_effort: Some("high".into()),
        approval_mode: "guarded".into(),
        sandbox_mode: None,
    }
}
fn turn(id: &str, prompt: &str) -> StartTurnInput {
    StartTurnInput {
        provider_session_id: id.into(),
        prompt: prompt.into(),
        model: None,
        reasoning_effort: None,
        sandbox_mode: None,
        collaboration_mode: None,
        approval_mode: None,
        performance_mode: None,
        thread_id: "thread".into(),
        turn_id: "turn".into(),
        hidden: false,
        images: vec![],
        title: None,
        context_delivered: true,
    }
}
fn runtime() -> (tempfile::TempDir, Arc<CodexRuntime>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.py");
    std::fs::write(&path, include_str!("test_server.py")).unwrap();
    let mut runtime = CodexRuntime::new(
        Provider::Codex,
        Arc::new(crate::fake::FakeRuntime::new(Provider::Codex)),
        dir.path().into(),
    );
    runtime.command = Some(format!(
        "python3 {}",
        shell_words::quote(&path.to_string_lossy())
    ));
    (dir, Arc::new(runtime))
}
#[tokio::test]
async fn scripted_start_stream_fork_rollback_compact_resume_release() {
    let (_dir, r) = runtime();
    let caps = r.capabilities(Some("codex")).await.unwrap();
    assert_eq!(caps.negotiated.unwrap()["protocol"], "codex-app-server");
    assert!(caps.effective_capabilities.unwrap().branching.hard_rollback);
    let id = r.start_session(input()).await.unwrap().provider_session_id;
    assert_eq!(id, "codex::shared");
    let items = r
        .start_turn(
            turn(&id, "hello"),
            EventBus::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(items.iter().any(|i| i.text == "Hello"));
    r.compact_session(&id, "thread", EventBus::new())
        .await
        .unwrap();
    r.rewind_session(&id, 1).await.unwrap();
    let fork = r.fork_session(&id).await.unwrap();
    r.release_session(&fork.provider_session_id).await.unwrap();
    r.release_session(&id).await.unwrap();
    assert!(!r.session_loaded(&id));
    assert_eq!(
        r.resume_session(&id, Some("."), SessionSettings::default())
            .await
            .unwrap()
            .provider_session_id,
        id
    );
    r.release_session(&id).await.unwrap();
}
#[tokio::test]
async fn scripted_crash_is_execution_uncertain_then_resumes() {
    let (_dir, r) = runtime();
    let id = r.start_session(input()).await.unwrap().provider_session_id;
    let error = r
        .start_turn(
            turn(&id, "crash"),
            EventBus::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.is::<ExecutionUncertain>(), "{error}");
    r.resume_session(&id, Some("."), SessionSettings::default())
        .await
        .unwrap();
    r.release_session(&id).await.unwrap();
}
#[tokio::test]
async fn scripted_guarded_approval_is_visible_and_answerable() {
    for prompt in ["approval", "approval-permissions"] {
        let (_dir, r) = runtime();
        let id = r.start_session(input()).await.unwrap().provider_session_id;
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let runner = r.clone();
        let session = id.clone();
        let task = tokio::spawn(async move {
            runner
                .start_turn(turn(&session, prompt), bus, CancellationToken::new())
                .await
        });
        let request = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = rx.recv().await.unwrap();
                if event.event_type == "thread.request.created" {
                    break event.payload["request"]["id"].as_str().unwrap().to_owned();
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(r.pending_requests("thread").await.len(), 1);
        r.respond_permission(&request, true, None).await.unwrap();
        assert!(!task.await.unwrap().unwrap().is_empty());
        assert!(r.pending_requests("thread").await.is_empty());
        r.release_session(&id).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires isolated HOME and a real Codex connection"]
async fn real_codex_smoke() {
    let cwd = std::env::var("NATIVE_CODEX_LAB_WORKSPACE").expect("isolated lab workspace");
    let runtime = CodexRuntime::new(
        Provider::Codex,
        Arc::new(crate::fake::FakeRuntime::new(Provider::Codex)),
        PathBuf::from(&cwd).join("upstreams"),
    );
    let models = runtime.list_models(None, Some(&cwd)).await.unwrap();
    assert!(!models.is_empty());
    let mut input = input();
    input.cwd = cwd.clone();
    input.model = std::env::var("NATIVE_CODEX_LAB_MODEL").unwrap_or_else(|_| {
        models
            .iter()
            .find(|m| m.is_default)
            .unwrap_or(&models[0])
            .model
            .clone()
    });
    input.approval_mode = "yolo".into();
    let id = runtime
        .start_session(input)
        .await
        .unwrap()
        .provider_session_id;
    let mut prompt=turn(&id,"Use a shell tool to write native-smoke.txt containing exactly 'native-ok', then reply NATIVE_OK.");
    prompt.turn_id = "real-turn".into();
    let items = tokio::time::timeout(
        Duration::from_secs(120),
        runtime.start_turn(prompt, EventBus::new(), CancellationToken::new()),
    )
    .await
    .unwrap()
    .unwrap();
    std::fs::write(
        std::path::Path::new(&cwd).join("transcript.json"),
        serde_json::to_vec_pretty(&items).unwrap(),
    )
    .unwrap();
    assert!(items.iter().any(|i| i.kind == "commandExecution"));
    assert!(
        std::fs::read_to_string(std::path::Path::new(&cwd).join("native-smoke.txt"))
            .unwrap()
            .contains("native-ok")
    );
    runtime.release_session(&id).await.unwrap();
    let resumed = runtime
        .resume_session(&id, Some(&cwd), SessionSettings::default())
        .await
        .unwrap();
    assert_eq!(resumed.provider_session_id, id);
    let fork = runtime.fork_session(&id).await.unwrap();
    runtime
        .rewind_session(&fork.provider_session_id, 1)
        .await
        .unwrap();
    runtime
        .apply_session_settings(
            &id,
            SessionSettings {
                effort: Some("low".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let goal = runtime
        .set_goal(&id, Some("Native lab goal".into()), Some("paused".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(goal.status, "paused");
    assert_eq!(
        runtime.get_goal(&id).await.unwrap().unwrap().objective,
        "Native lab goal"
    );
    println!("real model catalog, tool use, file edit, release/resume, fork, rewind, effort and goal passed");
    runtime
        .release_session(&fork.provider_session_id)
        .await
        .unwrap();
    runtime.release_session(&id).await.unwrap();
}

#[tokio::test]
async fn steering_and_interrupt_use_native_turn_identity() {
    for steer in [true, false] {
        let (_dir, runtime) = runtime();
        let id = runtime
            .start_session(input())
            .await
            .unwrap()
            .provider_session_id;
        let runner = runtime.clone();
        let session = id.clone();
        let task = tokio::spawn(async move {
            runner
                .start_turn(
                    turn(&session, "wait"),
                    EventBus::new(),
                    CancellationToken::new(),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(10), async {
            while !matches!(
                runtime.execution_state(&id).await,
                ExecutionState::Running { .. }
            ) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if steer {
            runtime
                .send_input(&id, "pockymoe-turn", "continue")
                .await
                .unwrap();
        } else {
            runtime.interrupt(&id, "pockymoe-turn").await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        runtime.release_session(&id).await.unwrap();
    }
}
