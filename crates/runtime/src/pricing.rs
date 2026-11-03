//! Local overrides and durable account pricing snapshots; usage stays on device.
use crate::Supervisor;
use anyhow::{bail, Result};
use serde_json::{json, Value};

pub(crate) use pockymoe_protocol::pricing::match_model;

impl Supervisor {
    pub fn model_pricing(&self) -> Value {
        let synced = self.cached_relay_model_pricing();
        let mut config = synced
            .clone()
            .unwrap_or_else(|| crate::usage::pricing().clone());
        if synced.is_none() {
            pockymoe_protocol::pricing::apply_overrides(
                &mut config,
                &self.local_model_price_overrides(),
            );
        }
        // A harness may advertise opaque IDs and readable names. Remember both.
        let names: Value = self
            .db
            .get_kv("model_display_names")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(json!({}));
        for (id, name) in names.as_object().into_iter().flatten() {
            if let Some(key) = match_model(&config, id)
                .or_else(|| name.as_str().and_then(|n| match_model(&config, n)))
            {
                let mut aliases = config["models"][&key]["aliases"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for alias in [json!(id), name.clone()] {
                    if !aliases.contains(&alias) {
                        aliases.push(alias);
                    }
                }
                config["models"][&key]["aliases"] = json!(aliases);
            }
        }
        config
    }

    pub fn update_model_pricing(&self, input: &Value) -> Result<Value> {
        if self.config.mode == pockymoe_protocol::Mode::Relay {
            bail!("Manage reference prices in account settings");
        }
        let mut overrides = self.local_model_price_overrides();
        pockymoe_protocol::pricing::update_override(&self.model_pricing(), &mut overrides, input)
            .map_err(anyhow::Error::msg)?;
        self.db.set_kv("model_pricing", &overrides.to_string())?;
        Ok(self.model_pricing())
    }

    pub fn local_model_price_overrides(&self) -> Value {
        self.db
            .get_kv("model_pricing")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(json!({}))
    }

    // Bind cached prices to this Relay credential without persisting the secret.
    fn pricing_cache_key(&self) -> String {
        use sha2::{Digest, Sha256};
        format!(
            "relay_model_pricing:{:x}",
            Sha256::digest(
                format!(
                    "{}\n{}",
                    self.config.relay_server_url.as_deref().unwrap_or(""),
                    self.config.relay_agent_token.as_deref().unwrap_or("")
                )
                .as_bytes()
            )
        )
    }

    fn cached_relay_model_pricing(&self) -> Option<Value> {
        (self.config.mode == pockymoe_protocol::Mode::Relay)
            .then(|| {
                self.db
                    .get_kv(&self.pricing_cache_key())
                    .ok()
                    .flatten()
                    .and_then(|s| serde_json::from_str(&s).ok())
            })
            .flatten()
    }

    /// Gather historical model names at connect, or only one changed thread.
    /// No token counts, prompts or credentials enter the synchronization frame.
    pub fn model_pricing_sync(&self, observe: bool, thread: Option<&str>) -> Result<Value> {
        let mut frame = json!({"type":"pricing.sync", "version":self.cached_relay_model_pricing().and_then(|v|v["version"].as_str().map(str::to_owned))});
        if observe {
            let models = self.db.with(|c| {
                let mut names=std::collections::BTreeSet::new();
                let mut stmt=c.prepare("SELECT model,pricing_model_key,token_usage_json FROM thread_turns WHERE (?1 IS NULL OR thread_id=?1) AND token_usage_json IS NOT NULL")?;
                let rows=stmt.query_map([thread],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?)))?;
                for row in rows {
                    let (model,key,usage)=row?;
                    for name in [model,key].into_iter().flatten() { names.insert(crate::usage::model_name(&name)); }
                    if let Ok(usage)=serde_json::from_str::<Value>(&usage) {
                        for request in usage["requests"].as_array().into_iter().flatten() {
                            if let Some(name)=request["model"].as_str() {names.insert(crate::usage::model_name(name));}
                        }
                    }
                }
                names.retain(|n|!n.is_empty() && n.len()<=160 && !n.chars().any(char::is_control) && n!="<synthetic>");
                Ok(names.into_iter().take(10000).collect::<Vec<_>>())
            })?;
            frame["models"] = json!(models);
        }
        Ok(frame)
    }

    pub fn accept_relay_model_pricing(&self, catalog: &Value) -> Result<bool> {
        anyhow::ensure!(
            catalog["models"].is_object() && catalog["version"].is_string(),
            "Invalid price snapshot"
        );
        if self
            .cached_relay_model_pricing()
            .is_some_and(|old| old["version"] == catalog["version"])
        {
            return Ok(false);
        }
        self.db
            .set_kv(&self.pricing_cache_key(), &catalog.to_string())?;
        // Existing readers already refresh summaries on thread.updated. Pricing
        // changes are infrequent; no transcript or usage record is rewritten.
        let threads = self.db.with(|c| {
            let mut stmt = c.prepare(
                "SELECT DISTINCT thread_id FROM thread_turns WHERE token_usage_json IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        for thread in threads {
            self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
                thread_id: thread,
                timestamp: pockymoe_protocol::now_rfc3339(),
                payload: json!({"reason":"model_pricing_updated"}),
            });
        }
        Ok(true)
    }
}
