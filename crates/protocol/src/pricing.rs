//! Model identity and editable reference rates shared by Relay and devices.
use serde_json::{json, Value};
use std::borrow::Cow;

/// DSH uses an ACP provider/model tuple as its selectable ID. Reporting and
/// shared metadata use the model component; never change the selectable ID.
pub fn model_id(raw: &str) -> Cow<'_, str> {
    if let Ok([provider, model]) = serde_json::from_str::<[String; 2]>(raw) {
        if !provider.is_empty() && !model.is_empty() {
            return Cow::Owned(model);
        }
    }
    Cow::Borrowed(raw)
}

pub fn normalized(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn match_model(catalog: &Value, model: &str) -> Option<String> {
    resolve_model(catalog, model).map(|(key, _)| key)
}

/// Exact IDs and configured aliases win. A gateway effort suffix is a reference
/// alias only when the known base model explicitly offers that level. This is
/// for presentation/accounting; callers must retain the upstream's wire ID.
pub fn resolve_model(catalog: &Value, model: &str) -> Option<(String, Option<String>)> {
    let model = model_id(model);
    let model = model.trim();
    if let Some(key) = match_plain_model(catalog, model) {
        return Some((key, None));
    }
    let (base, effort) = model.rsplit_once('-')?;
    let effort = effort.to_ascii_lowercase();
    let effort = match effort.as_str() {
        "off" => "none",
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => &effort,
        _ => return None,
    };
    let key = match_plain_model(catalog, base)?;
    catalog["models"][&key]["reasoningEfforts"]
        .as_array()?
        .iter()
        .any(|e| e["reasoningEffort"] == effort)
        .then(|| (key, Some(effort.to_owned())))
}

fn match_plain_model(catalog: &Value, model: &str) -> Option<String> {
    let models = catalog["models"].as_object()?;
    for candidate in [
        model.trim(),
        model.rsplit('/').next().unwrap_or(model).trim(),
    ] {
        if models
            .get(candidate)
            .is_some_and(|rates| rates["unpriced"] != true)
        {
            return Some(candidate.into());
        }
        let key = normalized(candidate);
        for (id, rates) in models {
            if rates["unpriced"] == true {
                continue;
            }
            if normalized(id) == key
                || rates["aliases"].as_array().is_some_and(|aliases| {
                    aliases
                        .iter()
                        .any(|alias| alias.as_str().is_some_and(|a| normalized(a) == key))
                })
            {
                return Some(id.clone());
            }
        }
        for (id, rates) in models {
            if rates["unpriced"] == true {
                continue;
            }
            let lower = candidate.to_lowercase();
            if let Some(suffix) = lower.strip_prefix(&format!("{}-", id.to_lowercase())) {
                let digits: String = suffix.chars().filter(|c| *c != '-').collect();
                if [4, 8].contains(&digits.len()) && digits.bytes().all(|c| c.is_ascii_digit()) {
                    return Some(id.clone());
                }
            }
        }
    }
    None
}

/// Apply a complete account/local override; absent fields keep their documented
/// calculator fallback rather than borrowing unrelated rules from another model.
pub fn update_override(
    catalog: &Value,
    overrides: &mut Value,
    input: &Value,
) -> Result<(), String> {
    let id = input["id"]
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.len() <= 160 && !v.chars().any(char::is_control))
        .ok_or("Model ID is required (maximum 160 characters)")?;
    if input["reset"] == true {
        overrides
            .as_object_mut()
            .ok_or("Invalid overrides")?
            .remove(id);
        return Ok(());
    }
    let mut rates = input["rates"].clone();
    if !rates.is_object() {
        return Err("Model rates are required".into());
    }
    for field in [
        "inputUsdPerMillion",
        "cachedInputUsdPerMillion",
        "outputUsdPerMillion",
    ] {
        if !rates[field]
            .as_f64()
            .is_some_and(|v| v.is_finite() && (0.0..=1e6).contains(&v))
        {
            return Err(format!("{field} must be between 0 and 1000000"));
        }
    }
    for field in [
        "cacheWriteInputUsdPerMillion",
        "cacheWriteOneHourInputUsdPerMillion",
        "longContextInputMultiplier",
        "longContextOutputMultiplier",
        "fastMultiplier",
        "offPeakMultiplier",
        "ratesChangeMultiplier",
    ] {
        if rates.get(field).is_some_and(|v| {
            !v.is_null()
                && !v
                    .as_f64()
                    .is_some_and(|v| v.is_finite() && (0.0..=1e6).contains(&v))
        }) {
            return Err(format!("Invalid {field}"));
        }
    }
    let aliases = rates.get("aliases").cloned().unwrap_or(json!([]));
    if !aliases.as_array().is_some_and(|a| {
        a.len() <= 50
            && a.iter().all(|v| {
                v.as_str()
                    .is_some_and(|s| !s.trim().is_empty() && s.len() <= 160)
            })
    }) {
        return Err("Aliases must be a list of model IDs or names".into());
    }
    if rates.get("displayName").is_some_and(|name| {
        !name.as_str().is_some_and(|name| {
            !name.trim().is_empty() && name.len() <= 200 && !name.chars().any(char::is_control)
        })
    }) {
        return Err("Invalid displayName".into());
    }
    if rates.get("defaultReasoningEffort").is_some_and(|effort| {
        !effort.as_str().is_some_and(|effort| {
            matches!(
                effort,
                "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
            )
        })
    }) {
        return Err("Invalid defaultReasoningEffort".into());
    }
    if rates.get("reasoningEfforts").is_some_and(|efforts| {
        !efforts.as_array().is_some_and(|efforts| {
            efforts.len() <= 10
                && efforts.iter().all(|entry| {
                    entry["reasoningEffort"].as_str().is_some_and(|effort| {
                        matches!(
                            effort,
                            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
                        )
                    }) && entry["description"].as_str().is_some_and(|description| {
                        !description.is_empty()
                            && description.len() <= 200
                            && !description.chars().any(char::is_control)
                    })
                })
        })
    }) {
        return Err("Invalid reasoningEfforts".into());
    }
    for alias in std::iter::once(json!(id)).chain(aliases.as_array().unwrap().iter().cloned()) {
        if let Some(existing) = match_plain_model(catalog, alias.as_str().unwrap()) {
            if existing != id {
                return Err(format!("Model name already belongs to {existing}"));
            }
        }
    }
    // Only calculation fields and plain descriptive metadata are accepted. In
    // particular a browser cannot manufacture a synchronized source/version.
    let allowed = [
        "inputUsdPerMillion",
        "cachedInputUsdPerMillion",
        "outputUsdPerMillion",
        "cacheWriteInputUsdPerMillion",
        "cacheWriteOneHourInputUsdPerMillion",
        "aliases",
        "displayName",
        "defaultReasoningEffort",
        "reasoningEfforts",
        "longContextThresholdTokens",
        "longContextInputMultiplier",
        "longContextOutputMultiplier",
        "supportsFastMode",
        "fastMultiplier",
        "offPeakMultiplier",
        "ratesChangeAt",
        "ratesChangeMultiplier",
        "notes",
        "contextWindowTokens",
        "contextTiers",
        "serviceTiers",
    ];
    rates
        .as_object_mut()
        .unwrap()
        .retain(|k, _| allowed.contains(&k.as_str()));
    // Omitted presentation fields continue following the feed at read time;
    // copying them into a saved rate override would pin stale capabilities.
    // Nested rates originate from the catalog but are editable request input too.
    fn valid_nested(v: &Value) -> bool {
        match v {
            Value::Number(n) => n
                .as_f64()
                .is_some_and(|n| n.is_finite() && (0.0..=1e9).contains(&n)),
            Value::Object(o) => o.len() <= 50 && o.values().all(valid_nested),
            Value::Array(a) => a.len() <= 20 && a.iter().all(valid_nested),
            _ => false,
        }
    }
    for key in ["contextTiers", "serviceTiers"] {
        if rates.get(key).is_some_and(|v| !valid_nested(v)) {
            return Err(format!("Invalid {key}"));
        }
    }
    rates["aliases"] = aliases;
    overrides[id] = rates;
    Ok(())
}

