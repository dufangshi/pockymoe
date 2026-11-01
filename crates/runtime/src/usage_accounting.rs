//! Shared reporting of persisted usage. Request snapshots replace the parent
//! aggregate; they are never added to it. No transcript I/O or guessed requests.
use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::usage::Tokens;

#[derive(Clone)]
pub(crate) struct ModelUsage {
    pub model: String,
    pub tokens: Option<Tokens>,
    pub cost: Option<f64>,
    pub known_requests: u64,
    pub requests_complete: bool,
    pub usage_complete: bool,
}

pub(crate) fn components(
    usage: Option<&Value>,
    model: Option<&str>,
    tier: Option<&str>,
    catalog: &Value,
    at: Option<&str>,
) -> Vec<ModelUsage> {
    let one = |usage: Option<&Value>, model: Option<&str>, known_requests, requests_complete| {
        let normalized = usage.and_then(crate::usage::normalize_usage);
        let tokens = normalized.as_ref().and_then(|u| Tokens::parse(&u["total"]));
        let price_usage = usage
            .filter(|u| u["total"].is_object())
            .or(normalized.as_ref());
        let cost = price_usage
            .and_then(|u| crate::usage::estimate_price_with_catalog(u, model, tier, catalog, at))
            .and_then(|p| p["totalUsd"].as_f64());
        ModelUsage {
            model: model
                .and_then(|m| crate::pricing::match_model(catalog, m))
                .or_else(|| model.filter(|m| !m.is_empty()).map(str::to_owned))
                .unwrap_or_else(|| "unknown".into()),
            usage_complete: tokens.is_some(),
            tokens,
            cost,
            known_requests,
            requests_complete,
        }
    };
    if let Some(requests) = usage
        .and_then(|u| u["requests"].as_array())
        .filter(|r| !r.is_empty())
    {
        return request_snapshots(requests)
            .into_iter()
            .map(|r| {
                let known = r["model"] != "<synthetic>"
                    && (r["messageId"].is_string() || r["requestId"].is_string());
                let mut row = one(Some(r), r["model"].as_str(), u64::from(known), known);
                if usage.is_some_and(|u| {
                    matches!(
                        u["source"].as_str(),
                        Some("claudeImport" | "claudeScheduledImport")
                    )
                }) {
                    row.requests_complete = false;
                    row.usage_complete = false;
                }
                row.cost = crate::usage::estimate_price_with_catalog(
                    r,
                    r["model"].as_str(),
                    tier,
                    catalog,
                    r["observedAt"].as_str().or(at),
                )
                .and_then(|p| p["totalUsd"].as_f64());
                row
            })
            .collect();
    }
    vec![one(usage, model, 0, false)]
}

