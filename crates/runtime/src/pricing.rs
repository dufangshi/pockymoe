//! Local overrides and durable account pricing snapshots; usage stays on device.
use crate::Supervisor;
use anyhow::{bail, Result};
use pockymoe_protocol::{ModelOptionDto, ReasoningEffortOptionDto};
use serde_json::{json, Value};

pub(crate) use pockymoe_protocol::pricing::match_model;

fn enrich_model_options_with_catalog(models: &mut [ModelOptionDto], catalog: &Value) {
    for model in models {
        let Some(key) = match_model(catalog, &model.model) else {
            continue;
        };
        let metadata = &catalog["models"][&key];
        if model.display_name.is_empty() || model.display_name == model.model {
            if let Some(name) = metadata["displayName"]
                .as_str()
                .filter(|name| !name.is_empty())
            {
                model.display_name = name.to_owned();
            }
        }
        // A harness or the upstream /models response is more precise than a
        // shared LiteLLM capability hint. Use the shared list only when the
        // active endpoint did not advertise per-model effort choices.
        if !model.supported_reasoning_efforts.is_empty() {
            continue;
        }
        let Some(efforts) = metadata["reasoningEfforts"].as_array() else {
            continue;
        };
        let efforts: Vec<_> = efforts
            .iter()
            .filter_map(|entry| {
                let value = entry["reasoningEffort"].as_str()?.trim();
                (!value.is_empty()).then(|| ReasoningEffortOptionDto {
                    reasoning_effort: value.to_owned(),
                    description: entry["description"]
                        .as_str()
                        .filter(|description| !description.is_empty())
                        .unwrap_or(value)
                        .to_owned(),
                })
            })
            .collect();
        if efforts.is_empty() {
            continue;
        }
        model.supported_reasoning_efforts = efforts;
        model.default_reasoning_effort = metadata["defaultReasoningEffort"]
            .as_str()
            .filter(|effort| {
                model
                    .supported_reasoning_efforts
                    .iter()
                    .any(|option| option.reasoning_effort == *effort)
            })
            .map(str::to_owned)
            .or_else(|| {
                model
                    .supported_reasoning_efforts
                    .iter()
                    .find(|option| option.reasoning_effort == "medium")
                    .or_else(|| model.supported_reasoning_efforts.first())
                    .map(|option| option.reasoning_effort.clone())
            });
    }
}

impl Supervisor {
    /// Add account-wide presentation hints to model choices. Context windows
    /// deliberately stay on the upstream/ACP path, where endpoint and config
    /// specific limits are available.
    pub fn enrich_model_options(&self, models: &mut [ModelOptionDto]) {
        enrich_model_options_with_catalog(models, &self.model_pricing());
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_catalog_enriches_opaque_model_options() {
        let mut models = vec![ModelOptionDto {
            id: "claude-opus-5-5".into(),
            model: "claude-opus-5-5".into(),
            display_name: "claude-opus-5-5".into(),
            description: "upstream · claude".into(),
            is_default: true,
            hidden: false,
            supported_reasoning_efforts: vec![],
            default_reasoning_effort: None,
            selection_kind: Some("model".into()),
            acp_agent: None,
        }];
        let catalog = json!({"models": {"claude-opus-5-5": {
            "displayName": "Opus 5.5",
            "aliases": ["anthropic/claude-opus-5-5"],
            "reasoningEfforts": [
                {"reasoningEffort":"low", "description":"Low"},
                {"reasoningEffort":"medium", "description":"Medium"},
                {"reasoningEffort":"high", "description":"High"}
            ],
            "defaultReasoningEffort": "medium"
        }}});

        enrich_model_options_with_catalog(&mut models, &catalog);

        assert_eq!(models[0].display_name, "Opus 5.5");
        assert_eq!(
            models[0].default_reasoning_effort.as_deref(),
            Some("medium")
        );
        assert_eq!(
            models[0]
                .supported_reasoning_efforts
                .iter()
                .map(|option| option.reasoning_effort.as_str())
                .collect::<Vec<_>>(),
            ["low", "medium", "high"]
        );
    }

    #[test]
    fn upstream_model_metadata_wins_over_shared_hints() {
        let mut models = vec![ModelOptionDto {
            id: "claude-opus-5-5".into(),
            model: "claude-opus-5-5".into(),
            display_name: "Proxy Opus".into(),
            description: "upstream · claude".into(),
            is_default: true,
            hidden: false,
            supported_reasoning_efforts: vec![ReasoningEffortOptionDto {
                reasoning_effort: "high".into(),
                description: "Endpoint high".into(),
            }],
            default_reasoning_effort: Some("high".into()),
            selection_kind: Some("model".into()),
            acp_agent: None,
        }];
        let catalog = json!({"models": {"claude-opus-5-5": {
            "displayName": "Opus 5.5",
            "reasoningEfforts": [
                {"reasoningEffort":"low", "description":"Low"},
                {"reasoningEffort":"medium", "description":"Medium"}
            ],
            "defaultReasoningEffort": "medium"
        }}});

        enrich_model_options_with_catalog(&mut models, &catalog);

        assert_eq!(models[0].display_name, "Proxy Opus");
        assert_eq!(models[0].default_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(
            models[0].supported_reasoning_efforts[0].reasoning_effort,
            "high"
        );
    }
}
