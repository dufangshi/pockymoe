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
        let chars = if settings["suffix"] == "high-threshold" {
            1000
        } else {
            12
        };
        Ok(Some(serde_json::from_value(
            json!({"name":"Automatic fixture","trigger":{"kind":"threadEnded","sourceThreadId":thread},"condition":{"kind":"all","conditions":[{"kind":"statusIn","values":["completed"]},{"kind":"replyLengthAtLeast","chars":chars}]},"action":{"kind":"pluginAction","pluginId":self.id(),"action":"echo","input":{}}}),
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
        .with_builtin_plugins(vec![Arc::new(Fixture(calls)), Arc::new(BoundaryFixture)])
        .unwrap(),
    )
}

async fn thread(s: &Supervisor, dir: &std::path::Path) -> String {
    let workspace = s
        .list_workspaces()
        .unwrap()
        .into_iter()
        .find(|w| w.abs_path == dir.to_string_lossy())
        .unwrap_or_else(|| {
            s.create_workspace(CreateWorkspaceInput {
                abs_path: Some(dir.to_string_lossy().into()),
                git_url: None,
                label: None,
            })
            .unwrap()
        });
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

fn overlay(
    revision: u64,
    enabled: Option<bool>,
    suffix: Option<&str>,
) -> pockymoe_runtime::plugins::ThreadPluginOverride {
    pockymoe_runtime::plugins::ThreadPluginOverride {
        revision,
        enabled,
        values: suffix.map(|suffix| json!({"suffix":suffix})),
    }
}

#[tokio::test]
async fn thread_plugin_overrides_are_removed_with_the_thread_only() {
    let dir = tempfile::tempdir().unwrap();
    let s = setup(dir.path(), Arc::new(AtomicUsize::new(0)));
    let deleted = thread(&s, dir.path()).await;
    let retained = thread(&s, dir.path()).await;
    for id in [&deleted, &retained] {
        s.save_thread_plugin_settings(
            id,
            "fixture.feature",
            overlay(0, Some(false), Some("custom")),
        )
        .unwrap();
    }
    s.delete_thread(&deleted).unwrap();
    assert!(s
        .db
        .get_kv(&format!("thread:{deleted}:plugin:fixture.feature:settings"))
        .unwrap()
        .is_none());
    assert_eq!(
        s.thread_plugin_settings(&retained, "fixture.feature")
            .unwrap()
            .values
            .unwrap()["suffix"],
        "custom"
    );
    assert_eq!(
        s.plugin_settings("fixture.feature").unwrap().values["suffix"],
        "default"
    );
}

#[tokio::test]
async fn thread_plugin_scope_inheritance_master_switch_cas_and_reset_are_persistent() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let s = setup(dir.path(), calls.clone());
    let a = thread(&s, dir.path()).await;
    let b = thread(&s, dir.path()).await;
    let id = "fixture.feature";
    let initial = s.thread_plugin_settings(&a, id).unwrap();
    assert_eq!((initial.revision, initial.global_revision), (0, 0));
    assert!(initial.enabled.is_none() && initial.values.is_none());
    assert!(initial.effective.enabled);
    let saved = s
        .save_thread_plugin_settings(&a, id, overlay(0, Some(false), Some("A")))
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert!(!saved.effective.enabled);
    assert_eq!(
        s.thread_plugin_settings(&b, id).unwrap().effective.values["suffix"],
        "default"
    );
    assert!(s
        .save_thread_plugin_settings(&a, id, overlay(0, None, None))
        .unwrap_err()
        .to_string()
        .starts_with("conflict:"));
    s.save_plugin_settings(
        id,
        PluginSettings {
            revision: 0,
            values: json!({"suffix":"global"}),
        },
    )
    .unwrap();
    let a_settings = s.thread_plugin_settings(&a, id).unwrap();
    assert_eq!(a_settings.global_revision, 1);
    assert_eq!(a_settings.effective.values["suffix"], "A");
    assert_eq!(
        s.thread_plugin_settings(&b, id).unwrap().effective.values["suffix"],
        "global"
    );
    s.set_plugin_enabled(id, false).unwrap();
    let on = s
        .save_thread_plugin_settings(&a, id, overlay(1, Some(true), Some("A")))
        .unwrap();
    assert!(!on.effective.enabled);
    assert!(s
        .submit_plugin_action(&a, id, "echo", json!({}), 120, "master-off")
        .is_err());
    s.set_plugin_enabled(id, true).unwrap();
    assert!(s.thread_plugin_settings(&a, id).unwrap().effective.enabled);
    let row = s
        .list_thread_plugins(&a)
        .unwrap()
        .into_iter()
        .find(|p| p.plugin.id == id)
        .unwrap();
    let row = serde_json::to_value(row).unwrap();
    assert_eq!(row["enabled"], true);
    assert_eq!(row["globalEnabled"], true);
    assert_eq!(row["enabledOverride"], true);
    assert_eq!(row["settingsOverridden"], true);
    assert_eq!(row["capabilities"]["scopes"], json!(["device", "thread"]));
    assert!(row.get("plugin").is_none());
    let reset = s
        .save_thread_plugin_settings(&a, id, overlay(2, None, None))
        .unwrap();
    assert_eq!(reset.revision, 3);
    assert_eq!(reset.effective.values["suffix"], "global");
    assert!(s
        .save_thread_plugin_settings(&a, id, overlay(0, None, Some("stale")))
        .is_err());
    assert!(s.thread_plugin_settings("missing", id).is_err());
    assert!(s.list_thread_plugins("missing").is_err());
    assert!(s
        .save_thread_plugin_settings("missing", id, overlay(0, None, None))
        .is_err());
    assert!(s.thread_plugin_settings(&a, "missing").is_err());
    s.save_thread_plugin_settings(&b, id, overlay(0, Some(false), None))
        .unwrap();
    s.set_plugin_enabled(id, false).unwrap();
    drop(s);
    let s = setup(dir.path(), calls);
    let reset = s.thread_plugin_settings(&a, id).unwrap();
    assert_eq!(reset.revision, 3);
    assert!(reset.enabled.is_none() && reset.values.is_none());
    assert!(!reset.effective.enabled);
    s.set_plugin_enabled(id, true).unwrap();
    assert!(s.thread_plugin_settings(&a, id).unwrap().effective.enabled);
    assert!(!s.thread_plugin_settings(&b, id).unwrap().effective.enabled);
}

struct BoundaryFixture;
#[async_trait]
impl BuiltinPlugin for BoundaryFixture {
    fn id(&self) -> &'static str {
        "fixture.boundary"
    }
    fn descriptor(&self, s: &Supervisor) -> PluginDto {
        let mut dto = s.get_plugin("remote-codex.terminal").unwrap();
        dto.available = Some(false);
        dto
    }
    fn default_settings(&self) -> Value {
        json!({"blob":""})
    }
    fn validate_settings(&self, values: &Value) -> Result<()> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Settings {
            blob: String,
        }
        let settings: Settings = serde_json::from_value(values.clone())?;
        ensure!(!settings.blob.contains('\0'), "invalid blob");
        Ok(())
    }
}

