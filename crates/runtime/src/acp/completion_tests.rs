use super::*;

#[tokio::test]
async fn claude_background_wake_output_stays_live_and_persists_after_foreground_reply() {
    use crate::{config::RuntimeConfig, db::Database, fake::FakeRuntime, Supervisor};
    use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput};
    use rusqlite::params;

    // Isolate native usage logs without mutating process-wide environment in
    // parallel Rust tests, just as the existing Claude completion fixtures do.
    if std::env::var("POCKYMOE_WAKE_TEST_CHILD").is_err() {
        let home = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "acp::runtime::completion_tests::claude_background_wake_output_stays_live_and_persists_after_foreground_reply", "--nocapture"])
            .env("POCKYMOE_WAKE_TEST_CHILD", "1")
            .env("CLAUDE_CONFIG_DIR", home.path()).output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    for scenario in [
        "normal",
        "idle-steer",
        "idle-native",
        "idle-steer-cancel",
        "silent",
        "merged",
        "cancel",
        "disconnect",
        "native",
        "monitor",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("background.py");
        std::fs::write(
            &script,
            include_str!("../../tests/fixtures/claude_background_wake_agent.py"),
        )
        .unwrap();
        let python = which::which("python3").unwrap();
        let command = shell_words::join([python.to_str().unwrap(), script.to_str().unwrap()]);
        let def = AcpAgentDef {
            id: "claude".into(),
            display_name: "Background wake fixture".into(),
            description: String::new(),
            transport: "native".into(),
            base_command: command.clone(),
            server_command: command,
            install_command: None,
            model_list_command: None,
        };
        let runtime = AcpRuntime::bound(Provider::Acp, "claude", 5_000);
        let (session, live) = runtime
            .spawn_session(
                &def,
                dir.path().to_str().unwrap(),
                ProductSessionPolicy::default(),
                None,
                None,
            )
            .await
            .unwrap();
        let _cleanup = StartupProcess(Some(live.process.clone()));
        runtime
            .inner
            .sessions
            .lock()
            .await
            .insert(session.clone(), live);

        // Use the real durable event persister and history API, with an isolated
        // fake-owned thread. No live account, model call or formal DB is touched.
        let mut config = RuntimeConfig::from_env().unwrap();
        config.database_url = dir.path().join("history.sqlite");
        config.workspace_root = dir.path().join("workspaces");
        config.fake_runtime = true;
        config.relay_server_url = None;
        config.relay_agent_token = None;
        let supervisor = Arc::new(Supervisor::new(
            config.clone(),
            Database::open(&config.database_url).unwrap(),
            vec![Arc::new(FakeRuntime::new(Provider::Codex))],
        ));
        let workspace = supervisor
            .create_workspace(CreateWorkspaceInput {
                abs_path: Some(dir.path().to_string_lossy().into_owned()),
                git_url: None,
                label: None,
            })
            .unwrap();
        let thread = supervisor
            .create_thread(CreateThreadInput {
                workspace_id: workspace.id,
                title: Some("native wake".into()),
                provider: Some(Provider::Codex),
                agent_id: None,
                model: "default".into(),
                reasoning_effort: None,
                approval_mode: "yolo".into(),
                parent_thread_id: None,
            })
            .await
            .unwrap();
        let turn_id = "owned-turn";
        supervisor.db.with(|c| {
            c.execute("INSERT INTO thread_turns(id,thread_id,status,started_at,ordinal) VALUES(?1,?2,'inProgress',?3,1)",
                params![turn_id,thread.id,now_rfc3339()])?;
            Ok(())
        }).unwrap();
        supervisor.spawn_live_item_persister();
        let bus = supervisor.bus.for_turn(&thread.id, turn_id);
        let cancel = CancellationToken::new();
        let mut events = bus.subscribe();
        let turn = runtime.start_turn(
            StartTurnInput {
                provider_session_id: session.clone(),
                prompt: scenario.into(),
                model: None,
                reasoning_effort: None,
                sandbox_mode: None,
                collaboration_mode: None,
                approval_mode: None,
                performance_mode: None,
                thread_id: thread.id.clone(),
                turn_id: turn_id.into(),
                hidden: false,
                images: vec![],
                title: None,
                context_delivered: false,
            },
            bus,
            cancel.clone(),
        );
        tokio::pin!(turn);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                tokio::select! {
                    result = &mut turn => panic!("{scenario}: foreground prematurely completed: {result:?}"),
                    event = events.recv() => {
                        if event.unwrap().event_type == "thread.output.delta" { break; }
                    }
                }
            }
        }).await.unwrap();
        // Past the old 250ms foreground drain: no wake yet, so the turn must
        // remain owned, cancellable, and not report a manufactured completion.
        assert!(tokio::time::timeout(Duration::from_millis(400), &mut turn)
            .await
            .is_err());
        assert!(dir.path().join("foreground-returned").exists());
        assert!(matches!(
            runtime.execution_state(&session).await,
            crate::actor::ExecutionState::Running { .. }
        ));
        let waiting = supervisor
            .get_thread_turn_detail(&thread.id, turn_id)
            .await
            .unwrap();
        assert!(
            waiting.items.iter().any(|i| i.extra.get("origin")
                == Some(&json!("nativeBackgroundWait"))
                && i.status.as_deref() == Some("waiting")),
            "{scenario}: waiting marker persists before wake"
        );
        let summary = supervisor
            .get_thread_detail_view(&thread.id, Some(1), true)
            .await
            .unwrap();
        assert!(
            summary.turns[0]
                .items
                .iter()
                .any(|i| i.extra.get("origin") == Some(&json!("nativeBackgroundWait"))),
            "{scenario}: lightweight reload must retain waiting anchor"
        );
        if scenario.starts_with("idle-") {
            runtime
                .send_input(&session, "wrong-turn", "do not deliver")
                .await
                .unwrap_err();
            runtime
                .send_input(&session, turn_id, "reject-waiting-steer")
                .await
                .unwrap_err();
            tokio::time::timeout(
                Duration::from_secs(2),
                runtime.send_input(&session, turn_id, "correct the waiting work"),
            )
            .await
            .expect("waiting delivery must not wait for the response to finish")
            .unwrap();
            assert!(tokio::time::timeout(Duration::from_millis(400), &mut turn)
                .await
                .is_err());
            assert!(dir.path().join("idle-steering-started").exists());
            let correction = supervisor
                .get_thread_turn_detail(&thread.id, turn_id)
                .await
                .unwrap();
            assert!(correction
                .items
                .iter()
                .any(|item| item.text == "Waiting correction processed."));
            if scenario != "idle-steer-cancel" {
                std::fs::write(dir.path().join("finish-correction"), "").unwrap();
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(400), &mut turn)
                    .await
                    .is_err(),
                "the original native task still owns the later wake"
            );
        }
        if matches!(scenario, "cancel" | "idle-steer-cancel") {
            cancel.cancel();
        } else {
            std::fs::write(dir.path().join("release"), "").unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    tokio::select! {
                        result = &mut turn => panic!("{scenario}: task notification ended follow-up early: {result:?}"),
                        event = events.recv() => {
                            let event = event.unwrap();
                            if event.payload["item"]["id"] == "verify-release" { break; }
                        }
                    }
                }
            }).await.unwrap();
            let before = supervisor
                .get_thread_turn_detail(&thread.id, turn_id)
                .await
                .unwrap();
            assert_eq!(before.status, "inProgress");
            assert_eq!(
                before
                    .items
                    .iter()
                    .filter(|i| i.extra.get("origin") == Some(&json!("nativeTaskNotification")))
                    .count(),
                1
            );
            assert!(before
                .items
                .iter()
                .any(|i| i.text == "Checking the completed release."));
            assert!(before
                .items
                .iter()
                .any(|i| i.id == "verify-release" && i.status.as_deref() == Some("running")));
            assert!(tokio::time::timeout(Duration::from_millis(350), &mut turn)
                .await
                .is_err());
            if scenario == "merged" {
                runtime
                    .send_input(&session, turn_id, "check the deployment too")
                    .await
                    .unwrap();
            }
            std::fs::write(dir.path().join("finish"), "").unwrap();
        }
        let result = tokio::time::timeout(Duration::from_secs(4), &mut turn)
            .await
            .unwrap();
        let detail = supervisor
            .get_thread_turn_detail(&thread.id, turn_id)
            .await
            .unwrap();
        let expected = match scenario {
            "cancel" | "idle-steer-cancel" => "interrupted",
            "disconnect" => "recovering",
            _ => "completed",
        };
        assert_eq!(detail.status, expected, "{scenario}");
        let summary = supervisor
            .get_thread_detail_view(&thread.id, Some(1), true)
            .await
            .unwrap();
        assert!(
            summary.turns[0]
                .items
                .iter()
                .any(|i| i.extra.get("origin").is_some()),
            "{scenario}: completed/interrupted reload retains the anchor"
        );
        if !matches!(scenario, "cancel" | "idle-steer-cancel") {
            assert!(
                summary.turns[0]
                    .items
                    .iter()
                    .any(|i| i.text == "Checking the completed release."),
                "{scenario}: summary retains intermediate narrative"
            );
            assert!(
                summary.turns[0]
                    .items
                    .iter()
                    .all(|i| i.kind != "commandExecution"),
                "{scenario}: tool details remain lazy"
            );
        }
        if scenario == "disconnect" {
            assert!(result.unwrap_err().is::<crate::actor::ExecutionUncertain>());
        } else {
            result.unwrap();
        }
        if !matches!(scenario, "cancel" | "idle-steer-cancel") {
            assert_eq!(
                detail
                    .items
                    .iter()
                    .filter(|i| i.extra.get("origin") == Some(&json!("nativeTaskNotification")))
                    .count(),
                if scenario == "monitor" { 2 } else { 1 }
            );
            assert!(detail.items.iter().any(|i| i.id == "verify-release"));
            if scenario != "disconnect" {
                let tool = detail
                    .items
                    .iter()
                    .find(|i| i.id == "verify-release")
                    .unwrap();
                assert_eq!(tool.status.as_deref(), Some("completed"));
                assert!(tool
                    .detail_text
                    .as_deref()
                    .unwrap()
                    .contains("Verified without repeating"));
                let usage = detail.token_usage.as_ref().unwrap();
                assert_eq!(
                    usage["total"]["inputTokens"],
                    if scenario == "silent" { 20 } else { 30 }
                );
                assert_eq!(
                    usage["total"]["outputTokens"],
                    if scenario == "silent" { 7 } else { 10 }
                );
            }
            if scenario == "merged" {
                assert!(detail
                    .items
                    .iter()
                    .any(|i| i.text == "User correction processed."));
            }
            if scenario == "silent" {
                assert!(!detail.items.iter().any(|i| i.text == "Release verified."));
            }
        }
        let transcript = supervisor
            .transcript(&thread.id, &crate::interaction::TranscriptQuery::default())
            .unwrap();
        assert_eq!(
            transcript["turns"].as_array().unwrap().len(),
            1,
            "no duplicate synthetic turn"
        );
    }
}

