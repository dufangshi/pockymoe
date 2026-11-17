use std::collections::HashMap;

use pockymoe_protocol::ReasoningEffortOptionDto;
use serde_json::{json, Value};

use super::adapter::HarnessProjection;

// Older Codex catalogs can omit a configured model that thread/start still accepts.
// Repair the catalog before ACP validates settings, using the same owned connection.
#[derive(Default)]
pub(super) struct ModelCatalogBridge {
    pending: HashMap<String, String>,
    current_model: Option<String>,
    catalog: Vec<Value>,
    upstream: Vec<crate::upstreams::DiscoveredModel>,
}

impl ModelCatalogBridge {
    pub fn set_upstream(&mut self, models: Vec<crate::upstreams::DiscoveredModel>) {
        self.upstream = models;
    }

    pub fn observe_request(&mut self, message: &Value) {
        let Some(method) = message["method"].as_str() else {
            return;
        };
        if matches!(
            method,
            "thread/start" | "thread/resume" | "thread/fork" | "model/list"
        ) {
            if method == "model/list" && message["params"]["cursor"].is_null() {
                self.catalog.clear();
            }
            self.pending
                .insert(message["id"].to_string(), method.into());
        }
    }

    pub fn observe_response(&mut self, message: &mut Value) {
        let Some(method) = self.pending.remove(&message["id"].to_string()) else {
            return;
        };
        let Some(result) = message.get_mut("result") else {
            return;
        };
        if method != "model/list" {
            if let Some(model) = result["model"].as_str() {
                self.current_model = Some(model.into());
            }
            return;
        }
        let last_page = result["nextCursor"].is_null();
        let Some(data) = result["data"].as_array_mut() else {
            return;
        };
        self.catalog.extend(data.iter().cloned());
        let donor = self
            .catalog
            .iter()
            .filter(|model| has_efforts(model))
            .find(|model| model["id"] == "gpt-5.6-sol")
            .or_else(|| self.catalog.iter().find(|model| has_efforts(model)));
        let efforts = donor
            .map(|model| model["supportedReasoningEfforts"].clone())
            .unwrap_or_else(|| {
                json!([
                    {"reasoningEffort":"low","description":"Low"},
                    {"reasoningEffort":"medium","description":"Medium"},
                    {"reasoningEffort":"high","description":"High"}
                ])
            });
        let default = donor
            .and_then(|model| model["defaultReasoningEffort"].as_str())
            .filter(|value| {
                efforts
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|effort| effort["reasoningEffort"] == *value)
            })
            .or_else(|| {
                efforts
                    .as_array()?
                    .iter()
                    .find_map(|effort| (effort["reasoningEffort"] == "medium").then_some("medium"))
            })
            .or_else(|| efforts[0]["reasoningEffort"].as_str())
            .unwrap_or("medium")
            .to_string();
        for model in data.iter_mut().filter(|model| !has_efforts(model)) {
            model["supportedReasoningEfforts"] = efforts.clone();
            model["defaultReasoningEffort"] = json!(default);
        }
        if last_page {
            for model in &self.upstream {
                if self.catalog.iter().any(|entry| entry["id"] == model.id) {
                    continue;
                }
                let declared = model.reasoning.as_ref();
                let mut supported = declared
                    .map(|cap| {
                        cap.efforts
                            .iter()
                            .map(|effort| {
                                json!({
                                    "reasoningEffort": effort.reasoning_effort,
                                    "description": effort.description
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_else(|| efforts.as_array().unwrap().clone());
                // ACP represents models as model[effort], so a declared lack
                // of reasoning needs its explicit `none` value to be selectable.
                if supported.is_empty() {
                    supported.push(json!({"reasoningEffort":"none", "description":"None"}));
                }
                let preferred = declared
                    .and_then(|cap| cap.default.as_deref())
                    .unwrap_or(&default);
                let selected = supported
                    .iter()
                    .find(|entry| entry["reasoningEffort"] == preferred)
                    .unwrap_or(&supported[0])["reasoningEffort"]
                    .clone();
                data.push(json!({
                    "id": model.id, "model": model.id, "displayName": model.name,
                    "description": "", "hidden": false, "isDefault": false,
                    "supportedReasoningEfforts": supported,
                    "defaultReasoningEffort": selected,
                    "inputModalities": ["text"], "supportsPersonality": false
                }));
            }
        }
        if let Some(model) = self.current_model.as_deref().filter(|id| {
            last_page
                && !self
                    .catalog
                    .iter()
                    .chain(data.iter())
                    .any(|model| model["id"] == *id)
        }) {
            data.push(json!({
                "id": model, "model": model, "displayName": model,
                "description": "", "hidden": false, "isDefault": false,
                "supportedReasoningEfforts": efforts, "defaultReasoningEffort": default,
                "inputModalities": ["text", "image"], "supportsPersonality": false
            }));
        }
    }
}

fn has_efforts(model: &Value) -> bool {
    model["supportedReasoningEfforts"]
        .as_array()
        .is_some_and(|values| !values.is_empty())
}

pub(super) fn project_session(response: &Value) -> Option<HarnessProjection> {
    let model_state = response.get("models")?;
    let available = model_state["availableModels"].as_array()?;
    let mut models = super::runtime::models_from_config_options(&response["configOptions"]);
    for model in &mut models {
        model.supported_reasoning_efforts = available
            .iter()
            .filter_map(|entry| {
                let (id, effort) = split_model_id(entry["modelId"].as_str()?)?;
                (id == model.id).then(|| ReasoningEffortOptionDto {
                    reasoning_effort: effort.into(),
                    description: entry["description"].as_str().unwrap_or(effort).into(),
                })
            })
            .collect();
    }
    let fallback = models
        .iter()
        .filter(|model| !model.supported_reasoning_efforts.is_empty())
        .find(|model| model.id == "gpt-5.6-sol")
        .or_else(|| {
            models
                .iter()
                .find(|model| !model.supported_reasoning_efforts.is_empty())
        })
        .map(|model| model.supported_reasoning_efforts.clone())
        .unwrap_or_else(|| {
            ["low", "medium", "high"]
                .into_iter()
                .map(|effort| ReasoningEffortOptionDto {
                    reasoning_effort: effort.into(),
                    description: effort.into(),
                })
                .collect()
        });
    for model in &mut models {
        if model.supported_reasoning_efforts.is_empty() {
            model.supported_reasoning_efforts = fallback.clone();
        }
        model.default_reasoning_effort = model
            .supported_reasoning_efforts
            .iter()
            .find(|effort| effort.reasoning_effort == "medium")
            .or(model.supported_reasoning_efforts.first())
            .map(|effort| effort.reasoning_effort.clone());
    }
    let current = model_state["currentModelId"]
        .as_str()
        .and_then(split_model_id);
    Some(HarnessProjection {
        state: model_state.clone(),
        models,
        model: current.map(|(model, _)| model.into()),
        reasoning_effort: current.map(|(_, effort)| effort.into()),
    })
}

fn split_model_id(value: &str) -> Option<(&str, &str)> {
    let (model, effort) = value.strip_suffix(']')?.rsplit_once('[')?;
    (!model.is_empty() && !effort.is_empty()).then_some((model, effort))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upstreams::DiscoveredModel;

    fn page(bridge: &mut ModelCatalogBridge, cursor: Value, data: Value, next: Value) -> Value {
        bridge.observe_request(&json!({"id":1,"method":"model/list","params":{"cursor":cursor}}));
        let mut response = json!({"id":1,"result":{"data":data,"nextCursor":next}});
        bridge.observe_response(&mut response);
        response["result"]["data"].clone()
    }

    #[test]
    fn upstream_models_reach_acp_on_last_page_without_duplicates() {
        let mut bridge = ModelCatalogBridge::default();
        bridge.set_upstream(serde_json::from_value(json!([
            {"id":"gpt-6.2-sol","name":"New model","reasoning":{"efforts":[{"reasoningEffort":"high","description":"High"}],"default":"high"}},
            {"id":"existing","name":"Must not override native metadata"}
        ])).unwrap());
        bridge.current_model = Some("gpt-6.2-sol".into());
        let first = page(
            &mut bridge,
            Value::Null,
            json!([{"id":"existing"}]),
            json!("next"),
        );
        assert_eq!(first.as_array().unwrap().len(), 1);
        let last = page(&mut bridge, json!("next"), json!([]), Value::Null);
        assert_eq!(last.as_array().unwrap().len(), 1);
        assert_eq!(last[0]["id"], "gpt-6.2-sol");
        assert_eq!(last[0]["defaultReasoningEffort"], "high");
        assert_eq!(
            last[0]["supportedReasoningEfforts"][0]["reasoningEffort"],
            "high"
        );
    }

    #[test]
    fn refreshed_upstream_replaces_old_ids_and_preserves_no_reasoning() {
        let mut bridge = ModelCatalogBridge::default();
        bridge.set_upstream(vec![DiscoveredModel {
            id: "removed".into(),
            ..Default::default()
        }]);
        assert_eq!(
            page(&mut bridge, Value::Null, json!([]), Value::Null)[0]["id"],
            "removed"
        );
        bridge.set_upstream(
            serde_json::from_value(json!([
                {"id":"new","name":"New","reasoning":{"efforts":[],"default":null}}
            ]))
            .unwrap(),
        );
        let refreshed = page(&mut bridge, Value::Null, json!([]), Value::Null);
        assert_eq!(refreshed.as_array().unwrap().len(), 1);
        assert_eq!(refreshed[0]["id"], "new");
        assert_eq!(refreshed[0]["defaultReasoningEffort"], "none");
        assert_eq!(
            refreshed[0]["supportedReasoningEfforts"],
            json!([{"reasoningEffort":"none","description":"None"}])
        );
    }
}
