//! Read-only account usage, independent of conversation writers and ACP prompts.
use crate::acp::rpc::{parse_spawn_command, AcpProcess};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const MAX_STALE_AGE: Duration = Duration::from_secs(30 * 60);
const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);

enum RefreshResult {
    Available(Value),
    // No subscription credentials (or rejected credentials): do not retain old account data.
    Unavailable,
    TemporaryFailure(Option<Duration>),
}

struct CacheEntry {
    refresh_at: Instant,
    success: Option<(Instant, Value)>,
    failures: u32,
    unavailable: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionUsageReport {
    pub usage: Option<Value>,
    pub unavailable: bool,
}

impl CacheEntry {
    fn new(now: Instant) -> Self {
        Self {
            refresh_at: now,
            success: None,
            failures: 0,
            unavailable: false,
        }
    }

    fn value(&self, now: Instant) -> Option<Value> {
        let (observed, value) = self.success.as_ref()?;
        if now.duration_since(*observed) >= MAX_STALE_AGE {
            return None;
        }
        let mut value = value.clone();
        value["stale"] = json!(self.failures > 0);
        Some(value)
    }

    fn update(&mut self, result: RefreshResult, now: Instant) -> Option<Value> {
        let delay = match result {
            RefreshResult::Available(value) => {
                self.success = Some((now, value));
                self.failures = 0;
                self.unavailable = false;
                REFRESH_INTERVAL
            }
            RefreshResult::Unavailable => {
                self.success = None;
                self.failures = 0;
                self.unavailable = true;
                REFRESH_INTERVAL
            }
            RefreshResult::TemporaryFailure(retry_after) => {
                self.failures = self.failures.saturating_add(1);
                self.unavailable = false;
                let backoff = (REFRESH_INTERVAL * (1 << self.failures.min(4).saturating_sub(1)))
                    .min(MAX_BACKOFF);
                // Some OAuth usage responses say Retry-After: 0. Never retry immediately.
                backoff.max(retry_after.unwrap_or_default())
            }
        };
        self.refresh_at = now + delay;
        self.value(now)
    }
}

#[derive(Default)]
pub struct SubscriptionUsage {
    cache: Mutex<HashMap<String, CacheEntry>>,
    /// Allowance reported by a running harness itself (Codex app-server
    /// `account/rateLimits/updated`, rollout `token_count.rate_limits`). It is
    /// newer than the periodic account read while a turn runs.
    observed: std::sync::Mutex<HashMap<String, (Instant, Value)>>,
}

impl SubscriptionUsage {
    pub async fn read(&self, provider: &str) -> SubscriptionUsageReport {
        self.read_with(provider, async {
            match provider {
                "claude" => claude().await,
                "codex" => codex()
                    .await
                    .map(RefreshResult::Available)
                    .unwrap_or(RefreshResult::Unavailable),
                "grok" => grok()
                    .await
                    .map(RefreshResult::Available)
                    .unwrap_or(RefreshResult::Unavailable),
                _ => RefreshResult::Unavailable,
            }
        })
        .await
    }

    /// Merges a harness-reported update (sparse: absent windows are unchanged)
    /// into the newest known report. Returns whether anything changed.
    pub fn observe(&self, provider: &str, update: &Value) -> bool {
        let now = Instant::now();
        let previous = self.latest(provider, now);
        let Some(merged) = merge_windows(previous.as_ref(), update) else {
            return false;
        };
        let changed = previous.as_ref().map(|p| &p["windows"]) != Some(&merged["windows"]);
        self.observed
            .lock()
            .unwrap()
            .insert(provider.into(), (now, merged));
        changed
    }

    fn latest(&self, provider: &str, now: Instant) -> Option<Value> {
        let observed = self.observed.lock().unwrap().get(provider).cloned();
        let cached = self
            .cache
            .try_lock()
            .ok()
            .and_then(|cache| cache.get(provider).and_then(|e| e.success.clone()));
        [observed, cached]
            .into_iter()
            .flatten()
            .filter(|(at, _)| now.duration_since(*at) < MAX_STALE_AGE)
            .max_by_key(|(at, _)| *at)
            .map(|(_, value)| value)
    }