#[tokio::test]
async fn claude_coalesced_prompt_drains_only_proven_completed_work() {
    let dir = tempfile::tempdir().unwrap();
    if std::env::var("POCKYMOE_COALESCED_TEST_CHILD").is_err() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "acp::runtime::completion_tests::claude_coalesced_prompt_drains_only_proven_completed_work", "--nocapture"])
            .env("POCKYMOE_COALESCED_TEST_CHILD", "1")
            .env("CLAUDE_CONFIG_DIR", dir.path()).output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let script = dir.path().join("coalesced.py");
    std::fs::write(
        &script,
        include_str!("../../tests/fixtures/claude_coalesced_agent.py"),
    )
    .unwrap();
    let python = which::which("python3").unwrap();
    let command = shell_words::join([python.to_str().unwrap(), script.to_str().unwrap()]);
    let def = AcpAgentDef {
        id: "claude".into(),
        display_name: "Coalesced fixture".into(),
        description: String::new(),
        transport: "native".into(),
        base_command: command.clone(),
        server_command: command,
        install_command: None,
        model_list_command: None,
    };
    let runtime = AcpRuntime::bound(Provider::Acp, "claude", 5_000);
    for scenario in [
        "coalesced",
        "cancel-error",
        "background",
        "mismatch",
        "unfinished-tool",
        "missing-lifecycle",
        "provider-error",
        "user-cancel",
        "steer-before-drain",
    ] {
        if std::env::var("POCKYMOE_COALESCED_TEST_SCENARIO")
            .is_ok_and(|selected| selected != scenario)
        {
            continue;
        }
        let (session, live) = runtime
            .spawn_session(
                &def,
                dir.path().to_str().unwrap(),
                ProductSessionPolicy::default(),
                None,
                None,
            )
            .await
            .unwrap();
        let cleanup = StartupProcess(Some(live.process.clone()));
        let native_session = live.session_id.clone();
        runtime
            .inner
            .sessions
            .lock()
            .await
            .insert(session.clone(), live);
        let cancel = CancellationToken::new();
        let bus = EventBus::new();
        let mut events = bus.subscribe();
        let input = |prompt: &str| StartTurnInput {
            provider_session_id: session.clone(),
            prompt: prompt.into(),
            model: None,
            reasoning_effort: None,
            sandbox_mode: None,
            collaboration_mode: None,
            approval_mode: None,
            performance_mode: None,
            thread_id: "fixture".into(),
            turn_id: prompt.into(),
            hidden: false,
            images: Vec::new(),
            title: None,
            context_delivered: false,
        };
        let turn = runtime.start_turn(input(scenario), bus.clone(), cancel.clone());
        tokio::pin!(turn);
        let marker = PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").unwrap())
            .join("projects/fixture")
            .join(native_session);
        let success = matches!(
            scenario,
            "coalesced" | "cancel-error" | "background" | "steer-before-drain"
        );
        if scenario == "steer-before-drain" {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let event = tokio::select! {
                        result = &mut turn => panic!("turn ended before steering: {result:?}"),
                        event = events.recv() => event.unwrap(),
                    };
                    if event.event_type == "thread.output.delta" && event.payload["delta"] == "done"
                    {
                        break;
                    }
                }
            })
            .await
            .unwrap();
            runtime
                .send_input(&session, scenario, "change direction")
                .await
                .unwrap();
        }
        if !success || scenario == "background" {
            let wait = if scenario == "user-cancel" {
                Duration::from_secs(1)
            } else {
                Duration::from_secs(5)
            };
            assert!(
                tokio::time::timeout(wait, &mut turn).await.is_err(),
                "{scenario} settled without proof"
            );
            assert!(
                !marker.with_extension("cancelled").exists(),
                "{scenario} cancelled real/unproven work"
            );
            if scenario == "background" {
                std::fs::write(marker.with_extension("release"), "").unwrap();
            } else {
                cancel.cancel();
            }
        }
        let result = tokio::time::timeout(Duration::from_secs(8), &mut turn)
            .await
            .expect(scenario)
            .unwrap();
        let completed = std::iter::from_fn(|| events.try_recv().ok())
            .find(|event| event.event_type == "thread.turn.completed")
            .unwrap();
        assert_eq!(
            completed.payload["status"],
            if success { "completed" } else { "interrupted" },
            "{scenario}"
        );
        assert_eq!(
            cancel.is_cancelled(),
            !success,
            "housekeeping must not cancel the user turn"
        );
        if success {
            assert!(result
                .iter()
                .any(|item| item.text.starts_with("done")
                    && item.status.as_deref() == Some("completed")));
            assert_eq!(
                marker.with_extension("cancelled").exists(),
                scenario != "steer-before-drain",
                "{scenario}"
            );
            if scenario == "steer-before-drain" {
                assert!(result
                    .iter()
                    .any(|item| item.text.contains("steering processed")));
            }
            let next = runtime
                .start_turn(input("follow-up"), bus, CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(
                next.iter()
                    .filter(|item| item.text == "follow-up completed once")
                    .count(),
                1
            );
        }
        assert!(runtime
            .inner
            .sessions
            .lock()
            .await
            .get(&session)
            .unwrap()
            .active
            .is_none());
        drop(cleanup);
    }
}

