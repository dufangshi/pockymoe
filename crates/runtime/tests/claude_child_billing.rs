use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Provider, ThreadEventEnvelope};
use pockymoe_runtime::{fake::FakeRuntime, Database, RuntimeConfig, Supervisor};
use serde_json::{json, Value};
use std::sync::Arc;

#[tokio::test]
async fn child_request_prices_survive_updates_final_fallback_and_database_reload() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = RuntimeConfig::from_env().unwrap();
    config.database_url = dir.path().join("billing.sqlite");
    config.workspace_root = dir.path().join("workspaces");
    config.fake_runtime = true;
    config.relay_server_url = None;
    config.relay_agent_token = None;
    let state = Arc::new(Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![Arc::new(FakeRuntime::new(Provider::Claude))],
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
            model: "claude-opus-5-5".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: None,
            title: None,
        })
        .await
        .unwrap();
    state.db.with(|conn| {
        conn.execute("INSERT INTO thread_turns(id,thread_id,status,model,started_at,ordinal) VALUES('bill',?1,'inProgress','claude-opus-5-5','2030-01-01T00:00:00Z',1)", [&thread.id])?;
        Ok(())
    }).unwrap();
    let emit = |usage: Value| {
        state.bus.emit(ThreadEventEnvelope {
            event_type: "runtime.usage.updated".into(),
            thread_id: thread.id.clone(),
            timestamp: "2030-01-01T00:01:00Z".into(),
            payload: json!({"turnId":"bill","usage":usage}),
        })
    };
    let stored = || {
        state
            .db
            .with(|conn| {
                let raw: String = conn.query_row(
                    "SELECT token_usage_json FROM thread_turns WHERE id='bill'",
                    [],
                    |row| row.get(0),
                )?;
                Ok(serde_json::from_str::<Value>(&raw).unwrap())
            })
            .unwrap()
    };
    let parent = json!({"messageId":"parent","model":"claude-opus-5-5",
        "total":{"inputTokens":1000,"outputTokens":100},"last":{"inputTokens":1000,"outputTokens":100}});
    let mut child = json!({"messageId":"child","model":"claude-haiku-4-5",
        "total":{"inputTokens":1000,"outputTokens":100},"last":{"inputTokens":1000,"outputTokens":100}});
    emit(
        json!({"source":"claudeRollout","model":"claude-opus-5-5","total":parent["total"],"last":parent["last"],"requests":[parent]}),
    );
    let parent_cost = stored()["priceEstimate"]["totalUsd"].as_f64().unwrap();
    let catalog: Value =
        serde_json::from_str(include_str!("../../../config/codex-model-pricing.json")).unwrap();
    let rates = &catalog["models"]["claude-haiku-4-5"];
    let expected_child = (1000.0 * rates["inputUsdPerMillion"].as_f64().unwrap()
        + 100.0 * rates["outputUsdPerMillion"].as_f64().unwrap())
        / 1e6;
    let snapshot = |child: &Value, output: u64| {
        json!({"source":"claudeRollout","model":"claude-opus-5-5",
        "total":{"inputTokens":2000,"outputTokens":output},"last":parent["last"],"requests":[parent,child]})
    };
    emit(snapshot(&child, 200));
    let mixed = stored();
    assert!(
        (mixed["priceEstimate"]["totalUsd"].as_f64().unwrap() - parent_cost - expected_child).abs()
            < 1e-12
    );
    emit(snapshot(&child, 200));
    assert_eq!(stored()["priceEstimate"], mixed["priceEstimate"]);
    child["total"]["outputTokens"] = json!(200);
    child["last"]["outputTokens"] = json!(200);
    emit(snapshot(&child, 300));
    let final_usage = stored();
    let expected =
        parent_cost + expected_child + 100.0 * rates["outputUsdPerMillion"].as_f64().unwrap() / 1e6;
    assert!((final_usage["priceEstimate"]["totalUsd"].as_f64().unwrap() - expected).abs() < 1e-12);
    assert_eq!(final_usage["requests"].as_array().unwrap().len(), 2);
    emit(json!({"inputTokens":1,"outputTokens":1}));
    assert_eq!(stored()["total"], final_usage["total"]);
    state.bus.emit(ThreadEventEnvelope {
        event_type: "thread.turn.completed".into(),
        thread_id: thread.id.clone(),
        timestamp: "2030-01-01T00:02:00Z".into(),
        payload: json!({"turnId":"bill","status":"completed"}),
    });
    drop(state);
    let reopened = Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![Arc::new(FakeRuntime::new(Provider::Claude))],
    );
    let detail = reopened.get_thread_detail(&thread.id, None).await.unwrap();
    let turn = &detail.turns[0];
    assert_eq!(turn.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(
        turn.token_usage.as_ref().unwrap()["total"]["outputTokens"],
        300
    );
    assert!(
        (turn.price_estimate.as_ref().unwrap()["totalUsd"]
            .as_f64()
            .unwrap()
            - expected)
            .abs()
            < 1e-12
    );
}
