use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use anyhow::{ensure, Result};
use async_trait::async_trait;
use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, PluginDto, Provider};
use pockymoe_runtime::{
    fake::FakeRuntime,
    plugins::{BuiltinPlugin, PluginActionContext, PluginSettings},
    Database, RuntimeConfig, Supervisor,
};
use serde_json::{json, Value};

struct Fixture(Arc<AtomicUsize>);
#[async_trait]
impl BuiltinPlugin for Fixture {
    fn id(&self) -> &'static str {
        "fixture.feature"
    }
    fn descriptor(&self, s: &Supervisor) -> PluginDto {
        let mut dto = s.get_plugin("remote-codex.terminal").unwrap();
        dto.name = "Feature fixture".into();
        dto.available = Some(true);
        dto.capabilities = json!({"artifactTypes":[],"timelineRenderers":[],"threadPanels":[]});
        dto
    }
    fn default_settings(&self) -> Value {
        json!({"suffix":"default"})
    }
    fn validate_settings(&self, values: &Value) -> Result<()> {
        ensure!(
            values.as_object().is_some_and(|m| m.len() == 1)
                && values["suffix"].as_str().is_some_and(|s| s.len() < 100),
            "invalid fixture settings"
        );
        Ok(())
    }
    fn actions(&self) -> &'static [&'static str] {
        &["echo", "panic", "wait"]
    }
    fn turn_automation(
        &self,
        thread: &str,
        settings: &Value,
    ) -> Result<Option<pockymoe_protocol::AutomationDefinition>> {
        if settings["suffix"] == "hook-panic" {
            panic!("fixture hook panic");
        }
        if settings["suffix"] == "off" {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(
            json!({"name":"Automatic fixture","trigger":{"kind":"threadEnded","sourceThreadId":thread},"condition":{"kind":"all","conditions":[{"kind":"statusIn","values":["completed"]},{"kind":"replyLengthAtLeast","chars":12}]},"action":{"kind":"pluginAction","pluginId":self.id(),"action":"echo","input":{}}}),
        )?))
    }
    async fn invoke(&self, action: &str, context: PluginActionContext) -> Result<Value> {
        self.0.fetch_add(1, Ordering::SeqCst);
        match action {
            "panic" => panic!("fixture panic"),
            "wait" => {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
            _ => {}
        }
        Ok(
            json!({"suffix":context.settings.values["suffix"],"input":context.input,"thread":context.thread_id,"event":context.event}),
        )
    }
}

