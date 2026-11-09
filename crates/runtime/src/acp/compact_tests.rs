use super::*;

#[tokio::test]
async fn compact_uses_the_loaded_sessions_late_command_and_rejects_withdrawal() {
    for agent in ["claude", "codex"] {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fixture.py");
        std::fs::write(
            &script,
            include_str!("../../tests/fixtures/fake_acp_agent.py"),
        )
        .unwrap();
        let python = which::which("python3").unwrap();
        let command = shell_words::join([
            python.to_str().unwrap(),
            script.to_str().unwrap(),
            "--compact",
        ]);
        let def = AcpAgentDef {
            id: agent.into(),
            display_name: agent.into(),
            description: String::new(),
            transport: "native".into(),
            base_command: command.clone(),
            server_command: command,
            install_command: None,
            model_list_command: None,
        };
        let runtime = AcpRuntime::bound(
            if agent == "claude" {
                Provider::Claude
            } else {
                Provider::Codex
            },
            agent,
            5_000,
        );
        let (session, live) = runtime
            .spawn_session(
                &def,
                dir.path().to_str().unwrap(),
                ProductSessionPolicy::default(),
                None,
                None,
                None,
            )
            .await
            .unwrap();
        let _cleanup = StartupProcess(Some(live.process.clone()));
        let key = (live.process.id.clone(), live.session_id.clone());
        runtime
            .inner
            .sessions
            .lock()
            .await
            .insert(session.clone(), live);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if runtime
                    .inner
                    .commands
                    .lock()
                    .await
                    .get(&key)
                    .is_some_and(|commands| commands.iter().any(|c| c.name == "compact"))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let caps = runtime
            .session_capabilities(Some(agent), &session)
            .await
            .unwrap();
        assert!(caps.effective_capabilities.unwrap().turns.compact);
        assert!(caps
            .toolbox_items
            .iter()
            .any(|item| item.action == "compact"));
        runtime
            .compact_session(&session, "context-test", EventBus::new())
            .await
            .unwrap();
        let request: Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("compact-request.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            request,
            json!({"sessionId":"fake-session","text":"/compact"})
        );
        if agent == "claude" {
            runtime.inner.commands.lock().await.insert(key, Vec::new());
            let caps = runtime
                .session_capabilities(Some(agent), &session)
                .await
                .unwrap();
            assert!(!caps.effective_capabilities.unwrap().turns.compact);
            assert!(runtime
                .compact_session(&session, "context-test", EventBus::new())
                .await
                .is_err());
        }
    }
}
