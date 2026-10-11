//! Opt-in real Claude acceptance. Separate session/workspace/database; never
//! restarts the host Supervisor. Uses the cheapest advertised Haiku model.
use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Mode, Provider};
use pockymoe_runtime::{
    acp::AcpRuntime,
    actor::{AgentRuntime, StartTurnInput},
    Database, RuntimeConfig, Supervisor,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore = "uses real Claude authentication and tokens in an isolated session"]
async fn haiku_reports_native_tokens_and_speed_during_first_tool_wait() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(AcpRuntime::bound(Provider::Claude, "claude", 60000));
    let models = runtime
        .list_models(Some("claude"), Some(dir.path().to_str().unwrap()))
        .await
        .unwrap();
    let model = models
        .iter()
        .find(|m| {
            m.id.to_lowercase().contains("haiku") || m.display_name.to_lowercase().contains("haiku")
        })
        .expect("an advertised Haiku model is required to bound test cost")
        .id
        .clone();
    let mut config = RuntimeConfig::from_env();
    config.database_url = dir.path().join("isolated.sqlite");
    config.workspace_root = dir.path().join("workspaces");
    config.mode = Mode::Local;
    config.fake_runtime = false;
    config.relay_server_url = None;
    config.relay_agent_token = None;
    let state = Arc::new(Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![runtime.clone()],
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
            provider: Some(Provider::Claude),
            agent_id: None,
            model: model.clone(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: None,
            title: Some("isolated token acceptance".into()),
        })
        .await
        .unwrap();
    let turn_id = uuid::Uuid::new_v4().to_string();
    state.db.with(|conn| {
        conn.execute("INSERT INTO thread_turns(id,thread_id,status,model,started_at,ordinal) VALUES(?1,?2,'inProgress',?3,?4,1)", rusqlite::params![turn_id,thread.id,model,pockymoe_protocol::now_rfc3339()])?;
        Ok(())
    }).unwrap();
    let mut events = state.bus.subscribe();
    let cancel = CancellationToken::new();
    let input = StartTurnInput {
        provider_session_id: thread.provider_session_id.clone().unwrap(), thread_id:thread.id.clone(), turn_id:turn_id.clone(),
        prompt:"This is an isolated throughput acceptance test. Do not edit files, create agents or call pockymoe. First write two short sentences. Then call Bash with exactly sleep 12 and wait for it to finish. Finally reply with CLAUDE_THROUGHPUT_DONE. Only one tool call is needed.".into(),
        model:Some(model.clone()), reasoning_effort:None, sandbox_mode:Some("danger-full-access".into()), collaboration_mode:None,
        approval_mode:Some("yolo".into()), performance_mode:None, hidden:false, images:vec![], title:None, context_delivered:false,
    };
    let runner = runtime.clone();
    let bus = state.bus.clone();
    let token = cancel.clone();
    let mut task = tokio::spawn(async move { runner.start_turn(input, bus, token).await });
    let observed = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let event = events.recv().await.map_err(|error| error.to_string())?;
            if event.event_type == "thread.turn.completed" {
                return Err("turn completed before live native usage appeared".to_string());
            }
            if event.event_type != "thread.turn.token.updated" {
                continue;
            }
            let usage = &event.payload["tokenUsage"];
            if usage["generationSpeed"]["state"] == "tool"
                && usage["generationSpeed"]["latestOutputTokensPerSecond"]
                    .as_f64()
                    .is_some_and(|r| r > 0.0)
            {
                return Ok(usage.clone());
            }
        }
    })
    .await;
    if !matches!(&observed, Ok(Ok(_))) {
        cancel.cancel();
    }
    let done = tokio::time::timeout(Duration::from_secs(90), &mut task).await;
    if done.is_err() {
        cancel.cancel();
        task.abort();
    }
    // This runtime owns only its isolated test process, not the installed service.
    runtime.restart("claude").await.unwrap();
    let live = observed
        .expect("live usage while first tool executes")
        .unwrap();
    assert!(live["total"]["outputTokens"].as_u64().unwrap() > 0);
    let items = done.expect("test completes").unwrap().unwrap();
    assert!(items
        .iter()
        .any(|item| item.text.contains("CLAUDE_THROUGHPUT_DONE")));
    let raw = state
        .db
        .with(|c| {
            Ok(c.query_row(
                "SELECT token_usage_json FROM thread_turns WHERE id=?1",
                [&turn_id],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap();
    let final_usage: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(final_usage["source"], "claudeRollout");
    assert_eq!(final_usage["generationSpeed"]["active"], false);
    println!(
        "{}",
        serde_json::json!({"model":model,"liveOutputTokens":live["total"]["outputTokens"],"liveSpeed":live["generationSpeed"]["latestOutputTokensPerSecond"],"finalOutputTokens":final_usage["total"]["outputTokens"],"finalSpeed":final_usage["generationSpeed"]["averageOutputTokensPerSecond"]})
    );
}

#[tokio::test]
#[ignore = "uses real Claude authentication and tokens in an isolated session"]
async fn haiku_native_background_wake_stays_in_one_durable_turn() {
    native_background_wake_acceptance(
        "Isolated background-wake regression probe. Do not edit files, create agents or run pockymoe. Launch exactly one background Bash task with command sleep 15; printf WAKE_ACCEPTANCE_EVENT using run_in_background=true. Then reply WAKE_ACCEPTANCE_WAITING and end your foreground response. Do not poll, wait or run other commands. When its native task completion notification arrives, run Bash printf WAKE_ACCEPTANCE_VERIFIED once and finally reply WAKE_ACCEPTANCE_DONE. Keep all replies very short.",
        "WAKE_ACCEPTANCE_DONE", "WAKE_ACCEPTANCE_VERIFIED", 1, None,
    ).await;
}

#[tokio::test]
#[ignore = "uses real Claude authentication and tokens in an isolated session"]
async fn haiku_steer_during_background_wait_keeps_output_in_same_turn() {
    native_background_wake_acceptance(
        "Isolated waiting-steer acceptance. Do not edit files, create agents or run pockymoe. Launch exactly one background Bash task with command sleep 30; printf WAIT_STEER_EVENT using run_in_background=true. Then reply WAIT_STEER_WAITING and end your foreground response. Do not poll, wait or run other commands. If a user correction arrives while waiting, reply WAIT_STEER_RECEIVED immediately without interrupting the background task. When its native completion notification arrives, run Bash printf WAIT_STEER_VERIFIED once and finally reply WAIT_STEER_DONE. Keep all replies very short.",
        "WAIT_STEER_DONE", "WAIT_STEER_VERIFIED", 1,
        Some(("Please acknowledge this correction with WAIT_STEER_RECEIVED now; keep the existing background task running and report its completion later.", "WAIT_STEER_RECEIVED")),
    ).await;
}

#[tokio::test]
#[ignore = "uses real Claude authentication and tokens in an isolated session"]
async fn haiku_monitor_events_and_final_report_stay_in_one_durable_turn() {
    native_background_wake_acceptance(
        "Isolated Monitor wake acceptance test. Do not edit files, create agents, poll or run pockymoe. Use exactly one Monitor tool with command sleep 10; echo MONITOR_ACCEPTANCE_EVENT; sleep 10; echo MONITOR_ACCEPTANCE_FINISHED. It exits by itself. End the foreground response with MONITOR_ACCEPTANCE_WAITING. When the first monitor event arrives, run exactly one Bash command printf MONITOR_ACCEPTANCE_VERIFIED, then reply MONITOR_ACCEPTANCE_PROGRESS and wait for the next notification. When the monitor stream finishes, finally reply MONITOR_ACCEPTANCE_DONE. ToolSearch may be used to enable Monitor. No other tools or commands. Keep replies very short.",
        "MONITOR_ACCEPTANCE_DONE", "MONITOR_ACCEPTANCE_VERIFIED", 2, None,
    ).await;
}

async fn native_background_wake_acceptance(
    prompt: &str,
    final_marker: &str,
    command_marker: &str,
    expected_wakes: usize,
    correction: Option<(&str, &str)>,
) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(AcpRuntime::bound(Provider::Claude, "claude", 60000));
    let models = runtime
        .list_models(Some("claude"), Some(dir.path().to_str().unwrap()))
        .await
        .unwrap();
    let model = models
        .iter()
        .find(|m| {
            m.id.to_lowercase().contains("haiku") || m.display_name.to_lowercase().contains("haiku")
        })
        .expect("an advertised Haiku model is required to bound test cost")
        .id
        .clone();
    let mut config = RuntimeConfig::from_env();
    config.database_url = dir.path().join("isolated.sqlite");
    config.workspace_root = dir.path().join("workspaces");
    config.mode = Mode::Local;
    config.fake_runtime = false;
    config.relay_server_url = None;
    config.relay_agent_token = None;
    let state = Arc::new(Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![runtime.clone()],
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
            provider: Some(Provider::Claude),
            agent_id: None,
            model: model.clone(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: None,
            title: Some("isolated native wake acceptance".into()),
        })
        .await
        .unwrap();
    let turn_id = uuid::Uuid::new_v4().to_string();
    state.db.with(|conn| {
        conn.execute("INSERT INTO thread_turns(id,thread_id,status,model,started_at,ordinal) VALUES(?1,?2,'inProgress',?3,?4,1)", rusqlite::params![turn_id,thread.id,model,pockymoe_protocol::now_rfc3339()])?;
        Ok(())
    }).unwrap();
    let mut events = state.bus.subscribe();
    let cancel = CancellationToken::new();
    let input = StartTurnInput {
        provider_session_id: thread.provider_session_id.clone().unwrap(),
        thread_id: thread.id.clone(),
        turn_id: turn_id.clone(),
        prompt: prompt.into(),
        model: Some(model.clone()),
        reasoning_effort: None,
        sandbox_mode: Some("danger-full-access".into()),
        collaboration_mode: None,
        approval_mode: Some("yolo".into()),
        performance_mode: None,
        hidden: false,
        images: vec![],
        title: None,
        context_delivered: false,
    };
    let runner = runtime.clone();
    let bus = state.bus.clone();
    let token = cancel.clone();
    let mut task = tokio::spawn(async move { runner.start_turn(input, bus, token).await });
    let observed = tokio::time::timeout(Duration::from_secs(180), async {
        let mut wait_ids = std::collections::HashSet::new();
        let mut wake_ids = std::collections::HashSet::new();
        let mut steered = false;
        let mut correction_seen = correction.is_none();
        let mut streamed_text = std::collections::HashMap::<String, String>::new();
        loop {
            let event = events.recv().await.map_err(|error| error.to_string())?;
            let item = &event.payload["item"];
            if event.event_type == "thread.output.delta" {
                let text = streamed_text
                    .entry(event.payload["itemId"].as_str().unwrap().into())
                    .or_default();
                text.push_str(event.payload["delta"].as_str().unwrap());
                if correction.is_some_and(|(_, marker)| text.contains(marker)) {
                    correction_seen = true;
                }
            }
            if item["origin"] == "nativeBackgroundWait" && item["status"] == "waiting" {
                wait_ids.insert(item["id"].as_str().unwrap().to_owned());
                if let Some((prompt, _)) = correction.filter(|_| !steered) {
                    tokio::time::timeout(
                        Duration::from_secs(10),
                        runtime.send_input(
                            thread.provider_session_id.as_deref().unwrap(),
                            &turn_id,
                            prompt,
                        ),
                    )
                    .await
                    .map_err(|_| "waiting steer blocked on completion".to_owned())?
                    .map_err(|error| error.to_string())?;
                    steered = true;
                }
            }
            if correction.is_some_and(|(_, marker)| {
                item["text"]
                    .as_str()
                    .is_some_and(|text| text.contains(marker))
            }) {
                correction_seen = true;
            }
            if item["origin"] == "nativeTaskNotification" {
                if !correction_seen {
                    return Err(
                        "user correction was not answered before the native task wake".to_owned(),
                    );
                }
                if !wait_ids.contains(item["id"].as_str().unwrap()) {
                    return Err("wake must update the durable waiting anchor".to_owned());
                }
                wake_ids.insert(item["id"].as_str().unwrap().to_owned());
            }
            if event.event_type == "thread.turn.completed" {
                if !correction_seen {
                    return Err("waiting steer response missing".to_owned());
                }
                if wake_ids.len() != expected_wakes {
                    return Err("completed without the waiting-to-wake transition".to_owned());
                }
                return Ok(wake_ids);
            }
        }
    })
    .await;
    if !matches!(&observed, Ok(Ok(_))) {
        cancel.cancel();
    }
    let done = tokio::time::timeout(Duration::from_secs(10), &mut task).await;
    if done.is_err() {
        cancel.cancel();
        task.abort();
    }
    runtime.restart("claude").await.unwrap();
    let wake_ids = observed.expect("bounded real wake").unwrap();
    let items = done.expect("test completes").unwrap().unwrap();
    assert!(items.iter().any(|item| item.text.contains(final_marker)));
    let replies: Vec<_> = items
        .iter()
        .filter(|item| item.kind == "agentMessage")
        .collect();
    assert_eq!(
        replies
            .iter()
            .filter(|item| item.extra.get("responsePhase").and_then(Value::as_str) == Some("final"))
            .count(),
        1
    );
    assert!(replies
        .iter()
        .any(|item| item.text.contains(final_marker) && item.extra["responsePhase"] == "final"));
    assert!(replies
        .iter()
        .filter(|item| !item.text.contains(final_marker))
        .all(|item| item.extra["responsePhase"] == "commentary"));
    let detail = state
        .get_thread_turn_detail(&thread.id, &turn_id)
        .await
        .unwrap();
    if let Some((_, marker)) = correction {
        assert!(
            detail
                .items
                .iter()
                .any(|item| item.kind == "agentMessage" && item.text.contains(marker)),
            "waiting correction response persisted in original turn"
        );
    }
    assert_eq!(detail.status, "completed");
    assert!(
        detail.items.iter().any(|i| i.text.contains(final_marker)),
        "final report persisted before completion"
    );
    assert!(
        detail.items.iter().any(|item| item.kind == "agentMessage"
            && item.text.contains(final_marker)
            && item.extra["responsePhase"] == "final"),
        "native-confirmed final phase survives persistence"
    );
    for id in &wake_ids {
        let notice = detail.items.iter().find(|i| &i.id == id).unwrap();
        assert_eq!(notice.extra["origin"], "nativeTaskNotification");
        assert!(notice.extra["waitingStartedAt"].is_string());
        assert!(notice.extra["awakenedAt"].is_string());
    }
    assert!(detail
        .items
        .iter()
        .all(|i| i.source_turn_id.as_deref() == Some(turn_id.as_str())));
    assert!(
        detail
            .items
            .iter()
            .any(|i| i.kind == "commandExecution" && i.text.contains(command_marker)),
        "post-wake execution persisted"
    );
    assert!(
        detail.token_usage.as_ref().unwrap()["total"]["outputTokens"]
            .as_u64()
            .unwrap()
            > 0
    );
    let turn_count = state
        .db
        .with(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM thread_turns WHERE thread_id=?1",
                [&thread.id],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert_eq!(turn_count, 1);
    println!(
        "{}",
        serde_json::json!({"model":model,"turnCount":turn_count,"items":detail.items.len(),"wakeAnchors":wake_ids,"finalReportSaved":true,"postWakeCommandSaved":true,"usageSaved":true})
    );
}
