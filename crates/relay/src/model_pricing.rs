//! Relay-owned reference prices. Only model names leave a device; usage stays local.
use super::*;
use pockymoe_protocol::pricing::{apply_overrides, match_model, update_override};

const SOURCE: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const REFRESH_SECS: i64 = 6 * 60 * 60;

pub(super) fn ensure_schema(c: &Connection) -> Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS relay_model_pricing (
        id INTEGER PRIMARY KEY CHECK(id=1), catalog TEXT NOT NULL, fetched_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS relay_model_price_overrides (
        user_id TEXT PRIMARY KEY REFERENCES relay_users(id) ON DELETE CASCADE, rates TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS relay_unpriced_models (
        user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
        model TEXT NOT NULL, PRIMARY KEY(user_id,model));",
    )?;
    Ok(())
}

fn bundled() -> Value {
    serde_json::from_str(include_str!("../../../config/codex-model-pricing.json"))
        .expect("bundled prices")
}

fn overrides(c: &Connection, user: &str) -> Result<Value> {
    let raw: Option<String> = c
        .query_row(
            "SELECT rates FROM relay_model_price_overrides WHERE user_id=?1",
            [user],
            |r| r.get(0),
        )
        .optional()?;
    Ok(raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(json!({})))
}

fn snapshot(c: &Connection, user: &str) -> Result<Value> {
    let row: Option<(String, String)> = c
        .query_row(
            "SELECT catalog,fetched_at FROM relay_model_pricing WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let mut catalog = bundled();
    if let Some((raw, at)) = row {
        let live: Value = serde_json::from_str(&raw)?;
        for (id, rates) in live["models"].as_object().into_iter().flatten() {
            let mut rates = rates.clone();
            if let Some(aliases) = catalog["models"][id]["aliases"].as_array() {
                let imported = rates["aliases"].as_array_mut().expect("imported aliases");
                for alias in aliases {
                    if !imported.contains(alias) {
                        imported.push(alias.clone());
                    }
                }
            }
            catalog["models"][id] = rates;
        }
        catalog["syncedAt"] = json!(at);
        catalog["source"] = json!("litellm");
    }
    apply_overrides(&mut catalog, &overrides(c, user)?);
    let mut stmt =
        c.prepare("SELECT model FROM relay_unpriced_models WHERE user_id=?1 ORDER BY model")?;
    for name in stmt.query_map([user], |r| r.get::<_, String>(0))? {
        let name = name?;
        if match_model(&catalog, &name).is_none() {
            catalog["models"][&name] = json!({"unpriced":true});
        }
    }
    let mut content = catalog.clone();
    content.as_object_mut().unwrap().remove("syncedAt");
    let version = hex_digest(&content.to_string());
    catalog["version"] = json!(version);
    catalog["scope"] = json!("account");
    Ok(catalog)
}

fn hex_digest(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}

/// Convert only first-party text prices, never a reseller's price for that model.
fn import_litellm(raw: &Value) -> Result<Value> {
    let mut models = serde_json::Map::new();
    for (key, entry) in raw.as_object().context("Invalid LiteLLM catalog")? {
        let provider = entry["litellm_provider"].as_str().unwrap_or("");
        if ![
            "openai",
            "anthropic",
            "gemini",
            "xai",
            "deepseek",
            "zai",
            "bigmodel",
            "moonshot",
            "minimax",
            "mistral",
            "codestral",
            "cohere",
            "cohere_chat",
            "ai21",
            "perplexity",
            "dashscope",
            "qwen_ai_platform",
            "xiaomi_mimo",
            "inception",
            "morph",
            "meta",
        ]
        .contains(&provider)
        {
            continue;
        }
        if !matches!(entry["mode"].as_str(), Some("chat" | "completion")) {
            continue;
        }
        let id = key.strip_prefix(&format!("{provider}/")).unwrap_or(key);
        if ["dashscope", "qwen_ai_platform"].contains(&provider)
            && !["qwen", "qwq", "qvq"]
                .iter()
                .any(|prefix| id.to_lowercase().starts_with(prefix))
        {
            continue;
        }
        if id.contains('/') || id.is_empty() || id.len() > 160 {
            continue;
        }
        let convert = |field: &str| {
            entry[field]
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0)
                .map(|v| json!((v * 1e6 * 1e12).round() / 1e12))
        };
        let (Some(input), Some(output)) = (
            convert("input_cost_per_token"),
            convert("output_cost_per_token"),
        ) else {
            continue;
        };
        let mut rates = json!({"inputUsdPerMillion":input,"outputUsdPerMillion":output,
            "cachedInputUsdPerMillion":convert("cache_read_input_token_cost").unwrap_or(input.clone()),
            "aliases":[key],"sourceUrl":SOURCE});
        let fields = [
            ("input_cost_per_token", "inputUsdPerMillion"),
            ("output_cost_per_token", "outputUsdPerMillion"),
            ("cache_read_input_token_cost", "cachedInputUsdPerMillion"),
            (
                "cache_creation_input_token_cost",
                "cacheWriteInputUsdPerMillion",
            ),
            (
                "cache_creation_input_token_cost_above_1hr",
                "cacheWriteOneHourInputUsdPerMillion",
            ),
        ];
        for (source, target) in fields {
            if let Some(v) = convert(source) {
                rates[target] = v;
            }
        }
        let mut tiers = std::collections::BTreeMap::<u64, Value>::new();
        for (field, _) in entry.as_object().into_iter().flatten() {
            for (source, target) in fields {
                if let Some(n) = field
                    .strip_prefix(&format!("{source}_above_"))
                    .and_then(|v| v.strip_suffix("_tokens"))
                {
                    let threshold = n
                        .strip_suffix('k')
                        .and_then(|n| n.parse::<u64>().ok())
                        .map(|n| n * 1000)
                        .or_else(|| n.parse().ok());
                    if let (Some(threshold), Some(price)) = (threshold, convert(field)) {
                        tiers
                            .entry(threshold)
                            .or_insert_with(|| json!({"aboveTokens":threshold}))[target] = price;
                    }
                }
            }
        }
        if !tiers.is_empty() {
            rates["contextTiers"] = json!(tiers.into_values().collect::<Vec<_>>());
        }
        let mut fast = json!({});
        for (source, target) in fields {
            if let Some(v) = convert(&format!("{source}_priority")) {
                fast[target] = v;
            }
        }
        if !fast.as_object().unwrap().is_empty() {
            rates["serviceTiers"] = json!({"fast":fast});
        }
        // Prefer the direct unprefixed official entry if the source duplicates it.
        if key == id || !models.contains_key(id) {
            models.insert(id.into(), rates);
        }
    }
    anyhow::ensure!(
        !models.is_empty(),
        "LiteLLM returned no usable first-party prices"
    );
    Ok(json!({"currency":"USD","models":models}))
}

