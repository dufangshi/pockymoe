//! Indexed turn summaries; never load transcript/tool payloads or contact a harness.
use std::collections::BTreeMap;

use anyhow::{bail, Result};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Timelike, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};

use crate::Supervisor;

enum Zone {
    Iana(chrono_tz::Tz),
    Offset(FixedOffset),
}

impl Zone {
    fn parse(raw: &str) -> Result<Self> {
        if let Ok(minutes) = raw.parse::<i32>() {
            // Signed minutes east of UTC (the inverse of JS getTimezoneOffset).
            if (-840..=840).contains(&minutes) {
                return Ok(Self::Offset(FixedOffset::east_opt(minutes * 60).unwrap()));
            }
        } else if let Ok(zone) = raw.parse::<chrono_tz::Tz>() {
            return Ok(Self::Iana(zone));
        }
        bail!("tz must be an IANA timezone or minutes east of UTC (-840..840)")
    }

    fn date(&self, at: DateTime<Utc>) -> NaiveDate {
        match self {
            Self::Iana(zone) => at.with_timezone(zone).date_naive(),
            Self::Offset(zone) => at.with_timezone(zone).date_naive(),
        }
    }

    fn hour(&self, at: DateTime<Utc>) -> u32 {
        match self {
            Self::Iana(zone) => at.with_timezone(zone).hour(),
            Self::Offset(zone) => at.with_timezone(zone).hour(),
        }
    }
}

const ORIGINS: [&str; 5] = ["owner", "guest", "agent", "automation", "unknown"];

