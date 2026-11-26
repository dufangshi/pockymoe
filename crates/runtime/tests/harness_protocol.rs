//! Per-thread harness protocol: Codex and Claude run natively unless the thread
//! was created under the ACP agent (`acp:codex`); the choice is pinned on the
//! thread and kept across reloads, forks and protocol switches.
use std::sync::Arc;

use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Provider, SendThreadPromptInput};
use pockymoe_runtime::config::RuntimeConfig;
use pockymoe_runtime::db::Database;
use pockymoe_runtime::fake::FakeRuntime;
use pockymoe_runtime::service::HarnessSwitchInput;
use pockymoe_runtime::Supervisor;
use tempfile::tempdir;

fn config(dir: &std::path::Path) -> RuntimeConfig {
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

fn create(workspace: &str, agent: Option<&str>) -> CreateThreadInput {
    CreateThreadInput {
        workspace_id: workspace.into(),
        title: None,
        provider: Some(Provider::Codex),
        agent_id: agent.map(str::to_string),
        model: "fake".into(),
        reasoning_effort: None,
        approval_mode: "guarded".into(),
        parent_thread_id: None,
    }
}

#[tokio::test]
async fn thread_protocol_is_pinned_at_creation_and_kept_by_forks_and_switches() {
    let dir = tempdir().unwrap();
    let config = config(dir.path());
    let db = Database::open(&config.database_url).unwrap();
    let fake = Arc::new(FakeRuntime::new(Provider::Codex));
    let state = Arc::new(Supervisor::new(config, db, vec![fake.clone()]));
    let path = dir.path().join("project");
    std::fs::create_dir(&path).unwrap();
    let ws = state
        .create_workspace(CreateWorkspaceInput {
            abs_path: Some(path.to_string_lossy().into()),
            git_url: None,
            label: None,
        })
        .unwrap()
        .id;

    // `acp:codex` is Codex through its ACP adapter: the thread is plain Codex
    // with protocol acp, and the runtime is asked for the adapter.
    let acp = state
        .create_thread(create(&ws, Some("acp:codex")))
        .await
        .unwrap();
    assert_eq!(acp.agent_id.as_deref(), Some("codex"));
    assert_eq!(acp.harness_protocol.as_deref(), Some("acp"));
    assert_eq!(acp.agent_key().as_deref(), Some("acp:codex"));
    let native = state
        .create_thread(create(&ws, Some("codex")))
        .await
        .unwrap();
    assert_eq!(native.harness_protocol.as_deref(), Some("native"));
    let starts: Vec<_> = fake
        .started_sessions()
        .into_iter()
        .map(|s| s.agent_id)
        .collect();
    assert_eq!(starts, [Some("acp:codex".into()), Some("codex".into())]);
    // The API names the protocol beside the agent id.
    let json = serde_json::to_value(&acp).unwrap();
    assert_eq!(json["agentId"], "codex");
    assert_eq!(json["harnessProtocol"], "acp");

    // A thread from before the choice (NULL) reads as native, the default.
    state
        .db
        .with(|c| {
            c.execute(
                "UPDATE threads SET harness_protocol=NULL WHERE id=?1",
                [&native.id],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        state
            .get_thread(&native.id)
            .unwrap()
            .harness_protocol
            .as_deref(),
        Some("native")
    );

    // A fork stays on its source's protocol.
    state
        .prompt(
            &acp.id,
            SendThreadPromptInput {
                prompt: "hello".into(),
                client_request_id: None,
                model: None,
                reasoning_effort: None,
                collaboration_mode: None,
                images: vec![],
            },
        )
        .await
        .unwrap();
    let target = |agent: &str| HarnessSwitchInput {
        provider: Provider::Codex,
        agent_id: agent.into(),
        model: None,
        reasoning_effort: None,
    };
    let fork = state
        .fork_thread_to_harness(&acp.id, None, target("acp:codex"))
        .await
        .unwrap();
    assert_eq!(fork.harness_protocol.as_deref(), Some("acp"));
    let fork = state
        .fork_thread_to_harness(&acp.id, None, target("codex"))
        .await
        .unwrap();
    assert_eq!(fork.harness_protocol.as_deref(), Some("native"));

    // ACP Codex → native Codex keeps the session (both read the same store),
    // sends no handoff and records the change in the transcript.
    let session = state.get_thread(&acp.id).unwrap().provider_session_id;
    let preview = state
        .preview_harness_switch(&acp.id, &target("codex"))
        .unwrap();
    assert!(preview.resumes_session && preview.turns == 0 && preview.text.is_empty());
    let switched = state
        .switch_thread_harness(&acp.id, target("codex"))
        .await
        .unwrap();
    assert_eq!(switched.harness_protocol.as_deref(), Some("native"));
    assert_eq!(switched.provider_session_id, session);
    assert_eq!(switched.model.as_deref(), Some("fake"));
    assert!(state
        .db
        .get_kv(&format!("handoff:pending:{}", acp.id))
        .unwrap()
        .is_none());
    let back = state
        .switch_thread_harness(&acp.id, target("acp:codex"))
        .await
        .unwrap();
    assert_eq!(back.harness_protocol.as_deref(), Some("acp"));
    assert_eq!(back.provider_session_id, session);
    let detail = state.get_thread_detail(&acp.id, None).await.unwrap();
    let switches: Vec<_> = serde_json::to_value(&detail).unwrap()["harnessSwitches"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let protocols: Vec<_> = switches
        .iter()
        .map(|s| s["harnessSwitch"]["harnessProtocol"].clone())
        .collect();
    assert_eq!(
        protocols,
        [serde_json::json!("native"), serde_json::json!("acp")],
        "{switches:?}"
    );
}
