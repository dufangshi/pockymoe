//! Lifetime ledger statistics, independent of the bounded execution-history API.
use super::*;
use crate::usage::Tokens;
use std::collections::BTreeSet;

#[derive(Default)]
struct Statistics {
    triggers: u64,
    runs: u64,
    actions: u64,
    running: u64,
    turns: BTreeSet<String>,
    unattributed: u64,
}

impl Supervisor {
    pub(super) fn automation_statistics(&self, thread: &str) -> Result<HashMap<String, Value>> {
        // All metadata, including persisted running usage, with the same per-turn
        // pricing catalog as native watches and the timeline. Never reprice a sum.
        let mut turns = self.load_turns_meta(thread)?;
        let catalog = self.model_pricing();
        let archived=self.db.with(|c|{let mut query=c.prepare("SELECT id,status,NULL,model,NULL,token_usage_json,started_at,completed_at,pricing_model_key,pricing_tier_key,origin FROM rewound_turn_accounting WHERE thread_id=?1")?;let rows=query.query_map([thread],|r|turn_meta_row(r,&catalog))?.collect::<rusqlite::Result<Vec<_>>>()?;Ok(rows)})?;
        turns.extend(archived);
        let mut stats = self.db.with(|c| {
            let mut stats = HashMap::<String, Statistics>::new();
            let mut q = c.prepare("SELECT id FROM automations WHERE thread_id=?1")?;
            for id in q.query_map([thread], |r| r.get::<_, String>(0))? {
                stats.insert(id?, Statistics::default());
            }
            let mut q = c.prepare("SELECT r.automation_id,r.missed_count,r.turn_id,r.started_at,r.state,json_extract(r.definition_json,'$.action.kind') FROM automation_runs r JOIN automations a ON a.id=r.automation_id WHERE a.thread_id=?1")?;
            let rows = q.query_map([thread], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?)))?;
            for row in rows {
                let (id, missed, turn, started, state, action) = row?;
                let entry = stats.entry(id).or_default();
                entry.runs += 1;
                entry.running += u64::from(state == "running");
                // A coalesced row represents one occurrence plus missed ticks,
                // but its single associated turn must be billed only once.
                entry.triggers = entry.triggers.saturating_add(1u64.saturating_add(missed));
                entry.actions += u64::from(started.is_some() || (action == "notifyInbox" && state == "completed"));
                if action == "prompt" {
                    if let Some(turn) = turn.filter(|id| !id.is_empty()) {
                        entry.turns.insert(turn);
                    } else if started.is_some() || matches!(state.as_str(), "running" | "completed" | "uncertain") {
                        entry.unattributed += 1;
                    }
                }
            }
            Ok(stats)
        })?;
        let mut owners = HashMap::<String, u64>::new();
        for entry in stats.values() {
            for turn in &entry.turns {
                *owners.entry(turn.clone()).or_default() += 1;
            }
        }
        let turns: HashMap<_, _> = turns.iter().map(|turn| (turn.id.as_str(), turn)).collect();
        Ok(stats.drain().map(|(id, entry)| {
            let mut tokens = Tokens::default();
            let mut price = json!({"currency":"USD","inputUsd":0.0,"cachedInputUsd":0.0,"cacheWriteInputUsd":0.0,"outputUsd":0.0,"totalUsd":0.0});
            let (mut usage_count, mut priced_count, mut ambiguous, mut missing) = (0u64, 0u64, 0u64, 0u64);
            for turn_id in &entry.turns {
                if owners.get(turn_id).copied().unwrap_or_default() > 1 {
                    ambiguous += 1;
                    continue;
                }
                let Some(turn) = turns.get(turn_id.as_str()) else {
                    missing += 1;
                    continue;
                };
                if let Some(usage) = turn.token_usage.as_ref().and_then(|u| Tokens::parse(&u["total"])) {
                    tokens = tokens.add(&usage);
                    usage_count += 1;
                }
                if let Some(estimate) = turn.price_estimate.as_ref().filter(|p| p["totalUsd"].as_f64().is_some_and(|n| n.is_finite() && n >= 0.0)) {
                    for field in ["inputUsd", "cachedInputUsd", "cacheWriteInputUsd", "outputUsd", "totalUsd"] {
                        price[field] = json!(price[field].as_f64().unwrap_or_default() + estimate[field].as_f64().unwrap_or_default());
                    }
                    priced_count += 1;
                }
            }
            // Zero is known only when no potentially charged turn is missing.
            let zero = entry.turns.is_empty() && entry.unattributed == 0;
            (id, json!({
                "triggerCount":entry.triggers,"runCount":entry.runs,"executedActionCount":entry.actions,"runningActionCount":entry.running,
                "promptTurnCount":entry.turns.len(),"ambiguousTurnCount":ambiguous,"missingTurnCount":missing,
                "unattributedRunCount":entry.unattributed,"usageTurnCount":usage_count,"pricedTurnCount":priced_count,
                "tokenUsage":if usage_count > 0 || zero {json!(tokens)} else {Value::Null},
                "priceEstimate":if priced_count > 0 || zero {price} else {Value::Null}
            }))
        }).collect())
    }
}