#[tokio::test]
async fn thread_plugin_settings_enforce_typed_values_size_credentials_and_availability() {
    let dir = tempfile::tempdir().unwrap();
    let s = setup(dir.path(), Arc::new(AtomicUsize::new(0)));
    let t = thread(&s, dir.path()).await;
    let id = "fixture.boundary";
    use pockymoe_runtime::plugins::ThreadPluginOverride;
    for values in [
        json!([]),
        json!({"blob":3}),
        json!({"blob":"", "extra":true}),
        json!({"apiKey":"do-not-save"}),
        json!({"blob":"a".repeat(65526)}),
    ] {
        assert!(s
            .save_thread_plugin_settings(
                &t,
                id,
                ThreadPluginOverride {
                    revision: 0,
                    enabled: None,
                    values: Some(values)
                }
            )
            .is_err());
        assert_eq!(s.thread_plugin_settings(&t, id).unwrap().revision, 0);
    }
    let exact = json!({"blob":"a".repeat(65525)});
    assert_eq!(serde_json::to_vec(&exact).unwrap().len(), 65536);
    let saved = s
        .save_thread_plugin_settings(
            &t,
            id,
            ThreadPluginOverride {
                revision: 0,
                enabled: Some(true),
                values: Some(exact),
            },
        )
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert!(!saved.effective.enabled);
    assert_eq!(saved.enabled, Some(true));
    assert!(s
        .save_thread_plugin_settings(
            &t,
            "fixture.feature",
            ThreadPluginOverride {
                revision: 0,
                enabled: None,
                values: Some(json!({"suffix":3}))
            }
        )
        .is_err());
}

