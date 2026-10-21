use super::*;

#[tokio::test]
async fn codex_fork_releases_writer_before_independent_load() {
    let dir = tempfile::tempdir().unwrap();
    let python = which::which("python3").unwrap();
    let script = dir.path().join("agent.py");
    std::fs::write(
        &script,
        include_str!("../../tests/fixtures/fork_writer_agent.py"),
    )
    .unwrap();
    let command = shell_words::join([python.to_str().unwrap(), script.to_str().unwrap()]);
    let def = AcpAgentDef {
        id: "codex".into(),
        display_name: "Fork writer fixture".into(),
        description: String::new(),
        transport: "native".into(),
        base_command: command.clone(),
        server_command: command,
        install_command: None,
        model_list_command: None,
    };
    let runtime = AcpRuntime::bound(Provider::Codex, "codex", 5_000);
    let cwd = dir.path().to_str().unwrap();
    let policy = ProductSessionPolicy {
        sandbox_mode: Some("read-only".into()),
        approval_mode: Some("guarded".into()),
        ..Default::default()
    };
    let (source_id, source) = runtime
        .spawn_session(&def, cwd, policy.clone(), None, None)
        .await
        .unwrap();
    let source_process = source.process.clone();
    let _source_cleanup = StartupProcess(Some(source_process.clone()));
    runtime
        .inner
        .sessions
        .lock()
        .await
        .insert(source_id.clone(), source);

    let error = runtime
        .fork_session_at(
            &source_id,
            &crate::actor::ForkPoint {
                rollback_count: 99,
                reply: None,
            },
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("selected Codex turn was not found"));
    let requests = std::fs::read_to_string(dir.path().join("requests.jsonl")).unwrap();
    let last: Value = serde_json::from_str(requests.lines().last().unwrap()).unwrap();
    assert_eq!(last["method"], "thread/turns/list");
    // Even a failed fork must finish its helper before returning to the caller.
    assert_eq!(
        unsafe { libc::kill(last["pid"].as_i64().unwrap() as i32, 0) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );

    for rollback in [0, 1] {
        let fork = runtime
            .fork_session_at(
                &source_id,
                &crate::actor::ForkPoint {
                    rollback_count: rollback,
                    reply: None,
                },
            )
            .await
            .unwrap()
            .session;
        let raw = fork.provider_session_id.split_once("::").unwrap().1;
        // This is the independent load required when the fork gets its own CLI identity.
        let (_, loaded) = runtime
            .spawn_session(&def, cwd, policy.clone(), Some(raw), None)
            .await
            .expect("fork must release its writer before another process resumes it");
        let _fork_cleanup = StartupProcess(Some(loaded.process.clone()));
        assert!(
            !runtime.session_loaded(&fork.provider_session_id),
            "fork must not inherit the parent's process identity"
        );
        for (process, session) in [
            (&source_process, source_id.split_once("::").unwrap().1),
            (&loaded.process, raw),
        ] {
            process
                .request("session/prompt", json!({"sessionId":session,"prompt":[]}))
                .await
                .unwrap();
        }
        let params: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(format!("{raw}.json"))).unwrap(),
        )
        .unwrap();
        if rollback == 1 {
            assert_eq!(params["lastTurnId"], "turn-1");
        } else {
            assert!(params.get("lastTurnId").is_none());
        }
        assert_eq!(params["sandbox"], "read-only");
        assert_eq!(params["approvalPolicy"], "on-request");
        assert_eq!(params["deferGoalContinuation"], true);
        let child_process = loaded.process.clone();
        runtime
            .inner
            .sessions
            .lock()
            .await
            .insert(fork.provider_session_id.clone(), loaded);
        runtime
            .release_session(&fork.provider_session_id)
            .await
            .unwrap();
        assert!(!child_process.connection_open().await);
        assert!(
            source_process.connection_open().await,
            "deleting a child must preserve its parent's process"
        );
        assert!(!runtime
            .inner
            .sessions
            .lock()
            .await
            .contains_key(&fork.provider_session_id));
    }
    source_process.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an installed Codex CLI; uses an isolated CODEX_HOME and no model calls"]
async fn native_codex_fork_writer_handoff() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    let command = shell_words::join([
        "env".to_string(),
        format!("CODEX_HOME={cwd}"),
        which::which("codex")
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    ]);
    let (source, _, _) = AcpProcess::spawn(&format!("{command} app-server"), cwd, &[])
        .await
        .unwrap();
    let source = Arc::new(source);
    let _source_cleanup = StartupProcess(Some(source.clone()));
    let initialize = json!({"clientInfo":{"name":"fork-regression","version":"1"},"capabilities":{"experimentalApi":true}});
    source
        .request("initialize", initialize.clone())
        .await
        .unwrap();
    source.notify("initialized", json!({})).await.unwrap();
    let started = source
        .request(
            "thread/start",
            json!({"cwd":cwd,"model":"gpt-5.6-luna","persistExtendedHistory":true}),
        )
        .await
        .unwrap();
    let source_id = started["thread"]["id"].as_str().unwrap();
    source.request("thread/inject_items", json!({"threadId":source_id,"items":[{"type":"message","role":"user","content":[{"type":"input_text","text":"fork regression seed"}]}]})).await.unwrap();
    let policy = ProductSessionPolicy::default();
    let bridge = super::super::codex_bridge::CodexBridge::new(&command, &mut vec![], &policy)
        .await
        .unwrap();
    let (child, _, _) = AcpProcess::spawn(&format!("{command} app-server"), cwd, &[])
        .await
        .unwrap();
    let child = Arc::new(child);
    let _child_cleanup = StartupProcess(Some(child.clone()));
    child.request("initialize", initialize).await.unwrap();
    child.notify("initialized", json!({})).await.unwrap();
    // Reproduce the old ownership conflict against the real native implementation.
    let stranded = source
        .request(
            "thread/fork",
            json!({"threadId":source_id,"deferGoalContinuation":true}),
        )
        .await
        .unwrap();
    let error = child
        .request(
            "thread/resume",
            json!({"threadId":stranded["thread"]["id"]}),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("already has an active writer"),
        "{error:#}"
    );
    let fork_id = bridge.fork(source_id, 0, cwd, &policy).await.unwrap();
    let resumed = child
        .request("thread/resume", json!({"threadId":fork_id}))
        .await
        .expect("native fork can immediately resume in another app-server");
    // Injected context has no turn, and paginated forks reference the source
    // prefix rather than duplicating it in their own rollout file.
    let rollout = std::fs::read_to_string(resumed["thread"]["path"].as_str().unwrap()).unwrap();
    let meta: Value = serde_json::from_str(rollout.lines().next().unwrap()).unwrap();
    let base = &meta["payload"]["history_base"];
    assert_eq!(base["thread_id"], source_id);
    let source_rollout =
        std::fs::read_to_string(started["thread"]["path"].as_str().unwrap()).unwrap();
    assert!(
        source_rollout.lines().any(|line| {
            let item: Value = serde_json::from_str(line).unwrap();
            item["ordinal"].as_u64().unwrap() < base["end_ordinal_exclusive"].as_u64().unwrap()
                && line.contains("fork regression seed")
        }),
        "the fork's inherited prefix must include the source history"
    );
    let loaded = source
        .request("thread/loaded/list", json!({}))
        .await
        .unwrap();
    assert!(loaded["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == source_id));
    assert!(!loaded["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == &fork_id));
    child.shutdown().await.unwrap();
    source.shutdown().await.unwrap();
}