    /// A fresh harness observation wins over an older account read, including
    /// a definitive "unavailable" from the device's own login: the harness
    /// reports the account it actually runs on.
    fn prefer_observed(
        &self,
        provider: &str,
        entry: &CacheEntry,
        report: SubscriptionUsageReport,
        now: Instant,
    ) -> SubscriptionUsageReport {
        let observed = self.observed.lock().unwrap().get(provider).cloned();
        let Some((at, value)) = observed.filter(|(at, _)| now.duration_since(*at) < MAX_STALE_AGE)
        else {
            return report;
        };
        if entry.success.as_ref().is_some_and(|(read, _)| *read >= at) {
            return report;
        }
        let mut value = value;
        value["stale"] = json!(false);
        SubscriptionUsageReport {
            usage: Some(value),
            unavailable: false,
        }
    }

    async fn read_with(
        &self,
        provider: &str,
        fetch: impl std::future::Future<Output = RefreshResult>,
    ) -> SubscriptionUsageReport {
        // All threads in this Supervisor share the provider cache. Serialize misses so
        // concurrent viewers cannot create a burst of upstream OAuth requests.
        let mut cache = self.cache.lock().await;
        let now = Instant::now();
        if let Some(entry) = cache.get(provider) {
            if now < entry.refresh_at {
                let report = SubscriptionUsageReport {
                    usage: entry.value(now),
                    unavailable: entry.unavailable,
                };
                return self.prefer_observed(provider, entry, report, now);
            }
        }
        let result = tokio::time::timeout(Duration::from_secs(12), fetch)
            .await
            .unwrap_or(RefreshResult::TemporaryFailure(None));
        let now = Instant::now();
        let entry = cache
            .entry(provider.into())
            .or_insert_with(|| CacheEntry::new(now));
        let usage = entry.update(result, now);
        let report = SubscriptionUsageReport {
            usage,
            unavailable: entry.unavailable,
        };
        self.prefer_observed(provider, entry, report, now)
    }
}

fn merge_windows(previous: Option<&Value>, update: &Value) -> Option<Value> {
    let fresh = update["windows"].as_array().filter(|w| !w.is_empty())?;
    let mut windows = fresh.clone();
    let now = Utc::now();
    for old in previous
        .and_then(|p| p["windows"].as_array())
        .into_iter()
        .flatten()
    {
        let kept = !windows.iter().any(|w| w["id"] == old["id"])
            && old["resetsAt"]
                .as_str()
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .is_some_and(|t| t > now);
        if kept {
            windows.push(old.clone());
        }
    }
    windows.sort_by_key(|w| w["durationMinutes"].as_i64().unwrap_or(i64::MAX));
    let mut merged = update.clone();
    merged["windows"] = json!(windows);
    Some(merged)
}

/// A Codex `RateLimitSnapshot` from app-server `account/rateLimits/updated`.
/// Only the account's own `codex` limit feeds the badge; per-model aliases do not.
pub(crate) fn codex_rate_update(snapshot: &Value) -> Option<Value> {
    if !snapshot["limitId"].is_null() && snapshot["limitId"] != "codex" {
        return None;
    }
    parse_codex(&json!({ "rateLimits": snapshot }))
}

/// The same snapshot as Codex records it in a rollout's `token_count` event.
pub(crate) fn codex_rollout_rate_limits(rate_limits: &Value) -> Option<Value> {
    let window = |w: &Value| {
        (!w.is_null()).then(|| {
            json!({"usedPercent": w["used_percent"], "windowDurationMins": w["window_minutes"], "resetsAt": w["resets_at"]})
        })
    };
    codex_rate_update(&json!({
        "limitId": rate_limits["limit_id"],
        "primary": window(&rate_limits["primary"]),
        "secondary": window(&rate_limits["secondary"]),
    }))
}

/// A Claude Code SDK `rate_limit_event` (`rate_limit_info`), the allowance the
/// running CLI just received from the API. Its utilization is a fraction; the
/// OAuth usage read reports percent. Only the 5h and 7d windows feed the badge.
pub(crate) fn claude_rate_update(info: &Value) -> Option<Value> {
    let windows: Vec<Value> = [("five_hour", 300), ("seven_day", 10080)]
        .into_iter()
        .filter_map(|(id, minutes)| {
            let (used, resets) = match info["unifiedWindows"].get(id) {
                Some(w) => (&w["utilization"], &w["resetsAt"]),
                None if info["rateLimitType"] == id => (&info["utilization"], &info["resetsAt"]),
                None => return None,
            };
            let percent = json!(used.as_f64()? * 100.0);
            window(id, minutes, &percent, resets)
        })
        .collect();
    report("claude", windows)
}

/// One turn's announcements: a harness can report the same allowance through
/// more than one source (app-server push and rollout record).
#[derive(Default)]
pub(crate) struct Announcer {
    last: Option<Value>,
}

impl Announcer {
    pub(crate) fn announce(
        &mut self,
        bus: &crate::actor::EventBus,
        thread_id: &str,
        provider: &str,
        usage: Value,
        hidden: bool,
    ) {
        let windows = usage["windows"].clone();
        if self.last.as_ref() == Some(&windows) {
            return;
        }
        self.last = Some(windows);
        announce(bus, thread_id, provider, usage, hidden);
    }
}

/// Tells the owner's viewers that the harness reported fresher allowance. The
/// event carries no account data: viewers re-read the owner-only usage route.
pub(crate) fn announce(
    bus: &crate::actor::EventBus,
    thread_id: &str,
    provider: &str,
    usage: Value,
    hidden: bool,
) {
    if hidden {
        return;
    }
    bus.emit(pockymoe_protocol::ThreadEventEnvelope {
        event_type: pockymoe_protocol::ThreadEventType::RuntimeSubscriptionUpdated,
        thread_id: thread_id.into(),
        timestamp: pockymoe_protocol::now_rfc3339(),
        payload: json!({ "provider": provider, "usage": usage }),
    });
}

fn report(provider: &str, windows: Vec<Value>) -> Option<Value> {
    if windows.is_empty() {
        return None;
    }
    Some(
        json!({"provider":provider,"authKind":"subscription","observedAt":Utc::now().to_rfc3339(),"stale":false,"windows":windows}),
    )
}
fn reset(value: &Value) -> Option<String> {
    value
        .as_i64()
        .and_then(|s| DateTime::<Utc>::from_timestamp(s, 0))
        .or_else(|| {
            value.as_str().and_then(|s| {
                DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            })
        })
        .map(|d| d.to_rfc3339())
}
fn window(id: &str, minutes: i64, used: &Value, resets: &Value) -> Option<Value> {
    let used = used.as_f64().filter(|v| v.is_finite() && *v >= 0.0)?;
    let resets = reset(resets)?;
    if DateTime::parse_from_rfc3339(&resets).ok()? <= Utc::now() {
        return None;
    }
    let label = if minutes % 1440 == 0 {
        format!("{}d", minutes / 1440)
    } else if minutes % 60 == 0 {
        format!("{}h", minutes / 60)
    } else {
        format!("{}m", minutes)
    };
    Some(
        json!({"id":id,"durationMinutes":minutes,"label":label,"usedPercent":used.min(100.0),"resetsAt":resets}),
    )
}
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
fn has_env(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !v.trim().is_empty())
}