pub(super) fn start(state: &Arc<AppState>) {
    let weak = Arc::downgrade(state);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60 * 60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let Some(state) = weak.upgrade() else { break };
            if let Err(error) = refresh(&state).await {
                tracing::warn!(%error,"model price refresh failed; keeping previous prices");
            }
        }
    });
}

async fn refresh(state: &AppState) -> Result<()> {
    let last: Option<String> = state
        .store
        .conn
        .lock()
        .await
        .query_row(
            "SELECT fetched_at FROM relay_model_pricing WHERE id=1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if last
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(&v).ok())
        .is_some_and(|at| {
            (chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_seconds() < REFRESH_SECS
        })
    {
        return Ok(());
    }
    let source = std::env::var("POCKYMOE_MODEL_PRICING_URL").unwrap_or_else(|_| SOURCE.into());
    refresh_from(state, &source).await
}

async fn refresh_from(state: &AppState, source: &str) -> Result<()> {
    let response = state
        .oauth_client
        .get(source)
        .send()
        .await?
        .error_for_status()?;
    anyhow::ensure!(
        response.content_length().unwrap_or(0) <= 32 * 1024 * 1024,
        "Pricing source is too large"
    );
    let bytes = response.bytes().await?;
    anyhow::ensure!(
        bytes.len() <= 32 * 1024 * 1024,
        "Pricing source is too large"
    );
    let catalog = import_litellm(&serde_json::from_slice(&bytes)?)?;
    state.store.conn.lock().await.execute("INSERT INTO relay_model_pricing VALUES(1,?1,?2) ON CONFLICT(id) DO UPDATE SET catalog=excluded.catalog,fetched_at=excluded.fetched_at",params![catalog.to_string(),now_rfc3339()])?;
    notify(state, None).await;
    Ok(())
}

async fn notify(state: &AppState, user: Option<&str>) {
    let ids: Vec<String> = {
        let c = state.store.conn.lock().await;
        let Ok(mut stmt) =
            c.prepare("SELECT id FROM relay_devices WHERE ?1 IS NULL OR owner_user_id=?1")
        else {
            return;
        };
        let rows = stmt.query_map([user], |r| r.get(0));
        rows.map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    };
    let sockets = state.sockets.read().await;
    for id in ids {
        if let Some(s) = sockets.get(&id) {
            let _ = s.tx.try_send(json!({"type":"pricing.changed"}).to_string());
        }
    }
}

pub(super) async fn sync(state: &AppState, device: &str, connection: Uuid, frame: &Value) {
    let result = {
        let c = state.store.conn.lock().await;
        (|| -> Result<Option<Value>> {
            let owner:String=c.query_row("SELECT d.owner_user_id FROM relay_devices d JOIN relay_users u ON u.id=d.owner_user_id WHERE d.id=?1 AND u.enabled=1",[device],|r|r.get(0))?;
            if let Some(names) = frame["models"].as_array() {
                anyhow::ensure!(names.len() <= 10000, "Too many model names");
                for name in names {
                    let name = name
                        .as_str()
                        .filter(|n| {
                            !n.is_empty() && n.len() <= 160 && !n.chars().any(char::is_control)
                        })
                        .context("Invalid model name")?;
                    c.execute(
                        "INSERT OR IGNORE INTO relay_unpriced_models VALUES(?1,?2)",
                        params![owner, name],
                    )?;
                }
            }
            let catalog = snapshot(&c, &owner)?;
            Ok((catalog["version"] != frame["version"]).then_some(catalog))
        })()
    };
    if let Ok(Some(catalog)) = result {
        let sockets = state.sockets.read().await;
        if let Some(s) = sockets
            .get(device)
            .filter(|s| s.connection_id == connection)
        {
            let _ =
                s.tx.try_send(json!({"type":"pricing.snapshot","catalog":catalog}).to_string());
        }
    }
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/relay/account/model-pricing", get(load).patch(save))
}
async fn load(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
) -> Response {
    let c = state.store.conn.lock().await;
    let Some(user) = authenticated_user(&c, &state.store.session_secret, &headers, &query) else {
        return unauthorized();
    };
    match snapshot(&c, &user.id) {
        Ok(v) => ([(header::CACHE_CONTROL, "no-store")], Json(v)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
async fn save(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
    Json(input): Json<Value>,
) -> Response {
    let user_id;
    let catalog = {
        let c = state.store.conn.lock().await;
        let Some(user) = authenticated_user(&c, &state.store.session_secret, &headers, &query)
        else {
            return unauthorized();
        };
        user_id = user.id;
        (|| -> Result<Value> {
            let mut rates = overrides(&c, &user_id)?;
            let mut catalog = snapshot(&c, &user_id)?;
            if let Some(import) = input.get("import") {
                let import = import.as_object().context("Invalid device prices")?;
                anyhow::ensure!(import.len() <= 1000, "Too many device prices");
                for (id, value) in import {
                    // Account choices always survive importing another device.
                    if rates.get(id).is_some() {
                        continue;
                    }
                    update_override(&catalog, &mut rates, &json!({"id":id,"rates":value}))
                        .map_err(anyhow::Error::msg)?;
                    apply_overrides(&mut catalog, &rates);
                }
            } else {
                update_override(&catalog, &mut rates, &input).map_err(anyhow::Error::msg)?;
            }
            c.execute("INSERT INTO relay_model_price_overrides VALUES(?1,?2) ON CONFLICT(user_id) DO UPDATE SET rates=excluded.rates",params![user_id,rates.to_string()])?;
            snapshot(&c, &user_id)
        })()
    };
    match catalog {
        Ok(v) => {
            notify(&state, Some(&user_id)).await;
            ([(header::CACHE_CONTROL, "no-store")], Json(v)).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"code":"invalid_model_pricing","message":e.to_string()})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_pricing_import_uses_official_rates_and_preserves_context_and_cache_rules() {
        let source = json!({
            "future-model":{"litellm_provider":"openai","mode":"chat","input_cost_per_token":0.000002,"output_cost_per_token":0.000008,"cache_read_input_token_cost":0.0000002,
                "input_cost_per_token_above_200k_tokens":0.000004,"output_cost_per_token_above_200k_tokens":0.000012,
                "input_cost_per_token_priority":0.000003},
            "anthropic/claude-next":{"litellm_provider":"anthropic","mode":"chat","input_cost_per_token":0.000003,"output_cost_per_token":0.000015,
                "cache_creation_input_token_cost":0.00000375,"cache_creation_input_token_cost_above_1hr":0.000006},
            "deepinfra/future-model":{"litellm_provider":"deepinfra","mode":"chat","input_cost_per_token":0.00000001,"output_cost_per_token":0.00000001},
            "openai/missing":{"litellm_provider":"openai","mode":"chat","input_cost_per_token":0.001},
            "invalid":{"litellm_provider":"openai","mode":"chat","input_cost_per_token":-0.1,"output_cost_per_token":0.1},
            "image":{"litellm_provider":"openai","mode":"image_generation","input_cost_per_token":0.001,"output_cost_per_token":0.01}
        });
        let catalog = import_litellm(&source).unwrap();
        assert_eq!(catalog["models"].as_object().unwrap().len(), 2);
        let rates = &catalog["models"]["future-model"];
        assert_eq!(rates["inputUsdPerMillion"], 2.0);
        assert_eq!(rates["cachedInputUsdPerMillion"], 0.2);
        assert_eq!(rates["contextTiers"][0]["aboveTokens"], 200000);
        assert_eq!(rates["contextTiers"][0]["outputUsdPerMillion"], 12.0);
        assert_eq!(rates["serviceTiers"]["fast"]["inputUsdPerMillion"], 3.0);
        assert_eq!(
            catalog["models"]["claude-next"]["cacheWriteOneHourInputUsdPerMillion"],
            6.0
        );
        assert!(import_litellm(&json!({"error":"unavailable"})).is_err());
    }

    #[tokio::test]
    async fn model_pricing_refresh_persists_valid_feeds_and_keeps_last_good_prices_on_failure() {
        let (state, dir) = crate::tests::test_app_state("model-pricing-refresh");
        let body = Arc::new(std::sync::Mutex::new(
            json!({"future-model":{"litellm_provider":"openai","mode":"chat","input_cost_per_token":0.000002,"output_cost_per_token":0.000008}}),
        ));
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/prices", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/prices",
            get({
                let body = body.clone();
                let requests = requests.clone();
                move || {
                    let body = body.lock().unwrap().clone();
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    async move { Json(body) }
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        refresh_from(&state, &url).await.unwrap();
        let saved: String = state
            .store
            .conn
            .lock()
            .await
            .query_row("SELECT catalog FROM relay_model_pricing", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&saved).unwrap()["models"]["future-model"]
                ["inputUsdPerMillion"],
            2.0
        );
        // A fresh persisted feed also suppresses a startup refresh after restart.
        refresh(&state).await.unwrap();
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        *body.lock().unwrap() = json!({"error":"temporarily unavailable"});
        assert!(refresh_from(&state, &url).await.is_err());
        let retained: String = state
            .store
            .conn
            .lock()
            .await
            .query_row("SELECT catalog FROM relay_model_pricing", [], |r| r.get(0))
            .unwrap();
        assert_eq!(retained, saved);
        server.abort();
        let _ = server.await;
        assert!(refresh_from(&state, &url).await.is_err());
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn model_pricing_accounts_sync_unknown_names_override_reset_and_import_without_clobber() {
        let (state, dir) = crate::tests::test_app_state("model-pricing");
        let (owner, other) = {
            let c = state.store.conn.lock().await;
            c.execute_batch("INSERT INTO relay_users VALUES ('owner','o@test','owner','user',1,NULL,'now','salt','hash'),('other','b@test','other','user',1,NULL,'now','salt','hash');
                INSERT INTO relay_devices(id,owner_user_id,name,token_hash,token_preview,created_at) VALUES('a','owner','A','a','a','now'),('b','owner','B','b','b','now'),('c','other','C','c','c','now');").unwrap();
            (
                create_session(&c, &state.store.session_secret, "owner").unwrap(),
                create_session(&c, &state.store.session_secret, "other").unwrap(),
            )
        };
        let mut channels = Vec::new();
        for id in ["a", "b", "c"] {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let connection = Uuid::new_v4();
            state.sockets.write().await.insert(
                id.into(),
                DeviceSocket {
                    tx,
                    connection_id: connection,
                    connected_at: "now".into(),
                    last_heartbeat_at: "now".into(),
                },
            );
            channels.push((connection, rx));
        }
        sync(
            &state,
            "a",
            channels[0].0,
            &json!({"models":["future-model"],"version":null}),
        )
        .await;
        let first: Value = serde_json::from_str(&channels[0].1.recv().await.unwrap()).unwrap();
        assert_eq!(first["catalog"]["models"]["future-model"]["unpriced"], true);
        assert!(
            snapshot(&*state.store.conn.lock().await, "other").unwrap()["models"]["future-model"]
                .is_null()
        );
        sync(
            &state,
            "a",
            channels[0].0,
            &json!({"version":first["catalog"]["version"]}),
        )
        .await;
        assert!(channels[0].1.try_recv().is_err());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/relay/account/model-pricing",
            listener.local_addr().unwrap()
        );
        let app = routes().with_state(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        assert_eq!(
            client.get(&url).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let rates =
            json!({"inputUsdPerMillion":2,"cachedInputUsdPerMillion":0.2,"outputUsdPerMillion":8});
        let saved = client
            .patch(&url)
            .bearer_auth(&owner)
            .json(&json!({"id":"future-model","rates":rates}))
            .send()
            .await
            .unwrap();
        assert_eq!(saved.status(), StatusCode::OK);
        assert_eq!(
            saved.json::<Value>().await.unwrap()["models"]["future-model"]["custom"],
            true
        );
        for (_, rx) in &mut channels[..2] {
            assert!(rx.try_recv().unwrap().contains("pricing.changed"));
        }
        assert!(channels[2].1.try_recv().is_err());
        let stranger: Value = client
            .get(&url)
            .bearer_auth(&other)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(stranger["models"]["future-model"].is_null());
        let altered =
            json!({"inputUsdPerMillion":9,"cachedInputUsdPerMillion":1,"outputUsdPerMillion":9});
        let imported: Value = client
            .patch(&url)
            .bearer_auth(&owner)
            .json(&json!({"import":{"future-model":altered,"local-only":rates}}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(imported["models"]["future-model"]["inputUsdPerMillion"], 2);
        assert_eq!(imported["models"]["local-only"]["custom"], true);
        let reset: Value = client
            .patch(&url)
            .bearer_auth(&owner)
            .json(&json!({"id":"future-model","reset":true}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(reset["models"]["future-model"]["unpriced"], true);
        // A later feed fills the pending name for both of the owner's devices.
        state.store.conn.lock().await.execute("INSERT INTO relay_model_pricing VALUES(1,?1,'2030-01-01T00:00:00Z')",[json!({"models":{"future-model":{"inputUsdPerMillion":3,"cachedInputUsdPerMillion":0.3,"outputUsdPerMillion":12,"aliases":[]}}}).to_string()]).unwrap();
        sync(
            &state,
            "b",
            channels[1].0,
            &json!({"version":first["catalog"]["version"]}),
        )
        .await;
        let mut latest = None;
        while let Ok(msg) = channels[1].1.try_recv() {
            let msg: Value = serde_json::from_str(&msg).unwrap();
            if msg["type"] == "pricing.snapshot" {
                latest = Some(msg);
            }
        }
        assert_eq!(
            latest.unwrap()["catalog"]["models"]["future-model"]["inputUsdPerMillion"],
            3
        );
        let version =
            snapshot(&*state.store.conn.lock().await, "owner").unwrap()["version"].clone();
        state
            .store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE relay_model_pricing SET fetched_at='2031-01-01T00:00:00Z'",
                [],
            )
            .unwrap();
        assert_eq!(
            snapshot(&*state.store.conn.lock().await, "owner").unwrap()["version"],
            version
        );
        server.abort();
        let _ = server.await;
        drop(state);
        let reopened =
            RelayStore::open(dir.join("relay-store.sqlite"), "test-secret".into()).unwrap();
        assert_eq!(
            snapshot(&*reopened.conn.lock().await, "owner").unwrap()["models"]["local-only"]
                ["custom"],
            true
        );
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
