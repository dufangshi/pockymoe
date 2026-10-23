use super::*;
use pockymoe_protocol::{ThreadEventEnvelope, ThreadEventType};

async fn fixture() -> (tempfile::TempDir, Supervisor, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = RuntimeConfig::from_env().unwrap();
    config.database_url = dir.path().join("summary.sqlite");
    config.workspace_root = dir.path().to_owned();
    config.relay_server_url = None;
    config.relay_agent_token = None;
    config.fake_runtime = true;
    let db = Database::open(&config.database_url).unwrap();
    let s = Supervisor::new(
        config,
        db,
        vec![Arc::new(crate::fake::FakeRuntime::new(Provider::Codex))],
    );
    let ws = s
        .create_workspace(CreateWorkspaceInput {
            abs_path: Some(dir.path().to_string_lossy().into()),
            git_url: None,
            label: None,
        })
        .unwrap();
    let thread = s
        .create_thread(CreateThreadInput {
            workspace_id: ws.id,
            title: None,
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "default".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: None,
        })
        .await
        .unwrap();
    (dir, s, thread.id)
}

fn turn(s: &Supervisor, thread: &str, id: &str, n: i64, start: Option<&str>, prompt: Option<&str>) {
    s.db.with(|c| { c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal,started_at,display_prompt) VALUES(?1,?2,'inProgress',?3,?4,?5)",params![id,thread,n,start,prompt])?;Ok(()) }).unwrap();
}

fn item(
    s: &Supervisor,
    thread: &str,
    turn: &str,
    id: &str,
    kind: &str,
    text: &str,
    time: Option<&str>,
    extra: Value,
) {
    let mut value = json!({"id":id,"kind":kind,"text":text,"createdAt":time});
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    s.db.with(|c| {c.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'2099-01-01T00:00:00Z','2099-01-01T00:00:00Z')",params![format!("{thread}:{turn}:{id}"),thread,turn,id,value.to_string()])?;Ok(())}).unwrap();
}

fn texts(summary: &ThreadSummaryDto) -> Vec<&str> {
    summary.messages.iter().map(|m| m.text.as_str()).collect()
}

#[tokio::test]
async fn public_chronology_preserves_commentary_steer_and_every_final() {
    let (_d, s, id) = fixture().await;
    turn(
        &s,
        &id,
        "one",
        1,
        Some("2030-01-01T00:00:00Z"),
        Some("  question\n"),
    );
    // Deliberately insert the user steer before the earlier agent item, like
    // provider snapshots/UI projections which hoist users to the top.
    for (key, kind, text, time, extra) in [
        (
            "one:user",
            "userMessage",
            "  question\n",
            "2030-01-01T00:00:00Z",
            json!({}),
        ),
        (
            "steer:p",
            "userMessage",
            "please adjust",
            "2030-01-01T01:00:02+01:00",
            json!({}),
        ),
        (
            "codex-commentary",
            "agentMessage",
            "working",
            "2030-01-01T00:00:01Z",
            json!({"phase":"commentary"}),
        ),
        (
            "claude-commentary",
            "agentMessage",
            "adjusting",
            "2030-01-01T00:00:03Z",
            json!({"responsePhase":"commentary"}),
        ),
        (
            "claude-final",
            "agentMessage",
            "done one",
            "2030-01-01T00:00:04Z",
            json!({"responsePhase":"final"}),
        ),
        (
            "native-input",
            "userMessage",
            "  question\n",
            "2030-01-01T00:00:00Z",
            json!({}),
        ),
        (
            "native-steer",
            "userMessage",
            "please adjust",
            "2030-01-01T00:00:02Z",
            json!({}),
        ),
        (
            "reason",
            "reasoning",
            "private reasoning",
            "2030-01-01T00:00:02Z",
            json!({}),
        ),
        (
            "hidden",
            "agentMessage",
            "private channel",
            "2030-01-01T00:00:02Z",
            json!({"channel":"analysis"}),
        ),
        (
            "hidden-phase",
            "agentMessage",
            "private phase",
            "2030-01-01T00:00:02Z",
            json!({"phase":"analysis"}),
        ),
        (
            "notification",
            "userMessage",
            "native control",
            "2030-01-01T00:00:02Z",
            json!({"origin":{"kind":"task-notification"}}),
        ),
    ] {
        item(&s, &id, "one", key, kind, text, Some(time), extra);
    }
    item(
        &s,
        &id,
        "one",
        "tool",
        "commandExecution",
        &"giant secret".repeat(50_000),
        Some("2030-01-01T00:00:02Z"),
        json!({"detailText":"private tool detail"}),
    );
    turn(
        &s,
        &id,
        "two",
        2,
        Some("2030-01-01T00:01:00Z"),
        Some("next"),
    );
    item(
        &s,
        &id,
        "two",
        "two:user",
        "userMessage",
        "next",
        Some("2030-01-01T00:01:00Z"),
        json!({}),
    );
    item(
        &s,
        &id,
        "two",
        "final",
        "agentMessage",
        "done two",
        Some("2030-01-01T00:01:01Z"),
        json!({"phase":"final"}),
    );
    s.db.with(|c|{c.execute("INSERT INTO thread_pending_steers(id,thread_id,turn_id,display_prompt,submitted_prompt,delivery,created_at,updated_at) VALUES('p',?1,'one','please adjust','with system context','steer','2030-01-01T00:00:00Z','2030-01-01T00:00:02Z'),('unsent',?1,'two','not delivered','not delivered','continuation','now','now')",[&id])?;Ok(())}).unwrap();
    let summary = s
        .thread_summary(&id, &ThreadSummaryQuery::default())
        .unwrap();
    assert_eq!(
        texts(&summary),
        [
            "  question\n",
            "working",
            "please adjust",
            "adjusting",
            "done one",
            "next",
            "done two"
        ]
    );
    assert_eq!(
        summary.messages[2].time.as_deref(),
        Some("2030-01-01T00:00:02Z")
    );
    assert!(!summary.truncated);
    let value = serde_json::to_value(summary).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 2);
    assert!(value["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m.as_object().unwrap().len() == 4 && m["turnId"].is_string()));
    let (context, target) = s
        .thread_summary_for_reply(&id, "one", "claude-final", 1_000_000, 10_000_000)
        .unwrap();
    assert_eq!(
        texts(&context),
        [
            "  question\n",
            "working",
            "please adjust",
            "adjusting",
            "done one"
        ]
    );
    assert_eq!(target, 4);
    assert_eq!(context.messages[2].role, ThreadSummaryRole::User);
    assert_eq!(
        context.messages[2].time.as_deref(),
        Some("2030-01-01T00:00:02Z")
    );
}

#[tokio::test]
async fn stream_replacement_is_once_and_keeps_first_emission() {
    let (_d, s, id) = fixture().await;
    turn(&s, &id, "stream", 1, Some("2030-01-01T00:00:00Z"), None);
    for (time, delta) in [
        ("2030-01-01T00:00:01Z", "hello"),
        ("2030-01-01T00:00:03Z", " world"),
    ] {
        s.append_history_delta(&mut ThreadEventEnvelope { event_type:ThreadEventType::ThreadOutputDelta,thread_id:id.clone(),timestamp:time.into(),payload:json!({"turnId":"stream","itemId":"stream:assistant:1","delta":delta,"sequence":1}) }).unwrap();
    }
    let completed:ThreadHistoryItemDto=serde_json::from_value(json!({"id":"stream:assistant:1","kind":"agentMessage","text":"hello world!","createdAt":"2030-01-01T00:00:05Z","status":"completed"})).unwrap();
    s.upsert_history_item(&id, "stream", &completed).unwrap();
    s.persist_turn_result(
        &id,
        "stream",
        "completed",
        None,
        &[completed],
        "2030-01-01T00:00:06Z",
    )
    .unwrap();
    // A late stream update after terminal persistence is ignored.
    s.append_history_delta(&mut ThreadEventEnvelope {
        event_type: ThreadEventType::ThreadOutputDelta,
        thread_id: id.clone(),
        timestamp: "2030-01-01T00:00:07Z".into(),
        payload: json!({"turnId":"stream","itemId":"stream:assistant:1","delta":" duplicate"}),
    })
    .unwrap();
    let summary = s
        .thread_summary(&id, &ThreadSummaryQuery::default())
        .unwrap();
    assert_eq!(texts(&summary), ["hello world!"]);
    assert_eq!(
        summary.messages[0].time.as_deref(),
        Some("2030-01-01T00:00:01Z")
    );
}

#[tokio::test]
async fn filters_intersect_then_keep_newest_unicode_suffix() {
    let (_d, s, id) = fixture().await;
    for n in 0..3 {
        let key = format!("t{n}");
        let time = format!("2030-01-01T00:00:0{n}Z");
        turn(&s, &id, &key, n, Some(&time), None);
        item(
            &s,
            &id,
            &key,
            &key,
            "agentMessage",
            ["old", "中🙂e\u{301}", "尾🦀"][n as usize],
            Some(&time),
            json!({}),
        );
    }
    let now = timestamp(Some("2030-01-01T00:00:03Z")).unwrap();
    let mut q = ThreadSummaryQuery {
        since: Some("2030-01-01T01:00:01+01:00".into()),
        recent_seconds: Some(2),
        turns: Some(2),
        max_chars: Some(5),
    };
    let summary = s.thread_summary_at(&id, &q, now).unwrap();
    assert_eq!(texts(&summary), ["🙂e\u{301}", "尾🦀"]);
    assert!(summary.truncated && summary.messages[0].truncated);
    assert!(!summary.messages[1].truncated);
    q.max_chars = Some(2);
    let summary = s.thread_summary_at(&id, &q, now).unwrap();
    assert_eq!(texts(&summary), ["尾🦀"]);
    assert!(summary.truncated && !summary.messages[0].truncated);
    q.max_chars = None;
    q.since = Some("2030-01-01T00:00:02Z".into());
    assert_eq!(texts(&s.thread_summary_at(&id, &q, now).unwrap()), ["尾🦀"]);
    q.since = None;
    q.recent_seconds = None;
    q.turns = Some(1);
    assert_eq!(texts(&s.thread_summary_at(&id, &q, now).unwrap()), ["尾🦀"]);
}

#[tokio::test]
async fn missing_times_use_recorded_order_without_fabricating_dates_and_ties_are_stable() {
    let (_d, s, id) = fixture().await;
    turn(&s, &id, "legacy", 1, None, Some("legacy question"));
    item(
        &s,
        &id,
        "legacy",
        "a",
        "agentMessage",
        "untimed",
        None,
        json!({"sequence":1}),
    );
    item(
        &s,
        &id,
        "legacy",
        "b",
        "agentMessage",
        "bad date",
        Some("not a date"),
        json!({"sequence":2}),
    );
    turn(&s, &id, "timed", 2, Some("2030-01-01T00:00:00Z"), None);
    for (key, kind, text) in [
        ("first", "agentMessage", "first"),
        ("steer:tie", "userMessage", "tie steer"),
        ("last", "agentMessage", "last"),
    ] {
        item(
            &s,
            &id,
            "timed",
            key,
            kind,
            text,
            Some("2030-01-01T00:00:00Z"),
            json!({}),
        );
    }
    for _ in 0..2 {
        let summary = s
            .thread_summary(&id, &ThreadSummaryQuery::default())
            .unwrap();
        assert_eq!(
            texts(&summary),
            [
                "legacy question",
                "untimed",
                "bad date",
                "first",
                "tie steer",
                "last"
            ]
        );
        assert!(summary.messages[..3].iter().all(|m| m.time.is_none()));
    }
    let filtered = s
        .thread_summary(
            &id,
            &ThreadSummaryQuery {
                since: Some("2029-01-01T00:00:00Z".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(texts(&filtered), ["first", "tie steer", "last"]);
}

#[tokio::test]
async fn controls_are_not_user_prose_but_agent_responses_and_actual_user_steer_survive() {
    let (_d, s, id) = fixture().await;
    turn(&s, &id, "auto", 1, None, None);
    s.db.with(|c| {
        c.execute("INSERT INTO automations(id,thread_id,definition_json,state,event_cursor,created_at,updated_at) VALUES('summary-auto',?1,'{}','enabled',0,'now','now')",[&id])?;
        c.execute("INSERT INTO automation_runs(id,automation_id,occurrence_key,state,scheduled_at,observed_at,definition_json,event_json,pending_id,turn_id) VALUES('summary-run','summary-auto','once','completed','now','now','{}','{}','auto-pending','auto')",[])?;
        assert_eq!(prompt_origin(c,"auto-pending")?,Some("automation"));
        Ok(())
    }).unwrap();
    for (key, kind, text, extra) in [
        ("auto:user", "userMessage", "run automation", json!({})),
        (
            "steer:peer",
            "userMessage",
            "peer prompt",
            json!({"origin":"peer"}),
        ),
        (
            "steer:actual",
            "userMessage",
            "  actual user steer  ",
            json!({}),
        ),
        (
            "legacy-peer",
            "userMessage",
            "[Pockymoe status from thread abc | Ready]\nnotification",
            json!({}),
        ),
        (
            "system",
            "userMessage",
            "<environment_context>private context</environment_context>",
            json!({}),
        ),
        (
            "explicit-system",
            "userMessage",
            "hidden system prose",
            json!({"role":"system"}),
        ),
        (
            "steer:quoted-envelope",
            "userMessage",
            "[Pockymoe status from thread abc] is the text I want explained",
            json!({"origin":"user"}),
        ),
        ("reply", "agentMessage", "public agent response", json!({})),
    ] {
        item(&s, &id, "auto", key, kind, text, None, extra);
    }
    assert_eq!(
        texts(
            &s.thread_summary(&id, &ThreadSummaryQuery::default())
                .unwrap()
        ),
        [
            "  actual user steer  ",
            "[Pockymoe status from thread abc] is the text I want explained",
            "public agent response"
        ]
    );
    let (context, target) = s
        .thread_summary_for_reply(&id, "auto", "reply", 1_000_000, 10_000_000)
        .unwrap();
    assert_eq!(
        context,
        s.thread_summary(&id, &ThreadSummaryQuery::default())
            .unwrap()
    );
    assert_eq!(target, 2);
    for key in ["peer", "automation"] {
        assert!(
            serde_json::from_str::<ThreadSummaryQuery>(&format!("{{\"{key}\":true}}")).is_err()
        );
    }
}

#[tokio::test]
async fn validation_empty_not_found_and_unlimited_default() {
    let (_d, s, id) = fixture().await;
    assert_eq!(
        s.thread_summary(&id, &ThreadSummaryQuery::default())
            .unwrap(),
        ThreadSummaryDto {
            messages: vec![],
            truncated: false
        }
    );
    assert!(s
        .thread_summary("missing", &ThreadSummaryQuery::default())
        .unwrap_err()
        .to_string()
        .contains("not found"));
    for query in [
        ThreadSummaryQuery {
            since: Some("yesterday".into()),
            ..Default::default()
        },
        ThreadSummaryQuery {
            recent_seconds: Some(0),
            ..Default::default()
        },
        ThreadSummaryQuery {
            recent_seconds: Some(u64::MAX),
            ..Default::default()
        },
        ThreadSummaryQuery {
            turns: Some(0),
            ..Default::default()
        },
        ThreadSummaryQuery {
            turns: Some(1_000_001),
            ..Default::default()
        },
        ThreadSummaryQuery {
            max_chars: Some(0),
            ..Default::default()
        },
        ThreadSummaryQuery {
            max_chars: Some(10_000_001),
            ..Default::default()
        },
    ] {
        assert!(s.thread_summary(&id, &query).is_err(), "{query:?}");
    }
    turn(&s, &id, "large", 1, None, None);
    let large = "全文🙂".repeat(25_000);
    item(
        &s,
        &id,
        "large",
        "large",
        "agentMessage",
        &large,
        None,
        json!({}),
    );
    let summary = s
        .thread_summary(&id, &ThreadSummaryQuery::default())
        .unwrap();
    assert_eq!(summary.messages[0].text, large);
    assert!(!summary.truncated);
    let (context, target) = s
        .thread_summary_for_reply(&id, "large", "large", 1_000_000, 10_000_000)
        .unwrap();
    assert_eq!(context, summary);
    assert_eq!(target, 0);
}

#[tokio::test]
async fn reply_target_uses_exact_identity_after_timeline_sort_and_keeps_original_text() {
    let (_d, s, id) = fixture().await;
    turn(&s, &id, "old", 1, None, None);
    turn(&s, &id, "target", 2, None, None);
    let original = "  repeated reply\n";
    for (turn, key, time, extra) in [
        ("old", "reply", "2030-01-01T00:00:01Z", json!({})),
        ("target", "before", "2030-01-01T00:00:12Z", json!({})),
        (
            "target",
            "reply",
            "2030-01-01T00:00:11Z",
            json!({"phase":"final","polishedReply":{"text":"previously polished"}}),
        ),
        ("target", "after", "2030-01-01T00:00:10Z", json!({})),
    ] {
        item(
            &s,
            &id,
            turn,
            key,
            "agentMessage",
            original,
            Some(time),
            extra,
        );
    }
    let (context, target) = s
        .thread_summary_for_reply(&id, "target", "reply", 1_000_000, 10_000_000)
        .unwrap();
    assert_eq!(texts(&context), [original, original, original]);
    assert_eq!(target, 1);
    assert_eq!(
        context
            .messages
            .iter()
            .map(|m| m.time.as_deref())
            .collect::<Vec<_>>(),
        [
            Some("2030-01-01T00:00:01Z"),
            Some("2030-01-01T00:00:11Z"),
            Some("2030-01-01T00:00:12Z")
        ]
    );
    assert!(!context.truncated);
    // The ordinary endpoint still includes every item, including the later row.
    assert_eq!(
        s.thread_summary(&id, &ThreadSummaryQuery::default())
            .unwrap()
            .messages
            .len(),
        4
    );
}

#[tokio::test]
async fn reply_context_bounds_execution_turns_and_history_rows_not_timestamps() {
    let (_d, s, id) = fixture().await;
    turn(&s, &id, "earlier", 1, None, Some("legacy question"));
    turn(&s, &id, "target", 2, None, None);
    // Equal ordinals use the same stable rowid tie-break as the public summary.
    turn(&s, &id, "later-tie", 2, None, Some("later tied turn"));
    turn(&s, &id, "later", 3, None, Some("later user question"));
    item(
        &s,
        &id,
        "target",
        "reply",
        "agentMessage",
        "target",
        Some("2030-01-01T00:00:02Z"),
        json!({}),
    );
    // Backfilled earlier-turn records remain context even when their history
    // rowid and timestamp are both greater than the target's.
    item(
        &s,
        &id,
        "earlier",
        "reply",
        "agentMessage",
        "earlier reply",
        Some("2030-01-01T00:00:03Z"),
        json!({}),
    );
    for (turn, key, kind, text) in [
        (
            "target",
            "steer:later",
            "userMessage",
            "later same-turn steer",
        ),
        ("target", "after", "agentMessage", "later same-turn reply"),
        ("later", "later:user", "userMessage", "later user question"),
        ("later", "reply", "agentMessage", "later reply"),
    ] {
        item(
            &s,
            &id,
            turn,
            key,
            kind,
            text,
            Some("2029-01-01T00:00:00Z"),
            json!({}),
        );
    }
    let (context, target) = s
        .thread_summary_for_reply(&id, "target", "reply", 1_000_000, 10_000_000)
        .unwrap();
    assert_eq!(
        texts(&context),
        ["legacy question", "target", "earlier reply"]
    );
    assert_eq!(target, 1);
    assert!(context.messages[0].time.is_none());
    assert!(!context.truncated);
}

#[tokio::test]
async fn reply_context_selects_recent_execution_turns_through_target_including_empty_turns() {
    let (_d, s, id) = fixture().await;
    for n in 0..8 {
        let key = format!("turn-{n}");
        turn(&s, &id, &key, n, None, None);
        // An execution turn with no public text still occupies one turn in the
        // window; do not backfill extra older turns to reach five messages.
        if n == 5 {
            continue;
        }
        item(
            &s,
            &id,
            &key,
            "reply",
            if n == 7 {
                "userMessage"
            } else {
                "agentMessage"
            },
            &format!("text-{n}"),
            Some(if n == 7 {
                "2029-01-01T00:00:00Z"
            } else {
                "2030-01-01T00:00:00Z"
            }),
            json!({}),
        );
    }
    let (summary, target) = s
        .thread_summary_for_reply(&id, "turn-6", "reply", 5, 10_000)
        .unwrap();
    assert_eq!(texts(&summary), ["text-2", "text-3", "text-4", "text-6"]);
    assert_eq!(target, 3);
    assert!(!summary.truncated);
    for count in [1, 2] {
        let (summary, target) = s
            .thread_summary_for_reply(&id, "turn-6", "reply", count, 10_000)
            .unwrap();
        assert_eq!(texts(&summary), ["text-6"]);
        assert_eq!(target, 0);
    }
    assert_eq!(
        s.thread_summary(&id, &ThreadSummaryQuery::default())
            .unwrap()
            .messages
            .len(),
        7
    );
}

#[tokio::test]
async fn reply_budget_reserves_exact_target_before_newer_backfilled_duplicate_and_unicode_tail() {
    let (_d, s, id) = fixture().await;
    turn(&s, &id, "earlier", 1, None, None);
    turn(&s, &id, "target", 2, None, None);
    let original = "同\0🙂e\u{301}";
    for (turn, key, text, time) in [
        ("earlier", "oldest", "oldest", "2030-01-01T00:00:00Z"),
        ("target", "commentary", "最尾🦀x", "2030-01-01T00:00:03Z"),
        ("target", "reply", original, "2030-01-01T00:00:01Z"),
        // The same text from an older execution turn sorts after the target,
        // and is persisted later too. Neither order can substitute its identity.
        ("earlier", "reply", original, "2030-01-01T00:00:02Z"),
        (
            "target",
            "after",
            "excluded after target",
            "2030-01-01T00:00:00Z",
        ),
    ] {
        item(
            &s,
            &id,
            turn,
            key,
            "agentMessage",
            text,
            Some(time),
            json!({}),
        );
    }
    let (summary, target) = s
        .thread_summary_for_reply(&id, "target", "reply", 2, 12)
        .unwrap();
    assert_eq!(texts(&summary), [original, "🙂e\u{301}", "最尾🦀x"]);
    assert_eq!(target, 0);
    assert!(summary.truncated);
    assert!(!summary.messages[target].truncated);
    assert!(summary.messages[1].truncated);
    assert!(!summary.messages[2].truncated);
    assert_eq!(
        summary
            .messages
            .iter()
            .map(|m| m.text.chars().count())
            .sum::<usize>(),
        12
    );
    // Exactly the target's Unicode size leaves room for no other context.
    let (summary, target) = s
        .thread_summary_for_reply(&id, "target", "reply", 2, 5)
        .unwrap();
    assert_eq!(texts(&summary), [original]);
    assert_eq!(target, 0);
    assert!(summary.truncated);
    assert!(!summary.messages[0].truncated);
    let error = s
        .thread_summary_for_reply(&id, "target", "reply", 2, 4)
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("replyContextTargetTooLarge:"));
    assert!(error.contains("requires 5 Unicode characters"));
    assert!(error.contains("maxChars is 4"));
    // Public maxChars keeps its existing newest-suffix semantics; reserving the
    // target is exclusive to the internal rewrite projection.
    let public = s
        .thread_summary(
            &id,
            &ThreadSummaryQuery {
                max_chars: Some(12),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(texts(&public), ["🙂e\u{301}", original, "最尾🦀x"]);
    assert!(public.messages[0].truncated);
}

#[tokio::test]
async fn reply_context_validates_explicit_limits_without_changing_public_unlimited_default() {
    let (_d, s, id) = fixture().await;
    turn(&s, &id, "target", 1, None, None);
    item(
        &s,
        &id,
        "target",
        "reply",
        "agentMessage",
        "ok",
        None,
        json!({}),
    );
    for (turns, chars) in [
        (0, 10_000),
        (1_000_001, 10_000),
        (u64::MAX, 10_000),
        (5, 0),
        (5, 10_000_001),
        (5, u64::MAX),
    ] {
        let error = s
            .thread_summary_for_reply(&id, "target", "reply", turns, chars)
            .unwrap_err()
            .to_string();
        assert!(!error.starts_with("replyContextTargetTooLarge:"));
        assert!(error.contains("must be between"), "{error}");
    }
    let (summary, target) = s
        .thread_summary_for_reply(&id, "target", "reply", 5, 10_000)
        .unwrap();
    assert_eq!(target, 0);
    assert_eq!(
        summary,
        s.thread_summary(&id, &ThreadSummaryQuery::default())
            .unwrap()
    );
}

#[tokio::test]
async fn reply_target_must_exist_in_thread_turn_and_survive_public_assistant_filters() {
    let (_d, s, id) = fixture().await;
    turn(&s, &id, "one", 1, None, None);
    turn(&s, &id, "two", 2, None, None);
    for (key, kind, extra) in [
        ("one:user", "userMessage", json!({})),
        ("reasoning", "reasoning", json!({})),
        ("tool", "commandExecution", json!({})),
        ("channel", "agentMessage", json!({"channel":"analysis"})),
        ("phase", "agentMessage", json!({"phase":"private"})),
        (
            "response-phase",
            "agentMessage",
            json!({"responsePhase":"reasoning"}),
        ),
        ("status", "agentMessage", json!({"status":"internal"})),
        ("role", "agentMessage", json!({"role":"system"})),
        ("origin", "agentMessage", json!({"origin":{"kind":"peer"}})),
        ("no-text", "agentMessage", json!({"text":null})),
    ] {
        item(&s, &id, "one", key, kind, "same text", None, extra);
        assert!(
            s.thread_summary_for_reply(&id, "one", key, 1_000_000, 10_000_000)
                .unwrap_err()
                .to_string()
                .contains("not a public assistant message"),
            "{key}"
        );
    }
    item(&s, &id, "one", "empty", "agentMessage", "", None, json!({}));
    assert!(s
        .thread_summary_for_reply(&id, "one", "empty", 1_000_000, 10_000_000)
        .unwrap_err()
        .to_string()
        .contains("not a public assistant message"));
    item(
        &s,
        &id,
        "one",
        "visible",
        "agentMessage",
        "same text",
        None,
        json!({}),
    );
    // A visible identical message must not rescue a filtered or missing target.
    assert!(s
        .thread_summary_for_reply(&id, "one", "channel", 1_000_000, 10_000_000)
        .is_err());
    for (turn, item) in [
        ("one", "missing"),
        ("missing", "visible"),
        ("two", "visible"),
    ] {
        assert!(s
            .thread_summary_for_reply(&id, turn, item, 1_000_000, 10_000_000)
            .unwrap_err()
            .to_string()
            .contains("reply target not found"));
    }
    let other = s
        .create_thread(CreateThreadInput {
            workspace_id: s.get_thread(&id).unwrap().workspace_id,
            title: None,
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "default".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: None,
        })
        .await
        .unwrap();
    assert!(s
        .thread_summary_for_reply(&other.id, "one", "visible", 1_000_000, 10_000_000)
        .unwrap_err()
        .to_string()
        .contains("reply target not found"));
    assert!(s
        .thread_summary_for_reply("missing", "one", "visible", 1_000_000, 10_000_000)
        .unwrap_err()
        .to_string()
        .contains("not found"));
}

#[tokio::test]
async fn unicode_budget_includes_embedded_nul_and_does_not_cut_utf8() {
    let (_d, s, id) = fixture().await;
    turn(&s, &id, "nul", 1, None, None);
    item(
        &s,
        &id,
        "nul",
        "body",
        "agentMessage",
        "旧\0中🙂e\u{301}",
        None,
        json!({}),
    );
    let whole = s
        .thread_summary(&id, &ThreadSummaryQuery::default())
        .unwrap();
    assert_eq!(texts(&whole), ["旧\0中🙂e\u{301}"]);
    for (cap, expected) in [
        (5, "\0中🙂e\u{301}"),
        (4, "中🙂e\u{301}"),
        (3, "🙂e\u{301}"),
        (1, "\u{301}"),
    ] {
        let result = s
            .thread_summary(
                &id,
                &ThreadSummaryQuery {
                    max_chars: Some(cap),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(texts(&result), [expected]);
        assert!(result.truncated && result.messages[0].truncated);
    }
}

#[tokio::test]
async fn peer_origin_is_recorded_at_acceptance_and_preserved_when_queue_is_consumed() {
    let (_d, s, id) = fixture().await;
    let receiver = s.get_thread(&id).unwrap();
    let sender = s
        .create_thread(CreateThreadInput {
            workspace_id: receiver.workspace_id,
            title: None,
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "default".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: None,
        })
        .await
        .unwrap();
    let receipt=s.send_to_thread(&id,serde_json::from_value(json!({"text":"peer assignment","delivery":"queue","kind":"task","subject":"Assignment","fromThreadId":sender.id})).unwrap()).unwrap();
    let pending = receipt["pendingSteerId"].as_str().unwrap();
    assert_eq!(
        s.db.with(|c| prompt_origin(c, pending)).unwrap(),
        Some("peer")
    );
    assert!(s
        .thread_summary(&id, &ThreadSummaryQuery::default())
        .unwrap()
        .messages
        .is_empty());
    s.drain_steers(&id).await.unwrap();
    let summary = s
        .thread_summary(&id, &ThreadSummaryQuery::default())
        .unwrap();
    assert!(!summary.messages.is_empty());
    assert!(summary
        .messages
        .iter()
        .all(|m| m.role == ThreadSummaryRole::Assistant));
    let origin=s.db.with(|c|Ok(c.query_row("SELECT json_extract(item_json,'$.origin') FROM thread_history_items WHERE thread_id=?1 AND json_extract(item_json,'$.kind')='userMessage'",[&id],|r|r.get::<_,String>(0))?)).unwrap();
    assert_eq!(origin, "peer");
}

#[test]
fn the_recipient_guardrail_names_the_verified_sender_and_strips_cleanly() {
    let line = recipient_guardrail(&json!({"userId": "u-1", "name": "Ma]l\nlo[ry"}));
    assert!(
        line.starts_with("[Pockymoe: sent by Mallory, a share recipient"),
        "{line}"
    );
    assert!(line.ends_with("system settings.]"));
    assert_eq!(line.matches(']').count(), 1);
    assert!(recipient_guardrail(&json!({"userId": "u-1"})).contains("sent by u-1,"));
    assert_eq!(guarded_prompt(None, "as typed"), "as typed");
    let guarded = guarded_prompt(Some(&json!({"userId": "u-1"})), "as typed");
    assert_eq!(without_recipient_guardrail(&guarded), "as typed");
    assert_eq!(
        without_recipient_guardrail("[Pockymoe task from x] keep"),
        "[Pockymoe task from x] keep"
    );
}

fn summary_view(s: &Supervisor, id: &str, limit: Option<u32>, before: Option<&str>) -> Value {
    s.transcript(
        id,
        &crate::interaction::TranscriptQuery {
            view: Some("summary".into()),
            limit,
            before_turn_id: before.map(str::to_owned),
            ..Default::default()
        },
    )
    .unwrap()
}

fn turn_texts(turn: &Value) -> Vec<(&str, &str)> {
    turn["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["role"].as_str().unwrap(), m["text"].as_str().unwrap()))
        .collect()
}

#[tokio::test]
async fn messages_carry_their_turn_ids_including_legacy_display_prompts() {
    let (_d, s, id) = fixture().await;
    turn(
        &s,
        &id,
        "legacy",
        1,
        Some("2030-01-01T00:00:00Z"),
        Some("old question"),
    );
    item(
        &s,
        &id,
        "legacy",
        "a",
        "agentMessage",
        "old answer",
        Some("2030-01-01T00:00:01Z"),
        json!({}),
    );
    turn(&s, &id, "new", 2, Some("2030-01-02T00:00:00Z"), None);
    item(
        &s,
        &id,
        "new",
        "new:user",
        "userMessage",
        "new question",
        Some("2030-01-02T00:00:00Z"),
        json!({}),
    );
    let summary = s
        .thread_summary(&id, &ThreadSummaryQuery::default())
        .unwrap();
    assert_eq!(
        summary
            .messages
            .iter()
            .map(|m| (m.text.as_str(), m.turn_id.as_deref().unwrap()))
            .collect::<Vec<_>>(),
        [
            ("old question", "legacy"),
            ("old answer", "legacy"),
            ("new question", "new")
        ]
    );
    let json = serde_json::to_value(&summary.messages[0]).unwrap();
    assert_eq!(json["turnId"], "legacy");
    // Readers without a turn omit the field.
    let mut bare = summary.messages[0].clone();
    bare.turn_id = None;
    assert!(serde_json::to_value(&bare).unwrap().get("turnId").is_none());
}

#[tokio::test]
async fn summary_view_keeps_each_reply_groups_by_turn_and_pages_older_turns() {
    let (_d, s, id) = fixture().await;
    turn(&s, &id, "t1", 1, Some("2030-01-01T00:00:00Z"), None);
    for (key, kind, text, time) in [
        (
            "t1:user",
            "userMessage",
            "first question",
            "2030-01-01T00:00:00Z",
        ),
        ("p1", "agentMessage", "looking", "2030-01-01T00:00:01Z"),
        ("f1", "agentMessage", "first answer", "2030-01-01T00:00:02Z"),
    ] {
        item(&s, &id, "t1", key, kind, text, Some(time), json!({}));
    }
    // Tools only: the turn is listed without messages.
    turn(&s, &id, "t2", 2, Some("2030-01-02T00:00:00Z"), None);
    item(
        &s,
        &id,
        "t2",
        "tool",
        "commandExecution",
        "ls",
        Some("2030-01-02T00:00:01Z"),
        json!({}),
    );
    turn(&s, &id, "t3", 3, Some("2030-01-03T00:00:00Z"), None);
    for (key, kind, text, time) in [
        (
            "t3:user",
            "userMessage",
            "second question",
            "2030-01-03T00:00:00Z",
        ),
        (
            "a3",
            "agentMessage",
            "answer before steer",
            "2030-01-03T00:00:01Z",
        ),
        (
            "steer:s",
            "userMessage",
            "change it",
            "2030-01-03T00:00:02Z",
        ),
        ("p3", "agentMessage", "changing", "2030-01-03T00:00:03Z"),
        ("f3", "agentMessage", "changed", "2030-01-03T00:00:04Z"),
    ] {
        item(&s, &id, "t3", key, kind, text, Some(time), json!({}));
    }
    let page = summary_view(&s, &id, None, None);
    assert_eq!(page["view"], "summary");
    assert_eq!(page["truncated"], false);
    assert!(page["next"].is_null());
    let turns = page["turns"].as_array().unwrap();
    assert_eq!(
        turns
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["t1", "t2", "t3"]
    );
    assert_eq!(
        turn_texts(&turns[0]),
        [("user", "first question"), ("assistant", "first answer")]
    );
    assert_eq!(turn_texts(&turns[1]), []);
    assert_eq!(
        turn_texts(&turns[2]),
        [
            ("user", "second question"),
            ("assistant", "answer before steer"),
            ("user", "change it"),
            ("assistant", "changed")
        ]
    );
    assert_eq!(turns[2]["ordinal"], 3);
    assert_eq!(turns[2]["status"], "inProgress");
    assert_eq!(turns[2]["messages"][0]["time"], "2030-01-03T00:00:00Z");
    assert!(turns[2]["messages"][0].get("turnId").is_none());
    assert_eq!(
        turns[2]["overview"],
        format!("pockymoe transcript {id} --turn t3 --view overview")
    );

    let newest = summary_view(&s, &id, Some(2), None);
    assert_eq!(newest["turns"].as_array().unwrap().len(), 2);
    assert_eq!(newest["nextBeforeTurnId"], "t2");
    assert_eq!(
        newest["next"],
        format!("pockymoe transcript {id} --view summary --before-turn t2 --limit 2")
    );
    let older = summary_view(&s, &id, Some(2), Some("t2"));
    assert_eq!(older["turns"][0]["id"], "t1");
    assert_eq!(older["turns"].as_array().unwrap().len(), 1);
    assert!(older["next"].is_null());
}

#[tokio::test]
async fn summary_view_caps_messages_keeping_their_start_and_stops_at_the_budget() {
    let (_d, s, id) = fixture().await;
    for n in 1..=12 {
        let t = format!("t{n}");
        let time = format!("2030-01-{n:02}T00:00:00Z");
        turn(&s, &id, &t, n, Some(&time), None);
        let question = format!("Q{n}{}", "q".repeat(1_598));
        item(
            &s,
            &id,
            &t,
            &format!("{t}:user"),
            "userMessage",
            &question,
            Some(&time),
            json!({}),
        );
        let later = format!("2030-01-{n:02}T00:00:01Z");
        item(
            &s,
            &id,
            &t,
            "a",
            "agentMessage",
            &format!("A{n}{}", "a".repeat(1_598)),
            Some(&later),
            json!({}),
        );
    }
    let page = summary_view(&s, &id, Some(12), None);
    assert_eq!(page["truncated"], true);
    let turns = page["turns"].as_array().unwrap();
    // 16,000 characters: five whole turns of 2 x 1,500, then 1,000 of turn 7's reply.
    assert_eq!(
        turns
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["t7", "t8", "t9", "t10", "t11", "t12"]
    );
    let last = &turns[5]["messages"];
    assert_eq!(last[0]["text"].as_str().unwrap().chars().count(), 1_500);
    assert!(last[0]["text"].as_str().unwrap().starts_with("Q12q"));
    assert_eq!(last[0]["truncated"], true);
    assert_eq!(turn_texts(&turns[0]).len(), 1);
    let cut = turns[0]["messages"][0]["text"].as_str().unwrap();
    assert!(cut.starts_with("A7a"));
    assert_eq!(cut.chars().count(), 1_000);
    assert_eq!(page["nextBeforeTurnId"], "t7");
    let older = summary_view(&s, &id, Some(12), Some("t7"));
    assert_eq!(older["turns"][0]["id"], "t1");
    assert_eq!(older["turns"].as_array().unwrap().len(), 6);
}

#[tokio::test]
async fn summary_view_rejects_turn_reads_offsets_and_large_limits() {
    use crate::interaction::TranscriptQuery;
    let (_d, s, id) = fixture().await;
    turn(&s, &id, "t1", 1, None, Some("hello"));
    let error = |q: TranscriptQuery| {
        s.transcript(
            &id,
            &TranscriptQuery {
                view: Some("summary".into()),
                ..q
            },
        )
        .unwrap_err()
        .to_string()
    };
    for (query, message) in [
        (
            TranscriptQuery {
                turn_id: Some("t1".into()),
                ..Default::default()
            },
            "--turn TURN_ID --view overview",
        ),
        (
            TranscriptQuery {
                offset: Some(1),
                ..Default::default()
            },
            "beforeTurnId",
        ),
        (
            TranscriptQuery {
                limit: Some(51),
                ..Default::default()
            },
            "between 1 and 50",
        ),
        (
            TranscriptQuery {
                before_turn_id: Some("missing".into()),
                ..Default::default()
            },
            "before turn not found",
        ),
    ] {
        let error = error(query);
        assert!(error.contains(message), "{error}");
    }
    assert_eq!(
        summary_view(&s, &id, Some(50), None)["turns"][0]["messages"][0]["text"],
        "hello"
    );
}