async fn codex() -> Option<Value> {
    let command = format!(
        "{} app-server",
        std::env::var("CODEX_COMMAND").unwrap_or_else(|_| "codex".into())
    );
    let (process, _, _) = AcpProcess::spawn(&command, home()?.to_str()?, &[])
        .await
        .ok()?;
    process
        .request(
            "initialize",
            json!({"clientInfo":{"name":"pockymoe-usage","version":"1"},"capabilities":{}}),
        )
        .await
        .ok()?;
    process.notify("initialized", json!({})).await.ok()?;
    let account = process
        .request("account/read", json!({"refreshToken":false}))
        .await
        .ok()?;
    if account["requiresOpenaiAuth"] == false
        || !matches!(
            account.pointer("/account/type").and_then(Value::as_str),
            Some("chatgpt" | "chatgptAuthTokens")
        )
    {
        return None;
    }
    let data = process
        .request("account/rateLimits/read", json!({}))
        .await
        .ok()?;
    parse_codex(&data)
}
pub(crate) fn parse_codex(data: &Value) -> Option<Value> {
    let rates = data
        .pointer("/rateLimitsByLimitId/codex")
        .or_else(|| data.get("rateLimits"))?;
    let windows = ["primary", "secondary"]
        .iter()
        .filter_map(|key| {
            let value = &rates[*key];
            let minutes = value["windowDurationMins"].as_i64().filter(|v| *v > 0)?;
            window(key, minutes, &value["usedPercent"], &value["resetsAt"])
        })
        .collect();
    report("codex", windows)
}

