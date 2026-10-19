//! Token accounting shared by ACP adapters, persistence, and history responses.
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Tokens {
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    /// Subset of cache_write_input_tokens, not additional input tokens.
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_write_one_hour_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn number(value: &Value, names: &[&str]) -> Option<u64> {
    names.iter().find_map(|key| {
        value.get(key).and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
    })
}

impl Tokens {
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        let mut input = number(
            value,
            &[
                "inputTokens",
                "input_tokens",
                "prompt_tokens",
                "promptTokenCount",
            ],
        )?;
        let mut output = number(
            value,
            &[
                "outputTokens",
                "output_tokens",
                "completion_tokens",
                "candidatesTokenCount",
            ],
        )?;
        let reasoning = number(
            value,
            &[
                "reasoningOutputTokens",
                "reasoning_output_tokens",
                "thoughtTokens",
                "thought_tokens",
                "thoughtsTokenCount",
            ],
        )
        .or_else(|| {
            number(
                value.get("completion_tokens_details")?,
                &["reasoning_tokens"],
            )
        })
        .or_else(|| {
            number(
                value.get("output_tokens_details")?,
                &["reasoning_tokens", "thinking_tokens"],
            )
        })
        .unwrap_or_default();
        if value.get("candidatesTokenCount").is_some() {
            output = output.saturating_add(reasoning);
        }
        let cached = number(
            value,
            &[
                "cachedInputTokens",
                "cached_input_tokens",
                "cachedReadTokens",
                "cached_read_tokens",
                "cache_read_input_tokens",
                "prompt_cache_hit_tokens",
                "cachedContentTokenCount",
            ],
        )
        .or_else(|| number(value.get("input_tokens_details")?, &["cached_tokens"]))
        .or_else(|| number(value.get("prompt_tokens_details")?, &["cached_tokens"]))
        .or_else(|| number(value.get("cache")?, &["read"]))
        .unwrap_or_default();
        let one_hour_written = number(value, &["cacheWriteOneHourInputTokens"])
            .or_else(|| number(value.get("cache_creation")?, &["ephemeral_1h_input_tokens"]))
            .unwrap_or_default();
        let written = number(
            value,
            &[
                "cacheWriteInputTokens",
                "cache_write_input_tokens",
                "cachedWriteTokens",
                "cached_write_tokens",
                "cache_creation_input_tokens",
            ],
        )
        .or_else(|| number(value.get("cache")?, &["write"]))
        .or_else(|| {
            number(value.get("cache_creation")?, &["ephemeral_5m_input_tokens"])
                .map(|five_minute| five_minute.saturating_add(one_hour_written))
        })
        .unwrap_or(one_hour_written);
        // ACP Usage counts uncached input separately; our existing DTO includes caches.
        if value.get("cachedReadTokens").is_some()
            || value.get("cachedWriteTokens").is_some()
            || value.get("cached_read_tokens").is_some()
            || value.get("cached_write_tokens").is_some()
            || value.get("cache_read_input_tokens").is_some()
            || value.get("cache_creation_input_tokens").is_some()
            || value.get("cache_creation").is_some()
        {
            input = input.saturating_add(cached).saturating_add(written);
        }
        Some(Self {
            total_tokens: number(value, &["totalTokens", "total_tokens", "totalTokenCount"])
                .unwrap_or(input.saturating_add(output)),
            input_tokens: input,
            cached_input_tokens: cached,
            cache_write_input_tokens: written,
            cache_write_one_hour_input_tokens: one_hour_written.min(written),
            output_tokens: output,
            reasoning_output_tokens: reasoning,
        })
    }

    /// Counters may restart with a new Codex process; the first report then
    /// represents fresh usage rather than a negative increment.
    pub(crate) fn cumulative_delta(&self, previous: &Self, last: Option<&Self>) -> Self {
        if self.total_tokens < previous.total_tokens {
            self.clone()
        } else {
            let delta = self.subtract(previous);
            // A restarted process can already exceed the old counter. A changed
            // report still cannot bill fewer tokens than its latest request.
            if self.total_tokens != previous.total_tokens {
                if let Some(last) = last.filter(|last| last.total_tokens > delta.total_tokens) {
                    return last.clone();
                }
            }
            delta
        }
    }

    pub(crate) fn add(&self, other: &Self) -> Self {
        Self {
            total_tokens: self.total_tokens.saturating_add(other.total_tokens),
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            cached_input_tokens: self
                .cached_input_tokens
                .saturating_add(other.cached_input_tokens),
            cache_write_input_tokens: self
                .cache_write_input_tokens
                .saturating_add(other.cache_write_input_tokens),
            cache_write_one_hour_input_tokens: self
                .cache_write_one_hour_input_tokens
                .saturating_add(other.cache_write_one_hour_input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            reasoning_output_tokens: self
                .reasoning_output_tokens
                .saturating_add(other.reasoning_output_tokens),
        }
    }

    pub(crate) fn subtract(&self, baseline: &Self) -> Self {
        Self {
            total_tokens: self.total_tokens.saturating_sub(baseline.total_tokens),
            input_tokens: self.input_tokens.saturating_sub(baseline.input_tokens),
            cached_input_tokens: self
                .cached_input_tokens
                .saturating_sub(baseline.cached_input_tokens),
            cache_write_input_tokens: self
                .cache_write_input_tokens
                .saturating_sub(baseline.cache_write_input_tokens),
            cache_write_one_hour_input_tokens: self
                .cache_write_one_hour_input_tokens
                .saturating_sub(baseline.cache_write_one_hour_input_tokens),
            output_tokens: self.output_tokens.saturating_sub(baseline.output_tokens),
            reasoning_output_tokens: self
                .reasoning_output_tokens
                .saturating_sub(baseline.reasoning_output_tokens),
        }
    }
}

/// Normalize actual token breakdowns. `used` is context occupancy, never billable usage.
pub(crate) fn normalize_usage(raw: &Value) -> Option<Value> {
    let value = raw
        .get("tokenUsage")
        .or_else(|| raw.get("usageMetadata"))
        .or_else(|| raw.get("usage"))
        .or_else(|| raw.pointer("/_meta/tokenUsage"))
        .or_else(|| raw.pointer("/_meta/usage"))
        .unwrap_or(raw);
    let total = value
        .get("total")
        .or_else(|| value.get("total_token_usage"));
    let last = value.get("last").or_else(|| value.get("last_token_usage"));
    let (total, last) = match (total, last) {
        (Some(total), last) => {
            let total = Tokens::parse(total)?;
            let last = last
                .and_then(Tokens::parse)
                .unwrap_or_else(|| total.clone());
            (total, last)
        }
        _ => {
            let tokens = Tokens::parse(value)?;
            (tokens.clone(), tokens)
        }
    };
    Some(json!({
        "total": total, "last": last,
        "modelContextWindow": number(value, &["modelContextWindow", "model_context_window"])
            .or_else(|| number(raw, &["size"])),
        "cumulative": value.get("cumulative").and_then(Value::as_bool).unwrap_or(value.get("total_token_usage").is_some()),
        "baselineTotal": value.get("baselineTotal"),
        "requests": value.get("requests").filter(|v| v.is_array()),
    }))
}

/// Context occupancy is the latest request, not the cumulative billing total.
pub(crate) fn context_usage(raw: &Value, updated_at: &str) -> Option<Value> {
    let size = number(raw, &["size", "modelContextWindow", "model_context_window"])?;
    if size == 0 {
        return None;
    }
    let used = number(raw, &["used"]).or_else(|| {
        raw.get("last")
            .or_else(|| raw.get("last_token_usage"))
            .and_then(Tokens::parse)
            .map(|tokens| tokens.total_tokens)
    })?;
    Some(
        json!({"availability":"available", "tokensInContextWindow":used,
        "modelContextWindow":size, "remainingPercent":(100.0 * size.saturating_sub(used) as f64 / size as f64).round(),
        "updatedAt":updated_at}),
    )
}

pub(crate) fn pricing() -> &'static Value {
    static CONFIG: OnceLock<Value> = OnceLock::new();
    CONFIG.get_or_init(|| {
        serde_json::from_str(include_str!("../../../config/codex-model-pricing.json"))
            .expect("bundled model pricing")
    })
}

