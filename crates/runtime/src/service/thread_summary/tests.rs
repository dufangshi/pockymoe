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
        .all(|m| m.as_object().unwrap().len() == 3));
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