#[tokio::test]
async fn thread_plugin_claim_captures_effective_values_and_running_work_survives_disable() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let s = setup(dir.path(), calls.clone());
    let a = thread(&s, dir.path()).await;
    let b = thread(&s, dir.path()).await;
    let id = "fixture.feature";
    s.save_thread_plugin_settings(&a, id, overlay(0, None, Some("A")))
        .unwrap();
    let ra = s
        .submit_plugin_action(&a, id, "echo", json!({}), 120, "A")
        .unwrap();
    let rb = s
        .submit_plugin_action(&b, id, "echo", json!({}), 120, "B")
        .unwrap();
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    let receipt: Value =
        s.db.with(|c| {
            let raw: String = c.query_row(
                "SELECT receipt_json FROM automation_runs WHERE automation_id=?1",
                [ra["id"].as_str().unwrap()],
                |r| r.get(0),
            )?;
            Ok(serde_json::from_str(&raw)?)
        })
        .unwrap();
    assert_eq!(receipt["settings"]["values"]["suffix"], "A");
    assert_eq!(receipt["settingsRevisions"], json!({"thread":1,"global":0}));
    s.save_thread_plugin_settings(&a, id, overlay(1, Some(false), Some("changed")))
        .unwrap();
    s.save_plugin_settings(
        id,
        PluginSettings {
            revision: 0,
            values: json!({"suffix":"B-global"}),
        },
    )
    .unwrap();
    s.set_plugin_enabled(id, false).unwrap();
    let run = settled(&s, &a, ra["id"].as_str().unwrap()).await;
    assert_eq!(run["state"], "completed");
    assert_eq!(run["deliveryReceipt"]["output"]["suffix"], "A");
    assert!(run["deliveryReceipt"].get("settings").is_none());
    s.set_plugin_enabled(id, true).unwrap();
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    let run = settled(&s, &b, rb["id"].as_str().unwrap()).await;
    // The device master switch cancels all due actions, including B.
    assert_eq!(run["state"], "cancelled");
    let rb = s
        .submit_plugin_action(&b, id, "echo", json!({}), 120, "B-next")
        .unwrap();
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    let run = settled(&s, &b, rb["id"].as_str().unwrap()).await;
    assert_eq!(run["state"], "completed");
    assert_eq!(run["deliveryReceipt"]["output"]["suffix"], "B-global");
    assert_eq!(run["deliveryReceipt"]["output"]["thread"], b);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(s
        .submit_plugin_action(&a, id, "echo", json!({}), 120, "A-off")
        .is_err());
    let definition = serde_json::from_value(json!({"name":"bypass", "trigger":{"kind":"at","at":pockymoe_protocol::now_rfc3339()},"action":{"kind":"pluginAction","pluginId":id,"action":"echo","input":{}}})).unwrap();
    assert!(s.automation_create(&a, definition, None).is_err());
}

#[tokio::test]
async fn thread_plugin_hooks_pause_resume_and_global_edits_preserve_other_thread_rules() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let s = setup(dir.path(), calls.clone());
    let a = thread(&s, dir.path()).await;
    let b = thread(&s, dir.path()).await;
    let id = "fixture.feature";
    s.save_thread_plugin_settings(&a, id, overlay(0, None, Some("A")))
        .unwrap();
    s.save_thread_plugin_settings(&b, id, overlay(0, None, Some("B")))
        .unwrap();
    let hook = |t: &str| -> (String, String, String) {
        s.db.with(|c| Ok(c.query_row("SELECT id,state,definition_json FROM automations WHERE thread_id=?1 AND request_id='builtin-plugin:fixture.feature'", [t], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?)).unwrap()
    };
    let b_before = hook(&b);
    complete(&s, &a, "cancel-this", "A long reply that must never run", 0);
    s.save_thread_plugin_settings(&a, id, overlay(1, Some(false), Some("A")))
        .unwrap();
    assert_eq!(hook(&a).1, "paused");
    assert_eq!(hook(&b), b_before);
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    s.save_thread_plugin_settings(&a, id, overlay(2, Some(true), Some("A-new")))
        .unwrap();
    assert_eq!(hook(&a).1, "enabled");
    s.save_plugin_settings(
        id,
        PluginSettings {
            revision: 0,
            values: json!({"suffix":"off"}),
        },
    )
    .unwrap();
    // Both full thread snapshots continue to provide automatic behavior.
    assert_eq!(hook(&a).1, "enabled");
    assert_eq!(hook(&b), b_before);
    complete(
        &s,
        &a,
        "after-enable",
        "A new long reply after the scope is enabled",
        1,
    );
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    let run = settled(&s, &a, &hook(&a).0).await;
    assert_eq!(run["state"], "completed");
    assert_eq!(run["deliveryReceipt"]["output"]["suffix"], "A-new");
    assert_eq!(
        run["deliveryReceipt"]["output"]["event"]["turnId"],
        "after-enable"
    );
    s.save_thread_plugin_settings(&a, id, overlay(3, None, None))
        .unwrap();
    assert_eq!(hook(&a).1, "paused");
    s.save_plugin_settings(
        id,
        PluginSettings {
            revision: 1,
            values: json!({"suffix":"global-on"}),
        },
    )
    .unwrap();
    assert_eq!(hook(&a).1, "enabled");
    assert_eq!(hook(&b), b_before);
    s.automation_control(&b, &b_before.0, "pause").unwrap();
    s.set_plugin_enabled(id, false).unwrap();
    assert_eq!(hook(&a).1, "paused");
    s.set_plugin_enabled(id, true).unwrap();
    assert_eq!(hook(&a).1, "enabled");
    assert_eq!(hook(&b).1, "paused");
}