#[tokio::test]
async fn claude_incomplete_tool_reconciles_only_abandoned_streams() {
    // Isolate provider homes in a subprocess rather than changing process-wide
    // environment while other Rust tests are running.
    let dir = tempfile::tempdir().unwrap();
    if std::env::var("POCKYMOE_COMPLETION_TEST_CHILD").is_err() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "acp::runtime::completion_tests::claude_incomplete_tool_reconciles_only_abandoned_streams", "--nocapture"])
            .env("POCKYMOE_COMPLETION_TEST_CHILD", "1")
            .env("CLAUDE_CONFIG_DIR", dir.path())
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let script = dir.path().join("agent.py");
    std::fs::write(
        &script,
        include_str!("../../tests/fixtures/claude_completion_agent.py"),
    )
    .unwrap();
    let python = which::which("python3").unwrap();
    let command = shell_words::join([python.to_str().unwrap(), script.to_str().unwrap()]);
    let def = AcpAgentDef {
        id: "claude".into(),
        display_name: "Completion fixture".into(),
        description: String::new(),
        transport: "native".into(),
        base_command: command.clone(),
        server_command: command,
        install_command: None,
        model_list_command: None,
    };
    let runtime = AcpRuntime::bound(Provider::Acp, "claude", 5_000);
    for scenario in ["abandoned", "real", "missing", "background"] {
        let (session, live) = runtime
            .spawn_session(
                &def,
                dir.path().to_str().unwrap(),
                ProductSessionPolicy::default(),
                None,
                None,
            )
            .await
            .unwrap();
        let cleanup = StartupProcess(Some(live.process.clone()));
        let native_session = live.session_id.clone();
        runtime
            .inner
            .sessions
            .lock()
            .await
            .insert(session.clone(), live);
        let bus = EventBus::new();
        let mut events = bus.subscribe();
        let turn = runtime.start_turn(
            StartTurnInput {
                provider_session_id: session.clone(),
                prompt: scenario.into(),
                model: None,
                reasoning_effort: None,
                sandbox_mode: None,
                collaboration_mode: None,
                approval_mode: None,
                performance_mode: None,
                thread_id: "fixture".into(),
                turn_id: scenario.into(),
                hidden: false,
                images: Vec::new(),
                title: None,
                context_delivered: false,
            },
            bus,
            CancellationToken::new(),
        );
        tokio::pin!(turn);
        if scenario == "background" {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    tokio::select! {
                        result = &mut turn => panic!("Turn settled while its background agent was running: {result:?}"),
                        event = events.recv() => {
                            let event = event.unwrap();
                            if event.event_type == "thread.subagents.updated"
                                && event.payload["activeSubagents"][0]["isBackground"] == true
                            { break; }
                        }
                    }
                }
            }).await.unwrap();
            let active = runtime.active_subagents(&session).await;
            assert_eq!(active.len(), 1);
            assert_eq!(active[0].name.as_deref(), Some("Independent review"));
            assert_eq!(active[0].status, "running");
            assert!(matches!(
                runtime.execution_state(&session).await,
                crate::actor::ExecutionState::Running { .. }
            ));
            let home = std::env::var_os("CLAUDE_CONFIG_DIR").unwrap();
            std::fs::write(
                PathBuf::from(home)
                    .join("projects/fixture")
                    .join(format!("{native_session}.release")),
                "",
            )
            .unwrap();
        }
        let result = tokio::time::timeout(Duration::from_secs(5), &mut turn)
            .await
            .unwrap();
        let mut completed = None;
        while let Ok(event) = events.try_recv() {
            if event.event_type == "thread.turn.completed" {
                completed = Some(event);
            }
        }
        let completed = completed.unwrap();
        if scenario == "background" {
            assert!(result
                .unwrap()
                .iter()
                .any(|item| item.text == "Review processed"));
            assert_eq!(completed.payload["status"], "completed");
            assert!(runtime.active_subagents(&session).await.is_empty());
        } else if scenario == "abandoned" {
            let items = result.unwrap();
            assert_eq!(completed.payload["status"], "completed");
            assert!(completed.payload["error"].is_null());
            assert!(items
                .iter()
                .any(|item| item.id == "toolu_orphan"
                    && item.status.as_deref() == Some("interrupted")));
            assert!(items.iter().any(|item| item.kind == "agentMessage"
                && item.text == "done"
                && item.status.as_deref() == Some("completed")));
        } else {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("incomplete_tool_call"));
            assert_eq!(completed.payload["status"], "failed");
        }
        assert!(runtime
            .inner
            .sessions
            .lock()
            .await
            .get(&session)
            .unwrap()
            .active
            .is_none());
        drop(cleanup);
    }
}