fn days(range: &str) -> Result<i64> {
    match range {
        "today" => Ok(1),
        "7d" => Ok(7),
        "30d" => Ok(30),
        "all" => Ok(0),
        _ => bail!("range must be today, 7d, 30d or all"),
    }
}
pub fn validate_query(range: &str, tz: &str) -> Result<()> {
    days(range)?;
    Zone::parse(tz)?;
    Ok(())
}
pub fn validate_origin(origin: &str) -> Result<()> {
    anyhow::ensure!(
        origin == "all" || ORIGINS.contains(&origin),
        "origin must be all, owner, guest, agent, automation or unknown"
    );
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bucket {
    #[serde(flatten)]
    usage: crate::usage_accounting::UsageSum,
    failed: u64,
}
impl Bucket {
    fn empty() -> Self {
        Self {
            usage: crate::usage_accounting::UsageSum::empty(),
            failed: 0,
        }
    }
    fn add(&mut self, failed: bool, rows: &[crate::usage_accounting::ModelUsage]) {
        self.failed += u64::from(failed);
        self.usage.add_turn(rows);
    }
}

impl Supervisor {
    pub fn stats_summary(&self, range: &str, tz: &str) -> Result<Value> {
        self.stats_summary_with_origin(range, tz, "all")
    }
    pub fn stats_summary_with_origin(&self, range: &str, tz: &str, origin: &str) -> Result<Value> {
        validate_query(range, tz)?;
        validate_origin(origin)?;
        let catalog = self.model_pricing();
        self.db
            .with(|c| summarize_origin(c, range, tz, origin, Utc::now(), &catalog))
    }
}
#[cfg(test)]
fn summarize(
    c: &Connection,
    range: &str,
    tz: &str,
    now: DateTime<Utc>,
    catalog: &Value,
) -> Result<Value> {
    summarize_origin(c, range, tz, "all", now, catalog)
}
fn summarize_origin(
    c: &Connection,
    range: &str,
    tz: &str,
    origin: &str,
    now: DateTime<Utc>,
    catalog: &Value,
) -> Result<Value> {
    validate_origin(origin)?;
    let zone = Zone::parse(tz)?;
    let today = zone.date(now);
    let count = days(range)?;
    let first = (count > 0).then(|| today - Duration::days(count - 1));
    let lower =
        first.map(|d| (d.and_hms_opt(0, 0, 0).unwrap().and_utc() - Duration::days(1)).to_rfc3339());
    let mut daily: BTreeMap<String, Bucket> = BTreeMap::new();
    if let Some(first) = first {
        for day in 0..count {
            daily.insert((first + Duration::days(day)).to_string(), Bucket::empty());
        }
    }
    let mut hourly =
        (range == "today").then(|| (0..24).map(|_| Bucket::empty()).collect::<Vec<_>>());
    let mut models: BTreeMap<String, crate::usage_accounting::UsageSum> = BTreeMap::new();
    let mut origins: BTreeMap<&str, crate::usage_accounting::UsageSum> = ORIGINS
        .into_iter()
        .map(|o| (o, crate::usage_accounting::UsageSum::empty()))
        .collect();
    let mut workspaces: BTreeMap<String, (String, Bucket, Option<String>)> = BTreeMap::new();
    let mut threads: BTreeMap<String, (String, String, u64, u64)> = BTreeMap::new();
    let mut totals = Bucket::empty();
    let (mut completed, mut interrupted, mut undated) = (0u64, 0u64, 0u64);
    let mut durations = Vec::new();
    // Keep a bounded range scan on the expression index for finite ranges.
    let bounds = if count > 0 {
        "julianday(tt.started_at)>=julianday(?1) AND julianday(tt.started_at)<=julianday(?2)"
    } else {
        "(julianday(tt.started_at) IS NULL OR julianday(tt.started_at)<=julianday(?2)) AND ?1 IS NULL"
    };
    let sql=format!("SELECT tt.status,tt.started_at,tt.completed_at,tt.model,tt.token_usage_json,
        tt.pricing_model_key,tt.pricing_tier_key,t.id,t.title,t.workspace_id,w.label,tt.origin
        FROM thread_turns tt JOIN threads t ON t.id=tt.thread_id LEFT JOIN workspaces w ON w.id=t.workspace_id
        WHERE tt.history_copy_of IS NULL AND (?3='all' OR tt.origin=?3) AND {bounds}");
    let mut stmt = c.prepare_cached(&sql)?;
    let mut query = stmt.query(params![lower, now.to_rfc3339(), origin])?;
    while let Some(row) = query.next()? {
        let started: Option<String> = row.get(1)?;
        let start = started
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc));
        let local = start.map(|s| zone.date(s));
        if count > 0 && local.is_none_or(|d| d < first.unwrap() || d > today) {
            continue;
        }
        if local.is_none() {
            undated += 1;
        }
        let status: String = row.get(0)?;
        let end: Option<String> = row.get(2)?;
        let model: Option<String> = row.get(3)?;
        let raw: Option<String> = row.get(4)?;
        let usage: Option<Value> = raw.and_then(|s| serde_json::from_str(&s).ok());
        let price_model: Option<String> = row.get(5)?;
        let tier: Option<String> = row.get(6)?;
        let id: String = row.get(7)?;
        let title: String = row.get(8)?;
        let workspace: String = row.get(9)?;
        let label: Option<String> = row.get(10)?;
        let turn_origin: String = row.get(11)?;
        let parts = crate::usage_accounting::components(
            usage.as_ref(),
            price_model.as_deref().or(model.as_deref()),
            tier.as_deref(),
            catalog,
            started.as_deref(),
        );
        let failed = status == "failed";
        completed += u64::from(status == "completed");
        interrupted += u64::from(status == "interrupted");
        totals.add(failed, &parts);
        if let Some(local) = local {
            daily
                .entry(local.to_string())
                .or_insert_with(Bucket::empty)
                .add(failed, &parts);
            if let Some(hours) = &mut hourly {
                hours[zone.hour(start.unwrap()) as usize].add(failed, &parts);
            }
        }
        origins
            .get_mut(turn_origin.as_str())
            .unwrap()
            .add_turn(&parts);
        let mut per_model: BTreeMap<&str, Vec<crate::usage_accounting::ModelUsage>> =
            BTreeMap::new();
        for part in &parts {
            per_model.entry(&part.model).or_default().push(part.clone());
        }
        for (model, parts) in per_model {
            models
                .entry(model.to_owned())
                .or_insert_with(crate::usage_accounting::UsageSum::empty)
                .add_turn(&parts);
        }
        let finish = end
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc));
        let last = finish
            .filter(|end| start.is_some_and(|s| *end >= s))
            .or(start)
            .map(|d| d.to_rfc3339());
        let ws = workspaces.entry(workspace.clone()).or_insert_with(|| {
            (
                label.unwrap_or_else(|| workspace.clone()),
                Bucket::empty(),
                None,
            )
        });
        ws.1.add(failed, &parts);
        if last > ws.2 {
            ws.2 = last;
        }
        if matches!(status.as_str(), "completed" | "failed" | "interrupted") {
            if let Some(ms) = start
                .zip(finish)
                .filter(|(s, e)| e >= s)
                .map(|(s, e)| (e - s).num_milliseconds() as u64)
            {
                durations.push(ms);
                let entry = threads.entry(id).or_insert((title, workspace, 0, 0));
                entry.2 = entry.2.saturating_add(ms);
                entry.3 += 1;
            }
        }
    }
    durations.sort_unstable();
    let avg = (!durations.is_empty())
        .then(|| durations.iter().map(|d| *d as f64).sum::<f64>() / durations.len() as f64);
    let p50 = if durations.is_empty() {
        None
    } else {
        let m = durations.len() / 2;
        Some(if durations.len() % 2 == 0 {
            (durations[m - 1] as f64 + durations[m] as f64) / 2.0
        } else {
            durations[m] as f64
        })
    };
    let mut longest: Vec<_> = threads.into_iter().collect();
    longest.sort_by(|a, b| {
        (b.1 .2 as f64 / b.1 .3 as f64)
            .total_cmp(&(a.1 .2 as f64 / a.1 .3 as f64))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut by_model: Vec<_> = models.into_iter().collect();
    by_model.sort_by(|a, b| b.1.tokens.cmp(&a.1.tokens).then_with(|| a.0.cmp(&b.0)));
    let mut by_workspace: Vec<_> = workspaces.into_iter().collect();
    by_workspace.sort_by(|a, b| {
        b.1 .1
            .usage
            .turns
            .cmp(&a.1 .1.usage.turns)
            .then_with(|| a.0.cmp(&b.0))
    });
    let active:u64=c.query_row("SELECT count(*) FROM threads WHERE status IN ('running','recovering') AND closed_at IS NULL",[],|r|r.get(0))?;
    let running:u64=c.query_row("SELECT count(*) FROM thread_turns tt JOIN threads t ON t.id=tt.thread_id WHERE tt.history_copy_of IS NULL AND tt.status IN ('inProgress','recovering') AND t.status IN ('running','recovering')",[],|r|r.get(0))?;
    let (mut excluded_undated, mut excluded_copies) = (0u64, 0u64);
    let mut coverage_stmt=c.prepare_cached("SELECT tt.started_at,tt.history_copy_of FROM thread_turns tt JOIN threads t ON t.id=tt.thread_id WHERE ?1='all' OR tt.origin=?1")?;
    let mut coverage_rows = coverage_stmt.query([origin])?;
    while let Some(row) = coverage_rows.next()? {
        let copied: Option<String> = row.get(1)?;
        if copied.is_some() {
            excluded_copies += 1;
            continue;
        }
        let started: Option<String> = row.get(0)?;
        if started
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .is_none()
        {
            excluded_undated += 1;
        }
    }
    let mut total = serde_json::to_value(&totals)?;
    total["completed"] = json!(completed);
    total["interrupted"] = json!(interrupted);
    total["avgDurationMs"] = json!(avg);
    total["p50DurationMs"] = json!(p50);
    total["durationSamples"] = json!(durations.len());
    let mut summary = json!({"range":range,"origin":origin,"generatedAt":now.to_rfc3339(),"totals":total,
        "daily":daily.into_iter().map(|(date,b)|{let mut v=json!(b);v["date"]=json!(date);v}).collect::<Vec<_>>(),
        "byModel":by_model.into_iter().map(|(model,b)|{let mut v=json!(b);v["model"]=json!(model);v}).collect::<Vec<_>>(),
        "byOrigin":origins.iter().map(|(origin,b)|{let mut v=json!(b);v["origin"]=json!(origin);v}).collect::<Vec<_>>(),
        "byWorkspace":by_workspace.into_iter().map(|(id,(label,b,last))|{let mut v=json!(b);v["workspaceId"]=json!(id);v["label"]=json!(label);v["lastActivityAt"]=json!(last);v}).collect::<Vec<_>>(),
        "longestThreads":longest.into_iter().take(5).map(|(id,(title,ws,ms,n))|json!({"threadId":id,"title":title,"workspaceId":ws,"durationMs":ms as f64/n as f64})).collect::<Vec<_>>(),
        "activeThreads":active,"runningTurns":running,
        "coverage":{"source":"persisted","partialUsageTurns":totals.usage.partial_usage_turns,"missingUsageTurns":totals.usage.turns-totals.usage.usage_turns,"unknownOriginTurns":origins["unknown"].turns,"undatedTurns":undated,"excludedUndatedTurns":if count>0 {excluded_undated}else{0},"excludedHistoryCopies":excluded_copies}
    });
    if let Some(hours) = hourly {
        summary["hourly"] = json!(hours
            .into_iter()
            .enumerate()
            .map(|(hour, b)| {
                let mut v = json!(b);
                v["hour"] = json!(hour);
                v
            })
            .collect::<Vec<_>>());
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (tempfile::TempDir, crate::Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::Database::open(&dir.path().join("isolated.sqlite")).unwrap();
        db.with(|conn| {
            conn.execute("INSERT INTO workspaces(id,host_id,label,abs_path,created_at) VALUES ('ws','host','Workspace','/isolated','now')", [])?;
            conn.execute("INSERT INTO threads(id,workspace_id,provider,title,created_at,updated_at,status) VALUES ('thread','ws','codex','Thread','now','now','running')", [])?;
            Ok(())
        }).unwrap();
        (dir, db)
    }

    fn add(
        conn: &Connection,
        id: &str,
        status: &str,
        start: &str,
        end: Option<&str>,
        model: &str,
    ) -> Result<()> {
        let usage = json!({"total":{"inputTokens":100,"outputTokens":50,"cachedInputTokens":20},"last":{"inputTokens":100,"outputTokens":50}});
        conn.execute("INSERT INTO thread_turns(id,thread_id,status,started_at,completed_at,ordinal,model,pricing_model_key,token_usage_json) VALUES (?1,'thread',?2,?3,?4,1,?5,?5,?6)", params![id,status,start,end,model,usage.to_string()])?;
        Ok(())
    }

    #[test]
    fn stats_empty_database_and_invalid_queries() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = crate::Database::open(&dir.path().join("empty.sqlite"))?;
        db.with(|conn| {
            let stats = summarize(
                conn,
                "7d",
                "UTC",
                "2026-11-01T12:00:00Z".parse()?,
                crate::usage::pricing(),
            )?;
            assert!(stats.get("hourly").is_none());
            assert_eq!(stats["totals"]["turns"], 0);
            assert_eq!(stats["totals"]["costUsd"], 0.0);
            assert_eq!(stats["totals"]["durationSamples"], 0);
            assert!(stats["totals"]["avgDurationMs"].is_null());
            assert!(stats["totals"]["p50DurationMs"].is_null());
            assert_eq!(stats["daily"].as_array().unwrap().len(), 7);
            assert_eq!(stats["daily"][0]["date"], "2026-10-26");
            assert_eq!(stats["byModel"], json!([]));
            assert_eq!(stats["activeThreads"], 0);
            Ok(())
        })?;
        for (range, tz) in [("1d", "UTC"), ("7d", "invalid"), ("today", "841")] {
            assert!(validate_query(range, tz).is_err());
        }
        Ok(())
    }

    #[test]
    fn stats_audit_origins_statuses_unknown_dates_and_copied_context_reconcile() -> Result<()> {
        let (_dir, db) = db();
        db.with(|c| {
            for (id, origin, status) in [
                ("o", "owner", "completed"),
                ("g", "guest", "failed"),
                ("a", "agent", "interrupted"),
                ("auto", "automation", "recovering"),
                ("u", "unknown", "completed"),
            ] {
                add(c, id, status, "2026-11-01T10:00:00Z", None, "gpt-6.1-sol")?;
                c.execute(
                    "UPDATE thread_turns SET origin=?1 WHERE id=?2",
                    params![origin, id],
                )?;
            }
            c.execute("UPDATE thread_turns SET started_at=NULL WHERE id='g'", [])?;
            c.execute(
                "UPDATE thread_turns SET started_at='2026-11-01' WHERE id='u'",
                [],
            )?;
            add(
                c,
                "copy",
                "completed",
                "2026-11-01T10:00:00Z",
                None,
                "gpt-6.1-sol",
            )?;
            c.execute(
                "UPDATE thread_turns SET history_copy_of='o' WHERE id='copy'",
                [],
            )?;
            let now = "2026-11-01T12:00:00Z".parse()?;
            let all = summarize(c, "all", "UTC", now, crate::usage::pricing())?;
            assert_eq!(all["totals"]["turns"], 5);
            assert_eq!(all["totals"]["completed"], 2);
            assert_eq!(all["totals"]["failed"], 1);
            assert_eq!(all["totals"]["interrupted"], 1);
            assert_eq!(all["coverage"]["undatedTurns"], 2);
            assert_eq!(all["coverage"]["excludedHistoryCopies"], 1);
            for origin in ORIGINS {
                let filtered =
                    summarize_origin(c, "all", "UTC", origin, now, crate::usage::pricing())?;
                assert_eq!(filtered["totals"]["turns"], 1);
            }
            let today = summarize(c, "today", "UTC", now, crate::usage::pricing())?;
            assert_eq!(today["totals"]["turns"], 3);
            assert_eq!(today["coverage"]["excludedUndatedTurns"], 2);
            Ok(())
        })
    }

    #[test]
    fn stats_audit_all_includes_undated_and_keeps_known_price_subtotal() -> Result<()> {
        let (_dir, db) = db();
        db.with(|c| {
            add(
                c,
                "dated",
                "completed",
                "2026-11-01T10:00:00Z",
                Some("2026-11-01T10:00:01Z"),
                "gpt-6.1-sol",
            )?;
            add(
                c,
                "undated",
                "failed",
                "2026-11-01T10:00:00Z",
                None,
                "unpriced-model",
            )?;
            c.execute(
                "UPDATE thread_turns SET started_at=NULL WHERE id='undated'",
                [],
            )?;
            let stats = summarize(
                c,
                "all",
                "UTC",
                "2026-11-02T12:00:00Z".parse()?,
                crate::usage::pricing(),
            )?;
            assert_eq!(stats["totals"]["turns"], 2);
            assert_eq!(stats["coverage"]["undatedTurns"], 1);
            assert!(stats["totals"]["costUsd"].as_f64().unwrap() > 0.0);
            assert_eq!(stats["totals"]["unpricedTokens"], 150);
            Ok(())
        })
    }

    #[test]
    fn stats_audit_native_requests_use_actual_models_and_canonical_aliases() -> Result<()> {
        let (_dir, db) = db();
        db.with(|c| {
            add(c,"native","completed","2026-11-01T10:00:00Z",None,"opus")?;
            let requests=json!([
                {"messageId":"parent","parent":true,"model":"claude-opus-5-5","total":{"inputTokens":100,"cachedInputTokens":40,"outputTokens":10}},
                {"messageId":"child","parent":false,"model":"gpt-6.1-sol","total":{"inputTokens":200,"cacheWriteInputTokens":20,"outputTokens":30,"reasoningOutputTokens":5}}
            ]);
            c.execute("UPDATE thread_turns SET token_usage_json=?1 WHERE id='native'",[json!({"total":{"inputTokens":300,"cachedInputTokens":40,"cacheWriteInputTokens":20,"outputTokens":40},"last":{"inputTokens":200,"outputTokens":30},"requests":requests}).to_string()])?;
            add(c,"alias","completed","2026-11-01T10:01:00Z",None,"6.1 Sol")?;
            let stats=summarize(c,"today","UTC","2026-11-01T12:00:00Z".parse()?,crate::usage::pricing())?;
            assert_eq!(stats["totals"]["tokens"],490);
            assert_eq!(stats["byModel"].as_array().unwrap().len(),2);
            let sol=stats["byModel"].as_array().unwrap().iter().find(|m|m["model"]=="gpt-6.1-sol").unwrap();
            assert_eq!(sol["turns"],2);
            assert_eq!(sol["tokens"],380);
            assert_eq!(sol["cacheWriteTokens"],20);
            assert_eq!(sol["reasoningTokens"],5);
            assert_eq!(sol["knownRequests"],1);
            assert!(sol["requests"].is_null());
            Ok(())
        })
    }

    #[test]
    fn stats_today_hourly_zero_fill_and_unknown_costs() -> Result<()> {
        let (_dir, db) = db();
        db.with(|conn| {
            let now = "2026-11-02T04:59:00Z".parse()?;
            let empty = summarize(
                conn,
                "today",
                "America/New_York",
                now,
                crate::usage::pricing(),
            )?;
            assert_eq!(empty["hourly"].as_array().unwrap().len(), 24);
            for hour in 0..24 {
                for (key, value) in
                    json!({"hour":hour,"turns":0,"failed":0,"tokens":0,"costUsd":0.0})
                        .as_object()
                        .unwrap()
                {
                    assert_eq!(&empty["hourly"][hour][key], value);
                }
            }
            add(
                conn,
                "before",
                "failed",
                "2026-11-01T03:59:00Z",
                None,
                "unknown",
            )?;
            add(
                conn,
                "first",
                "completed",
                "2026-11-01T05:30:00Z",
                None,
                "gpt-6.1-sol",
            )?;
            add(
                conn,
                "second",
                "failed",
                "2026-11-01T06:30:00Z",
                None,
                "unknown",
            )?;
            add(
                conn,
                "future",
                "failed",
                "2026-11-02T05:00:00Z",
                None,
                "unknown",
            )?;
            let stats = summarize(
                conn,
                "today",
                "America/New_York",
                now,
                crate::usage::pricing(),
            )?;
            assert_eq!(stats["hourly"][1]["turns"], 2);
            assert_eq!(stats["hourly"][1]["failed"], 1);
            assert_eq!(stats["hourly"][1]["tokens"], 300);
            assert!(stats["hourly"][1]["costUsd"].as_f64().unwrap() > 0.0);
            assert_eq!(stats["hourly"][1]["unpricedTokens"], 150);
            assert_eq!(stats["hourly"][23]["costUsd"], 0.0);
            assert_eq!(
                stats["hourly"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|b| b["turns"].as_u64().unwrap())
                    .sum::<u64>(),
                stats["totals"]["turns"].as_u64().unwrap()
            );
            assert_eq!(stats["totals"]["turns"], 2);
            assert!(summarize(
                conn,
                "30d",
                "America/New_York",
                now,
                crate::usage::pricing()
            )?
            .get("hourly")
            .is_none());
            Ok(())
        })
    }

    #[test]
    fn stats_aggregation_prices_statuses_durations_and_unknowns() -> Result<()> {
        let (_dir, db) = db();
        db.with(|conn| {
            add(
                conn,
                "one",
                "completed",
                "2026-11-01T10:00:00Z",
                Some("2026-11-01T10:00:01Z"),
                "gpt-6.1-sol",
            )?;
            add(
                conn,
                "two",
                "failed",
                "2026-11-01T10:01:00Z",
                Some("2026-11-01T10:01:03Z"),
                "gpt-6.1-sol",
            )?;
            add(
                conn,
                "three",
                "interrupted",
                "2026-11-01T10:02:00Z",
                Some("2026-11-01T10:02:05Z"),
                "gpt-6.1-sol",
            )?;
            add(
                conn,
                "four",
                "inProgress",
                "2026-11-01T10:03:00Z",
                None,
                "gpt-6.1-sol",
            )?;
            add(
                conn,
                "outside",
                "completed",
                "2026-10-30T10:00:00Z",
                Some("2026-10-30T10:00:02Z"),
                "unpriced",
            )?;
            let now = "2026-11-01T12:00:00Z".parse()?;
            let stats = summarize(conn, "today", "UTC", now, crate::usage::pricing())?;
            let totals = &stats["totals"];
            assert_eq!(totals["turns"], 4);
            assert_eq!(totals["completed"], 1);
            assert_eq!(totals["failed"], 1);
            assert_eq!(totals["interrupted"], 1);
            assert_eq!(totals["inputTokens"], 400);
            assert_eq!(totals["outputTokens"], 200);
            assert_eq!(totals["cachedTokens"], 80);
            assert_eq!(totals["durationSamples"], 3);
            assert_eq!(totals["avgDurationMs"], 3000.0);
            assert_eq!(totals["p50DurationMs"], 3000.0);
            let expected = crate::usage::estimate_price(
                &json!({"total":{"inputTokens":100,"outputTokens":50,"cachedInputTokens":20}}),
                Some("gpt-6.1-sol"),
                None,
            )
            .unwrap()["totalUsd"]
                .as_f64()
                .unwrap()
                * 4.0;
            assert!((totals["costUsd"].as_f64().unwrap() - expected).abs() < 1e-10);
            assert_eq!(stats["hourly"].as_array().unwrap().len(), 24);
            assert_eq!(stats["hourly"][10]["hour"], 10);
            assert_eq!(stats["hourly"][10]["turns"], 4);
            assert_eq!(stats["hourly"][10]["failed"], 1);
            assert_eq!(stats["hourly"][10]["tokens"], 600);
            assert_eq!(stats["hourly"][10]["costUsd"], totals["costUsd"]);
            assert_eq!(stats["daily"][0]["tokens"], 600);
            assert_eq!(stats["byModel"][0]["turns"], 4);
            assert_eq!(stats["byWorkspace"][0]["label"], "Workspace");
            assert_eq!(
                stats["byWorkspace"][0]["lastActivityAt"],
                "2026-11-01T10:03:00+00:00"
            );
            assert_eq!(stats["longestThreads"][0]["durationMs"], 3000.0);
            assert_eq!(stats["activeThreads"], 1);
            assert_eq!(stats["runningTurns"], 1);
            let seven = summarize(conn, "7d", "UTC", now, crate::usage::pricing())?;
            assert_eq!(seven["totals"]["costUsd"], totals["costUsd"]);
            assert_eq!(seven["totals"]["unpricedTokens"], 150);
            assert_eq!(seven["byWorkspace"][0]["costUsd"], totals["costUsd"]);
            assert!(seven["byModel"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["model"] == "unpriced" && m["costUsd"].is_null()));
            Ok(())
        })
    }

    #[test]
    fn stats_duration_samples_exclude_invalid_missing_and_live_durations() -> Result<()> {
        let (_dir, db) = db();
        db.with(|conn| {
            for (id, status, end) in [
                ("valid", "completed", Some("2026-11-01T10:00:01Z")),
                ("failed", "failed", Some("2026-11-01T10:00:03Z")),
                ("missing", "completed", None),
                ("invalid", "interrupted", Some("invalid")),
                ("negative", "completed", Some("2026-11-01T09:59:59Z")),
                ("live", "inProgress", Some("2026-11-01T10:00:05Z")),
            ] {
                add(conn, id, status, "2026-11-01T10:00:00Z", end, "gpt-6.1-sol")?;
            }
            let stats = summarize(
                conn,
                "today",
                "UTC",
                "2026-11-01T12:00:00Z".parse()?,
                crate::usage::pricing(),
            )?;
            assert_eq!(stats["totals"]["turns"], 6);
            assert_eq!(stats["totals"]["durationSamples"], 2);
            assert_eq!(stats["totals"]["avgDurationMs"], 2000.0);
            assert_eq!(stats["totals"]["p50DurationMs"], 2000.0);
            Ok(())
        })
    }

    #[test]
    fn stats_timezone_buckets_include_dst_and_signed_offsets() -> Result<()> {
        let (_dir, db) = db();
        db.with(|conn| {
            // New York's fall-back day is 25 hours; both 01:30 occurrences count.
            for (id, start) in [
                ("before", "2026-11-01T03:59:00Z"),
                ("first", "2026-11-01T05:30:00Z"),
                ("second", "2026-11-01T06:30:00Z"),
                ("late", "2026-11-02T04:30:00Z"),
            ] {
                add(conn, id, "completed", start, Some(start), "gpt-6.1-sol")?;
            }
            let now = "2026-11-02T04:59:00Z".parse()?;
            let stats = summarize(
                conn,
                "today",
                "America/New_York",
                now,
                crate::usage::pricing(),
            )?;
            assert_eq!(stats["daily"][0]["date"], "2026-11-01");
            assert_eq!(stats["totals"]["turns"], 3);
            assert_eq!(stats["hourly"][1]["turns"], 2);
            assert_eq!(stats["hourly"][23]["turns"], 1);
            assert_eq!(stats["hourly"][0]["turns"], 0);
            let offset = summarize(conn, "today", "-300", now, crate::usage::pricing())?;
            assert_eq!(offset["totals"]["turns"], 3);
            assert_eq!(offset["hourly"][0]["turns"], 1);
            assert_eq!(offset["hourly"][1]["turns"], 1);
            let east = summarize(conn, "today", "330", now, crate::usage::pricing())?;
            assert_eq!(east["daily"][0]["date"], "2026-11-02");
            assert_eq!(east["totals"]["turns"], 1);
            assert_eq!(east["hourly"][10]["turns"], 1);
            // An explicitly offset timestamp must be included by the UTC index.
            add(
                conn,
                "offset",
                "completed",
                "2026-11-01T23:45:00-05:00",
                Some("2026-11-01T23:45:01-05:00"),
                "gpt-6.1-sol",
            )?;
            assert_eq!(
                summarize(
                    conn,
                    "today",
                    "America/New_York",
                    now,
                    crate::usage::pricing()
                )?["totals"]["turns"],
                4
            );
            // Spring-forward keeps a 23-hour calendar day.
            add(
                conn,
                "spring-before",
                "completed",
                "2026-03-08T04:59:00Z",
                None,
                "gpt-6.1-sol",
            )?;
            add(
                conn,
                "spring-first",
                "completed",
                "2026-03-08T05:00:00Z",
                None,
                "gpt-6.1-sol",
            )?;
            add(
                conn,
                "spring-last",
                "completed",
                "2026-03-09T03:59:00Z",
                None,
                "gpt-6.1-sol",
            )?;
            let spring = summarize(
                conn,
                "today",
                "America/New_York",
                "2026-03-09T03:59:30Z".parse()?,
                crate::usage::pricing(),
            )?;
            assert_eq!(spring["totals"]["turns"], 2);
            assert_eq!(spring["hourly"][0]["turns"], 1);
            assert_eq!(spring["hourly"][2]["turns"], 0);
            assert_eq!(spring["hourly"][23]["turns"], 1);
            Ok(())
        })
    }

    #[test]
    #[ignore = "latency benchmark: run with --release --ignored --nocapture"]
    fn stats_indexed_five_thousand_turns_under_150ms() -> Result<()> {
        let (_dir, db) = db();
        db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            for i in 0..5000 { add(&tx,&format!("turn-{i}"),"completed","2026-11-01T10:00:00Z",Some("2026-11-01T10:00:01Z"),"gpt-6.1-sol")?; }
            tx.commit()?;
            let plan: String = conn.query_row("EXPLAIN QUERY PLAN SELECT id FROM thread_turns WHERE julianday(started_at)>=julianday('2026-10-31') AND julianday(started_at)<=julianday('2026-11-02')", [], |r|r.get(3))?;
            assert!(plan.contains("thread_turns_started_day_idx"),"{plan}");
            let start = std::time::Instant::now();
            let stats = summarize(conn,"30d","America/Toronto","2026-11-01T12:00:00Z".parse()?,crate::usage::pricing())?;
            let elapsed = start.elapsed();
            eprintln!("5000-turn summary: {elapsed:?}");
            assert_eq!(stats["totals"]["turns"],5000);
            assert!(elapsed < std::time::Duration::from_millis(150),"{elapsed:?}");
            Ok(())
        })
    }
}