async fn grok() -> Option<Value> {
    if has_env("XAI_API_KEY") || has_env("GROK_API_KEY") {
        return None;
    }
    let command = format!(
        "{} agent stdio",
        std::env::var("GROK_COMMAND").unwrap_or_else(|_| "grok".into())
    );
    let (process, _, _) = AcpProcess::spawn(&command, home()?.to_str()?, &[])
        .await
        .ok()?;
    process.request("initialize",json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"pockymoe-usage","version":"1"}})).await.ok()?;
    let data = process.request("_x.ai/billing", json!({})).await.ok()?;
    parse_grok(&data)
}
pub(crate) fn parse_grok(data: &Value) -> Option<Value> {
    let tier = data["subscription_tier"].as_str()?;
    if tier.is_empty() || tier.eq_ignore_ascii_case("none") {
        return None;
    }
    let config = &data["config"];
    let minutes = match config
        .pointer("/currentPeriod/type")
        .and_then(Value::as_str)?
    {
        "USAGE_PERIOD_TYPE_WEEKLY" => 10080,
        "USAGE_PERIOD_TYPE_DAILY" => 1440,
        _ => return None,
    };
    report(
        "grok",
        vec![window(
            "subscription",
            minutes,
            &config["creditUsagePercent"],
            &config["currentPeriod"]["end"],
        )?],
    )
}

async fn claude_token() -> Result<Option<String>, ()> {
    // API-key sessions should not show the local OAuth account's allowance.
    if has_env("ANTHROPIC_API_KEY")
        || has_env("ANTHROPIC_AUTH_TOKEN")
        || has_env("CLAUDE_CODE_USE_BEDROCK")
        || has_env("CLAUDE_CODE_USE_VERTEX")
    {
        return Ok(None);
    }
    let parsed =
        parse_spawn_command(&std::env::var("CLAUDE_COMMAND").unwrap_or_else(|_| "claude".into()))
            .map_err(|_| ())?;
    let mut command = tokio::process::Command::new(parsed.program);
    crate::child_process::hide_tokio(&mut command);
    let status = command
        .args(parsed.args)
        .args(["auth", "status", "--json"])
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|_| ())?;
    if !status.status.success() {
        return Err(());
    }
    let status: Value = serde_json::from_slice(&status.stdout).map_err(|_| ())?;
    if status["loggedIn"] != true
        || !matches!(status["authMethod"].as_str(), Some("claude.ai" | "oauth"))
    {
        return Ok(None);
    }
    let Some(home) = home() else {
        return Ok(None);
    };
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or(home.join(".claude"));
    let token = if let Ok(token) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        token
    } else {
        let credentials = tokio::fs::read(config.join(".credentials.json")).await.ok();
        #[cfg(target_os = "macos")]
        let credentials = if credentials.is_none() && !has_env("CLAUDE_CONFIG_DIR") {
            let mut cmd = tokio::process::Command::new("security");
            cmd.args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ])
            .kill_on_drop(true)
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
        } else {
            credentials
        };
        let Some(credentials) = credentials else {
            return Ok(None);
        };
        let value: Value = serde_json::from_slice(&credentials).map_err(|_| ())?;
        let Some(token) = value
            .pointer("/claudeAiOauth/accessToken")
            .and_then(Value::as_str)
        else {
            return Ok(None);
        };
        token.to_string()
    };
    if token.is_empty() {
        return Ok(None);
    }
    Ok(Some(token))
}

