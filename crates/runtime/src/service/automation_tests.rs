use super::*;
use crate::fake::FakeRuntime;
use pockymoe_protocol::ThreadEventEnvelope;

#[test]
fn reply_threshold_counts_unicode_prose_without_code() {
    assert_eq!(reply_prose_chars("你好`command`世界"), 4);
    assert_eq!(reply_prose_chars("前``some ` command``后"), 2);
    assert_eq!(reply_prose_chars("前\n~~~~rust\nignored\n~~~~\n后"), 2);
    assert_eq!(reply_prose_chars("literal `unclosed"), 17);
    assert!(!condition_ok(
        &Condition::ReplyLengthAtLeast { chars: 0 },
        &json!({})
    ));
}

pub(crate) fn supervisor(dir: &Path) -> Arc<Supervisor> {
    let config = RuntimeConfig {
        treer_enabled: false,
        host_mode: Default::default(),
        mode: pockymoe_protocol::Mode::Local,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root: dir.into(),
        database_url: dir.join("db.sqlite"),
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
    let db = Database::open(&config.database_url).unwrap();
    let s = Arc::new(Supervisor::new(
        config,
        db,
        vec![Arc::new(FakeRuntime::new(Provider::Codex))],
    ));
    s.spawn_live_item_persister();
    s
}
async fn setup() -> (tempfile::TempDir, Arc<Supervisor>, String) {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    let w = s
        .create_workspace(CreateWorkspaceInput {
            abs_path: Some(dir.path().to_string_lossy().into()),
            git_url: None,
            label: None,
        })
        .unwrap();
    let t = s
        .create_thread(CreateThreadInput {
            workspace_id: w.id,
            title: None,
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: None,
        })
        .await
        .unwrap();
    (dir, s, t.id)
}
fn def(trigger: Value, action: Value) -> Definition {
    serde_json::from_value(json!({"name":"fixture","trigger":trigger,"action":action})).unwrap()
}
async fn child(s: &Supervisor, parent: &str) -> String {
    s.create_thread(CreateThreadInput {
        workspace_id: s.get_thread(parent).unwrap().workspace_id,
        title: None,
        provider: Some(Provider::Codex),
        agent_id: None,
        model: "ios-e2e-stream".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: Some(parent.into()),
    })
    .await
    .unwrap()
    .id
}
fn hourly(action: Value) -> Definition {
    def(
        json!({"kind":"interval","everySeconds":3600,"anchorAt":"2030-01-01T01:00:00Z"}),
        action,
    )
}
fn prompt() -> Value {
    json!({"kind":"prompt","text":"hello automation"})
}
fn notice() -> Value {
    json!({"kind":"notifyInbox","subject":"fixture result","text":"done","includeClosingMessage":true})
}
fn runs(s: &Supervisor, t: &str, id: &str) -> Vec<Value> {
    s.automation_runs(t, id, 100).unwrap()["runs"]
        .as_array()
        .unwrap()
        .clone()
}
fn pending(s: &Supervisor, t: &str) -> i64 {
    s.db.with(|c| {
        Ok(c.query_row(
            "SELECT count(*) FROM thread_pending_steers WHERE thread_id=?1",
            [t],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}
fn mail(s: &Supervisor, t: &str) -> i64 {
    s.db.with(|c| {
        Ok(c.query_row(
            "SELECT count(*) FROM kv WHERE key GLOB ?1",
            [format!("cli:inbox:{t}:*")],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}
fn turn(s: &Supervisor, t: &str, id: &str, status: &str) {
    s.db.with(|c|{c.execute("INSERT INTO thread_turns(id,thread_id,status,display_prompt,started_at,ordinal) VALUES(?1,?2,?3,'fixture','2030-01-01T00:00:00Z',(SELECT coalesce(max(ordinal),0)+1 FROM thread_turns WHERE thread_id=?2))",params![id,t,status])?;c.execute("UPDATE threads SET status='running' WHERE id=?1",[t])?;Ok(())}).unwrap();
}
fn finish(s: &Supervisor, t: &str, id: &str, status: &str) {
    s.persist_turn_result(t, id, status, None, &[], "2030-01-01T03:30:00Z")
        .unwrap();
}
async fn wait_command(s: &Supervisor, t: &str, run: &str) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let r = runs(s, t, run);
            if r[0]["state"] != "running" {
                return r[0].clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn automation_busy_two_and_half_periods_coalesces_until_whole_turn_ends() {
    let (_dir, s, t) = setup().await;
    turn(&s, &t, "user-turn", "inProgress");
    let a = s
        .automation_create(&t, hourly(prompt()), Some("hourly"))
        .unwrap();
    let id = a["id"].as_str().unwrap();
    s.automation_tick("2030-01-01T01:00:00Z").await.unwrap();
    s.drain_steers(&t).await.unwrap();
    assert_eq!(pending(&s, &t), 1);
    // Tool/item progress is deliberately not a completion boundary.
    s.bus.emit(pockymoe_protocol::ThreadEventEnvelope{event_type:pockymoe_protocol::ThreadEventType::ThreadItemCompleted,thread_id:t.clone(),timestamp:"2030-01-01T02:00:00Z".into(),payload:json!({"turnId":"user-turn","item":{"id":"tool","kind":"toolCall","text":"batch finished","status":"completed"}})});
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    s.drain_steers(&t).await.unwrap();
    let r = runs(&s, &t, id);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["state"], "queued");
    assert_eq!(r[0]["missedCount"], 2);
    assert!(r[0]["turnId"].is_null());
    assert_eq!(pending(&s, &t), 1);
    assert_eq!(
        s.automation_show(&t, id).unwrap()["nextRunAt"],
        "2030-01-01T04:00:00.000Z"
    );
    finish(&s, &t, "user-turn", "completed");
    s.drain_steers(&t).await.unwrap();
    assert_eq!(pending(&s, &t), 0);
    assert!(runs(&s, &t, id)[0]["turnId"].is_string());
}

#[tokio::test]
async fn automation_restart_merges_downtime_and_lost_ack_does_not_repeat_acceptance() {
    let (dir, s, t) = setup().await;
    let d = hourly(prompt());
    let a = s.automation_create(&t, d.clone(), Some("stable")).unwrap();
    let id = a["id"].as_str().unwrap().to_owned();
    s.automation_tick("2030-01-01T01:00:00Z").await.unwrap();
    drop(s);
    let s = supervisor(dir.path());
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    assert_eq!(pending(&s, &t), 1);
    assert_eq!(runs(&s, &t, &id)[0]["missedCount"], 2);
    assert_eq!(
        s.automation_create(&t, d, Some("stable")).unwrap()["id"],
        id
    );
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    assert_eq!(pending(&s, &t), 1);
    assert!(s
        .automation_create(&t, hourly(notice()), Some("stable"))
        .unwrap_err()
        .to_string()
        .contains("conflict"));
    s.automation_control(&t, &id, "pause").unwrap();
    assert_eq!(pending(&s, &t), 0);
}

#[tokio::test]
async fn automation_repeated_resume_preserves_unconsumed_events_and_interval_schedule() {
    let (_dir, s, t) = setup().await;
    turn(&s, &t, "resume-source", "inProgress");
    let event = s
        .automation_create(
            &t,
            def(
                json!({"kind":"turnEnded","sourceThreadId":t,"turnId":"resume-source"}),
                notice(),
            ),
            None,
        )
        .unwrap();
    let event_id = event["id"].as_str().unwrap();
    let timer = s.automation_create(&t, hourly(notice()), None).unwrap();
    let timer_id = timer["id"].as_str().unwrap();
    // Advance nextRun beyond its original anchor so resetting it is observable.
    s.automation_tick("2030-01-01T02:30:00Z").await.unwrap();
    let timer_before = s.automation_show(&t, timer_id).unwrap();
    finish(&s, &t, "resume-source", "completed");
    let mail_before = mail(&s, &t);
    for _ in 0..2 {
        assert_eq!(s.automation_control(&t, event_id, "resume").unwrap(), event);
        assert_eq!(
            s.automation_control(&t, timer_id, "resume").unwrap(),
            timer_before
        );
    }
    s.automation_control(&t, timer_id, "pause").unwrap();
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    assert_eq!(runs(&s, &t, event_id).len(), 1);
    assert_eq!(runs(&s, &t, event_id)[0]["state"], "completed");
    assert_eq!(mail(&s, &t), mail_before + 1);
    assert_eq!(pending(&s, &t), 0);
}

#[tokio::test]
async fn automation_thread_ended_cross_thread_repeats_only_for_new_complete_source_turns() {
    let (_dir, s, a) = setup().await;
    let b = child(&s, &a).await;
    let c = child(&s, &a).await;
    turn(&s, &b, "historical-b", "inProgress");
    finish(&s, &b, "historical-b", "completed");
    let d = def(json!({"kind":"threadEnded","sourceThreadId":b}), notice());
    let mut replay = d.clone();
    replay.replay_existing = true;
    assert!(s
        .automation_preview(&a, replay)
        .unwrap_err()
        .to_string()
        .contains("replayUnsupported"));
    let hook = s.automation_create(&a, d.clone(), None).unwrap();
    let id = hook["id"].as_str().unwrap();
    let mut successful = d;
    successful.condition = Condition::StatusIn {
        values: vec!["completed".into()],
    };
    let filtered = s.automation_create(&a, successful, None).unwrap();
    let filtered_id = filtered["id"].as_str().unwrap();
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    assert!(runs(&s, &a, id).is_empty()); // No historical or idle-state replay.
    turn(&s, &c, "unrelated-c", "inProgress");
    finish(&s, &c, "unrelated-c", "completed");
    turn(&s, &b, "first-b", "inProgress");
    s.bus.emit(ThreadEventEnvelope {
        event_type: pockymoe_protocol::ThreadEventType::ThreadItemCompleted, thread_id: b.clone(),
        timestamp: "2030-01-01T03:30:00Z".into(),
        payload: json!({"turnId":"first-b","item":{"id":"tool","kind":"toolCall","status":"completed"}}),
    });
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    assert_eq!(mail(&s, &a), 0);
    finish(&s, &b, "first-b", "completed");
    finish(&s, &b, "first-b", "completed");
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    assert_eq!(runs(&s, &a, id).len(), 1);
    assert_eq!(mail(&s, &a), 2); // One per matching hook, never per repeated event.
    turn(&s, &b, "second-b", "inProgress");
    finish(&s, &b, "second-b", "failed");
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    assert_eq!(runs(&s, &a, id).len(), 2);
    assert_eq!(runs(&s, &a, filtered_id)[0]["state"], "conditionSkipped");
    assert_eq!(mail(&s, &a), 3);
    // Multiple complete turns in one event-journal batch must all be consumed.
    turn(&s, &b, "third-b", "inProgress");
    finish(&s, &b, "third-b", "interrupted");
    turn(&s, &b, "fourth-b", "inProgress");
    finish(&s, &b, "fourth-b", "completed");
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    assert_eq!(runs(&s, &a, id).len(), 4);
    assert_eq!(runs(&s, &a, filtered_id).len(), 4);
    assert_eq!(mail(&s, &a), 6);
    assert_eq!(mail(&s, &b), 0);
    assert_eq!(pending(&s, &a), 0);
}

#[tokio::test]
async fn automation_thread_ended_prompt_waits_for_whole_target_turn() {
    let (_dir, s, a) = setup().await;
    let b = child(&s, &a).await;
    turn(&s, &a, "busy-a", "inProgress");
    let hook = s
        .automation_create(
            &a,
            def(json!({"kind":"threadEnded","sourceThreadId":b}), prompt()),
            None,
        )
        .unwrap();
    let id = hook["id"].as_str().unwrap();
    turn(&s, &b, "source-b", "inProgress");
    finish(&s, &b, "source-b", "completed");
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    s.drain_steers(&a).await.unwrap();
    assert_eq!(pending(&s, &a), 1);
    s.bus.emit(ThreadEventEnvelope {
        event_type: pockymoe_protocol::ThreadEventType::ThreadItemCompleted,
        thread_id: a.clone(),
        timestamp: "2030-01-01T03:30:00Z".into(),
        payload: json!({"turnId":"busy-a"}),
    });
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    s.drain_steers(&a).await.unwrap();
    assert_eq!(pending(&s, &a), 1);
    assert!(runs(&s, &a, id)[0]["turnId"].is_null());
    finish(&s, &a, "busy-a", "completed");
    s.drain_steers(&a).await.unwrap();
    assert_eq!(pending(&s, &a), 0);
    assert!(runs(&s, &a, id)[0]["turnId"].is_string());
}

#[tokio::test]
async fn automation_thread_ended_source_close_or_delete_pauses_without_a_completion() {
    let (_dir, s, a) = setup().await;
    let closed = child(&s, &a).await;
    let deleted = child(&s, &a).await;
    let close_hook = s
        .automation_create(
            &a,
            def(
                json!({"kind":"threadEnded","sourceThreadId":closed}),
                notice(),
            ),
            None,
        )
        .unwrap();
    let delete_hook = s
        .automation_create(
            &a,
            def(
                json!({"kind":"threadEnded","sourceThreadId":deleted}),
                prompt(),
            ),
            None,
        )
        .unwrap();
    turn(&s, &a, "busy-a", "inProgress");
    turn(&s, &deleted, "completed-before-delete", "inProgress");
    finish(&s, &deleted, "completed-before-delete", "completed");
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    assert_eq!(pending(&s, &a), 1);
    s.close_agent_thread(Some(&a), &closed, false).unwrap();
    s.delete_thread(&deleted).unwrap();
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    for h in [&close_hook, &delete_hook] {
        let id = h["id"].as_str().unwrap();
        let current = s.automation_show(&a, id).unwrap();
        assert_eq!(current["state"], "paused");
        assert_eq!(current["error"], "sourceUnavailable");
        assert!(s.automation_control(&a, id, "resume").is_err());
    }
    assert!(runs(&s, &a, close_hook["id"].as_str().unwrap()).is_empty());
    assert_eq!(runs(&s, &a, delete_hook["id"].as_str().unwrap()).len(), 1);
    assert_eq!(
        runs(&s, &a, delete_hook["id"].as_str().unwrap())[0]["state"],
        "cancelled"
    );
    assert_eq!(pending(&s, &a), 0);
    assert_eq!(mail(&s, &a), 0);
    assert!(s
        .automation_create(
            &a,
            def(
                json!({"kind":"threadEnded","sourceThreadId":closed}),
                notice()
            ),
            None
        )
        .unwrap_err()
        .to_string()
        .contains("sourceUnavailable"));
}

#[tokio::test]
async fn automation_exact_turn_conditions_and_repeated_terminal_events_are_passive() {
    let (_dir, s, t) = setup().await;
    turn(&s, &t, "source", "inProgress");
    turn(&s, &t, "other", "inProgress");
    let mut d = def(
        json!({"kind":"turnEnded","sourceThreadId":t,"turnId":"source"}),
        notice(),
    );
    d.condition = Condition::All {
        conditions: vec![
            Condition::StatusIn {
                values: vec!["completed".into()],
            },
            Condition::Not {
                condition: Box::new(Condition::StatusIn {
                    values: vec!["failed".into()],
                }),
            },
        ],
    };
    let a = s.automation_create(&t, d, None).unwrap();
    let id = a["id"].as_str().unwrap();
    finish(&s, &t, "other", "completed");
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    assert!(runs(&s, &t, id).is_empty());
    finish(&s, &t, "source", "completed");
    finish(&s, &t, "source", "completed");
    // Receiver can be running; ordinary result still never queues or prompts.
    s.db.with(|c| {
        c.execute("UPDATE threads SET status='running' WHERE id=?1", [&t])?;
        Ok(())
    })
    .unwrap();
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
    assert_eq!(mail(&s, &t), 1);
    assert_eq!(pending(&s, &t), 0);
    assert_eq!(runs(&s, &t, id).len(), 1);
    assert!(s
        .automation_create(
            &t,
            def(
                json!({"kind":"turnEnded","sourceThreadId":t,"turnId":"source"}),
                notice()
            ),
            None
        )
        .unwrap_err()
        .to_string()
        .contains("sourceAlreadyEnded"));
    let mut d = def(
        json!({"kind":"turnEnded","sourceThreadId":t,"turnId":"source"}),
        notice(),
    );
    d.replay_existing = true;
    let b = s.automation_create(&t, d, None).unwrap();
    s.automation_tick(&now_rfc3339()).await.unwrap();
    assert_eq!(
        runs(&s, &t, b["id"].as_str().unwrap())[0]["state"],
        "completed"
    );
    assert_eq!(mail(&s, &t), 2);
}

#[tokio::test]
async fn automation_task_failed_is_distinct_and_does_not_unblock_dependencies() {
    let (_dir, s, t) = setup().await;
    let task = s.task_add(&t, "build", None, &[], None).unwrap();
    let number = task["number"]
        .as_i64()
        .or_else(|| task["task"]["number"].as_i64())
        .unwrap();
    s.task_add(&t, "dependent", None, &[number], None).unwrap();
    let mut d = def(
        json!({"kind":"taskEnded","rootThreadId":t,"taskNumber":number}),
        notice(),
    );
    d.condition = Condition::StatusIn {
        values: vec!["completed".into()],
    };
    let a = s.automation_create(&t, d, None).unwrap();
    s.task_done(&t, number, Some("build failed"), true).unwrap();
    s.automation_tick(&now_rfc3339()).await.unwrap();
    assert_eq!(
        runs(&s, &t, a["id"].as_str().unwrap())[0]["state"],
        "conditionSkipped"
    );
    assert_eq!(pending(&s, &t), 0);
    assert_eq!(mail(&s, &t), 0);
    let claimed = s.task_claim(&t, Some(number + 1)).unwrap_err().to_string();
    assert!(claimed.contains("waiting on"), "{claimed}");
}

#[tokio::test]
async fn automation_pause_cancel_and_user_stop_remove_only_owned_pending() {
    let (_dir, s, t) = setup().await;
    turn(&s, &t, "user", "inProgress");
    s.send_to_thread(
        &t,
        serde_json::from_value(
            json!({"text":"user queued message","delivery":"queue","kind":"task"}),
        )
        .unwrap(),
    )
    .unwrap();
    let a = s.automation_create(&t, hourly(prompt()), None).unwrap();
    let id = a["id"].as_str().unwrap();
    s.automation_tick("2030-01-01T01:00:00Z").await.unwrap();
    assert_eq!(pending(&s, &t), 2);
    s.automation_control(&t, id, "pause").unwrap();
    assert_eq!(pending(&s, &t), 1);
    s.automation_tick("2030-01-01T05:00:00Z").await.unwrap();
    assert_eq!(pending(&s, &t), 1);
    s.automation_control(&t, id, "resume").unwrap();
    s.automation_tick("2030-01-01T05:00:00Z").await.unwrap();
    s.interrupt(&t).await.unwrap();
    assert_eq!(pending(&s, &t), 1);
    assert_eq!(s.automation_show(&t, id).unwrap()["state"], "paused");
    s.automation_control(&t, id, "cancel").unwrap();
    assert!(s.automation_control(&t, id, "resume").is_err());
    assert_eq!(pending(&s, &t), 1);
}

#[tokio::test]
async fn automation_at_inbox_is_passive_for_idle_and_missed_policy_skips_late_runs() {
    let (_dir, s, t) = setup().await;
    let a = s
        .automation_create(
            &t,
            def(json!({"kind":"at","at":"2030-01-01T01:00:00Z"}), notice()),
            None,
        )
        .unwrap();
    s.automation_tick("2030-01-01T01:00:00Z").await.unwrap();
    assert_eq!(mail(&s, &t), 1);
    assert_eq!(pending(&s, &t), 0);
    assert_eq!(s.get_thread(&t).unwrap().status, "idle");
    assert_eq!(
        runs(&s, &t, a["id"].as_str().unwrap())[0]["state"],
        "completed"
    );
    let mut d = hourly(prompt());
    d.missed_run_policy = MissedRunPolicy::Skip;
    let a = s.automation_create(&t, d, None).unwrap();
    s.automation_tick("2030-01-01T04:00:00Z").await.unwrap();
    assert_eq!(
        runs(&s, &t, a["id"].as_str().unwrap())[0]["state"],
        "skipped"
    );
    let a = s
        .automation_create(
            &t,
            def(json!({"kind":"at","at":"2030-01-01T01:00:00Z"}), prompt()),
            None,
        )
        .unwrap();
    s.automation_tick("2030-01-03T01:00:00Z").await.unwrap();
    assert_eq!(
        runs(&s, &t, a["id"].as_str().unwrap())[0]["state"],
        "skipped"
    );
    assert_eq!(pending(&s, &t), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn automation_time_and_command_events_execute_real_scripts_once_and_exclude_failures() {
    let (dir, s, t) = setup().await;
    let script = json!({"kind":"runScript","argv":["/bin/sh","-c","printf 'run\\n' >> executions; test -z \"$POCKYMOE_TOKEN$POCKYMOE_URL$POCKYMOE_CLI_CONFIG\""],"cwd":".","timeoutSeconds":3});
    let a = s
        .automation_create(
            &t,
            def(
                json!({"kind":"at","at":"2030-01-01T01:00:00Z"}),
                script.clone(),
            ),
            None,
        )
        .unwrap();
    let aid = a["id"].as_str().unwrap();
    s.automation_tick("2030-01-01T01:00:00Z").await.unwrap();
    assert_eq!(wait_command(&s, &t, aid).await["state"], "completed");
    let mut d = def(
        json!({"kind":"commandEnded","sourceThreadId":t,"commandKey":"build"}),
        script,
    );
    d.condition = Condition::ExitCodeEquals { value: 0 };
    let a = s.automation_create(&t, d, None).unwrap();
    let aid = a["id"].as_str().unwrap();
    let input:CommandRunInput=serde_json::from_value(json!({"argv":["/bin/sh","-c","printf wrapper"],"cwd":".","commandKey":"build","clientRequestId":"build-once"})).unwrap();
    let result = s.command_run(&t, input.clone()).await.unwrap();
    assert_eq!(result["state"], "completed");
    assert_eq!(result["stdout"], "wrapper");
    assert_eq!(s.command_run(&t, input).await.unwrap()["id"], result["id"]);
    s.automation_tick(&now_rfc3339()).await.unwrap();
    assert_eq!(wait_command(&s, &t, aid).await["state"], "completed");
    s.automation_tick(&now_rfc3339()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("executions")).unwrap(),
        "run\nrun\n"
    );
    assert_eq!(runs(&s, &t, aid).len(), 1);
    assert_eq!(pending(&s, &t), 0);
    for (key, shell, timeout, expected) in [
        ("fail", "exit 7", 3, "failed"),
        ("timeout", "sleep 30", 1, "timedOut"),
    ] {
        let result=s.command_run(&t,serde_json::from_value(json!({"shell":shell,"cwd":".","timeoutSeconds":timeout,"commandKey":"build","clientRequestId":key})).unwrap()).await.unwrap();
        assert_eq!(result["state"], expected);
    }
    s.automation_tick(&now_rfc3339()).await.unwrap();
    let r = runs(&s, &t, aid);
    assert_eq!(r.len(), 3);
    assert_eq!(
        r.iter()
            .filter(|r| r["state"] == "conditionSkipped")
            .count(),
        2
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("executions")).unwrap(),
        "run\nrun\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn automation_real_wrapper_nonzero_exit_matches_once_and_not_timeout_or_unknown() {
    let (_dir, s, t) = setup().await;
    let mut d = def(
        json!({"kind":"commandEnded","sourceThreadId":t,"commandKey":"expected-seven"}),
        notice(),
    );
    d.condition = Condition::ExitCodeEquals { value: 7 };
    for event in [
        json!({"status":"timedOut","exitCode":7}),
        json!({"status":"uncertain","exitCode":7}),
        json!({"status":"failed"}),
        json!({"status":"completed"}),
    ] {
        assert!(!condition_ok(&d.condition, &event));
    }
    let a = s.automation_create(&t, d, None).unwrap();
    let id = a["id"].as_str().unwrap();
    let input: CommandRunInput = serde_json::from_value(json!({
        "shell":"printf expected-failure; exit 7","cwd":".",
        "commandKey":"expected-seven","clientRequestId":"exit-seven-once"
    }))
    .unwrap();
    let result = s.command_run(&t, input.clone()).await.unwrap();
    assert_eq!(result["state"], "failed");
    assert_eq!(result["exitCode"], 7);
    assert_eq!(result["stdout"], "expected-failure");
    assert_eq!(s.command_run(&t, input).await.unwrap()["id"], result["id"]);
    assert_eq!(mail(&s, &t), 1); // Wrapper's own completion notice.
    s.automation_tick(&now_rfc3339()).await.unwrap();
    s.automation_tick(&now_rfc3339()).await.unwrap();
    assert_eq!(runs(&s, &t, id).len(), 1);
    assert_eq!(runs(&s, &t, id)[0]["state"], "completed");
    assert_eq!(mail(&s, &t), 2); // Exactly one additional hook notice.
    assert_eq!(pending(&s, &t), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn automation_spawn_then_crash_is_uncertain_and_does_not_repeat_effects() {
    let (dir, s, t) = setup().await;
    let input:CommandRunInput=serde_json::from_value(json!({"shell":"printf x >> effect; echo $$ > childpid; sleep 30","cwd":".","clientRequestId":"crash"})).unwrap();
    let id =
        s.db.with(|c| insert_command(c, &t, &input, None, &[], &now_rfc3339()))
            .unwrap();
    let worker = {
        let s = s.clone();
        let id = id.clone();
        tokio::spawn(async move { s.execute_command(&id).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !dir.path().join("childpid").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    worker.abort();
    let _ = worker.await;
    let pid = std::fs::read_to_string(dir.path().join("childpid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    kill_command_tree(pid).await;
    s.recover_automation_commands().unwrap();
    assert_eq!(s.command_show(&t, &id).unwrap()["state"], "uncertain");
    assert_eq!(
        s.command_run(&t, input).await.unwrap()["state"],
        "uncertain"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effect")).unwrap(),
        "x"
    );
    assert_eq!(mail(&s, &t), 1);
}

#[test]
fn automation_typed_conditions_and_loop_guard_do_not_treat_unknown_exit_as_success() {
    let cond = Condition::Any {
        conditions: vec![
            Condition::ExitCodeEquals { value: 0 },
            Condition::All {
                conditions: vec![
                    Condition::StatusIn {
                        values: vec!["completed".into()],
                    },
                    Condition::CommandId {
                        value: "selected".into(),
                    },
                ],
            },
        ],
    };
    assert!(!condition_ok(
        &cond,
        &json!({"status":"uncertain","exitCode":0})
    ));
    assert!(!condition_ok(&cond, &json!({"status":"completed"})));
    assert!(condition_ok(
        &cond,
        &json!({"status":"completed","commandId":"selected"})
    ));
    assert!(serde_json::from_value::<Definition>(
        json!({"name":"bad","trigger":{"kind":"ptyRegex"},"action":prompt()})
    )
    .is_err());
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("db.sqlite")).unwrap();
    db.with(|c|{c.execute("INSERT INTO automations(id,thread_id,definition_json,state,event_cursor,created_at,updated_at) VALUES('same','t','{}','enabled',0,'now','now')",[])?;create_run(c,"same",&hourly(prompt()),"loop","2030-01-01T00:00:00Z","2030-01-01T00:00:00Z",&json!({"ancestry":["same"]}),0)?;let state:String=c.query_row("SELECT state FROM automation_runs",[],|r|r.get(0))?;assert_eq!(state,"loopSkipped");Ok(())}).unwrap();
}

#[tokio::test]
async fn automation_failed_and_interrupted_turn_conditions_are_exact_and_expired_queue_is_removed()
{
    let (_dir, s, t) = setup().await;
    for status in ["failed", "interrupted"] {
        turn(&s, &t, status, "inProgress");
        let mut d = def(
            json!({"kind":"turnEnded","sourceThreadId":t,"turnId":status}),
            notice(),
        );
        d.condition = Condition::StatusIn {
            values: vec![status.into()],
        };
        let a = s.automation_create(&t, d, None).unwrap();
        finish(&s, &t, status, status);
        s.automation_tick("2030-01-01T03:30:00Z").await.unwrap();
        assert_eq!(
            runs(&s, &t, a["id"].as_str().unwrap())[0]["state"],
            "completed"
        );
    }
    turn(&s, &t, "busy", "inProgress");
    let mut d = def(json!({"kind":"at","at":"2020-01-01T00:00:00Z"}), prompt());
    d.max_lateness_seconds = u64::MAX.min(31536000);
    // Admit with a deterministic clock, then prove the real queue boundary checks lateness.
    let a = s.automation_create(&t, d, None).unwrap();
    let id = a["id"].as_str().unwrap();
    s.automation_tick("2020-01-01T00:00:00Z").await.unwrap();
    assert_eq!(pending(&s, &t), 1);
    finish(&s, &t, "busy", "completed");
    s.drain_steers(&t).await.unwrap();
    assert_eq!(pending(&s, &t), 0);
    assert_eq!(runs(&s, &t, id)[0]["state"], "skipped");
}

#[tokio::test]
async fn automation_stop_does_not_reemit_historical_interrupted_turns() {
    let (_dir, s, t) = setup().await;
    turn(&s, &t, "historical", "interrupted");
    let mut d = def(
        json!({"kind":"turnEnded","sourceThreadId":t,"turnId":"historical"}),
        notice(),
    );
    d.replay_existing = true;
    let a = s.automation_create(&t, d, None).unwrap();
    s.automation_tick(&now_rfc3339()).await.unwrap();
    assert_eq!(mail(&s, &t), 1);
    turn(&s, &t, "current", "inProgress");
    s.interrupt(&t).await.unwrap();
    s.automation_tick(&now_rfc3339()).await.unwrap();
    assert_eq!(runs(&s, &t, a["id"].as_str().unwrap()).len(), 1);
    assert_eq!(mail(&s, &t), 1);
    let old_events =
        s.db.with(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM automation_events WHERE event_key='turn:historical:terminal'",
                [],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert_eq!(old_events, 0);
}

#[cfg(unix)]
#[tokio::test]
async fn automation_target_deleted_before_spawn_records_failure_without_execution() {
    let (dir, s, t) = setup().await;
    let input: CommandRunInput =
        serde_json::from_value(json!({"shell":"touch should-not-exist","cwd":"."})).unwrap();
    let id =
        s.db.with(|c| insert_command(c, &t, &input, None, &[], &now_rfc3339()))
            .unwrap();
    s.delete_thread(&t).unwrap();
    s.execute_command(&id).await.unwrap();
    assert_eq!(s.command_show(&t, &id).unwrap()["state"], "failed");
    assert!(!dir.path().join("should-not-exist").exists());
}

#[tokio::test]
async fn lifetime_statistics_include_all_runs_deduplicate_turns_and_price_each_model() {
    let (_dir, s, t) = setup().await;
    let a = s.automation_create(&t, hourly(prompt()), None).unwrap();
    let id = a["id"].as_str().unwrap();
    let raw = a["definition"].to_string();
    let usage = json!({"total":{"inputTokens":10,"outputTokens":5},"last":{"inputTokens":10,"outputTokens":5}}).to_string();
    s.db.with(|c| {
        for n in 0..105 {
            let turn = format!("attributed-{n}");
            let model = if n == 103 { "unpriced-model" } else if n % 2 == 0 { "gpt-6.1-sol" } else { "gpt-6-astra" };
            let state = if n == 104 { "running" } else { "completed" };
            c.execute("INSERT INTO thread_turns(id,thread_id,status,model,token_usage_json,started_at,ordinal) VALUES(?1,?2,?3,?4,?5,'2030-01-01T01:00:00Z',?6)", params![turn,t,state,model,if n == 102 {None}else{Some(&usage)},n])?;
            c.execute("INSERT INTO automation_runs(id,automation_id,occurrence_key,state,scheduled_at,observed_at,definition_json,event_json,turn_id,started_at) VALUES(?1,?2,?1,?3,'2030-01-01T01:00:00Z','2030-01-01T01:00:00Z',?4,'{}',?5,'2030-01-01T01:00:00Z')",params![format!("run-{n}"),id,state,raw,turn])?;
        }
        // Duplicate binding and coalesced ticks must never multiply model charges.
        c.execute("INSERT INTO automation_runs(id,automation_id,occurrence_key,state,scheduled_at,observed_at,definition_json,event_json,turn_id,missed_count,started_at) VALUES('duplicate',?1,'duplicate','completed','2030-01-01T01:00:00Z','2030-01-01T01:00:00Z',?2,'{}','attributed-0',4,'2030-01-01T01:00:00Z')",params![id,raw])?;
        // Same-thread ordinary usage must not leak into automation totals.
        c.execute("INSERT INTO thread_turns(id,thread_id,status,model,token_usage_json,started_at,ordinal) VALUES('ordinary',?1,'completed','gpt-6.1-sol',?2,'2030-01-01T01:00:00Z',200)",params![t,usage])?;
        Ok(())
    }).unwrap();
    assert_eq!(runs(&s, &t, id).len(), 100);
    let shown = s.automation_show(&t, id).unwrap();
    let stats = &shown["statistics"];
    assert_eq!(stats["triggerCount"], 110);
    assert_eq!(stats["runCount"], 106);
    assert_eq!(stats["executedActionCount"], 106);
    assert_eq!(stats["runningActionCount"], 1);
    assert_eq!(stats["promptTurnCount"], 105);
    assert_eq!(stats["usageTurnCount"], 104);
    assert_eq!(stats["pricedTurnCount"], 103);
    assert_eq!(stats["tokenUsage"]["totalTokens"], 104 * 15);
    let expected: f64 = s
        .load_turns_meta(&t)
        .unwrap()
        .iter()
        .filter(|turn| turn.id != "ordinary")
        .filter_map(|turn| turn.price_estimate.as_ref()?.get("totalUsd")?.as_f64())
        .sum();
    assert!((stats["priceEstimate"]["totalUsd"].as_f64().unwrap() - expected).abs() < 1e-12);
    assert_eq!(
        s.automation_list(&t).unwrap()["automations"][0]["statistics"],
        *stats
    );
}

#[tokio::test]
async fn lifetime_statistics_exclude_ambiguous_and_missing_turns_and_unmeasured_actions() {
    let (_dir, s, t) = setup().await;
    let a = s.automation_create(&t, hourly(prompt()), None).unwrap();
    let b = s.automation_create(&t, hourly(prompt()), None).unwrap();
    let inbox = s.automation_create(&t, hourly(notice()), None).unwrap();
    let script = s
        .automation_create(
            &t,
            hourly(json!({"kind":"runScript","argv":["true"],"cwd":"."})),
            None,
        )
        .unwrap();
    s.db.with(|c| {
        c.execute("INSERT INTO thread_turns(id,thread_id,status,model,token_usage_json,started_at,ordinal) VALUES('shared-turn',?1,'completed','gpt-6.1-sol',?2,'2030-01-01T01:00:00Z',1)",params![t,json!({"total":{"inputTokens":100,"outputTokens":10}}).to_string()])?;
        for (n, a) in [&a,&b,&inbox,&script].iter().enumerate() {
            let id = a["id"].as_str().unwrap();
            c.execute("INSERT INTO automation_runs(id,automation_id,occurrence_key,state,scheduled_at,observed_at,definition_json,event_json,turn_id,started_at) VALUES(?1,?2,?1,'completed','2030-01-01T01:00:00Z','2030-01-01T01:00:00Z',?3,'{}','shared-turn','2030-01-01T01:00:00Z')",params![format!("shared-{n}"),id,a["definition"].to_string()])?;
        }
        let id = a["id"].as_str().unwrap();
        for (key, turn, state) in [("missing",Some("deleted-turn"),"completed"),("unbound",None,"uncertain"),("skipped",None,"conditionSkipped"),("queued",None,"queued")] {
            c.execute("INSERT INTO automation_runs(id,automation_id,occurrence_key,state,scheduled_at,observed_at,definition_json,event_json,turn_id,missed_count) VALUES(?1,?2,?1,?3,'2030-01-01T01:00:00Z','2030-01-01T01:00:00Z',?4,'{}',?5,2)",params![key,id,state,a["definition"].to_string(),turn])?;
        }
        Ok(())
    }).unwrap();
    let stats = s.automation_show(&t, a["id"].as_str().unwrap()).unwrap()["statistics"].clone();
    assert_eq!(stats["triggerCount"], 13);
    assert_eq!(stats["promptTurnCount"], 2);
    assert_eq!(stats["ambiguousTurnCount"], 1);
    assert_eq!(stats["missingTurnCount"], 1);
    assert_eq!(stats["unattributedRunCount"], 1);
    assert!(stats["tokenUsage"].is_null());
    assert!(stats["priceEstimate"].is_null());
    for a in [inbox, script] {
        let stats = s.automation_show(&t, a["id"].as_str().unwrap()).unwrap()["statistics"].clone();
        assert_eq!(stats["promptTurnCount"], 0);
        assert_eq!(stats["ambiguousTurnCount"], 0);
        assert_eq!(stats["tokenUsage"]["totalTokens"], 0);
        assert_eq!(stats["priceEstimate"]["totalUsd"], 0.0);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn command_env_keeps_basics_and_explicit_values_but_not_supervisor_secrets() {
    let (_d, s, t) = setup().await;
    // cargo sets CARGO_MANIFEST_DIR for the test process; like any Supervisor
    // variable outside the basics, a script must not inherit it.
    assert!(std::env::var_os("CARGO_MANIFEST_DIR").is_some());
    let input: CommandRunInput = serde_json::from_value(json!({
        "shell": "printf '%s|%s|%s' \"${HOME:+home}\" \"$EXTRA\" \"${CARGO_MANIFEST_DIR:-none}\"",
        "cwd": ".",
        "env": {"EXTRA": "given"},
    }))
    .unwrap();
    let result = s.command_run(&t, input).await.unwrap();
    assert_eq!(result["state"], "completed");
    assert_eq!(result["stdout"], "home|given|none");

    for bad in [
        json!({"POCKYMOE_COMMAND_ID": "x"}),
        json!({"1BAD": "x"}),
        json!({"A-B": "x"}),
    ] {
        let input: CommandRunInput =
            serde_json::from_value(json!({"argv": ["true"], "cwd": ".", "env": bad})).unwrap();
        assert!(s.command_run(&t, input).await.is_err(), "{bad}");
    }
}
