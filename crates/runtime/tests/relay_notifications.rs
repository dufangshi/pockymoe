use std::sync::Arc;

use remote_codex_protocol::{
    CreateThreadInput, CreateWorkspaceInput, Provider, ThreadEventEnvelope,
};
use remote_codex_runtime::{fake::FakeRuntime, Database, RuntimeConfig, Supervisor};
use serde_json::{json, Value};

#[tokio::test]
async fn awaiting_input_requests_persist_ack_and_resolve_independently_of_terminal_notices() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = RuntimeConfig::from_env();
    config.database_url = dir.path().join("isolated.sqlite");
    config.workspace_root = dir.path().join("workspaces");
    config.fake_runtime = true;
    config.relay_server_url = None;
    config.relay_agent_token = None;
    let state = Arc::new(Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![Arc::new(FakeRuntime::new(Provider::Codex))],
    ));
    state.spawn_live_item_persister();
    let workspace = state
        .create_workspace(CreateWorkspaceInput {
            abs_path: Some(dir.path().to_string_lossy().into()),
            git_url: None,
            label: None,
        })
        .unwrap();
    let thread = state
        .create_thread(CreateThreadInput {
            workspace_id: workspace.id,
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "gpt-6.1-sol".into(),
            reasoning_effort: None,
            approval_mode: "guarded".into(),
            parent_thread_id: None,
            title: Some("Notification fixture".into()),
        })
        .await
        .unwrap();
    let turn = uuid::Uuid::new_v4().to_string();
    state.db.with(|conn| {
        conn.execute("INSERT INTO thread_turns(id,thread_id,status,started_at,ordinal) VALUES (?1,?2,'inProgress',?3,1)",rusqlite::params![turn,thread.id,remote_codex_protocol::now_rfc3339()])?;
        Ok(())
    }).unwrap();
    let emit = |kind: &str, payload: Value| {
        state.bus.emit(ThreadEventEnvelope {
            event_type: kind.into(),
            thread_id: thread.id.clone(),
            timestamp: remote_codex_protocol::now_rfc3339(),
            payload,
        })
    };
    for (request, kind) in [
        ("perm-first", "permissionRequest"),
        ("input-second", "requestUserInput"),
    ] {
        let payload =
            json!({"request":{"id":request,"turnId":turn,"kind":kind,"title":"Sensitive content"}});
        emit("thread.request.created", payload.clone());
        emit("thread.request.created", payload);
        let notices = state.pending_relay_notifications().unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0]["requestId"], request);
        assert!(!notices[0].to_string().contains("Sensitive content"));
        // No relay ACK (e.g. old relay rejecting awaiting_input) cannot block
        // resolution or encrypted runtime event processing.
        emit("thread.request.resolved", json!({"requestId":request}));
        assert!(state.pending_relay_notifications().unwrap().is_empty());
    }
    emit(
        "thread.request.created",
        json!({"request":{"id":"input-durable","turnId":turn,"kind":"requestUserInput"}}),
    );
    drop(state);
    let state = Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![Arc::new(FakeRuntime::new(Provider::Codex))],
    );
    assert!(state
        .pending_relay_notifications()
        .unwrap()
        .iter()
        .any(|n| n["requestId"] == "input-durable"));
    state
        .acknowledge_relay_notification("input-durable")
        .unwrap();
    assert!(!state
        .pending_relay_notifications()
        .unwrap()
        .iter()
        .any(|n| n["requestId"] == "input-durable"));
}