#[tokio::test]
async fn thread_plugin_disable_cancels_unclaimed_manual_actions_without_affecting_other_threads() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let s = setup(dir.path(), calls.clone());
    let a = thread(&s, dir.path()).await;
    let b = thread(&s, dir.path()).await;
    let id = "fixture.feature";
    let ra = s
        .submit_plugin_action(&a, id, "echo", json!({}), 120, "not-yet-polled")
        .unwrap();
    let rb = s
        .submit_plugin_action(&b, id, "echo", json!({}), 120, "other-thread")
        .unwrap();
    // An unrelated paused prompt has an orphaned queue entry. Only the normal
    // worker may reconcile it, never a scoped plugin configuration write.
    s.db.with(|c| {
        let definition = json!({"name":"other prompt","trigger":{"kind":"at","at":pockymoe_protocol::now_rfc3339()},"action":{"kind":"prompt","text":"other thread"}}).to_string();
        c.execute("INSERT INTO automations(id,thread_id,definition_json,state,event_cursor,created_at,updated_at) VALUES('other-queue',?1,?2,'paused',0,'now','now')", rusqlite::params![b,definition])?;
        c.execute("INSERT INTO automation_runs(id,automation_id,occurrence_key,state,scheduled_at,observed_at,definition_json,event_json,pending_id) VALUES('other-run','other-queue','queued','queued','now','now',?1,'{}','removed-message')", [definition])?;
        Ok(())
    }).unwrap();
    // Neither action has been materialized by the periodic worker yet.
    s.save_thread_plugin_settings(&a, id, overlay(0, Some(false), None))
        .unwrap();
    let other_state: String =
        s.db.with(|c| {
            Ok(c.query_row(
                "SELECT state FROM automation_runs WHERE id='other-run'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(other_state, "queued");
    assert_eq!(
        s.automation_runs(&a, ra["id"].as_str().unwrap(), 10)
            .unwrap()["runs"][0]["state"],
        "cancelled"
    );
    assert!(s
        .automation_runs(&b, rb["id"].as_str().unwrap(), 10)
        .unwrap()["runs"]
        .as_array()
        .unwrap()
        .is_empty());
    s.save_thread_plugin_settings(&a, id, overlay(1, Some(true), None))
        .unwrap();
    s.automation_tick(&pockymoe_protocol::now_rfc3339())
        .await
        .unwrap();
    assert_eq!(
        settled(&s, &b, rb["id"].as_str().unwrap()).await["state"],
        "completed"
    );
    assert_eq!(
        s.automation_runs(&a, ra["id"].as_str().unwrap(), 10)
            .unwrap()["runs"][0]["state"],
        "cancelled"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn thread_plugin_hook_failure_rolls_back_overlay_and_managed_hook_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let s = setup(dir.path(), Arc::new(AtomicUsize::new(0)));
    let t = thread(&s, dir.path()).await;
    assert!(s
        .save_thread_plugin_settings(&t, "fixture.feature", overlay(0, None, Some("hook-panic")))
        .is_err());
    assert_eq!(
        s.thread_plugin_settings(&t, "fixture.feature")
            .unwrap()
            .revision,
        0
    );
    let count: i64 = s.db.with(|c| Ok(c.query_row("SELECT count(*) FROM automations WHERE thread_id=?1 AND request_id='builtin-plugin:fixture.feature'", [&t], |r| r.get(0))?)).unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn thread_plugin_threshold_updates_only_target_hooks_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let s = setup(dir.path(), Arc::new(AtomicUsize::new(0)));
    let a = thread(&s, dir.path()).await;
    let b = thread(&s, dir.path()).await;
    s.save_thread_plugin_settings(&a, "fixture.feature", overlay(0, None, Some("A")))
        .unwrap();
    s.save_thread_plugin_settings(&b, "fixture.feature", overlay(0, None, Some("B")))
        .unwrap();
    let threshold = |thread: &str| -> u64 {
        s.automation_list(thread).unwrap()["automations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|hook| hook["definition"]["action"]["pluginId"] == "fixture.feature")
            .unwrap()["definition"]["condition"]["conditions"][1]["chars"]
            .as_u64()
            .unwrap()
    };
    assert_eq!((threshold(&a), threshold(&b)), (12, 12));
    s.save_thread_plugin_settings(
        &a,
        "fixture.feature",
        overlay(1, None, Some("high-threshold")),
    )
    .unwrap();
    assert_eq!((threshold(&a), threshold(&b)), (1000, 12));
    s.save_plugin_settings(
        "fixture.feature",
        PluginSettings {
            revision: 0,
            values: json!({"suffix":"global-change"}),
        },
    )
    .unwrap();
    assert_eq!((threshold(&a), threshold(&b)), (1000, 12));
}