fn setup(dir: &std::path::Path, calls: Arc<AtomicUsize>) -> Arc<Supervisor> {
    let config = RuntimeConfig {
        treer_enabled: false,
        host_mode: Default::default(),
        mode: pockymoe_protocol::Mode::Local,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root: dir.into(),
        database_url: dir.join("test.sqlite"),
        app_name: "test".into(),
        app_version: "test".into(),
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
    Arc::new(
        Supervisor::new(
            config.clone(),
            Database::open(&config.database_url).unwrap(),
            vec![Arc::new(FakeRuntime::new(Provider::Codex))],
        )
        .with_builtin_plugins(vec![Arc::new(Fixture(calls))])
        .unwrap(),
    )
}

async fn thread(s: &Supervisor, dir: &std::path::Path) -> String {
    let workspace = s
        .create_workspace(CreateWorkspaceInput {
            abs_path: Some(dir.to_string_lossy().into()),
            git_url: None,
            label: None,
        })
        .unwrap();
    s.create_thread(CreateThreadInput {
        workspace_id: workspace.id,
        title: None,
        provider: Some(Provider::Codex),
        agent_id: None,
        model: "ios-e2e-stream".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: None,
    })
    .await
    .unwrap()
    .id
}

async fn settled(s: &Arc<Supervisor>, thread: &str, id: &str) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let runs = s.automation_runs(thread, id, 10).unwrap();
            if let Some(run) = runs["runs"].as_array().unwrap().first() {
                if run["state"] != "due" && run["state"] != "running" {
                    return run.clone();
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn plugin_settings_are_namespaced_validated_versioned_and_persistent() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let s = setup(dir.path(), calls.clone());
    assert_eq!(s.plugin_settings("fixture.feature").unwrap().revision, 0);
    assert!(s.plugin_settings("missing").is_err());
    assert!(s
        .save_plugin_settings(
            "fixture.feature",
            PluginSettings {
                revision: 0,
                values: json!({"suffix":3})
            }
        )
        .is_err());
    let saved = s
        .save_plugin_settings(
            "fixture.feature",
            PluginSettings {
                revision: 0,
                values: json!({"suffix":"saved"}),
            },
        )
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert!(s
        .save_plugin_settings(
            "fixture.feature",
            PluginSettings {
                revision: 0,
                values: json!({"suffix":"stale"})
            }
        )
        .unwrap_err()
        .to_string()
        .starts_with("conflict:"));
    assert_eq!(
        s.plugin_settings("remote-codex.terminal").unwrap().values,
        json!({})
    );
    drop(s);
    let reopened = setup(dir.path(), calls);
    assert_eq!(
        reopened.plugin_settings("fixture.feature").unwrap().values,
        json!({"suffix":"saved"})
    );
}

#[tokio::test]
async fn plugin_action_is_durable_idempotent_and_captures_settings_without_exposing_them() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let s = setup(dir.path(), calls.clone());
    let t = thread(&s, dir.path()).await;
    let submit = || {
        s.submit_plugin_action(
            &t,
            "fixture.feature",
            "echo",
            json!({"hello":"world"}),
            120,
            "request-1",
        )
        .unwrap()
    };
    let action = submit();
    assert_eq!(submit()["id"], action["id"]);
    assert!(s
        .submit_plugin_action(
            &t,
            "fixture.feature",
            "echo",
            json!({"changed":true}),
            120,
            "request-1"
        )
        .is_err());
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    // Dispatch already captured revision zero; an edit must not alter that invocation.
    s.save_plugin_settings(
        "fixture.feature",
        PluginSettings {
            revision: 0,
            values: json!({"suffix":"later"}),
        },
    )
    .unwrap();
    let run = settled(&s, &t, action["id"].as_str().unwrap()).await;
    assert_eq!(run["state"], "completed");
    assert_eq!(run["deliveryReceipt"]["output"]["suffix"], "default");
    assert!(run["deliveryReceipt"].get("settings").is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn plugin_disable_cancels_pending_and_rejects_unknown_actions() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let s = setup(dir.path(), calls.clone());
    let t = thread(&s, dir.path()).await;
    assert!(s
        .submit_plugin_action(&t, "fixture.feature", "missing", json!({}), 120, "unknown")
        .is_err());
    let action = s
        .submit_plugin_action(&t, "fixture.feature", "echo", json!({}), 120, "pending")
        .unwrap();
    s.set_plugin_enabled("fixture.feature", false).unwrap();
    assert!(s
        .submit_plugin_action(&t, "fixture.feature", "echo", json!({}), 120, "disabled")
        .is_err());
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    let run = settled(&s, &t, action["id"].as_str().unwrap()).await;
    assert_eq!(run["state"], "cancelled");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn plugin_panic_and_timeout_settle_without_blocking_other_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let s = setup(dir.path(), Arc::new(AtomicUsize::new(0)));
    let t = thread(&s, dir.path()).await;
    for action in ["panic", "wait"] {
        let run = s
            .submit_plugin_action(&t, "fixture.feature", action, json!({}), 1, action)
            .unwrap();
        s.automation_tick(&pockymoe_protocol::now_rfc3339())
            .await
            .unwrap();
        let result = settled(&s, &t, run["id"].as_str().unwrap()).await;
        assert_eq!(result["state"], "failed");
        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains(if action == "panic" {
                "panicked"
            } else {
                "timed out"
            }));
    }
}

#[tokio::test]
async fn plugin_restart_marks_inflight_uncertain_and_never_reexecutes() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let s = setup(dir.path(), calls.clone());
    let t = thread(&s, dir.path()).await;
    let action = s
        .submit_plugin_action(&t, "fixture.feature", "echo", json!({}), 120, "restart")
        .unwrap();
    let id = action["id"].as_str().unwrap().to_owned();
    s.db.with(|c| { c.execute("INSERT INTO automation_runs(id,automation_id,occurrence_key,state,scheduled_at,observed_at,definition_json,event_json) VALUES('crashed',?1,'crashed','running','2030-01-01T00:00:00Z','2030-01-01T00:00:00Z',?2,'{}')", rusqlite::params![id,action["definition"].to_string()])?; c.execute("UPDATE automations SET next_run_at=NULL WHERE id=?1",[&id])?; Ok(()) }).unwrap();
    drop(s);
    let reopened = setup(dir.path(), calls.clone());
    assert_eq!(
        reopened.automation_runs(&t, &id, 10).unwrap()["runs"][0]["state"],
        "uncertain"
    );
    reopened
        .automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

fn complete(s: &Arc<Supervisor>, thread: &str, turn: &str, reply: &str, ordinal: i64) {
    s.spawn_live_item_persister();
    s.db.with(|c| {
        c.execute(
            "INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES(?1,?2,'inProgress',?3)",
            rusqlite::params![turn, thread, ordinal],
        )?;
        Ok(())
    })
    .unwrap();
    s.bus.emit(pockymoe_protocol::ThreadEventEnvelope {event_type:"thread.item.completed".into(),thread_id:thread.into(),timestamp:pockymoe_protocol::now_rfc3339(),payload:json!({"turnId":turn,"item":{"id":format!("reply-{turn}"),"kind":"agentMessage","text":reply,"responsePhase":"final"}})});
    s.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
        event_type: "thread.turn.completed".into(),
        thread_id: thread.into(),
        timestamp: pockymoe_protocol::now_rfc3339(),
        payload: json!({"turnId":turn,"status":"completed"}),
    });
}