pub(crate) fn estimate_price(
    usage: &Value,
    model: Option<&str>,
    tier: Option<&str>,
) -> Option<Value> {
    estimate_price_with_catalog(usage, model, tier, pricing(), None)
}

pub(crate) fn estimate_price_with_catalog(
    usage: &Value,
    model: Option<&str>,
    tier: Option<&str>,
    catalog: &Value,
    at: Option<&str>,
) -> Option<Value> {
    if let Some(requests) = usage
        .get("requests")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty())
    {
        // Native Claude children may use a different model from the parent.
        // Price each request snapshot once, retaining its context size and time.
        let mut total = json!({"currency":"USD","inputUsd":0.0,"cachedInputUsd":0.0,
            "cacheWriteInputUsd":0.0,"outputUsd":0.0,"totalUsd":0.0});
        let mut signatures = std::collections::BTreeSet::new();
        let mut models = std::collections::BTreeSet::new();
        for request in requests {
            let price = estimate_price_with_catalog(
                request,
                request["model"].as_str().or(model),
                tier,
                catalog,
                request["observedAt"].as_str().or(at),
            )?;
            for field in [
                "inputUsd",
                "cachedInputUsd",
                "cacheWriteInputUsd",
                "outputUsd",
                "totalUsd",
            ] {
                total[field] = json!(total[field].as_f64()? + price[field].as_f64()?);
            }
            models.insert(price["pricingModelKey"].as_str()?.to_owned());
            signatures.insert(
                json!([
                    price["pricingModelKey"],
                    price["pricingTierKey"],
                    price["ratesSignature"]
                ])
                .to_string(),
            );
        }
        total["ratesSignature"] = json!(serde_json::to_string(&signatures).ok()?);
        total["pricingModelKey"] = json!(if models.len() == 1 {
            models.first()?.as_str()
        } else {
            "mixed"
        });
        total["pricingTierKey"] = json!(tier.unwrap_or("standard"));
        return Some(total);
    }
    let model = crate::pricing::match_model(catalog, model?)?;
    let rates = &catalog["models"][&model];
    for field in [
        "inputUsdPerMillion",
        "cachedInputUsdPerMillion",
        "outputUsdPerMillion",
    ] {
        rates[field].as_f64()?;
    }
    let mut signature_rates = rates.clone();
    for field in ["aliases", "sourceUrl", "verifiedAt", "notes", "custom"] {
        signature_rates.as_object_mut()?.remove(field);
    }
    let signature = signature_rates.to_string();
    if let Some(estimate) = usage.get("priceEstimate").filter(|v| v.is_object()) {
        if estimate["ratesSignature"] == signature
            || (estimate.get("ratesSignature").is_none() && rates["custom"] != true)
        {
            return Some(estimate.clone());
        }
    }
    let tokens = Tokens::parse(usage.get("total")?)?;
    let tier = if tier == Some("fast") && rates["supportsFastMode"] == true {
        "fast"
    } else {
        "standard"
    };
    let multiplier = if tier == "fast" {
        rates
            .get("fastMultiplier")
            .and_then(Value::as_f64)
            .unwrap_or(2.0)
    } else {
        1.0
    };
    let last_input = usage
        .pointer("/last/inputTokens")
        .and_then(Value::as_u64)
        .unwrap_or(tokens.input_tokens);
    let long = rates
        .get("longContextThresholdTokens")
        .and_then(Value::as_u64)
        .is_some_and(|threshold| last_input > threshold);
    let rate = |key: &str| rates.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    use chrono::{Datelike, Timelike};
    let when = at
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    let peak = when.weekday().num_days_from_monday() < 5
        && ((1..4).contains(&when.hour()) || (6..10).contains(&when.hour()));
    let multiplier = multiplier
        * if !peak {
            rates["offPeakMultiplier"].as_f64().unwrap_or(1.0)
        } else {
            1.0
        };
    let multiplier = multiplier
        * if rates["ratesChangeAt"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .is_some_and(|d| when >= d)
        {
            rates["ratesChangeMultiplier"].as_f64().unwrap_or(1.0)
        } else {
            1.0
        };
    let input_multiplier = multiplier
        * if long {
            rate("longContextInputMultiplier").max(1.0)
        } else {
            1.0
        };
    let output_multiplier = multiplier
        * if long {
            rate("longContextOutputMultiplier").max(1.0)
        } else {
            1.0
        };
    let input = tokens
        .input_tokens
        .saturating_sub(tokens.cached_input_tokens)
        .saturating_sub(tokens.cache_write_input_tokens);
    let input_usd = input as f64 * rate("inputUsdPerMillion") * input_multiplier / 1e6;
    let cached_usd =
        tokens.cached_input_tokens as f64 * rate("cachedInputUsdPerMillion") * input_multiplier
            / 1e6;
    let write_rate = rates
        .get("cacheWriteInputUsdPerMillion")
        .and_then(Value::as_f64)
        .unwrap_or(rate("inputUsdPerMillion"));
    let one_hour_rate = rates
        .get("cacheWriteOneHourInputUsdPerMillion")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            // Legacy custom Claude prices did not have a separate one-hour field.
            if model.starts_with("claude-") {
                rate("inputUsdPerMillion") * 2.0
            } else {
                write_rate
            }
        });
    let written_usd = ((tokens.cache_write_input_tokens - tokens.cache_write_one_hour_input_tokens)
        as f64
        * write_rate
        + tokens.cache_write_one_hour_input_tokens as f64 * one_hour_rate)
        * input_multiplier
        / 1e6;
    let output_usd =
        tokens.output_tokens as f64 * rate("outputUsdPerMillion") * output_multiplier / 1e6;
    Some(
        json!({"ratesSignature":signature,"pricingModelKey":model,"pricingTierKey":tier,"currency":"USD","inputUsd":input_usd,"cachedInputUsd":cached_usd,"cacheWriteInputUsd":written_usd,"outputUsd":output_usd,"totalUsd":input_usd+cached_usd+written_usd+output_usd}),
    )
}

