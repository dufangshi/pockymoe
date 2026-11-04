//! Model identity and editable reference rates shared by Relay and devices.
use serde_json::{json, Value};

pub fn normalized(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn match_model(catalog: &Value, model: &str) -> Option<String> {
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
        if let Some(existing) = match_model(catalog, alias.as_str().unwrap()) {
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
    // A pricing edit usually contains only rates. Keep relay-owned model
    // presentation metadata from the current catalog when it is omitted.
    let canonical = match_model(catalog, id).unwrap_or_else(|| id.to_owned());
    for field in ["displayName", "defaultReasoningEffort", "reasoningEfforts"] {
        if rates.get(field).is_none() {
            if let Some(value) = catalog["models"][&canonical].get(field) {
                rates[field] = value.clone();
            }
        }
    }
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
        let key = match_model(catalog, id).unwrap_or_else(|| id.clone());
        catalog["models"][&key] = rates.clone();
        catalog["models"][&key]["custom"] = json!(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