#[tokio::test]
async fn plugin_automatic_hook_counts_prose_and_dispatches_current_turn_once() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let s = setup(dir.path(), calls.clone());
    let t = thread(&s, dir.path()).await;
    complete(&s, &t, "short", "很短", 0);
    complete(
        &s,
        &t,
        "code",
        &format!("代码：\n```rust\n{}\n```", "x".repeat(1000)),
        1,
    );
    complete(
        &s,
        &t,
        "long",
        "修改已完成，但 Windows 验证失败，部署尚未验证。",
        2,
    );
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    let id: String = s.db.with(|c| Ok(c.query_row("SELECT id FROM automations WHERE thread_id=?1 AND request_id='builtin-plugin:fixture.feature'",[&t],|r|r.get(0))?)).unwrap();
    let run = settled(&s, &t, &id).await;
    assert_eq!(run["state"], "completed");
    assert_eq!(run["deliveryReceipt"]["output"]["event"]["turnId"], "long");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    s.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
        event_type: "thread.turn.completed".into(),
        thread_id: t.clone(),
        timestamp: pockymoe_protocol::now_rfc3339(),
        payload: json!({"turnId":"long","status":"completed"}),
    });
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        s.automation_runs(&t, &id, 10).unwrap()["runs"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn plugin_auto_settings_cancel_pending_and_resume_future_turns() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let s = setup(dir.path(), calls.clone());
    let t = thread(&s, dir.path()).await;
    complete(
        &s,
        &t,
        "before",
        "A long reply which should be cancelled",
        0,
    );
    let saved = s
        .save_plugin_settings(
            "fixture.feature",
            PluginSettings {
                revision: 0,
                values: json!({"suffix":"off"}),
            },
        )
        .unwrap();
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    s.save_plugin_settings(
        "fixture.feature",
        PluginSettings {
            revision: saved.revision,
            values: json!({"suffix":"on"}),
        },
    )
    .unwrap();
    complete(
        &s,
        &t,
        "after",
        "A new long reply to polish after enabling",
        1,
    );
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    let id: String = s.db.with(|c| Ok(c.query_row("SELECT id FROM automations WHERE thread_id=?1 AND request_id='builtin-plugin:fixture.feature'",[&t],|r|r.get(0))?)).unwrap();
    let run = settled(&s, &t, &id).await;
    assert_eq!(run["state"], "completed");
    assert_eq!(run["deliveryReceipt"]["output"]["event"]["turnId"], "after");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn plugin_hook_panic_does_not_undo_completed_turn() {
    let dir = tempfile::tempdir().unwrap();
    let s = setup(dir.path(), Arc::new(AtomicUsize::new(0)));
    let t = thread(&s, dir.path()).await;
    s.save_plugin_settings(
        "fixture.feature",
        PluginSettings {
            revision: 0,
            values: json!({"suffix":"hook-panic"}),
        },
    )
    .unwrap();
    complete(&s, &t, "finished", "The original work finished", 0);
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    let status: String =
        s.db.with(|c| {
            Ok(c.query_row(
                "SELECT status FROM thread_turns WHERE id='finished' AND thread_id=?1",
                [&t],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(status, "completed");
}
