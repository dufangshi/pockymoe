use std::time::{Duration, Instant};

use pockymoe_runtime::acp::AcpRuntime;
use pockymoe_runtime::actor::{AgentRuntime, EventBus, StartSessionInput, StartTurnInput};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn live_prompt_streams_tools_and_waits_for_agent() {
    let python = which_python();
    let dir = tempdir().unwrap();
    let (runtime, session_id) = start_runtime(dir.path(), &python).await;

    let bus = EventBus::new();
    let mut events = bus.subscribe();
    let cancel = CancellationToken::new();
    let started = Instant::now();
    let items = runtime
        .start_turn(turn_input(&session_id, "hello", "turn-1"), bus, cancel)
        .await
        .expect("complete ACP turn");
    assert!(
        started.elapsed() >= Duration::from_millis(1400),
        "session/prompt should wait for a long-running agent, not a 180s-or-shorter RPC cutoff"
    );

    let mut received = Vec::new();
    while let Ok(event) = events.try_recv() {
        received.push(event);
    }
    let types: Vec<_> = received
        .iter()
        .map(|event| event.event_type.as_str())
        .collect();
    assert!(
        types.contains(&"thread.item.started"),
        "live tool/thought items should be broadcast during ACP execution, got {types:?}"
    );
    assert!(
        types.iter().position(|ty| *ty == "thread.item.started")
            < types.iter().position(|ty| *ty == "thread.turn.completed"),
        "tool calls must appear before the turn completes, got {types:?}"
    );
    assert!(received.iter().any(|event| {
        event.event_type == "thread.item.started"
            && event.payload["item"]["kind"] == "commandExecution"
    }));
    assert!(received.iter().any(|event| {
        event.event_type == "thread.item.started" && event.payload["item"]["kind"] == "reasoning"
    }));
    assert!(items
        .iter()
        .any(|item| item.kind == "agentMessage" && item.text == "done"));
    assert!(items.iter().any(|item| item.kind == "commandExecution"));
    assert!(items.iter().all(|item| item.text != "(no output)"));
}

