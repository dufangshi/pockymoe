use pockymoe_protocol::Mode;
use pockymoe_runtime::{Database, RuntimeConfig, Supervisor};
use serde_json::{json, Value};

#[tokio::test]
async fn unknown_historical_usage_is_repriced_without_rewriting_tokens_and_cache_survives_restart()
{
    let dir = tempfile::tempdir().unwrap();
    let mut config = RuntimeConfig::from_env().unwrap();
    config.mode = Mode::Relay;
    config.database_url = dir.path().join("pricing.sqlite");
    config.workspace_root = dir.path().join("workspaces");
    config.relay_server_url = Some("https://pricing.test".into());
    config.relay_agent_token = Some("pricing-fixture".into());
    let state = Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![],
    );
    let usage=json!({"total":{"inputTokens":1000000,"cachedInputTokens":200000,"outputTokens":100000,"totalTokens":1100000},
        "priceEstimate":{"totalUsd":999}}).to_string();
    state.db.with(|c| {
        c.execute_batch("INSERT INTO workspaces(id,host_id,label,abs_path,created_at) VALUES('ws','host','Workspace','/isolated','now');
            INSERT INTO threads(id,workspace_id,provider,title,created_at,updated_at,status) VALUES('thread','ws','codex','Thread','now','now','idle');")?;
        c.execute("INSERT INTO thread_turns(id,thread_id,status,started_at,completed_at,ordinal,model,token_usage_json) VALUES('turn','thread','completed','2026-01-01T00:00:00Z','2026-01-01T00:01:00Z',1,'future-model',?1)",[&usage])?;
        Ok(())
    }).unwrap();
    let frame = state.model_pricing_sync(true, None).unwrap();
    assert_eq!(frame["models"], json!(["future-model"]));
    let pending = json!({"version":"pending","models":{"future-model":{"unpriced":true}}});
    state.accept_relay_model_pricing(&pending).unwrap();
    let stats = state.stats_summary("all", "UTC").unwrap();
    assert_eq!(stats["totals"]["unpricedTokens"], 1100000);
    assert!(stats["totals"]["costUsd"].is_null());
    let catalog = json!({"version":"official-v1","models":{"future-model":{"inputUsdPerMillion":2,"cachedInputUsdPerMillion":0.2,"outputUsdPerMillion":8}}});
    assert!(state.accept_relay_model_pricing(&catalog).unwrap());
    assert!(!state.accept_relay_model_pricing(&catalog).unwrap());
    let stats = state.stats_summary("all", "UTC").unwrap();
    assert_eq!(stats["totals"]["unpricedTokens"], 0);
    assert!((stats["totals"]["costUsd"].as_f64().unwrap() - 2.44).abs() < 1e-10);
    assert_eq!(stats["totals"]["tokens"], 1100000);
    let mut revised = catalog.clone();
    revised["version"] = json!("account-override");
    revised["models"]["future-model"]["inputUsdPerMillion"] = json!(4);
    state.accept_relay_model_pricing(&revised).unwrap();
    assert!(
        (state.stats_summary("all", "UTC").unwrap()["totals"]["costUsd"]
            .as_f64()
            .unwrap()
            - 4.04)
            .abs()
            < 1e-10
    );
    state
        .db
        .with(|c| {
            let saved: String = c.query_row(
                "SELECT token_usage_json FROM thread_turns WHERE id='turn'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(saved, usage);
            Ok(())
        })
        .unwrap();
    drop(state);
    let reopened = Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![],
    );
    assert_eq!(reopened.model_pricing()["version"], "account-override");
    assert_eq!(
        reopened.model_pricing_sync(false, None).unwrap()["version"],
        "account-override"
    );
    drop(reopened);
    config.relay_agent_token = Some("different-account".into());
    let moved = Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![],
    );
    assert!(moved.model_pricing()["models"]["future-model"].is_null());
    let empty: Value = moved.model_pricing_sync(false, None).unwrap();
    assert!(empty["version"].is_null());
}