pub fn apply_overrides(catalog: &mut Value, overrides: &Value) {
    for (id, rates) in overrides.as_object().into_iter().flatten() {
        // An alias override replaces the canonical entry as well, ensuring it
        // cannot lose to the default entry just because of map ordering.
        let key = match_plain_model(catalog, id).unwrap_or_else(|| id.clone());
        let mut replacement = rates.clone();
        // Older saved rate overrides predate the shared presentation fields.
        // Let those fields continue to follow the feed unless explicitly set.
        for field in ["displayName", "defaultReasoningEffort", "reasoningEfforts"] {
            if replacement.get(field).is_none() {
                if let Some(value) = catalog["models"][&key].get(field) {
                    replacement[field] = value.clone();
                }
            }
        }
        catalog["models"][&key] = replacement;
        catalog["models"][&key]["custom"] = json!(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dsh_tuple_and_effort_suffix_match_reference_model_without_corrupting_exact_ids() {
        let mut catalog = json!({"models":{
            "gemini-4-argon":{"reasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}]},
            "plain":{}, "gemini-4-argon-low":{"inputUsdPerMillion":3}
        }});
        assert_eq!(
            resolve_model(
                &catalog,
                r#"["remote-codex-connection","gemini-4-argon-high"]"#
            ),
            Some(("gemini-4-argon".into(), Some("high".into())))
        );
        assert_eq!(
            resolve_model(&catalog, "gemini-4-argon-low"),
            Some(("gemini-4-argon-low".into(), None))
        );
        for unknown in [
            "gemini-4-argon-ultra",
            "gemini-4-argon-cyber",
            "plain-high",
            "gemini-4-argon-high-low",
            "[1,2]",
            "[]",
        ] {
            assert_eq!(match_model(&catalog, unknown), None, "{unknown}");
        }
        // A channel-specific override stays independent of its base model.
        let mut overrides = json!({});
        update_override(
            &catalog,
            &mut overrides,
            &json!({"id":"gemini-4-argon-high", "rates":{
                "inputUsdPerMillion":4,"cachedInputUsdPerMillion":1,"outputUsdPerMillion":20
            }}),
        )
        .unwrap();
        apply_overrides(&mut catalog, &overrides);
        assert_eq!(
            match_model(&catalog, "gemini-4-argon-high").as_deref(),
            Some("gemini-4-argon-high")
        );
        assert!(catalog["models"]["gemini-4-argon"]
            .get("inputUsdPerMillion")
            .is_none());
    }

    #[test]
    fn older_rate_overrides_keep_current_metadata_unless_explicitly_overridden() {
        let mut catalog =
            json!({"models":{"model":{"displayName":"Shared name", "reasoningEfforts":[]}}});
        apply_overrides(&mut catalog, &json!({"model":{"inputUsdPerMillion":1}}));
        assert_eq!(catalog["models"]["model"]["displayName"], "Shared name");
        assert_eq!(catalog["models"]["model"]["reasoningEfforts"], json!([]));
        apply_overrides(&mut catalog, &json!({"model":{"displayName":"My name"}}));
        assert_eq!(catalog["models"]["model"]["displayName"], "My name");
    }
    #[test]
    fn pricing_matches_exact_ids_before_aliases_and_ignores_pending_placeholders() {
        let catalog = json!({"models":{
            "model":{"aliases":["model-20300101","opaque-used"]},
            "model-20300101":{"aliases":[]},
            "opaque-used":{"unpriced":true}
        }});
        assert_eq!(
            match_model(&catalog, "model-20300101").as_deref(),
            Some("model-20300101")
        );
        assert_eq!(
            match_model(&catalog, "opaque-used").as_deref(),
            Some("model")
        );
        assert_eq!(
            match_model(&catalog, "provider/Model").as_deref(),
            Some("model")
        );
        assert_eq!(
            match_model(&catalog, "model-20310101").as_deref(),
            Some("model")
        );
    }
    #[test]
    fn pricing_override_rejects_missing_negative_and_conflicting_rates_but_allows_explicit_free() {
        let mut catalog = json!({"models":{"official":{"aliases":["Official model"]}}});
        let mut overrides = json!({});
        for rates in [
            json!({}),
            json!({"inputUsdPerMillion":-1,"cachedInputUsdPerMillion":0,"outputUsdPerMillion":0}),
        ] {
            assert!(update_override(
                &catalog,
                &mut overrides,
                &json!({"id":"custom","rates":rates})
            )
            .is_err());
        }
        let free = json!({"inputUsdPerMillion":0,"cachedInputUsdPerMillion":0,"outputUsdPerMillion":0,"sourceUrl":"https://untrusted.test","custom":false,"unpriced":true});
        assert!(update_override(&catalog,&mut overrides,&json!({"id":"other","rates":{"aliases":["Official model"],"inputUsdPerMillion":0,"cachedInputUsdPerMillion":0,"outputUsdPerMillion":0}})).is_err());
        update_override(
            &catalog,
            &mut overrides,
            &json!({"id":"custom","rates":free}),
        )
        .unwrap();
        apply_overrides(&mut catalog, &overrides);
        assert_eq!(catalog["models"]["custom"]["custom"], true);
        assert_eq!(catalog["models"]["custom"]["inputUsdPerMillion"], 0);
        assert!(catalog["models"]["custom"]["sourceUrl"].is_null());
        assert!(catalog["models"]["custom"]["unpriced"].is_null());
    }
}
