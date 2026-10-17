//! Owner-managed API connections and explicitly authorized, encrypted device pulls.
use crate::{
    auth::PeerCaller,
    owner_trust::{self, BrowserPrincipal, EncryptedOwnerRequest},
};
use anyhow::{anyhow, ensure, Result};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    Extension, Json,
};
use pockymoe_runtime::{
    upstreams::{self, connections as saved},
    Supervisor,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type Failure = (StatusCode, Json<Value>);
fn failure(e: impl std::fmt::Display) -> Failure {
    let message = e.to_string();
    let lower = message.to_ascii_lowercase();
    let (status, code) = if lower.contains("in use") {
        (StatusCode::CONFLICT, "connection_in_use")
    } else if lower.contains("busy") && lower.contains("harness") {
        (StatusCode::CONFLICT, "harness_busy")
    } else if lower.contains("edit conflict") {
        (StatusCode::CONFLICT, "edit_conflict")
    } else if lower.contains("sync conflict") {
        (StatusCode::CONFLICT, "sync_conflict")
    } else if lower.contains("version conflict") {
        (StatusCode::CONFLICT, "revision_mismatch")
    } else if lower.contains("conflict") {
        (StatusCode::CONFLICT, "edit_conflict")
    } else if lower.contains("offline") {
        (StatusCode::SERVICE_UNAVAILABLE, "source_offline")
    } else if lower.contains("peer_access_disabled") || lower.contains("device-to-device access") {
        (StatusCode::FORBIDDEN, "peer_access_disabled")
    } else if lower.contains("transfer_token_invalid") {
        (StatusCode::FORBIDDEN, "transfer_token_invalid")
    } else if lower.contains("bind this device") || lower.contains("peer_target_unverified") {
        (StatusCode::FORBIDDEN, "device_not_certified")
    } else if lower.contains("incompatible") {
        (StatusCode::BAD_REQUEST, "protocol_unsupported")
    } else {
        (StatusCode::BAD_REQUEST, "connection_error")
    };
    (status, Json(json!({"code":code,"message":message})))
}
fn denied() -> Failure {
    (
        StatusCode::FORBIDDEN,
        Json(
            json!({"code":"owner_required","message":"An encrypted account owner request is required."}),
        ),
    )
}
fn dir(s: &Supervisor) -> std::path::PathBuf {
    upstreams::directory(&s.config.database_url)
}
pub(crate) fn snapshot(s: &Supervisor) -> Value {
    let Ok(_g) = s.upstream_gate.try_lock() else {
        return Value::Null;
    };
    if saved::migrate(&dir(s)).is_err() {
        return Value::Null;
    }
    saved::metadata(&dir(s)).unwrap_or(Value::Null)
}
pub(crate) fn announce(s: &Supervisor) {
    if let Ok(rows) = saved::metadata(&dir(s)) {
        crate::peer_link::announce_upstreams(s, rows);
    }
}
pub(crate) async fn no_store(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut r = next.run(req).await;
    r.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        "no-store".parse().unwrap(),
    );
    r
}
pub async fn list(State(s): State<Arc<Supervisor>>) -> Result<Json<Value>, Failure> {
    let mut result = {
        let _g = s.upstream_gate.lock().await;
        saved::migrate(&dir(&s)).map_err(failure)?;
        announce(&s);
        saved::list(&dir(&s)).map_err(failure)?
    };
    let identity = crate::peer_link::relay_identity(&s);
    result["deviceId"] = json!(identity.as_ref().map(|i| &i.device_id));
    result["catalogAvailable"] = json!(false);
    result["harnessProtocols"] = json!({"codex":["openai_responses"],"claude":["anthropic"],"gemini":["gemini"],"grok":["openai_responses","openai_chat"],"deepseek":["anthropic","openai_chat"]});
    if s.relay_connected.load(std::sync::atomic::Ordering::SeqCst) {
        match tokio::time::timeout(
            std::time::Duration::from_secs(3),
            crate::peer_link::upstream_directory(&s),
        )
        .await
        {
            Ok(Ok(rows)) => {
                merge_catalog(
                    &mut result,
                    rows,
                    identity.as_ref().map(|i| i.device_id.as_str()),
                );
                result["catalogAvailable"] = json!(true);
            }
            _ => {
                result["catalogError"] =
                    json!("Account connection catalog is temporarily unavailable");
            }
        }
    }
    Ok(Json(result))
}
fn merge_catalog(result: &mut Value, rows: Vec<Value>, self_id: Option<&str>) {
    let entries = result["connections"].as_array_mut().unwrap();
    for row in rows {
        if row["deviceId"].as_str() == self_id {
            continue;
        }
        let Some(id) = row["id"].as_str() else {
            continue;
        };
        if !entries.iter().any(|v| v["id"] == id) {
            entries.push(json!({"id":id,"name":row["name"],"protocol":row["protocol"],"revision":row["revision"],"local":false,"sources":[]}));
        }
        let item = entries.iter_mut().find(|v| v["id"] == id).unwrap();
        if item["revision"] != row["revision"] {
            item["conflict"] = json!(true);
        }
        item["sources"].as_array_mut().unwrap().push(json!({"deviceId":row["deviceId"],"name":row["deviceName"],"online":row["online"],"revision":row["revision"]}));
    }
}
// Missing headers preserve private values on an unchanged saved connection;
// explicit {} removes them. This lets editors keep secrets out of the browser.
fn draft(s: &Supervisor, mut input: Value) -> Result<saved::Connection, Failure> {
    if input.get("headers").is_none() {
        if let Some(id) = input["id"].as_str().filter(|id| !id.is_empty()) {
            let old = saved::get(&dir(s), id).map_err(failure)?;
            if input["baseUrl"] == old.base_url && input["protocol"] == old.protocol {
                input["headers"] = json!(old.headers);
            }
        }
    }
    serde_json::from_value(input).map_err(|_| failure("Invalid connection draft"))
}
pub async fn save(
    State(s): State<Arc<Supervisor>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, Failure> {
    let _g = s.upstream_gate.lock().await;
    saved::migrate(&dir(&s)).map_err(failure)?;
    let c = draft(&s, input)?;
    let value = saved::upsert(&dir(&s), c).map_err(failure)?;
    announce(&s);
    Ok(Json(value))
}
pub async fn remove(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Failure> {
    let _g = s.upstream_gate.lock().await;
    let plugin = s
        .plugin_settings("pockymoe.content-quality")
        .map_err(failure)?;
    if plugin.values["connectionId"] == id {
        return Err(failure("Connection is in use by Content Quality"));
    }
    saved::remove(&dir(&s), &id).map_err(failure)?;
    announce(&s);
    Ok(Json(json!({"ok":true})))
}
pub async fn models(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Failure> {
    let c = {
        let _g = s.upstream_gate.lock().await;
        saved::get(&dir(&s), &id).map_err(failure)?
    };
    let p = c.discovery_profile().map_err(failure)?;
    upstreams::discover_models(&p)
        .await
        .map(Json)
        .map_err(failure)
}
pub async fn discover(
    State(s): State<Arc<Supervisor>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, Failure> {
    let c = {
        let _g = s.upstream_gate.lock().await;
        saved::prepare(&dir(&s), draft(&s, input)?).map_err(failure)?
    };
    let p = c.discovery_profile().map_err(failure)?;
    upstreams::discover_models(&p)
        .await
        .map(Json)
        .map_err(failure)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelInput {
    #[serde(default)]
    model: String,
}
pub async fn test(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Json(input): Json<ModelInput>,
) -> Result<Json<Value>, Failure> {
    let c = {
        let _g = s.upstream_gate.lock().await;
        saved::get(&dir(&s), &id).map_err(failure)?
    };
    let start = std::time::Instant::now();
    saved::complete(
        &c,
        &input.model,
        "Reply briefly.",
        "Reply OK",
        saved::CONNECTION_PROBE_MAX_TOKENS,
    )
    .await
    .map_err(failure)?;
    Ok(Json(
        json!({"ok":true,"latencyMs":start.elapsed().as_millis(),"model":if input.model.is_empty(){&c.model}else{&input.model}}),
    ))
}
pub async fn test_draft(
    State(s): State<Arc<Supervisor>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, Failure> {
    let c = {
        let _g = s.upstream_gate.lock().await;
        saved::prepare(&dir(&s), draft(&s, input)?).map_err(failure)?
    };
    let start = std::time::Instant::now();
    saved::complete(
        &c,
        "",
        "Reply briefly.",
        "Reply OK",
        saved::CONNECTION_PROBE_MAX_TOKENS,
    )
    .await
    .map_err(failure)?;
    Ok(Json(
        json!({"ok":true,"latencyMs":start.elapsed().as_millis(),"model":c.model}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Apply {
    harness: String,
    #[serde(default)]
    model: String,
}
pub async fn apply(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Json(input): Json<Apply>,
) -> Result<Json<Value>, Failure> {
    let _g = s.upstream_gate.lock().await;
    let root = dir(&s);
    let c = saved::get(&root, &id).map_err(failure)?;
    c.validate_application(&input.harness).map_err(failure)?;
    let _maintenance = s
        .maintenance_gate
        .clone()
        .try_read_owned()
        .map_err(|_| failure("Harness is busy: apply conflict"))?;
    let _guard = s
        .harness_gate(&input.harness)
        .try_write_owned()
        .map_err(|_| failure("Harness is busy: apply conflict"))?;
    let p = c
        .profile(&input.harness, "", &input.model)
        .map_err(failure)?;
    upstreams::validate(&p, true).map_err(failure)?;
    let restarted = s.restart_harness(&input.harness).await.map_err(failure)?;
    let mut result = saved::apply(&root, &c, &input.harness, &input.model).map_err(failure)?;
    result["restartedSessions"] = json!(restarted);
    s.invalidate_models(&input.harness).await;
    Ok(Json(result))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Authorize {
    target_device_id: String,
    revision: String,
}
fn verified_device(s: &Supervisor) -> Result<String> {
    let cert = owner_trust::device_certificate(s)?.ok_or_else(|| {
        anyhow!("Bind this device to the account before synchronizing credentials")
    })?;
    owner_trust::verify_device_certificate(s, &cert, &cert.device_id, &cert.identity_key)?;
    Ok(cert.device_id)
}
pub async fn authorize(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    encrypted: Option<Extension<EncryptedOwnerRequest>>,
    owner: Option<Extension<BrowserPrincipal>>,
    Json(input): Json<Authorize>,
) -> Result<Json<Value>, Failure> {
    let Some(Extension(owner)) = owner else {
        return Err(denied());
    };
    if encrypted.is_none() || !owner.is_owner() || !owner.valid(&s) {
        return Err(denied());
    }
    let device = verified_device(&s).map_err(failure)?;
    if !s.peer_access_enabled() {
        return Err(failure(
            "Enable device-to-device access on the source device before synchronizing",
        ));
    }
    uuid::Uuid::parse_str(&input.target_device_id).map_err(|_| failure("Invalid target device"))?;
    if device == input.target_device_id {
        return Err(failure("The source and target devices must differ"));
    }
    let _g = s.upstream_gate.lock().await;
    let c = saved::get(&dir(&s), &id).map_err(failure)?;
    if c.revision != input.revision {
        return Err(failure("Connection version conflict: refresh the list"));
    }
    let token = uuid::Uuid::new_v4().to_string() + &uuid::Uuid::new_v4().to_string();
    let expires = owner_trust::now() + 120_000;
    let grant = json!({"connectionId":id,"revision":c.revision,"targetDeviceId":input.target_device_id,"expiresAt":expires,"browserId":owner.browser_id,"accountKeyId":owner.account_key_id});
    s.db.with(|db|{
        db.execute("DELETE FROM kv WHERE key LIKE 'upstream-transfer:%' AND CAST(json_extract(value,'$.expiresAt') AS INTEGER) < ?1",[owner_trust::now()])?;
        let count:i64=db.query_row("SELECT COUNT(*) FROM kv WHERE key LIKE 'upstream-transfer:%'",[],|r|r.get(0))?;
        ensure!(count<64,"Too many pending credential transfers");
        db.execute("INSERT INTO kv(key,value) VALUES(?1,?2)",rusqlite::params![format!("upstream-transfer:{}",owner_trust::hash(&token)),grant.to_string()])?;Ok(())
    }).map_err(failure)?;
    Ok(Json(json!({"token":token,"expiresAt":expires})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pull {
    token: String,
    revision: String,
}
/// Only a certified peer with a fresh owner-issued, target-scoped capability can export a key.
pub async fn export(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Extension(peer): Extension<PeerCaller>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    Json(input): Json<Pull>,
) -> Result<Json<Value>, Failure> {
    verified_device(&s).map_err(failure)?;
    if !s.peer_access_enabled()
        || owner_trust::device_revoked(&s, &peer.device_id).map_err(failure)?
    {
        return Err(denied());
    }
    if input.token.len() > 256 {
        return Err(denied());
    }
    let _g = s.upstream_gate.lock().await;
    let c = saved::get(&dir(&s), &id).map_err(failure)?;
    consume_grant(&s,&peer.device_id,&id,&c.revision,&input).map_err(|_|(StatusCode::FORBIDDEN,Json(json!({"code":"transfer_token_invalid","message":"Transfer authorization is invalid or expired. Retry synchronization."}))))?;
    Ok(Json(serde_json::to_value(c).map_err(failure)?))
}
fn consume_grant(
    s: &Supervisor,
    target: &str,
    id: &str,
    revision: &str,
    input: &Pull,
) -> Result<()> {
    s.db.with(|db|{
        let tx=db.unchecked_transaction()?;
        let key=format!("upstream-transfer:{}",owner_trust::hash(&input.token));
        let raw:String=tx.query_row("SELECT value FROM kv WHERE key=?1",[&key],|r|r.get(0))?;
        let g:Value=serde_json::from_str(&raw)?;
        ensure!(g["targetDeviceId"]==target && g["connectionId"]==id && g["revision"]==revision && input.revision==revision && g["expiresAt"].as_i64().is_some_and(|t|t>owner_trust::now()),"Invalid transfer authorization");
        // Re-evaluate owner authority at consumption, including browser revocation.
        let browser=BrowserPrincipal{browser_id:g["browserId"].as_str().unwrap_or("").into(),account_key_id:g["accountKeyId"].as_str().unwrap_or("").into(),grantee_user_id:None};
        // This function already holds a database transaction; validate against its tables.
        let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM browser_registrations b JOIN device_owner_trust d ON d.account_key_id=b.account_key_id WHERE b.browser_id=?1 AND b.account_key_id=?2 AND b.role='owner' AND NOT EXISTS(SELECT 1 FROM account_revocations r WHERE r.account_key_id=b.account_key_id AND r.kind='browser' AND r.subject_id=b.browser_id))",rusqlite::params![browser.browser_id,browser.account_key_id],|r|r.get(0))?;
        ensure!(valid,"Owner authority expired");
        tx.execute("DELETE FROM kv WHERE key=?1",[key])?;tx.commit()?;Ok(())
    })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Sync {
    source_device_id: String,
    revision: String,
    token: String,
    expected_revision: Option<String>,
}
pub async fn sync(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Json(input): Json<Sync>,
) -> Result<Json<Value>, Failure> {
    let self_id = verified_device(&s).map_err(failure)?;
    uuid::Uuid::parse_str(&id).map_err(|_| failure("Invalid connection ID"))?;
    uuid::Uuid::parse_str(&input.source_device_id).map_err(|_| failure("Invalid source device"))?;
    if self_id == input.source_device_id || input.token.len() > 256 {
        return Err(failure("Invalid credential transfer"));
    }
    let raw = crate::peer_link::request_json(
        &s,
        &input.source_device_id,
        &format!("/api/peer/connections/{id}/export"),
        &json!({"token":input.token,"revision":input.revision}),
    )
    .await
    .map_err(failure)?;
    let c: saved::Connection =
        serde_json::from_value(raw).map_err(|_| failure("Invalid remote connection"))?;
    if c.id != id || c.revision != input.revision {
        return Err(failure("Connection version conflict: refresh the list"));
    }
    let _g = s.upstream_gate.lock().await;
    let value = saved::import(&dir(&s), c, input.expected_revision.as_deref()).map_err(failure)?;
    announce(&s);
    Ok(Json(value))
}

#[cfg(test)]
#[path = "connections_tests.rs"]
mod tests;