#[tokio::test]
async fn prompt_rpc_error_is_failed_and_clears_active_state() {
    let python = which_python();
    let dir = tempdir().unwrap();
    let (runtime, session_id) = start_runtime(dir.path(), &python).await;
    let bus = EventBus::new();
    let mut events = bus.subscribe();
    let cancel = CancellationToken::new();

    let error = runtime
        .start_turn(
            turn_input(&session_id, "rpc-error", "failed-turn"),
            bus.clone(),
            cancel.clone(),
        )
        .await
        .expect_err("JSON-RPC error should fail the turn");
    assert!(format!("{error:#}").contains("forced prompt failure"));
    assert!(
        !cancel.is_cancelled(),
        "provider failure is not user cancellation"
    );
    let completed = completed_event(&mut events);
    assert_eq!(completed.payload["status"], "failed");
    assert!(completed.payload["error"]
        .as_str()
        .unwrap_or_default()
        .contains("forced prompt failure"));

    tokio::time::timeout(
        Duration::from_secs(2),
        runtime.start_turn(
            turn_input(&session_id, "quick-success", "recovery-turn"),
            bus,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("recovery turn should not hang")
    .expect("RPC failure must clear active state");
}

#[tokio::test]
async fn completed_tools_keep_their_results_when_the_enclosing_prompt_is_cancelled() {
    let dir = tempdir().unwrap();
    let (runtime, session) = start_runtime(dir.path(), &which_python()).await;
    let items = runtime
        .start_turn(
            turn_input(&session, "cancel-after-completed-tool", "cancelled-turn"),
            EventBus::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        items
            .iter()
            .find(|item| item.id == "finished")
            .unwrap()
            .status
            .as_deref(),
        Some("completed")
    );
    assert_eq!(
        items
            .iter()
            .find(|item| item.id == "unfinished")
            .unwrap()
            .status
            .as_deref(),
        Some("interrupted")
    );
}

#[tokio::test]
async fn process_exit_leaves_completion_unconfirmed() {
    let python = which_python();
    let dir = tempdir().unwrap();
    let (runtime, session_id) = start_runtime(dir.path(), &python).await;
    let bus = EventBus::new();
    let mut events = bus.subscribe();

    let error = tokio::time::timeout(
        Duration::from_secs(2),
        runtime.start_turn(
            turn_input(&session_id, "exit-before-response", "exit-turn"),
            bus,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("process exit must settle an unbounded prompt RPC")
    .expect_err("process exit should fail the turn");
    assert!(
        error.to_string().contains("stdout closed") || error.to_string().contains("process exited"),
        "{error:#}"
    );
    let completed = completed_event(&mut events);
    assert_eq!(completed.payload["status"], "recovering");
    assert!(!completed.payload["error"].is_null());
    runtime
        .resume_session(
            &session_id,
            Some(dir.path().to_str().unwrap()),
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        runtime.execution_state(&session_id).await,
        pockymoe_runtime::actor::ExecutionState::Idle
    );
    runtime
        .start_turn(
            turn_input(&session_id, "hello after reconnect", "reconnected-turn"),
            EventBus::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn steering_is_acknowledged_and_processed_before_the_active_turn_finishes() {
    let dir = tempdir().unwrap();
    let (runtime, session_id) = start_runtime(dir.path(), &which_python()).await;
    let runtime = std::sync::Arc::new(runtime);
    let bus = EventBus::new();
    let mut events = bus.subscribe();
    let task = {
        let runtime = runtime.clone();
        let session_id = session_id.clone();
        tokio::spawn(async move {
            runtime
                .start_turn(
                    turn_input(&session_id, "wait-for-steer", "steered-turn"),
                    bus,
                    CancellationToken::new(),
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.event_type == "thread.output.delta" {
                break;
            }
        }
    })
    .await
    .expect("agent is waiting within the original prompt");
    assert!(!task.is_finished());
    assert!(
        runtime.restart("custom").await.is_err(),
        "active turns must refuse restart"
    );
    assert!(runtime
        .send_input(&session_id, "wrong-turn", "hello")
        .await
        .is_err());
    for prompt in [
        "reject-steer",
        "fail-steer",
        "unknown-steer",
        "idle-race-steer",
        "detached-steer",
    ] {
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            runtime.send_input(&session_id, "steered-turn", prompt),
        )
        .await
        .expect("steering response must not wait for turn completion");
        assert!(result.is_err(), "must not report accepted for {prompt}");
        assert!(
            !task.is_finished(),
            "failed steering must not end the active turn"
        );
    }
    tokio::time::timeout(
        Duration::from_secs(2),
        runtime.send_input(&session_id, "steered-turn", "change direction"),
    )
    .await
    .expect("steering must receive its own acknowledgement")
    .unwrap();
    let items = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("steering unblocks the original prompt")
        .unwrap()
        .unwrap();
    assert!(items
        .iter()
        .any(|item| item.text.contains("handled steer: change direction")));
}

#[tokio::test]
async fn restored_session_applies_advertised_effort_and_recovers_after_settings_failure() {
    let dir = tempdir().unwrap();
    let python = which_python();
    let (_original, session_id) = start_runtime(dir.path(), &python).await;
    // A fresh runtime has no in-memory session/settings cache, as after restart.
    let command = format!(
        r#"{python} "{}""#,
        dir.path().join("fake_acp_agent.py").display()
    );
    let restored = AcpRuntime::catalog(Some(command), 5_000);
    restored.start().await.unwrap();
    assert!(!restored.session_loaded(&session_id));
    restored
        .resume_session(&session_id, dir.path().to_str(), Default::default())
        .await
        .unwrap();
    for (turn_id, effort) in [("after-restart", "high"), ("retry", "medium")] {
        let mut input = turn_input(&session_id, "report-effort", turn_id);
        input.reasoning_effort = Some(effort.into());
        let items = restored
            .start_turn(input, EventBus::new(), CancellationToken::new())
            .await
            .expect("restored session must accept follow-up prompts");
        assert!(items
            .iter()
            .any(|item| item.text == format!("effort={effort}")));
        let mut invalid = turn_input(&session_id, "report-effort", "invalid-setting");
        invalid.reasoning_effort = Some("unsupported-effort".into());
        assert!(restored
            .start_turn(invalid, EventBus::new(), CancellationToken::new())
            .await
            .is_err());
    }
}

async fn start_runtime(dir: &std::path::Path, python: &str) -> (AcpRuntime, String) {
    start_runtime_with_args(dir, python, "").await
}

#[tokio::test]
async fn harness_restart_reloads_configuration_and_resumes_the_same_session() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("fixture-config.txt"), "upstream-before").unwrap();
    let (runtime, session) = start_runtime(dir.path(), &which_python()).await;
    let read = |turn: &str| turn_input(&session, "read-startup-config", turn);
    let before = runtime
        .start_turn(read("before"), EventBus::new(), CancellationToken::new())
        .await
        .unwrap();
    assert!(before.iter().any(|i| i.text == "upstream-before"));
    std::fs::write(dir.path().join("fixture-config.txt"), "upstream-after").unwrap();
    assert_eq!(runtime.restart("custom").await.unwrap(), 1);
    runtime
        .resume_session(
            &session,
            Some(dir.path().to_str().unwrap()),
            Default::default(),
        )
        .await
        .unwrap();
    let after = runtime
        .start_turn(read("after"), EventBus::new(), CancellationToken::new())
        .await
        .unwrap();
    assert!(after.iter().any(|i| i.text == "upstream-after"));
}

async fn start_runtime_with_args(
    dir: &std::path::Path,
    python: &str,
    args: &str,
) -> (AcpRuntime, String) {
    let script = dir.join("fake_acp_agent.py");
    std::fs::write(&script, include_str!("fixtures/fake_acp_agent.py")).unwrap();
    let command = format!(r#"{python} "{}" {args}"#, script.display());
    let runtime = AcpRuntime::catalog(Some(command), 5_000);
    runtime.start().await.unwrap();
    let session = runtime
        .start_session(StartSessionInput {
            cwd: dir.to_string_lossy().into_owned(),
            agent_id: Some("custom".into()),
            model: "default".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            sandbox_mode: Some("danger-full-access".into()),
        })
        .await
        .expect("start custom ACP session");
    (runtime, session.provider_session_id)
}

fn turn_input(session_id: &str, prompt: &str, turn_id: &str) -> StartTurnInput {
    StartTurnInput {
        provider_session_id: session_id.into(),
        prompt: prompt.into(),
        model: None,
        reasoning_effort: None,
        sandbox_mode: None,
        collaboration_mode: None,
        approval_mode: None,
        performance_mode: None,
        thread_id: "thread-1".into(),
        turn_id: turn_id.into(),
        hidden: false,
        images: Vec::new(),
        title: None,
        context_delivered: false,
    }
}

fn completed_event(
    events: &mut tokio::sync::broadcast::Receiver<pockymoe_protocol::ThreadEventEnvelope>,
) -> pockymoe_protocol::ThreadEventEnvelope {
    let mut completed = None;
    while let Ok(event) = events.try_recv() {
        if event.event_type == "thread.turn.completed" {
            completed = Some(event);
        }
    }
    completed.expect("turn should emit a completion event")
}

fn which_python() -> String {
    for candidate in ["python3", "python"] {
        if std::process::Command::new(candidate)
            .arg("-c")
            .arg("import sys; sys.exit(0)")
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            return candidate.to_string();
        }
    }
    panic!("python3 is required for ACP turn tests");
}

#[tokio::test]
async fn device_mcp_servers_reach_new_sessions_of_matching_agents() {
    let dir = tempdir().unwrap();
    pockymoe_runtime::acp::mcp_servers::configure(&dir.path().join("mcp.sqlite"));
    for (name, agents) in [
        ("files", vec!["custom".to_string()]),
        ("other", vec!["deepseek".to_string()]),
    ] {
        pockymoe_runtime::acp::mcp_servers::upsert(pockymoe_runtime::acp::mcp_servers::McpServer {
            name: name.into(),
            transport: "stdio".into(),
            command: "files-mcp".into(),
            agents,
            enabled: true,
            ..Default::default()
        })
        .unwrap();
    }
    // The fake agent advertises no HTTP MCP support, so this one is withheld.
    pockymoe_runtime::acp::mcp_servers::upsert(pockymoe_runtime::acp::mcp_servers::McpServer {
        name: "remote".into(),
        transport: "http".into(),
        url: "https://mcp.example.com/mcp".into(),
        enabled: true,
        ..Default::default()
    })
    .unwrap();
    let (runtime, session) = start_runtime(dir.path(), &which_python()).await;
    let items = runtime
        .start_turn(
            turn_input(&session, "read-mcp-servers", "mcp"),
            EventBus::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let text = items.iter().map(|i| i.text.as_str()).collect::<String>();
    let servers: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(
        servers,
        serde_json::json!([{"name":"files","command":"files-mcp","args":[],"env":[]}])
    );
}
