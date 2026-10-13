use super::*;

pub(super) fn schedule_label(cron: &str, recurring: bool) -> String {
    if !recurring {
        return format!("Once ({cron})");
    }
    let parts: Vec<_> = cron.split_whitespace().collect();
    if parts.len() != 5 {
        return format!("Cron: {cron}");
    }
    if parts[1..] == ["*", "*", "*", "*"] {
        if parts[0] == "*" {
            return "Every minute".into();
        }
        if let Some(n) = parts[0]
            .strip_prefix("*/")
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|n| *n > 0 && *n <= 60 && 60 % *n == 0)
        {
            return format!("Every {n} minutes");
        }
        let mut minutes = parts[0]
            .split(',')
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap_or_default();
        minutes.sort_unstable();
        minutes.dedup();
        if !minutes.is_empty() && minutes.iter().all(|n| *n < 60) {
            let gaps: Vec<_> = (0..minutes.len())
                .map(|i| {
                    if i + 1 == minutes.len() {
                        60 + minutes[0] - minutes[i]
                    } else {
                        minutes[i + 1] - minutes[i]
                    }
                })
                .collect();
            if gaps.iter().all(|gap| *gap == gaps[0]) {
                return format!(
                    "Every {} minutes (at {})",
                    gaps[0],
                    minutes
                        .iter()
                        .map(|m| format!(":{m:02}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }
    if parts[0] == "0" && parts[2..] == ["*", "*", "*"] {
        if let Some(n) = parts[1]
            .strip_prefix("*/")
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|n| *n > 0 && *n <= 24 && 24 % *n == 0)
        {
            return format!("Every {n} hours");
        }
    }
    format!("Cron: {cron}")
}

fn input_and_result(detail: &str) -> Option<(Value, &str)> {
    let (_, input) = detail.split_once("Input:\n")?;
    let (input, result) = input
        .split_once("\n\nResult:\n")
        .or_else(|| input.split_once("\n\nOutput:\n"))?;
    Some((serde_json::from_str(input).ok()?, result))
}

fn timestamp(value: Option<&str>) -> Option<i64> {
    value
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .map(|at| at.timestamp_millis())
}

fn tool_completed(item: &ThreadHistoryItemDto) -> bool {
    // Older runtimes overwrote tool status with the enclosing turn's status.
    // The mapper-generated header retains the actual tool outcome. Read only
    // the header before Input, never quoted statuses in a prompt/result.
    let header = item
        .detail_text
        .as_deref()
        .unwrap_or("")
        .split("Input:\n")
        .next()
        .unwrap_or("");
    match header
        .lines()
        .find_map(|line| line.strip_prefix("Status: "))
    {
        Some(status) => status == "completed",
        None => item.status.as_deref() == Some("completed"),
    }
}

struct WatchEntry {
    value: Value,
    start: Option<i64>,
    end: Option<i64>,
    triggers: u64,
    ambiguous: u64,
    usage_count: u64,
    priced_count: u64,
    tokens: crate::usage::Tokens,
    price: Value,
}

fn watch_entries(
    records: Vec<(String, Option<String>)>,
    current: Option<&str>,
    current_started: Option<&str>,
    turns: &[ThreadTurnDto],
    prompts: &HashMap<String, String>,
    thread_id: &str,
    now: i64,
) -> Result<Vec<Value>> {
    let mut entries = Vec::<WatchEntry>::new();
    for (raw, instance) in records {
        let item: ThreadHistoryItemDto = serde_json::from_str(&raw)?;
        if !tool_completed(&item) {
            continue;
        }
        let Some((input, result)) = item.detail_text.as_deref().and_then(input_and_result) else {
            continue;
        };
        let created = timestamp(item.created_at.as_deref());
        if item.text == "CronList" {
            if result.trim() == "No scheduled jobs." {
                for entry in entries.iter_mut().filter(|entry| {
                    matches!(
                        entry.value["status"].as_str(),
                        Some("active" | "unconfirmed")
                    )
                }) {
                    entry.value["status"] = json!("notScheduled");
                    entry.value["statusCheckedAt"] = json!(item.created_at);
                    entry.end = match (created, entry.end) {
                        (Some(at), Some(expiry)) => Some(at.min(expiry)),
                        (Some(at), None) => Some(at),
                        (None, end) => end,
                    };
                }
            }
            continue;
        }
        if item.text == "CronDelete" {
            let Some(id) = input["id"].as_str() else {
                continue;
            };
            let result = result.trim();
            if result != format!("Cancelled job {id}.")
                && result != format!("Deleted job {id}.")
                && !matches!(result, "Deleted job" | "Deleted job.")
            {
                continue;
            }
            for entry in entries.iter_mut().filter(|entry| {
                entry.value["id"] == input["id"] && entry.value["status"] != "deleted"
            }) {
                entry.value["status"] = json!("deleted");
                entry.value["statusCheckedAt"] = json!(item.created_at);
                // Missing cancellation timing cannot safely establish a lifetime.
                entry.end = match (created.or(entry.start), entry.end) {
                    (Some(cancelled), Some(expiry)) => Some(cancelled.min(expiry)),
                    (cancelled, None) => cancelled,
                    (None, expiry) => expiry,
                };
            }
            continue;
        }
        let (Some(cron), Some(prompt)) = (input["cron"].as_str(), input["prompt"].as_str()) else {
            continue;
        };
        if !result.to_ascii_lowercase().contains("scheduled") {
            continue;
        }
        let Some(job) = result
            .split_once("job ")
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .map(|id| id.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'))
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let recurring = input["recurring"]
            .as_bool()
            .unwrap_or_else(|| result.contains("recurring"));
        let expiry = result
            .split_once("expires after ")
            .and_then(|(_, tail)| tail.split_whitespace().next())
            .and_then(|n| n.parse::<i64>().ok())
            .filter(|n| *n > 0 && *n < 366)
            .and_then(|days| {
                item.created_at
                    .as_deref()
                    .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                    .map(|at| at + chrono::Duration::days(days))
            });
        let status = if expiry.is_some_and(|at| at.timestamp_millis() <= now) {
            "expired"
        } else if input["durable"] != true
            && created
                .zip(timestamp(current_started))
                .is_some_and(|(created, started)| created < started)
        {
            "sessionEnded"
        } else {
            match (instance.as_deref(), current) {
                (Some(a), Some(b)) if a == b => "active",
                (Some(_), _) if input["durable"] != true => "sessionEnded",
                _ => "unconfirmed",
            }
        };
        entries.push(WatchEntry {
            value: json!({"id":job,"cron":cron,"schedule":schedule_label(cron,recurring),"prompt":prompt,"recurring":recurring,"createdAt":item.created_at,"expiresAt":expiry.map(|at|at.to_rfc3339()),"status":status,"lastTriggeredAt":null}),
            start: created, end: match (status, created.zip(timestamp(current_started)), expiry.map(|at|at.timestamp_millis())) {
                ("sessionEnded", Some((created, started)), expiry) if created < started => Some(expiry.map_or(started, |expiry| expiry.min(started))),
                (_, _, expiry) => expiry,
            },
            triggers: 0, ambiguous: 0, usage_count: 0, priced_count: 0,
            tokens: crate::usage::Tokens::default(),
            price: json!({"currency":"USD","inputUsd":0.0,"cachedInputUsd":0.0,"cacheWriteInputUsd":0.0,"outputUsd":0.0,"totalUsd":0.0}),
        });
    }
    let mut scheduled: Vec<_> = turns
        .iter()
        .filter(|turn| turn.id.starts_with(&format!("{thread_id}:scheduled:")))
        .collect();
    scheduled.sort_by_key(|turn| timestamp(turn.started_at.as_deref()));
    for turn in scheduled {
        let Some(at) = timestamp(turn.started_at.as_deref()) else {
            continue;
        };
        let Some(prompt) = prompts.get(&turn.id) else {
            continue;
        };
        let candidates: Vec<_> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.value["prompt"].as_str() == Some(prompt.as_str())
                    && entry.start.is_some_and(|start| at >= start)
                    && entry.end.is_none_or(|end| at < end)
                    && (entry.value["recurring"] == true || entry.triggers == 0)
            })
            .map(|(index, _)| index)
            .collect();
        if candidates.len() > 1 {
            for index in candidates {
                entries[index].ambiguous += 1;
            }
            continue;
        }
        let Some(index) = candidates.first().copied() else {
            continue;
        };
        let entry = &mut entries[index];
        entry.triggers += 1;
        entry.value["lastTriggeredAt"] = json!(turn.started_at);
        if let Some(tokens) = turn
            .token_usage
            .as_ref()
            .and_then(|usage| crate::usage::Tokens::parse(&usage["total"]))
        {
            entry.tokens = entry.tokens.add(&tokens);
            entry.usage_count += 1;
        }
        if let Some(price) = turn.price_estimate.as_ref().filter(|price| {
            price["totalUsd"]
                .as_f64()
                .is_some_and(|usd| usd.is_finite() && usd >= 0.0)
        }) {
            for field in [
                "inputUsd",
                "cachedInputUsd",
                "cacheWriteInputUsd",
                "outputUsd",
                "totalUsd",
            ] {
                entry.price[field] = json!(
                    entry.price[field].as_f64().unwrap_or(0.0)
                        + price[field].as_f64().unwrap_or(0.0)
                );
            }
            entry.priced_count += 1;
        }
    }
    Ok(entries
        .into_iter()
        .map(|mut entry| {
            if entry.value["recurring"] == false
                && entry.triggers > 0
                && entry.value["status"] != "deleted"
            {
                entry.value["status"] = json!("completed");
            }
            entry.value["triggerCount"] = if entry.start.is_some() {
                json!(entry.triggers)
            } else {
                Value::Null
            };
            entry.value["ambiguousTriggerCount"] = json!(entry.ambiguous);
            entry.value["usageTriggerCount"] = json!(entry.usage_count);
            entry.value["pricedTriggerCount"] = json!(entry.priced_count);
            entry.value["tokenUsage"] = if entry.usage_count > 0
                || (entry.triggers == 0 && entry.ambiguous == 0 && entry.start.is_some())
            {
                json!(entry.tokens)
            } else {
                Value::Null
            };
            entry.value["priceEstimate"] = if entry.priced_count > 0
                || (entry.triggers == 0 && entry.ambiguous == 0 && entry.start.is_some())
            {
                entry.price
            } else {
                Value::Null
            };
            entry.value
        })
        .collect())
}

impl Supervisor {
    /// Conservative history projection for scheduler admission. Unknown native
    /// timers remain a conflict until their successful cancellation is recorded.
    pub(super) fn recorded_native_watch_conflict(&self, id: &str) -> Result<bool> {
        let thread = self.get_thread(id)?;
        if thread.provider != Provider::Claude && thread.agent_id.as_deref() != Some("claude") {
            return Ok(false);
        }
        let records=self.db.with(|c|{let mut q=c.prepare("SELECT item_json,NULL FROM thread_history_items WHERE thread_id=?1 AND json_extract(item_json,'$.text') IN ('CronCreate','CronDelete','CronList') ORDER BY created_at,rowid")?;let rows=q.query_map([id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;Ok(rows)})?;
        Ok(watch_entries(
            records,
            None,
            None,
            &[],
            &HashMap::new(),
            id,
            chrono::Utc::now().timestamp_millis(),
        )?
        .iter()
        .any(|w| matches!(w["status"].as_str(), Some("active" | "unconfirmed"))))
    }

    pub async fn thread_watches(&self, id: &str) -> Result<Value> {
        let thread = self.get_thread(id)?;
        if thread.provider != Provider::Claude
            && !(thread.provider == Provider::Acp && thread.agent_id.as_deref() == Some("claude"))
        {
            return Ok(json!({"watches":[]}));
        }
        self.sync_claude_scheduled_history(id).await?;
        let current = if let Some(session) = thread.provider_session_id.as_deref() {
            self.runtime(thread.provider)?
                .session_instance_id(session)
                .await
        } else {
            None
        };
        let current_started = if let Some(session) = thread.provider_session_id.as_deref() {
            self.runtime(thread.provider)?
                .session_instance_started_at(session)
                .await
        } else {
            None
        };
        let records = self.db.with(|conn| {
            let mut stmt = conn.prepare("SELECT h.item_json,k.value FROM thread_history_items h
                LEFT JOIN kv k ON k.key='turn-process:'||h.turn_id
                WHERE h.thread_id=?1 AND json_extract(h.item_json,'$.text') IN ('CronCreate','CronDelete','CronList')
                ORDER BY h.created_at,h.rowid")?;
            let rows = stmt.query_map([id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })?;
        let prompts = self.db.with(|conn| {
            let mut stmt = conn.prepare("SELECT id,display_prompt FROM thread_turns WHERE thread_id=?1 AND id LIKE '%:scheduled:%'")?;
            let rows = stmt.query_map([id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows.into_iter().filter_map(|(id, prompt)| prompt.map(|prompt| (id, prompt))).collect::<HashMap<_, _>>())
        })?;
        // Metadata covers all turns, including those outside the chat history page,
        // and uses the same per-turn pricing calculation as the timeline.
        let turns = self.load_turns_meta(id)?;
        let mut watches = watch_entries(
            records,
            current.as_deref(),
            current_started.as_deref(),
            &turns,
            &prompts,
            id,
            chrono::Utc::now().timestamp_millis(),
        )?;
        watches.sort_by(|a, b| b["createdAt"].as_str().cmp(&a["createdAt"].as_str()));
        Ok(
            json!({"watches":watches,"timezone":format!("Device local time (UTC{})",chrono::Local::now().offset())}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_real_claude_watch_result_and_supported_cadences_without_guessing() {
        let raw="Tool: CronCreate\n\nInput:\n{\"cron\":\"13,43 * * * *\",\"prompt\":\"Read inbox\",\"recurring\":true}\n\nResult:\nScheduled recurring job 9b3254c0 (13,43 * * * *). Auto-expires after 7 days.";
        let (input, result) = input_and_result(raw).unwrap();
        assert_eq!(input["prompt"], "Read inbox");
        assert!(result.contains("9b3254c0"));
        assert_eq!(
            schedule_label("13,43 * * * *", true),
            "Every 30 minutes (at :13, :43)"
        );
        assert_eq!(schedule_label("*/5 * * * *", true), "Every 5 minutes");
        assert_eq!(schedule_label("*/7 * * * *", true), "Cron: */7 * * * *");
        assert_eq!(schedule_label("0 */3 * * *", true), "Every 3 hours");
        assert_eq!(schedule_label("0 9 * * MON", true), "Cron: 0 9 * * MON");
        assert_eq!(schedule_label("* * * * *", false), "Once (* * * * *)");
        assert!(input_and_result("Input:\n{incomplete").is_none());
    }

    fn create(id: &str, at: Option<&str>, recurring: bool) -> (String, Option<String>) {
        let result =
            format!("Scheduled recurring job {id} (* * * * *). Auto-expires after 7 days.");
        (json!({"id":id,"createdAt":at,"kind":"toolCall","text":"CronCreate","status":"completed",
            "detailText":format!("Input:\n{}\n\nResult:\n{result}", json!({"cron":"* * * * *","prompt":"Read inbox","recurring":recurring}))}).to_string(), None)
    }

    fn delete(id: &str, at: &str) -> (String, Option<String>) {
        (json!({"id":format!("delete-{id}"),"createdAt":at,"kind":"toolCall","text":"CronDelete","status":"completed",
            "detailText":format!("Input:\n{}\n\nResult:\nDeleted job",json!({"id":id}))}).to_string(), None)
    }

    #[test]
    fn successful_cancel_in_interrupted_turn_and_empty_list_reconcile_legacy_watches() {
        let at = "2030-01-01T00:02:00Z";
        let deleted = json!({"id":"cancel","createdAt":at,"kind":"toolCall","text":"CronDelete","status":"interrupted",
            "detailText":"Tool: CronDelete\n\nStatus: completed\n\nInput:\n{\"id\":\"old\"}\n\nResult:\nCancelled job old."}).to_string();
        let list = json!({"id":"list","createdAt":at,"kind":"toolCall","text":"CronList","status":"completed",
            "detailText":"Input:\n{}\n\nResult:\nNo scheduled jobs."}).to_string();
        let watches = summarize(
            vec![
                create("old", Some("2030-01-01T00:00:00Z"), true),
                (deleted, None),
                create("missing", Some("2030-01-01T00:00:01Z"), true),
                (list, None),
            ],
            vec![turn("t:scheduled:after", "2030-01-01T00:03:00Z", true)],
        );
        assert_eq!(watches[0]["status"], "deleted");
        assert_eq!(watches[1]["status"], "notScheduled");
        assert_eq!(watches[1]["statusCheckedAt"], at);
        assert_eq!(watches[0]["triggerCount"], 0);
        assert_eq!(watches[1]["triggerCount"], 0);
        // A pending or failed cancellation, or a completed tool returning an
        // error, must never prove that the schedule disappeared.
        for (status, result) in [
            ("pending", "Cancelled job old."),
            ("failed", "Cancelled job old."),
            ("completed", "Job not found"),
        ] {
            let bad = json!({"id":"bad-cancel","kind":"toolCall","text":"CronDelete","status":"interrupted","createdAt":at,
                "detailText":format!("Status: {status}\n\nInput:\n{{\"id\":\"old\"}}\n\nResult:\n{result}")});
            let values = summarize(
                vec![
                    create("old", Some("2030-01-01T00:00:00Z"), true),
                    (bad.to_string(), None),
                ],
                vec![],
            );
            assert_eq!(values[0]["status"], "unconfirmed");
        }
    }

    #[test]
    fn process_start_proves_old_session_only_watch_ended_without_guessing_current_or_durable_jobs()
    {
        let mut durable: Value =
            serde_json::from_str(&create("durable", Some("2030-01-01T00:00:00Z"), true).0).unwrap();
        durable["detailText"] = json!("Input:\n{\"cron\":\"* * * * *\",\"prompt\":\"Read inbox\",\"recurring\":true,\"durable\":true}\n\nResult:\nScheduled recurring job durable (* * * * *).");
        let values = watch_entries(
            vec![
                create("old", Some("2030-01-01T00:00:00Z"), true),
                create("unknown", None, true),
                create("new", Some("2030-01-01T01:01:00Z"), true),
                (durable.to_string(), None),
            ],
            Some("current"),
            Some("2030-01-01T01:00:00Z"),
            &[],
            &HashMap::new(),
            "t",
            timestamp(Some("2030-01-02T00:00:00Z")).unwrap(),
        )
        .unwrap();
        assert_eq!(values[0]["status"], "sessionEnded");
        for value in &values[1..] {
            assert_eq!(value["status"], "unconfirmed");
        }
    }

    fn turn(id: &str, at: &str, priced: bool) -> ThreadTurnDto {
        serde_json::from_value(json!({"id":id,"startedAt":at,"status":"completed","error":null,"items":[],
            "tokenUsage":{"total":{"inputTokens":100,"outputTokens":20,"cachedInputTokens":40,"cacheWriteInputTokens":10,"reasoningOutputTokens":5,"totalTokens":120}},
            "priceEstimate":if priced { json!({"totalUsd":0.3,"inputUsd":0.1,"cachedInputUsd":0.05,"cacheWriteInputUsd":0.05,"outputUsd":0.1}) } else { Value::Null }
        })).unwrap()
    }

    fn summarize(records: Vec<(String, Option<String>)>, turns: Vec<ThreadTurnDto>) -> Vec<Value> {
        let prompts = turns
            .iter()
            .map(|turn| (turn.id.clone(), "Read inbox".to_owned()))
            .collect();
        watch_entries(
            records,
            None,
            None,
            &turns,
            &prompts,
            "t",
            timestamp(Some("2030-01-02T00:00:00Z")).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn sums_each_scheduled_turn_price_and_ignores_manual_and_prior_turns() {
        let mut expensive = turn("t:scheduled:2", "2030-01-01T00:02:00Z", true);
        expensive.price_estimate = Some(
            json!({"totalUsd":0.9,"inputUsd":0.3,"cachedInputUsd":0.15,"cacheWriteInputUsd":0.15,"outputUsd":0.3}),
        );
        let watches = summarize(
            vec![create("job", Some("2030-01-01T00:00:00Z"), true)],
            vec![
                expensive,
                turn("t:scheduled:1", "2030-01-01T00:01:00Z", true),
                turn("manual", "2030-01-01T00:03:00Z", true),
                turn("t:scheduled:prior", "2029-12-31T23:59:00Z", true),
            ],
        );
        let w = &watches[0];
        assert_eq!(w["triggerCount"], 2);
        assert_eq!(w["lastTriggeredAt"], "2030-01-01T00:02:00Z");
        assert_eq!(w["tokenUsage"]["totalTokens"], 240);
        assert_eq!(w["tokenUsage"]["cachedInputTokens"], 80);
        assert_eq!(w["tokenUsage"]["cacheWriteInputTokens"], 20);
        assert!((w["priceEstimate"]["totalUsd"].as_f64().unwrap() - 1.2).abs() < 1e-10);
        assert!((w["priceEstimate"]["outputUsd"].as_f64().unwrap() - 0.4).abs() < 1e-10);
    }

    #[test]
    fn cancellation_recreation_and_expiry_bound_each_watch_lifetime() {
        let watches = summarize(
            vec![
                create("old", Some("2030-01-01T00:00:00Z"), true),
                delete("old", "2030-01-01T00:02:00Z"),
                create("new", Some("2030-01-01T00:03:00Z"), true),
            ],
            vec![
                turn("t:scheduled:first", "2030-01-01T00:01:00Z", true),
                turn("t:scheduled:gap", "2030-01-01T00:02:30Z", true),
                turn("t:scheduled:second", "2030-01-01T00:04:00Z", true),
                turn("t:scheduled:expired", "2030-01-08T00:03:00Z", true),
            ],
        );
        assert_eq!(watches[0]["status"], "deleted");
        assert_eq!(watches[0]["triggerCount"], 1);
        assert_eq!(watches[1]["triggerCount"], 1);
        let expired = summarize(
            vec![
                create("expired", Some("2029-12-01T00:00:00Z"), true),
                delete("expired", "2030-01-01T00:00:00Z"),
            ],
            vec![turn("t:scheduled:late", "2029-12-20T00:00:00Z", true)],
        );
        assert_eq!(
            expired[0]["triggerCount"], 0,
            "a later deletion must not extend expiry"
        );
    }

    #[test]
    fn overlapping_identical_prompts_are_not_counted_or_billed_twice() {
        let watches = summarize(
            vec![
                create("one", Some("2030-01-01T00:00:00Z"), true),
                create("two", Some("2030-01-01T00:00:00Z"), true),
            ],
            vec![turn("t:scheduled:ambiguous", "2030-01-01T00:01:00Z", true)],
        );
        for w in watches {
            assert_eq!(w["triggerCount"], 0);
            assert_eq!(w["ambiguousTriggerCount"], 1);
            assert!(w["tokenUsage"].is_null());
            assert!(w["priceEstimate"].is_null());
        }
    }

    #[test]
    fn once_jobs_complete_and_missing_usage_keeps_price_coverage_explicit() {
        let watches = summarize(
            vec![create("once", Some("2030-01-01T00:00:00Z"), false)],
            vec![
                turn("t:scheduled:first", "2030-01-01T00:01:00Z", false),
                turn("t:scheduled:later", "2030-01-01T00:02:00Z", true),
            ],
        );
        assert_eq!(watches[0]["status"], "completed");
        assert_eq!(watches[0]["triggerCount"], 1);
        assert_eq!(watches[0]["pricedTriggerCount"], 0);
        assert!(watches[0]["priceEstimate"].is_null());
        let mut missing = turn("t:scheduled:missing", "2030-01-01T00:02:00Z", false);
        missing.token_usage = None;
        let partial = summarize(
            vec![create("partial", Some("2030-01-01T00:00:00Z"), true)],
            vec![
                turn("t:scheduled:priced", "2030-01-01T00:01:00Z", true),
                missing,
            ],
        );
        assert_eq!(partial[0]["triggerCount"], 2);
        assert_eq!(partial[0]["pricedTriggerCount"], 1);
        assert_eq!(partial[0]["usageTriggerCount"], 1);
        assert_eq!(partial[0]["priceEstimate"]["totalUsd"], 0.3);
    }

    #[test]
    fn missing_creation_time_is_unknown_but_confirmed_zero_runs_cost_zero() {
        let watches = summarize(
            vec![
                create("unknown", None, true),
                create("zero", Some("2030-01-01T00:00:00Z"), true),
            ],
            vec![],
        );
        assert!(watches[0]["triggerCount"].is_null());
        assert!(watches[0]["priceEstimate"].is_null());
        assert_eq!(watches[1]["triggerCount"], 0);
        assert_eq!(watches[1]["priceEstimate"]["totalUsd"], 0.0);
    }

    #[tokio::test]
    async fn statistics_read_all_persisted_turns_and_use_timeline_prices() {
        use crate::fake::FakeRuntime;
        let dir = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig::from_env().unwrap();
        config.database_url = dir.path().join("isolated.sqlite");
        config.workspace_root = dir.path().join("workspaces");
        config.fake_runtime = true;
        config.relay_server_url = None;
        config.relay_agent_token = None;
        let supervisor = Supervisor::new(
            config,
            Database::open(&dir.path().join("isolated.sqlite")).unwrap(),
            vec![Arc::new(FakeRuntime::new(Provider::Claude))],
        )
        .with_local_session_homes(LocalSessionHomes {
            claude_home: dir.path().join("claude"),
            codex_home: dir.path().join("codex"),
            grok_home: dir.path().join("grok"),
        });
        let workspace = supervisor
            .create_workspace(CreateWorkspaceInput {
                abs_path: Some(dir.path().to_string_lossy().into_owned()),
                git_url: None,
                label: None,
            })
            .unwrap();
        let thread = supervisor
            .create_thread(CreateThreadInput {
                workspace_id: workspace.id,
                title: Some("watch".into()),
                provider: Some(Provider::Claude),
                agent_id: None,
                model: "default".into(),
                reasoning_effort: None,
                approval_mode: "yolo".into(),
                parent_thread_id: None,
            })
            .await
            .unwrap();
        let at = chrono::Utc::now();
        let created = (at - chrono::Duration::hours(1)).to_rfc3339();
        let raw = create("job", Some(&created), true).0;
        supervisor.db.with(|conn| {
            conn.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES('cron',?1,'owner','cron',?2,?3,?3)",params![thread.id,raw,created])?;
            for n in 1..=15 {
                let id = if n<=2 {format!("{}:scheduled:{n}",thread.id)} else {format!("manual-{n}")};
                let usage = json!({"total":{"inputTokens":100,"outputTokens":20,"cachedInputTokens":40,"totalTokens":120}});
                conn.execute("INSERT INTO thread_turns(id,thread_id,status,model,display_prompt,token_usage_json,started_at,completed_at,ordinal) VALUES(?1,?2,'completed','claude-opus-4-6','Read inbox',?3,?4,?4,?5)",
                    params![id,thread.id,usage.to_string(),(at-chrono::Duration::minutes(30-n)).to_rfc3339(),n])?;
            }
            Ok(())
        }).unwrap();
        let (page, _) = supervisor
            .load_turns_meta_page(&thread.id, Some(10), None)
            .unwrap();
        assert!(page.iter().all(|turn| !turn.id.contains(":scheduled:")));
        let expected: f64 = supervisor
            .load_turns_meta(&thread.id)
            .unwrap()
            .iter()
            .filter(|turn| turn.id.contains(":scheduled:"))
            .map(|turn| {
                turn.price_estimate.as_ref().unwrap()["totalUsd"]
                    .as_f64()
                    .unwrap()
            })
            .sum();
        let snapshot = supervisor.thread_watches(&thread.id).await.unwrap();
        let w = &snapshot["watches"][0];
        assert_eq!(w["triggerCount"], 2);
        assert_eq!(w["tokenUsage"]["totalTokens"], 240);
        assert!((w["priceEstimate"]["totalUsd"].as_f64().unwrap() - expected).abs() < 1e-10);
    }
}