pub(crate) fn public_usage(value: &Value) -> Option<Value> {
    let mut usage = json!({"total": Tokens::parse(value.get("total")?)?, "last": Tokens::parse(value.get("last")?)?, "modelContextWindow": value.get("modelContextWindow").filter(|v| v.is_number())});
    if let Some(speed) = value.get("generationSpeed") {
        usage["generationSpeed"] = speed.clone();
    }
    let models = turn_models(value);
    if !models.is_empty() {
        usage["models"] = json!(models);
    }
    Some(usage)
}

/// The models the turn's own agent called, in order: consecutive calls to one
/// model form a run, so a switch mid-turn shows where it took effect.
/// Subagents' calls are left out; records without the `parent` mark predate it.
fn turn_models(value: &Value) -> Vec<Value> {
    let Some(requests) = value.get("requests").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut calls: Vec<(&str, &str)> = requests
        .iter()
        .filter(|request| request.get("parent").and_then(Value::as_bool) == Some(true))
        .filter_map(|request| {
            let model = request["model"]
                .as_str()
                .filter(|m| !m.is_empty() && *m != "<synthetic>")?;
            Some((request["observedAt"].as_str().unwrap_or_default(), model))
        })
        .collect();
    calls.sort_by(|a, b| a.0.cmp(b.0));
    let mut runs: Vec<(&str, u64)> = Vec::new();
    for (_, model) in calls {
        match runs.last_mut() {
            Some((last, count)) if *last == model => *count += 1,
            _ => runs.push((model, 1)),
        }
    }
    runs.into_iter()
        .map(|(model, calls)| json!({"model": model, "calls": calls}))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_usage_lists_the_models_the_turn_switched_between() {
        let raw = json!({"total":{"inputTokens":3,"outputTokens":3},"last":{"inputTokens":1,"outputTokens":1},"requests":[
            {"messageId":"a","model":"claude-sonnet-4-6","parent":true,"observedAt":"2026-10-18T10:02:00Z"},
            {"messageId":"b","model":"claude-opus-4-7","parent":true,"observedAt":"2026-10-18T10:00:00Z"},
            {"messageId":"c","model":"claude-opus-4-7","parent":true,"observedAt":"2026-10-18T10:01:00Z"},
            {"messageId":"d","model":"claude-haiku-4-5","parent":false,"observedAt":"2026-10-18T10:01:30Z"},
            {"messageId":"e","model":"<synthetic>","parent":true,"observedAt":"2026-10-18T10:03:00Z"},
            {"messageId":"f","model":"claude-opus-4-6","observedAt":"2026-10-18T09:00:00Z"}
        ]});
        assert_eq!(
            public_usage(&raw).unwrap()["models"],
            json!([{"model":"claude-opus-4-7","calls":2},{"model":"claude-sonnet-4-6","calls":1}])
        );
        // Switching back is a run of its own.
        let mut back = raw.clone();
        back["requests"].as_array_mut().unwrap().push(
            json!({"messageId":"g","model":"claude-opus-4-7","parent":true,"observedAt":"2026-10-18T10:04:00Z"}),
        );
        assert_eq!(
            public_usage(&back).unwrap()["models"],
            json!([{"model":"claude-opus-4-7","calls":2},{"model":"claude-sonnet-4-6","calls":1},{"model":"claude-opus-4-7","calls":1}])
        );
        let single = json!({"total":{"inputTokens":1,"outputTokens":1},"last":{"inputTokens":1,"outputTokens":1}});
        assert!(public_usage(&single).unwrap().get("models").is_none());
    }

    #[test]
    fn native_request_snapshots_price_mixed_models_and_reprice_custom_rates() {
        let mut catalog = pricing().clone();
        let requests = json!([
            {"messageId":"parent","model":"claude-opus-5-5","total":{"inputTokens":1000,"outputTokens":100},"last":{"inputTokens":1000,"outputTokens":100}},
            {"messageId":"child","model":"claude-haiku-4-5","total":{"inputTokens":1000,"outputTokens":100},"last":{"inputTokens":1000,"outputTokens":100}}
        ]);
        let raw = json!({"total":{"inputTokens":2000,"outputTokens":200},"last":{"inputTokens":1000,"outputTokens":100},"requests":requests});
        let normalized = normalize_usage(&raw).unwrap();
        assert_eq!(normalized["requests"], requests);
        let price =
            estimate_price_with_catalog(&normalized, Some("claude-opus-5-5"), None, &catalog, None)
                .unwrap();
        let expected: f64 = requests
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                estimate_price(r, r["model"].as_str(), None).unwrap()["totalUsd"]
                    .as_f64()
                    .unwrap()
            })
            .sum();
        assert!((price["totalUsd"].as_f64().unwrap() - expected).abs() < 1e-12);
        assert_eq!(price["pricingModelKey"], "mixed");
        let mut saved = normalized;
        saved["priceEstimate"] = price.clone();
        catalog["models"]["claude-haiku-4-5"]["inputUsdPerMillion"] = json!(10.0);
        let repriced =
            estimate_price_with_catalog(&saved, Some("claude-opus-5-5"), None, &catalog, None)
                .unwrap();
        assert!(repriced["totalUsd"].as_f64().unwrap() > expected);
        assert_ne!(repriced["ratesSignature"], price["ratesSignature"]);
        saved["requests"][1]["model"] = json!("unknown-model");
        assert!(estimate_price(&saved, Some("claude-opus-5-5"), None).is_none());
    }

    #[test]
    fn claude_audited_image_turn_prices_one_hour_writes_without_losing_input() {
        let native = json!({"input_tokens":138,"cache_read_input_tokens":14045452,
            "cache_creation_input_tokens":1076364,"output_tokens":56095,
            "cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":1076364}});
        let tokens = Tokens::parse(&native).unwrap();
        assert_eq!(tokens.input_tokens, 15121954);
        assert_eq!(tokens.total_tokens, 15178049);
        assert_eq!(tokens.cache_write_one_hour_input_tokens, 1076364);
        // Normalization, persistence/public DTOs and arithmetic must not add the
        // one-hour subset to input a second time or drop its more expensive rate.
        assert_eq!(Tokens::parse(&json!(tokens)), Some(tokens.clone()));
        assert_eq!(tokens.add(&tokens).subtract(&tokens), tokens);
        let usage = json!({"total":tokens,"last":tokens});
        assert_eq!(
            public_usage(&usage).unwrap()["total"]["cacheWriteOneHourInputTokens"],
            1076364
        );
        let price = estimate_price(&usage, Some("claude-opus-5-5"), None).unwrap();
        assert!((price["inputUsd"].as_f64().unwrap() - 0.000552).abs() < 1e-10);
        assert!((price["cacheWriteInputUsd"].as_f64().unwrap() - 8.610912).abs() < 1e-10);
        assert!((price["totalUsd"].as_f64().unwrap() - 12.5424544).abs() < 1e-10);
    }

    #[test]
    fn mixed_cache_durations_support_custom_rates_fast_mode_and_legacy_usage() {
        let raw = json!({"input_tokens":100,"cache_read_input_tokens":200,"output_tokens":10,
            "cache_creation_input_tokens":100,"cache_creation":{"ephemeral_5m_input_tokens":40,"ephemeral_1h_input_tokens":60}});
        let tokens = Tokens::parse(&raw).unwrap();
        let usage = json!({"total":tokens,"last":tokens});
        let standard = estimate_price(&usage, Some("opus 5.5"), None).unwrap();
        assert!((standard["cacheWriteInputUsd"].as_f64().unwrap() - 0.00068).abs() < 1e-12);
        let fast = estimate_price(&usage, Some("opus 5.5"), Some("fast")).unwrap();
        assert!(
            (fast["totalUsd"].as_f64().unwrap() - 2.0 * standard["totalUsd"].as_f64().unwrap())
                .abs()
                < 1e-12
        );
        let mut catalog = pricing().clone();
        catalog["models"]["claude-opus-5-5"]["cacheWriteOneHourInputUsdPerMillion"] = json!(12.0);
        let custom =
            estimate_price_with_catalog(&usage, Some("opus 5.5"), None, &catalog, None).unwrap();
        assert!((custom["cacheWriteInputUsd"].as_f64().unwrap() - 0.00092).abs() < 1e-12);
        catalog["models"]["claude-opus-5-5"]
            .as_object_mut()
            .unwrap()
            .remove("cacheWriteOneHourInputUsdPerMillion");
        let fallback =
            estimate_price_with_catalog(&usage, Some("opus 5.5"), None, &catalog, None).unwrap();
        assert_eq!(
            fallback["cacheWriteInputUsd"],
            standard["cacheWriteInputUsd"]
        );
        let legacy=Tokens::parse(&json!({"inputTokens":400,"cachedInputTokens":200,"cacheWriteInputTokens":100,"outputTokens":10})).unwrap();
        assert_eq!(legacy.cache_write_one_hour_input_tokens, 0);
        assert!(json!(legacy).get("cacheWriteOneHourInputTokens").is_none());
        assert_eq!(Tokens::parse(&json!({"inputTokens":400,"outputTokens":10,"cacheWriteInputTokens":100,"cacheWriteOneHourInputTokens":999})).unwrap().cache_write_one_hour_input_tokens,100);
    }

    #[test]
    fn gpt_61_sol_prices_caches_output_and_long_context() {
        let usage = json!({"total":{"inputTokens":1000000,"cachedInputTokens":400000,"cacheWriteInputTokens":100000,"outputTokens":100000},"last":{"inputTokens":10000,"outputTokens":1000}});
        let price = estimate_price(&usage, Some("gpt-6.1-sol"), None).unwrap();
        assert!((price["totalUsd"].as_f64().unwrap() - 2.29).abs() < 1e-10);
        assert!(
            (estimate_price(&usage, Some("6.1 Sol"), Some("fast")).unwrap()["totalUsd"]
                .as_f64()
                .unwrap()
                - 4.58)
                .abs()
                < 1e-10
        );
        let mut long = usage.clone();
        long["last"]["inputTokens"] = json!(272001);
        assert!(
            (estimate_price(&long, Some("gpt-6.1-sol"), None).unwrap()["totalUsd"]
                .as_f64()
                .unwrap()
                - 4.08)
                .abs()
                < 1e-10
        );
    }

    #[test]
    fn claude_55_prices_real_model_ids_aliases_caches_and_fast_mode() {
        let usage = json!({"total":{"inputTokens":1000000,"cachedInputTokens":400000,"cacheWriteInputTokens":100000,"outputTokens":100000},"last":{"inputTokens":900000}});
        for alias in ["claude-opus-5-5", "claude-opus-5.5", "opus 5.5", "opus[1m]"] {
            let price = estimate_price(&usage, Some(alias), None).unwrap();
            assert!((price["totalUsd"].as_f64().unwrap() - 4.58).abs() < 1e-10);
            assert!(
                (estimate_price(&usage, Some(alias), Some("fast")).unwrap()["totalUsd"]
                    .as_f64()
                    .unwrap()
                    - 9.16)
                    .abs()
                    < 1e-10
            );
        }
        let sonnet = estimate_price(&usage, Some("claude-sonnet-5-5"), None).unwrap();
        assert!((sonnet["totalUsd"].as_f64().unwrap() - 2.33).abs() < 1e-10);
        assert!(
            (estimate_price(&usage, Some("claude-opus-5"), None).unwrap()["totalUsd"]
                .as_f64()
                .unwrap()
                - 5.825)
                .abs()
                < 1e-10
        );
    }

    #[test]
    fn prices_cached_tokens_and_fast_mode_without_double_charging() {
        let usage = json!({"total":{"inputTokens":1000000,"cachedInputTokens":600000,"outputTokens":10000},"last":{"inputTokens":200000}});
        let price = estimate_price(&usage, Some("openai/gpt-6-astra"), Some("fast")).unwrap();
        assert!((price["totalUsd"].as_f64().unwrap() - 10.2).abs() < 1e-10);
        assert!(estimate_price(&usage, Some("unknown-model"), None).is_none());
    }
}