async fn claude() -> RefreshResult {
    let token = match claude_token().await {
        Ok(Some(token)) => token,
        Ok(None) => return RefreshResult::Unavailable,
        Err(()) => return RefreshResult::TemporaryFailure(None),
    };
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return RefreshResult::TemporaryFailure(None);
    };
    let Ok(response) = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
    else {
        return RefreshResult::TemporaryFailure(None);
    };
    if matches!(response.status().as_u16(), 401 | 403) {
        return RefreshResult::Unavailable;
    }
    if !response.status().is_success() {
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            // Bound an untrusted header to a day; still honor server delays over our backoff.
            .map(|seconds| Duration::from_secs(seconds.min(86400)));
        return RefreshResult::TemporaryFailure(retry_after);
    }
    match response
        .json::<Value>()
        .await
        .ok()
        .as_ref()
        .and_then(parse_claude)
    {
        Some(value) => RefreshResult::Available(value),
        None => RefreshResult::TemporaryFailure(None),
    }
}
pub(crate) fn parse_claude(data: &Value) -> Option<Value> {
    let windows = [("five_hour", 300), ("seven_day", 10080)]
        .into_iter()
        .filter_map(|(id, mins)| {
            window(
                id,
                mins,
                data[id]
                    .get("utilization")
                    .or_else(|| data[id].get("used_percentage"))
                    .unwrap_or(&Value::Null),
                &data[id]["resets_at"],
            )
        })
        .collect();
    report("claude", windows)
}

#[cfg(test)]
mod claude_rate_tests {
    use super::*;

