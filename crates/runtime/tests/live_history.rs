use std::sync::Arc;

use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Provider, ThreadEventEnvelope};
use pockymoe_runtime::actor::SharedRuntime;
use pockymoe_runtime::config::RuntimeConfig;
use pockymoe_runtime::db::Database;
use pockymoe_runtime::fake::FakeRuntime;
use pockymoe_runtime::Supervisor;
use rusqlite::params;
use serde_json::{json, Value};
use tempfile::{tempdir, TempDir};

async fn running_thread() -> (TempDir, Arc<Supervisor>, String) {
    let dir = tempdir().unwrap();
    let config = RuntimeConfig {
        mode: pockymoe_protocol::Mode::Local,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root: dir.path().join("workspaces"),
        database_url: dir.path().join("test.sqlite"),
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
    };
    let db = Database::open(&config.database_url).unwrap();
    let runtime: SharedRuntime = Arc::new(FakeRuntime::new(Provider::Codex));
    let supervisor = Arc::new(Supervisor::new(config, db, vec![runtime]));
    let workspace = supervisor
        .create_workspace(CreateWorkspaceInput {
            abs_path: Some(dir.path().to_string_lossy().into()),
            git_url: None,
            label: Some("test".into()),
        })
        .unwrap();
    let thread = supervisor
        .create_thread(CreateThreadInput {
            workspace_id: workspace.id,
            title: Some("live history".into()),
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: None,
        })
        .await
        .unwrap();
    supervisor
        .db
        .with(|conn| {
            conn.execute(
                "INSERT INTO thread_turns(id, thread_id, status, started_at, ordinal)
             VALUES ('live-turn', ?1, 'inProgress', '2026-09-05T10:00:00Z', 1)",
                params![thread.id],
            )?;
            conn.execute(
                "UPDATE threads SET status='running' WHERE id=?1",
                params![thread.id],
            )?;
            Ok(())
        })
        .unwrap();
    (dir, supervisor, thread.id)
}

fn emit(supervisor: &Supervisor, thread_id: &str, event_type: &str, payload: Value) {
    supervisor.bus.emit(ThreadEventEnvelope {
        event_type: event_type.into(),
        thread_id: thread_id.into(),
        timestamp: "2026-09-05T10:00:01Z".into(),
        payload,
    });
}

#[tokio::test]
async fn tool_details_are_durable_but_not_broadcast() {
    let (_dir, supervisor, thread_id) = running_thread().await;
    supervisor.spawn_live_item_persister();
    let mut events = supervisor.bus.subscribe();
    for kind in ["commandExecution", "fileChange"] {
        emit(
            &supervisor,
            &thread_id,
            "thread.item.completed",
            json!({"turnId":"live-turn","item":{"id":kind,"kind":kind,"text":"summary","detailText":"full private tool output","status":"completed"}}),
        );
        let event = events.recv().await.unwrap();
        assert!(event.payload["item"].get("detailText").is_none());
        assert_eq!(event.payload["item"]["hasDeferredDetail"], true);
        assert_eq!(
            supervisor
                .get_history_item_detail(&thread_id, kind)
                .unwrap()["text"],
            "full private tool output"
        );
    }
}

#[test]
fn old_acp_file_records_gain_paths_counts_and_diff_without_rewriting_storage() {
    let mut item = serde_json::from_value(json!({"id":"legacy-edit","kind":"fileChange","text":"Editing files","detailText":"Tool: apply_patch\n\nResult:\nFile: src/app.ts\n\nBefore:\nconst value = 1;\n\n\nAfter:\nconst value = 2;\n"})).unwrap();
    pockymoe_runtime::history::normalize_legacy_file_change(&mut item);
    assert_eq!(item.text, "src/app.ts");
    assert_eq!(item.extra["addedLines"], 1);
    assert_eq!(item.extra["removedLines"], 1);
    assert!(item
        .detail_text
        .as_ref()
        .unwrap()
        .contains("+const value = 2;"));
    let once = serde_json::to_value(&item).unwrap();
    pockymoe_runtime::history::normalize_legacy_file_change(&mut item);
    assert_eq!(serde_json::to_value(&item).unwrap(), once);
}

#[tokio::test]
async fn broadcast_output_is_already_persisted_and_contains_the_complete_prefix() {
    let (_dir, supervisor, thread_id) = running_thread().await;
    supervisor.spawn_live_item_persister();
    supervisor.spawn_live_item_persister();
    let mut events = supervisor.bus.subscribe();
    for (delta, expected) in [("Hello", "Hello"), (" 世界", "Hello 世界")] {
        emit(
            &supervisor,
            &thread_id,
            "thread.output.delta",
            json!({
                "turnId": "live-turn", "itemId": "assistant-1", "sequence": 1,
                "delta": delta,
            }),
        );
        // No yielding or retries: receiving an event guarantees a refresh can read it.
        let event = events.try_recv().unwrap();
        assert_eq!(event.payload["delta"], delta);
        assert_eq!(event.payload["text"], expected);
        let restored = supervisor
            .get_thread_turn_detail(&thread_id, "live-turn")
            .await
            .unwrap();
        assert_eq!(restored.items[0].text, expected);
    }
    emit(
        &supervisor,
        &thread_id,
        "thread.item.completed",
        json!({
            "turnId": "live-turn", "item": {
                "id": "assistant-1", "kind": "agentMessage", "text": "Hello 世界",
                "status": "failed", "sequence": 1,
            },
        }),
    );
    let restored = supervisor
        .get_thread_turn_detail(&thread_id, "live-turn")
        .await
        .unwrap();
    assert_eq!(restored.items[0].text, "Hello 世界");
    assert_eq!(restored.items[0].status.as_deref(), Some("failed"));

    let weak = Arc::downgrade(&supervisor);
    let bus = supervisor.bus.clone();
    drop(supervisor);
    assert!(
        weak.upgrade().is_none(),
        "the persistence hook must not keep the supervisor alive"
    );
    drop(bus);
}
