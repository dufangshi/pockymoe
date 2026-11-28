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
async fn startup_notifications_do_not_block_native_control_replies() {
    let (_dir, runtime) = runtime();
    let mut session_input = input();
    session_input.model = "startup-burst".into();
    let (items, session) = tokio::time::timeout(Duration::from_secs(10), async {
        let session = runtime
            .start_session(session_input)
            .await
            .unwrap()
            .provider_session_id;
        let items = runtime
            .start_turn(
                turn(&session, "hello"),
                EventBus::new(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        (items, session)
    })
    .await
    .expect("control replies must reach the runtime during an MCP startup burst");
    assert!(items.iter().any(|item| item.text == "Hello"));
    assert!(items.iter().any(|item| item.text.contains("broken-docs")));
    runtime.release_session(&session).await.unwrap();
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
        // A mid-turn model change reaches the running native turn.
        runtime
            .apply_session_settings(
                &id,
                SessionSettings {
                    model: Some("live-model".into()),
                    ..Default::default()
                },
            )
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

#[test]
fn isolated_native_file_requires_the_reviewed_image_codex() {
    let init = |agent: &str| json!({"userAgent": agent});
    assert_eq!(
        super::codex_version(&init(
            "pockymoe/0.167.0 (Ubuntu 24.4.0; x86_64) unknown (pockymoe; 1)"
        )),
        Some("0.167.0")
    );
    assert!(super::verify_isolated_native_file(&init("pockymoe/0.167.0 (Ubuntu)")).is_ok());
    for other in ["pockymoe/0.169.0 (Ubuntu)", "", "pockymoe"] {
        let error = super::verify_isolated_native_file(&init(other))
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("unsupported_environment_codex_cli"));
    }
    assert!(super::isolated_native_file(&[(
        "POCKYMOE_ENV_CODEX_NATIVE",
        "1".into()
    )]));
    assert!(!super::isolated_native_file(&[(
        "POCKYMOE_ENV_CODEX",
        "1".into()
    )]));
}

#[test]
fn isolated_sessions_hide_native_forks_like_acp() {
    let runtime = super::CodexRuntime::new(
        pockymoe_protocol::Provider::Acp,
        std::sync::Arc::new(crate::fake::FakeRuntime::new(
            pockymoe_protocol::Provider::Acp,
        )),
        std::env::temp_dir(),
    );
    let host = runtime.session_negotiated_caps(Some("codex"), "codex::abc");
    assert!(host.branching.fork && host.branching.fork_at && host.branching.hard_rollback);
    let isolated = runtime.session_negotiated_caps(Some("codex"), "codex::env/e/1/abc");
    assert!(!isolated.branching.fork && !isolated.branching.fork_at);
    assert!(isolated.branching.hard_rollback && isolated.turns.steer);
}

#[tokio::test]
async fn account_rate_limit_pushes_reach_viewers_during_a_turn() {
    let (_dir, r) = runtime();
    let id = r.start_session(input()).await.unwrap().provider_session_id;
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    r.start_turn(turn(&id, "limits"), bus, CancellationToken::new())
        .await
        .unwrap();
    let mut announced = vec![];
    let mut lifecycle = vec![];
    while let Ok(event) = rx.try_recv() {
        if event.event_type == "runtime.subscription.updated" {
            announced.push(event.payload);
        } else if event.event_type.as_str().starts_with("thread.turn.") {
            lifecycle.push((
                event.event_type.as_str().to_owned(),
                event.payload["status"].clone(),
            ));
        }
    }
    // The Supervisor's turn lifecycle events, exactly as ACP emits them.
    assert_eq!(
        lifecycle,
        vec![
            ("thread.turn.started".to_owned(), Value::Null),
            ("thread.turn.completed".to_owned(), json!("completed")),
        ]
    );
    assert_eq!(announced.len(), 1, "{announced:?}");
    assert_eq!(announced[0]["provider"], "codex");
    let windows = announced[0]["usage"]["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0]["id"], "secondary");
    assert_eq!(windows[0]["label"], "7d");
    assert_eq!(windows[0]["usedPercent"], 42.0);
    r.release_session(&id).await.unwrap();
}

#[tokio::test]
async fn approvals_offer_codex_decisions_and_codex_settled_requests_close() {
    let (_dir, r) = runtime();
    let id = r.start_session(input()).await.unwrap().provider_session_id;
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    r.start_turn(turn(&id, "settled"), bus, CancellationToken::new())
        .await
        .unwrap();
    let mut shown = vec![];
    let mut resolved = vec![];
    while let Ok(event) = rx.try_recv() {
        match event.event_type.as_str() {
            "thread.request.created" => shown.push(event.payload["request"].clone()),
            "thread.request.resolved" => resolved.push(event.payload["requestId"].clone()),
            _ => {}
        }
    }
    // Every card shown is closed: by Codex's own settlement or by the turn's end.
    for card in &shown {
        assert!(resolved.contains(&card["id"]), "{card} never resolved");
    }
    assert!(shown.iter().any(|c| c["title"] == "Edit files"));
    assert!(!shown.iter().any(|c| c["title"] == "pwd") || resolved.len() == shown.len());
    let created = shown[0].clone();
    assert_eq!(created["title"], "ls");
    let labels: Vec<_> = created["questions"][0]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["label"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(labels.len(), 4, "{labels:?}");
    assert!(labels[1].contains("don't ask again"));
    assert!(r.pending_requests("thread").await.is_empty());
    r.release_session(&id).await.unwrap();
}

#[tokio::test]
async fn slash_commands_run_review_mode_and_answer_mcp_status_natively() {
    let (_dir, r) = runtime();
    let id = r.start_session(input()).await.unwrap().provider_session_id;
    let review = r
        .start_turn(
            turn(&id, "/review-branch main"),
            EventBus::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(review
        .iter()
        .any(|i| i.kind == "agentMessage" && i.text == "No issues found."));
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mcp = r
        .start_turn(turn(&id, "/mcp"), bus, CancellationToken::new())
        .await
        .unwrap();
    assert!(
        mcp[0]
            .text
            .contains("| docs | connected | 1 | unsupported |"),
        "{:?}",
        mcp
    );
    let mut kinds = vec![];
    while let Ok(e) = rx.try_recv() {
        kinds.push(e.event_type.as_str().to_owned());
    }
    assert_eq!(
        kinds.first().map(String::as_str),
        Some("thread.turn.started")
    );
    assert_eq!(
        kinds.last().map(String::as_str),
        Some("thread.turn.completed")
    );
    let toolbox = r.toolbox(Some("codex"));
    assert!(toolbox.iter().any(|t| t.command == "/review-commit"));
    r.release_session(&id).await.unwrap();
}

#[tokio::test]
async fn plan_mode_asks_to_implement_and_continues_in_the_same_turn() {
    for (answer, implement) in [
        (super::plan_review::IMPLEMENT, true),
        (super::plan_review::REVISE, false),
    ] {
        let (_dir, r) = runtime();
        let id = r.start_session(input()).await.unwrap().provider_session_id;
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut planned = turn(&id, "make a plan");
        planned.collaboration_mode = Some("plan".into());
        let runner = r.clone();
        let task = tokio::spawn(async move {
            runner
                .start_turn(planned, bus, CancellationToken::new())
                .await
        });
        let request = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let e = rx.recv().await.unwrap();
                if e.event_type == "thread.request.created" {
                    break e.payload["request"].clone();
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(request["title"], "Implement this plan?");
        assert_eq!(r.pending_requests("thread").await.len(), 1);
        let reply = format!(r#"{{"permission":{{"answers":["{answer}"]}}}}"#);
        r.respond_permission(request["id"].as_str().unwrap(), true, Some(&reply))
            .await
            .unwrap();
        let items = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(items.iter().any(|i| i.text.contains("1. Add tests")));
        assert_eq!(
            items.iter().any(|i| i.text == "Implemented"),
            implement,
            "{items:?}"
        );
        let mut collaboration = false;
        while let Ok(e) = rx.try_recv() {
            collaboration |= e.event_type == "thread.collaboration.updated";
        }
        assert_eq!(collaboration, implement);
        assert!(r.pending_requests("thread").await.is_empty());
        r.release_session(&id).await.unwrap();
    }
}

#[test]
fn approval_titles_match_the_command_row() {
    let wrapped = json!({"command":"/bin/bash -lc 'touch ~/x'"});
    assert_eq!(
        super::approval_title("item/commandExecution/requestApproval", &wrapped),
        "touch ~/x"
    );
    let parsed = json!({"command":"/bin/bash -lc 'cat a'","commandActions":[{"type":"read","command":"cat a","path":"a"}]});
    assert_eq!(
        super::approval_title("item/commandExecution/requestApproval", &parsed),
        "cat a"
    );
    assert_eq!(
        super::approval_title("item/fileChange/requestApproval", &json!({})),
        "Edit files"
    );
}

#[tokio::test]
async fn codex_warnings_reach_the_native_transcript() {
    let (_dir, r) = runtime();
    let id = r.start_session(input()).await.unwrap().provider_session_id;
    let items = r
        .start_turn(
            turn(&id, "notices"),
            EventBus::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        items
            .iter()
            .any(|i| i.kind == "generic" && i.text.contains("Model metadata")),
        "{items:?}"
    );
    r.release_session(&id).await.unwrap();
}
