use super::*;

async fn setup() -> (tempfile::TempDir, Arc<Supervisor>, String) {
    let dir = tempfile::tempdir().unwrap();
    let state = automation::tests::supervisor(dir.path());
    let ws = state
        .create_workspace(CreateWorkspaceInput {
            abs_path: Some(dir.path().to_string_lossy().into()),
            git_url: None,
            label: None,
        })
        .unwrap();
    let thread = state
        .create_thread(CreateThreadInput {
            workspace_id: ws.id,
            title: None,
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "gpt-6.1-sol".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: None,
        })
        .await
        .unwrap();
    (dir, state, thread.id)
}

#[tokio::test]
async fn accounting_admission_records_origins_and_resume_is_not_new_turn() {
    let (_dir, s, id) = setup().await;
    for (origin, marker, sender) in [
        ("owner", None, None),
        (
            "guest",
            None,
            Some(json!({"userId":"guest","name":"Guest"})),
        ),
        ("agent", Some("peer"), None),
        ("agent", Some("system"), None),
        ("automation", Some("automation"), None),
    ] {
        let input: SendThreadPromptInput =
            serde_json::from_value(json!({"prompt":"run accounting fixture"})).unwrap();
        let sender = sender.map(|v| serde_json::from_value(v).unwrap());
        s.accept_prompt_from(&id, &input, sender.as_ref())
            .await
            .unwrap();
        let pending =
            s.db.with(|c| {
                Ok(c.query_row(
                    "SELECT id FROM thread_pending_steers WHERE thread_id=?1",
                    [&id],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .unwrap();
        if let Some(marker) = marker {
            if marker == "automation" {
                // Explicit automation provenance can also be recorded on a queued
                // native prompt; no display text/lineage inference is involved.
                s.db.set_kv(&format!("cli:prompt-origin:{pending}"), "automation")
                    .unwrap();
            } else {
                s.db.set_kv(&format!("cli:prompt-origin:{pending}"), marker)
                    .unwrap();
            }
        }
        s.run_turn(
            s.get_thread(&id).unwrap(),
            input.prompt,
            None,
            None,
            vec![],
            Some(&pending),
            None,
        )
        .await
        .unwrap();
        let turn = s.load_turns_meta(&id).unwrap().pop().unwrap();
        assert_eq!(turn.origin.as_deref(), Some(origin));
    }
    let before = s.stats_summary("all", "UTC").unwrap()["totals"]["turns"].clone();
    s.resume_thread(&id).await.unwrap();
    assert_eq!(
        s.stats_summary("all", "UTC").unwrap()["totals"]["turns"],
        before
    );
    let stats = s.stats_summary_with_origin("all", "UTC", "agent").unwrap();
    assert_eq!(stats["totals"]["turns"], 2);
    assert!(s
        .stats_summary_with_origin("all", "UTC", "invented")
        .is_err());
}

#[tokio::test]
async fn accounting_import_keeps_completion_and_unknown_origin_and_forks_are_context_only() {
    let (_dir, s, id) = setup().await;
    let turn:pockymoe_protocol::ThreadTurnDto=serde_json::from_value(json!({"id":"native-import","status":"completed","startedAt":"2026-01-01T00:00:00Z","completedAt":"2026-01-01T00:00:02Z","error":null,"model":"gpt-6.1-sol","items":[],"tokenUsage":{"total":{"inputTokens":100,"outputTokens":10},"last":{"inputTokens":100,"outputTokens":10}}})).unwrap();
    s.persist_imported_turns(
        &id,
        &ImportSessionMeta {
            turns: vec![turn],
            ..Default::default()
        },
    )
    .unwrap();
    let before = s.stats_summary("all", "UTC").unwrap();
    assert_eq!(before["totals"]["turns"], 1);
    assert_eq!(before["totals"]["avgDurationMs"], 2000.0);
    let copied = s.load_turns_meta(&id).unwrap();
    assert_eq!(copied[0].origin.as_deref(), Some("unknown"));
    // Exercise the real persistence path without requiring this fake harness
    // to advertise a remote session/fork operation.
    let detail = s.get_thread_detail(&id, None).await.unwrap();
    let (fork, _, _) = s
        .persist_forked_thread(
            &detail,
            crate::actor::StartSessionResult {
                provider_session_id: Uuid::new_v4().to_string(),
                model: None,
                reasoning_effort: None,
            },
            1,
            None,
            None,
        )
        .unwrap();
    let after = s.stats_summary("all", "UTC").unwrap();
    assert_eq!(after["totals"]["turns"], 1);
    assert_eq!(after["totals"]["tokens"], 110);
    assert_eq!(after["coverage"]["excludedHistoryCopies"], 1);
    assert_eq!(s.load_turns_meta(&fork.id).unwrap().len(), 1);
    let activity = s
        .thread_activity("2025-12-31T00:00:00Z", "2026-01-02T00:00:00Z", 48)
        .unwrap();
    assert_eq!(activity.iter().map(|t| t.turns).sum::<u32>(), 1);
    assert_eq!(activity[0].total_ms, 2000);
    assert_eq!(activity[0].by_origin.as_ref().unwrap()["unknown"], 1);
}

#[tokio::test]
async fn accounting_codex_resume_counter_reset_and_late_acp_fallback_do_not_rebill() {
    let (_dir, s, id) = setup().await;
    s.run_turn(
        s.get_thread(&id).unwrap(),
        "counter fixture".into(),
        None,
        None,
        vec![],
        None,
        None,
    )
    .await
    .unwrap();
    let turn = s.load_turns_meta(&id).unwrap()[0].id.clone();
    let emit = |usage: Value| {
        s.persist_usage_event(&pockymoe_protocol::ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::RuntimeUsageUpdated,
            thread_id: id.clone(),
            timestamp: now_rfc3339(),
            payload: json!({"turnId":turn,"usage":usage}),
        })
        .unwrap()
    };
    emit(
        json!({"source":"codexRollout","cumulative":true,"total":{"inputTokens":1400,"outputTokens":100},"last":{"inputTokens":190,"outputTokens":10},"baselineTotal":{"inputTokens":950,"outputTokens":50}}),
    );
    let tokens = || {
        s.load_turns_meta(&id).unwrap()[0]
            .token_usage
            .as_ref()
            .unwrap()["total"]["totalTokens"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(tokens(), 500);
    emit(
        json!({"source":"codexRollout","cumulative":true,"total":{"inputTokens":90,"outputTokens":10},"last":{"inputTokens":90,"outputTokens":10}}),
    );
    assert_eq!(tokens(), 600);
    emit(
        json!({"source":"codexRollout","cumulative":true,"total":{"inputTokens":90,"outputTokens":10},"last":{"inputTokens":90,"outputTokens":10}}),
    );
    assert_eq!(tokens(), 600);
    emit(json!({"inputTokens":1,"outputTokens":1}));
    assert_eq!(tokens(), 600);
    assert_eq!(s.stats_summary("all", "UTC").unwrap()["totals"]["turns"], 1);
}

#[tokio::test]
async fn accounting_persists_normalized_turn_and_usage_model_labels() {
    let (_dir, s, id) = setup().await;
    let raw_model = r#"["remote-codex","gpt-6.1-sol"]"#;
    let mut thread = s.get_thread(&id).unwrap();
    thread.model = Some(raw_model.into());
    s.run_turn(
        thread,
        "model label fixture".into(),
        None,
        None,
        vec![],
        None,
        None,
    )
    .await
    .unwrap();
    let turn = s.load_turns_meta(&id).unwrap()[0].clone();
    assert_eq!(turn.model.as_deref(), Some("gpt-6.1-sol"));
    s.persist_usage_event(&pockymoe_protocol::ThreadEventEnvelope {
        event_type: pockymoe_protocol::ThreadEventType::RuntimeUsageUpdated,
        thread_id: id.clone(),
        timestamp: now_rfc3339(),
        payload: json!({"turnId":turn.id,"usage":{"model":raw_model,"total":{"inputTokens":100,"outputTokens":10}}}),
    })
    .unwrap();
    let summary = s.stats_summary("all", "UTC").unwrap();
    assert_eq!(summary["byModel"][0]["model"], "gpt-6.1-sol");
    assert_eq!(summary["totals"]["tokens"], 110);
}