/// Keep the largest response snapshot per native ID. IDs are unique within
/// the owning turn; anonymous records remain unknown rather than guessed calls.
pub(crate) fn request_snapshots(requests: &[Value]) -> Vec<&Value> {
    let mut unique: BTreeMap<&str, &Value> = BTreeMap::new();
    let mut anonymous = Vec::new();
    for request in requests {
        if let Some(id) = request["messageId"]
            .as_str()
            .or(request["requestId"].as_str())
        {
            let previous = unique.entry(id).or_insert(request);
            let output = |v: &Value| {
                v.pointer("/total/outputTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            };
            if output(request) >= output(previous) {
                *previous = request;
            }
        } else {
            anonymous.push(request);
        }
    }
    unique.into_values().chain(anonymous).collect()
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageSum {
    pub turns: u64,
    pub usage_turns: u64,
    pub partial_usage_turns: u64,
    pub tokens: u64,
    pub input_tokens: u64,
    pub cached_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub known_requests: u64,
    pub requests: Option<u64>,
    pub cost_usd: Option<f64>,
    pub unpriced_tokens: u64,
}

impl UsageSum {
    pub fn empty() -> Self {
        Self {
            cost_usd: Some(0.0),
            requests: Some(0),
            ..Self::default()
        }
    }
    /// Call once per turn for this group; a model can have many requests in it.
    pub fn add_turn(&mut self, rows: &[ModelUsage]) {
        if self.turns == 0 {
            self.cost_usd = None;
        }
        self.turns += 1;
        self.partial_usage_turns += u64::from(rows.iter().any(|r| !r.usage_complete));
        self.usage_turns += u64::from(rows.iter().any(|r| r.tokens.is_some()));
        for row in rows {
            self.known_requests += row.known_requests;
            self.requests = self
                .requests
                .filter(|_| row.requests_complete)
                .map(|n| n + row.known_requests);
            if let Some(t) = &row.tokens {
                self.tokens = self.tokens.saturating_add(t.total_tokens);
                self.input_tokens = self.input_tokens.saturating_add(t.input_tokens);
                self.cached_tokens = self.cached_tokens.saturating_add(t.cached_input_tokens);
                self.cache_write_tokens = self
                    .cache_write_tokens
                    .saturating_add(t.cache_write_input_tokens);
                self.output_tokens = self.output_tokens.saturating_add(t.output_tokens);
                self.reasoning_tokens = self
                    .reasoning_tokens
                    .saturating_add(t.reasoning_output_tokens);
                if row.cost.is_none() {
                    self.unpriced_tokens = self.unpriced_tokens.saturating_add(t.total_tokens);
                }
            }
            if let Some(cost) = row.cost {
                self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + cost);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn accounting_request_snapshots_keep_latest_and_unknown_prices_preserve_priced_siblings() {
        let usage = json!({"total":{"inputTokens":999999,"outputTokens":999999},"requests":[
            {"messageId":"same","model":"gpt-6.1-sol","total":{"inputTokens":100,"outputTokens":1}},
            {"messageId":"same","model":"gpt-6.1-sol","total":{"inputTokens":100,"outputTokens":10}},
            {"messageId":"child","model":"unpriced-child","total":{"inputTokens":200,"cachedInputTokens":80,"cacheWriteInputTokens":20,"outputTokens":30,"reasoningOutputTokens":5}}
        ]});
        let parts = components(
            Some(&usage),
            Some("parent"),
            None,
            crate::usage::pricing(),
            None,
        );
        assert_eq!(parts.len(), 2);
        let mut sum = UsageSum::empty();
        sum.add_turn(&parts);
        assert_eq!(sum.turns, 1);
        assert_eq!(sum.requests, Some(2));
        assert_eq!(sum.tokens, 340);
        assert_eq!(sum.cached_tokens, 80);
        assert_eq!(sum.cache_write_tokens, 20);
        assert_eq!(sum.reasoning_tokens, 5);
        assert_eq!(sum.unpriced_tokens, 230);
        assert!(sum.cost_usd.unwrap() > 0.0);
        assert_eq!(
            crate::usage::public_usage(&usage).unwrap()["total"]["totalTokens"],
            340
        );
        let price = crate::usage::estimate_price(&usage, Some("parent"), None).unwrap();
        assert_eq!(price["unpricedTokens"], 230);
        assert!((price["totalUsd"].as_f64().unwrap() - sum.cost_usd.unwrap()).abs() < 1e-12);
    }

    #[test]
    fn accounting_legacy_flat_usage_is_normalized_and_imported_request_coverage_is_partial() {
        let raw = json!({"input_tokens":100,"cache_read_input_tokens":20,"cache_creation_input_tokens":10,"output_tokens":5});
        let parts = components(
            Some(&raw),
            Some("claude-opus-5-5"),
            None,
            crate::usage::pricing(),
            None,
        );
        let mut sum = UsageSum::empty();
        sum.add_turn(&parts);
        assert_eq!(sum.tokens, 135);
        assert_eq!(sum.input_tokens, 130);
        assert!(sum.requests.is_none());
        assert_eq!(
            crate::usage::public_usage(&raw).unwrap()["total"]["totalTokens"],
            135
        );
        assert!(crate::usage::estimate_price(&raw, Some("claude-opus-5-5"), None).is_some());
        let imported = json!({"source":"claudeImport","requests":[{"messageId":"parent","model":"claude-opus-5-5","total":{"inputTokens":100,"outputTokens":5}}]});
        let mut sum = UsageSum::empty();
        sum.add_turn(&components(
            Some(&imported),
            None,
            None,
            crate::usage::pricing(),
            None,
        ));
        assert_eq!(sum.usage_turns, 1);
        assert_eq!(sum.partial_usage_turns, 1);
        assert_eq!(sum.known_requests, 1);
        assert!(sum.requests.is_none());
    }
}