    #[test]
    fn sdk_rate_limit_events_become_allowance_windows() {
        let soon = Utc::now().timestamp() + 3600;
        let update = claude_rate_update(
            &json!({"status":"allowed","rateLimitType":"seven_day","resetsAt":soon,
            "unifiedWindows":{"seven_day":{"utilization":0.04,"resetsAt":soon,"status":"allowed"},
                "five_hour":{"utilization":0.5,"resetsAt":soon}}}),
        )
        .unwrap();
        let windows = update["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0]["id"], "five_hour");
        assert_eq!(windows[0]["usedPercent"], 50.0);
        assert_eq!(windows[1]["usedPercent"], 4.0);
        let single = claude_rate_update(
            &json!({"rateLimitType":"five_hour","utilization":0.2,"resetsAt":soon}),
        )
        .unwrap();
        assert_eq!(single["windows"][0]["label"], "5h");
        assert!(claude_rate_update(&json!({"status":"allowed"})).is_none());
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn usage() -> Value {
        json!({"observedAt":"2026-10-02T12:00:00Z","stale":false,"windows":[{"label":"5h"}]})
    }

    #[test]
    fn success_is_cached_for_five_minutes() {
        let now = Instant::now();
        let mut entry = CacheEntry::new(now);
        assert_eq!(
            entry.update(RefreshResult::Available(usage()), now),
            Some(usage())
        );
        assert_eq!(entry.refresh_at, now + REFRESH_INTERVAL);
        assert_eq!(entry.value(now + Duration::from_secs(299)), Some(usage()));
    }

    #[test]
    fn transient_failures_keep_observation_and_back_off_even_with_retry_after_zero() {
        let now = Instant::now();
        let mut entry = CacheEntry::new(now);
        entry.update(RefreshResult::Available(usage()), now);
        let mut at = now + REFRESH_INTERVAL;
        for minutes in [5, 10, 20, 30, 30] {
            let cached = entry.update(RefreshResult::TemporaryFailure(Some(Duration::ZERO)), at);
            if at.duration_since(now) < MAX_STALE_AGE {
                let cached = cached.unwrap();
                assert_eq!(cached["stale"], true);
                assert_eq!(cached["observedAt"], usage()["observedAt"]);
            } else {
                assert!(cached.is_none());
            }
            assert_eq!(entry.refresh_at, at + Duration::from_secs(minutes * 60));
            assert!(!entry.unavailable);
            at = entry.refresh_at;
        }
        assert!(entry.value(now + MAX_STALE_AGE).is_none());
        assert_eq!(
            entry.update(RefreshResult::Available(usage()), at),
            Some(usage())
        );
        assert_eq!(entry.refresh_at, at + REFRESH_INTERVAL);
    }

    #[test]
    fn rejected_or_missing_auth_clears_old_account_data() {
        let now = Instant::now();
        let mut entry = CacheEntry::new(now);
        entry.update(RefreshResult::Available(usage()), now);
        assert!(entry
            .update(RefreshResult::Unavailable, now + REFRESH_INTERVAL)
            .is_none());
        assert!(entry.unavailable);
        assert!(entry.success.is_none());
    }

    #[test]
    fn negative_cache_and_server_retry_after_are_respected() {
        let now = Instant::now();
        let mut entry = CacheEntry::new(now);
        assert!(entry
            .update(
                RefreshResult::TemporaryFailure(Some(Duration::from_secs(1200))),
                now
            )
            .is_none());
        assert_eq!(entry.refresh_at, now + Duration::from_secs(1200));
        assert!(!entry.unavailable);
    }

    fn windows(report: &Value) -> Vec<(String, f64)> {
        report["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["id"].as_str().unwrap().into(),
                    w["usedPercent"].as_f64().unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn codex_pushes_and_rollout_records_parse_to_the_account_read_shape() {
        let future = 4102444800i64;
        let read = parse_codex(&json!({"rateLimits":{"primary":{"usedPercent":12,"windowDurationMins":300,"resetsAt":future},"secondary":{"usedPercent":40,"windowDurationMins":10080,"resetsAt":future}}})).unwrap();
        let push = codex_rate_update(&json!({"limitId":"codex","primary":{"usedPercent":12,"windowDurationMins":300,"resetsAt":future},"secondary":{"usedPercent":40,"windowDurationMins":10080,"resetsAt":future}})).unwrap();
        let rollout = codex_rollout_rate_limits(&json!({"limit_id":"codex","primary":{"used_percent":12.0,"window_minutes":300,"resets_at":future},"secondary":{"used_percent":40.0,"window_minutes":10080,"resets_at":future},"credits":null,"plan_type":"pro"})).unwrap();
        for report in [&push, &rollout] {
            assert_eq!(report["windows"], read["windows"]);
            assert_eq!(report["authKind"], "subscription");
        }
        assert!(codex_rate_update(&json!({"limitId":"codex_bengalfox","primary":{"usedPercent":1,"windowDurationMins":300,"resetsAt":future}})).is_none());
        // An API-key provider records an empty snapshot: nothing to show.
        assert!(codex_rollout_rate_limits(
            &json!({"limit_id":"codex","primary":null,"secondary":null})
        )
        .is_none());
        assert!(codex_rollout_rate_limits(&Value::Null).is_none());
    }

    #[tokio::test]
    async fn harness_observations_merge_sparsely_and_win_over_older_account_reads() {
        let future = 4102444800i64;
        let cache = SubscriptionUsage::default();
        let five = |used: i64| {
            codex_rate_update(
                &json!({"primary":{"usedPercent":used,"windowDurationMins":300,"resetsAt":future}}),
            )
            .unwrap()
        };
        let week = |used: i64| {
            codex_rate_update(&json!({"secondary":{"usedPercent":used,"windowDurationMins":10080,"resetsAt":future}})).unwrap()
        };
        // The device's own login is an API key: the account read is unavailable.
        let report = cache
            .read_with("codex", async { RefreshResult::Unavailable })
            .await;
        assert!(report.unavailable && report.usage.is_none());
        assert!(cache.observe("codex", &five(10)));
        assert!(cache.observe("codex", &week(30)));
        assert!(!cache.observe("codex", &week(30)));
        let report = cache.read_with("codex", async { unreachable!() }).await;
        assert!(!report.unavailable);
        assert_eq!(
            windows(report.usage.as_ref().unwrap()),
            vec![("primary".into(), 10.0), ("secondary".into(), 30.0)]
        );
        assert_eq!(report.usage.unwrap()["stale"], false);
        assert!(!cache.observe("claude", &json!({"windows":[]})));
        // A later account read replaces an older observation.
        let fresh = SubscriptionUsage::default();
        fresh.observe("codex", &five(10));
        let read = fresh
            .read_with("codex", async { RefreshResult::Available(five(55)) })
            .await;
        assert_eq!(
            windows(read.usage.as_ref().unwrap()),
            vec![("primary".into(), 55.0)]
        );
        fresh.observe("codex", &week(20));
        let merged = fresh.read_with("codex", async { unreachable!() }).await;
        assert_eq!(
            windows(merged.usage.as_ref().unwrap()),
            vec![("primary".into(), 55.0), ("secondary".into(), 20.0)]
        );
    }

    #[tokio::test]
    async fn concurrent_threads_share_one_upstream_request_and_providers_are_isolated() {
        let cache = SubscriptionUsage::default();
        let calls = AtomicUsize::new(0);
        let fetch = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            RefreshResult::Available(usage())
        };
        let (first, second) = tokio::join!(
            cache.read_with("claude", fetch()),
            cache.read_with("claude", fetch())
        );
        assert_eq!(first.usage, second.usage);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cache.read_with("codex", fetch()).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
