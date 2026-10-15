use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Provider};
use pockymoe_runtime::{
    fake::FakeRuntime,
    interaction::{SendInput, TranscriptQuery},
    Database, RuntimeConfig, Supervisor,
};
use rusqlite::params;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn setup() -> (tempfile::TempDir, Arc<Supervisor>) {
    let dir = tempfile::tempdir().unwrap();
    let config = RuntimeConfig {
        treer_enabled: false,
        host_mode: Default::default(),
        mode: pockymoe_protocol::Mode::Local,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root: dir.path().into(),
        database_url: dir.path().join("db.sqlite"),
        app_name: "test".into(),
        app_version: "test".into(),
        environment: "test".into(),
        auth_required: false,
        admin_username: None,
        admin_password: None,
        session_secret: None,
        relay_server_url: Some("https://relay.example.test".into()),
        web_origin: None,
        relay_agent_token: None,
        enabled_providers: vec![Provider::Codex, Provider::Acp],
        acp_command: None,
        acp_startup_timeout_ms: 1000,
        fake_runtime: true,
        supervisor_restart_strategy: Default::default(),
    };
    let db = Database::open(&config.database_url).unwrap();
    let state = Arc::new(Supervisor::new(
        config,
        db,
        vec![
            Arc::new(FakeRuntime::new(Provider::Codex)),
            Arc::new(FakeRuntime::new(Provider::Acp)),
        ],
    ));
    state.spawn_live_item_persister();
    (dir, state)
}
async fn thread(s: &Supervisor, p: Provider) -> String {
    let ws = s
        .list_workspaces()
        .unwrap()
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            s.create_workspace(CreateWorkspaceInput {
                abs_path: Some(s.config.workspace_root.to_string_lossy().into()),
                git_url: None,
                label: Some("test".into()),
            })
            .unwrap()
        });
    s.create_thread(CreateThreadInput {
        workspace_id: ws.id,
        title: None,
        provider: Some(p),
        agent_id: Some(if p == Provider::Acp { "grok" } else { "codex" }.into()),
        model: "ios-e2e-stream".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: None,
    })
    .await
    .unwrap()
    .id
}
fn send(from: &str, text: &str, notify: bool, key: &str) -> SendInput {
    SendInput {
        delivery: "queue".into(),
        notify_delivery: "inbox".into(),
        text: text.into(),
        from_thread_id: Some(from.into()),
        notify_on_complete: notify,
        client_request_id: Some(key.into()),
        subject: None,
        kind: Some("task".into()),
        in_reply_to: None,
        interrupt_reason: None,
        topic_key: None,
    }
}

fn decrypt_publication(envelope: &Value) -> Value {
    use aes_gcm::{
        aead::{Aead, KeyInit, Payload},
        Aes256Gcm, Nonce,
    };
    use base64::Engine;
    let fixture: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/public-link-v2.json")).unwrap();
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let key = b64.decode(fixture["key"].as_str().unwrap()).unwrap();
    let iv = b64.decode(envelope["iv"].as_str().unwrap()).unwrap();
    let ciphertext = b64
        .decode(envelope["ciphertext"].as_str().unwrap())
        .unwrap();
    let plaintext = Aes256Gcm::new_from_slice(&key)
        .unwrap()
        .decrypt(
            Nonce::from_slice(&iv),
            Payload {
                msg: &ciphertext,
                aad: b"pockymoe-public-link-v1",
            },
        )
        .unwrap();
    serde_json::from_slice(&plaintext).unwrap()
}

#[tokio::test]
async fn publication_selects_history_and_includes_future_until_revoked() {
    let (_dir, s) = setup();
    let thread = thread(&s, Provider::Codex).await;
    let seed = |ordinal: i64| {
        let turn = uuid::Uuid::new_v4().to_string();
        s.db.with(|conn| {
            conn.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES (?1,?2,'completed',?3)", params![turn,thread,ordinal])?;
            conn.execute("UPDATE thread_turns SET token_usage_json=?1 WHERE id=?2", params![json!({"total":{"totalTokens":10,"private":"internal"},"last":{"inputTokens":5},"private":"internal"}).to_string(),turn])?;
            for (n, kind, text, phase) in [(0,"userMessage",format!("prompt {ordinal}"),""),(1,"reasoning","private reasoning".into(),""),(2,"agentMessage","private commentary".into(),"commentary"),(3,"agentMessage",format!("answer {ordinal}"),"final_answer")] {
                let item = format!("{turn}-{n}");
                conn.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES (?1,?2,?3,?1,?4,?5,?5)",params![item,thread,turn,json!({"id":item,"kind":kind,"text":text,"phase":phase}).to_string(),format!("2026-09-20T00:00:0{n}Z")])?;
            }
            Ok(())
        }).unwrap();
        turn
    };
    let first = seed(1);
    let _hidden = seed(2);
    let publication = s
        .create_publication(
            &thread,
            pockymoe_runtime::publications::CreatePublication {
                turn_ids: vec![first],
                theme: "light".into(),
                encryption_key: serde_json::from_str::<Value>(include_str!(
                    "../../../tests/fixtures/public-link-v2.json"
                ))
                .unwrap()["key"]
                    .as_str()
                    .unwrap()
                    .into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(publication["snapshot"]["version"], 2);
    assert!(publication["snapshot"].get("title").is_none());
    assert!(!publication.to_string().contains("prompt 1"));
    let original = decrypt_publication(&publication["snapshot"]);
    assert_eq!(original["turnCount"], 1);
    assert_eq!(original["theme"], "light");
    assert!(!publication.to_string().contains("prompt 2"));
    seed(3);
    let token = publication["token"].as_str().unwrap();
    let updated = s.read_publication(token).await.unwrap();
    assert_ne!(updated["iv"], publication["snapshot"]["iv"]);
    let updated = decrypt_publication(&updated);
    assert_eq!(updated["turnCount"], 2);
    assert!(updated.to_string().contains("answer 3"));
    assert!(!updated.to_string().contains("prompt 2"));
    assert!(!updated.to_string().contains("private"));
    assert!(!updated.to_string().contains(&thread));
    assert!(s
        .read_publication(&uuid::Uuid::new_v4().simple().to_string())
        .await
        .is_err());
    // Previously persisted policies still produce plaintext for existing v1 links.
    s.db.with(|conn| {
        conn.execute(
            "UPDATE kv SET value=json_remove(value,'$.encryptionKey') WHERE key=?1",
            [format!("publication:{token}")],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(s.read_publication(token).await.unwrap()["turnCount"], 2);
    // Even a surviving/restored legacy capability cannot leak plaintext once bound.
    s.db.with(|c| {
        c.execute(
            "UPDATE device_owner_trust SET account_key_id='bound',strict=1 WHERE singleton=1",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(s.read_publication(token).await.is_err());
    s.db.with(|c| {
        c.execute(
            "UPDATE device_owner_trust SET account_key_id=NULL,strict=0 WHERE singleton=1",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(s.read_publication(token).await.unwrap()["turnCount"], 2);
    s.revoke_publication(token).unwrap();
    assert!(s.read_publication(token).await.is_err());
}
async fn until(mut check: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn conversation_is_recent_bounded_and_every_stored_detail_is_discoverable() {
    let (_dir, s) = setup();
    let a = thread(&s, Provider::Codex).await;
    s.db.with(|c|{
        for n in 1..=5 {
            let turn=format!("turn-{n}");
            c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal,started_at) VALUES(?1,?2,'completed',?3,'2026-09-11T00:00:00Z')",params![turn,a,n])?;
            for (i,kind,text) in [(0,"userMessage",format!("question {n}")),(1,"agentMessage","progress".into()),(2,"commandExecution","secret-tool-output".repeat(20000)),(3,"agentMessage","字".repeat(10000)),(4,"agentMessage","final".into())] {
                let value=json!({"id":format!("item-{i}"),"kind":kind,"text":text,"createdAt":"2026-09-11T00:00:01Z","vendor":{"preserved":true}});
                c.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'2026-09-11T00:00:01Z','2026-09-11T00:00:02Z')",params![format!("{n}-{i}"),a,turn,format!("item-{i}"),value.to_string()])?;
            }
        } Ok(())
    }).unwrap();
    let latest = s.transcript(&a, &TranscriptQuery::default()).unwrap();
    assert_eq!(latest["turns"][0]["id"], "turn-3");
    assert_eq!(latest["turns"].as_array().unwrap().len(), 3);
    assert_eq!(latest["nextBeforeTurnId"], "turn-3");
    assert!(!latest.to_string().contains("secret-tool-output"));
    let items = latest["turns"][0]["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(items[1]["text"], "progress");
    assert_eq!(items[2]["truncated"], true);
    assert_eq!(items[3]["text"], "final");
    assert!(latest.to_string().len() < 30000);
    let early = s
        .transcript(
            &a,
            &TranscriptQuery {
                before_turn_id: Some("turn-3".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(early["turns"][0]["items"][0]["text"], "question 1");
    let expanded = s
        .transcript(
            &a,
            &TranscriptQuery {
                turn_id: Some("turn-3".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(expanded["turns"][0]["items"].as_array().unwrap().len(), 5);
    let mut offset = 0;
    let mut all = String::new();
    loop {
        let part = s
            .transcript(
                &a,
                &TranscriptQuery {
                    turn_id: Some("turn-3".into()),
                    item_id: Some("item-3".into()),
                    text_offset: Some(offset),
                    ..Default::default()
                },
            )
            .unwrap();
        all.push_str(part["text"].as_str().unwrap());
        match part["nextTextOffset"].as_u64() {
            Some(next) => offset = next as u32,
            None => break,
        }
    }
    assert_eq!(all, "字".repeat(10000));
    let raw = s
        .transcript(
            &a,
            &TranscriptQuery {
                turn_id: Some("turn-3".into()),
                item_id: Some("item-1".into()),
                raw: true,
                ..Default::default()
            },
        )
        .unwrap();
    let raw: Value = serde_json::from_str(raw["text"].as_str().unwrap()).unwrap();
    assert_eq!(raw["vendor"]["preserved"], true);
    assert!(s
        .transcript(
            &a,
            &TranscriptQuery {
                before_turn_id: Some("missing".into()),
                ..Default::default()
            }
        )
        .is_err());
}

#[tokio::test]
async fn managed_path_resolves_pockymoe_to_this_executable() {
    let (_dir, s) = setup();
    // Test executables are not named pockymoe, like the suffixed release binaries.
    let dir = s
        .configure_cli("http://127.0.0.1:8787".into())
        .bin_dir
        .expect("a pockymoe link for managed agents");
    // Skills and hook scripts written before the rename still run `remote-codex`.
    for command in ["pockymoe", "remote-codex"] {
        let link = dir.join(if cfg!(windows) {
            format!("{command}.exe")
        } else {
            command.to_string()
        });
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            std::env::current_exe().unwrap()
        );
        assert!(link.is_file());
    }
    let env = s
        .with_cli_context("thread-a", async {
            pockymoe_runtime::interaction::launch_env()
        })
        .await;
    let path = &env.iter().find(|(key, _)| key == "PATH").unwrap().1;
    assert_eq!(std::env::split_paths(path).next(), Some(dir));
    let value = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    for name in ["THREAD_ID", "URL", "TOKEN"] {
        assert!(value(&format!("POCKYMOE_{name}")).is_some());
        assert_eq!(
            value(&format!("REMOTE_CODEX_{name}")),
            value(&format!("POCKYMOE_{name}"))
        );
    }
}

#[tokio::test]
async fn cli_identity_is_rebound_when_a_loaded_session_changes_thread_context() {
    use pockymoe_runtime::{
        acp::AcpRuntime,
        actor::{AgentRuntime, EventBus, SessionSettings, StartSessionInput, StartTurnInput},
    };
    let (dir, s) = setup();
    s.configure_cli("http://127.0.0.1:8787".into());
    let script = dir.path().join("agent.py");
    std::fs::write(&script,format!("import os,json\nwith open('cli-context.json','w') as f: json.dump({{'threadId':os.getenv('POCKYMOE_THREAD_ID'),'hasToken':bool(os.getenv('POCKYMOE_TOKEN'))}},f)\n{}",include_str!("fixtures/fake_acp_agent.py"))).unwrap();
    let python = which::which("python3")
        .or_else(|_| which::which("python"))
        .unwrap();
    let runtime = AcpRuntime::catalog(
        Some(format!("\"{}\" \"{}\"", python.display(), script.display())),
        5000,
    );
    let started = s
        .with_cli_context(
            "thread-a",
            runtime.start_session(StartSessionInput {
                cwd: dir.path().to_string_lossy().into(),
                agent_id: Some("custom".into()),
                model: "default".into(),
                reasoning_effort: None,
                approval_mode: "yolo".into(),
                sandbox_mode: Some("danger-full-access".into()),
            }),
        )
        .await
        .unwrap();
    let read = || {
        serde_json::from_str::<Value>(
            &std::fs::read_to_string(dir.path().join("cli-context.json")).unwrap(),
        )
        .unwrap()
    };
    assert_eq!(read()["threadId"], "thread-a");
    assert_eq!(read()["hasToken"], true);
    std::fs::write(dir.path().join("record-prompts"), "").unwrap();
    let prompt = |identity: &str, text: &str| StartTurnInput {
        provider_session_id: started.provider_session_id.clone(),
        thread_id: identity.into(),
        turn_id: uuid::Uuid::new_v4().to_string(),
        prompt: text.into(),
        model: None,
        reasoning_effort: None,
        sandbox_mode: None,
        collaboration_mode: None,
        approval_mode: None,
        performance_mode: None,
        hidden: false,
        images: vec![],
        title: None,
        context_delivered: false,
    };
    let bus = EventBus::new();
    let mut events = bus.subscribe();
    for text in ["first", "second"] {
        s.with_cli_context(
            "thread-a",
            runtime.start_turn(
                prompt("thread-a", text),
                bus.clone(),
                tokio_util::sync::CancellationToken::new(),
            ),
        )
        .await
        .unwrap();
    }
    // The service persists this so a restarted process does not resend it.
    let mut delivered = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.event_type == "thread.context.delivered" {
            delivered.push(event.payload["providerSessionId"].clone());
        }
    }
    assert_eq!(delivered, [serde_json::json!(started.provider_session_id)]);
    let prompts = || {
        std::fs::read_to_string(dir.path().join("prompts.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<String>(line).unwrap())
            .collect::<Vec<_>>()
    };
    assert!(prompts()[0].contains("pockymoe thread self"));
    assert!(prompts()[0].ends_with("\n\nfirst"));
    assert_eq!(prompts()[1], "second");
    assert!(
        !s.with_cli_context("thread-b", async {
            runtime.session_loaded(&started.provider_session_id)
        })
        .await
    );
    s.with_cli_context(
        "thread-b",
        runtime.resume_session(
            &started.provider_session_id,
            Some(&dir.path().to_string_lossy()),
            SessionSettings::default(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(read()["threadId"], "thread-b");
    s.with_cli_context(
        "thread-b",
        runtime.start_turn(
            prompt("thread-b", "resumed"),
            EventBus::new(),
            tokio_util::sync::CancellationToken::new(),
        ),
    )
    .await
    .unwrap();
    assert!(prompts()[2].contains("pockymoe thread self"));
    assert!(prompts()[2].ends_with("\n\nresumed"));
    s.with_cli_context(
        "thread-b",
        runtime.start_turn(
            prompt("thread-b", "next"),
            EventBus::new(),
            tokio_util::sync::CancellationToken::new(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(prompts()[3], "next");
    assert!(
        s.with_cli_context("thread-b", async {
            runtime.session_loaded(&started.provider_session_id)
        })
        .await
    );
    // A new process resuming the same session replays its history, which
    // already holds the context: the turn the service marks delivered gets none.
    runtime
        .release_session(&started.provider_session_id)
        .await
        .unwrap();
    s.with_cli_context(
        "thread-b",
        runtime.resume_session(
            &started.provider_session_id,
            Some(&dir.path().to_string_lossy()),
            SessionSettings::default(),
        ),
    )
    .await
    .unwrap();
    s.with_cli_context(
        "thread-b",
        runtime.start_turn(
            StartTurnInput {
                context_delivered: true,
                ..prompt("thread-b", "after restart")
            },
            EventBus::new(),
            tokio_util::sync::CancellationToken::new(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(prompts()[4], "after restart");
}

#[tokio::test]
async fn inbox_is_passive_bounded_durable_and_acknowledged_explicitly() {
    let (_dir, state) = setup();
    let a = thread(&state, Provider::Codex).await;
    let b = thread(&state, Provider::Acp).await;
    state.start_interaction_worker();
    let mut input = send(&a, &"你好".repeat(6000), false, "mail-1");
    input.delivery = "inbox".into();
    let receipt = state.send_to_thread(&b, input.clone()).unwrap();
    assert_eq!(state.send_to_thread(&b, input).unwrap(), receipt);
    assert!(state.pending_relay_notifications().unwrap().is_empty());
    assert!(receipt["pendingSteerId"].is_null());
    let id = receipt["messageId"].as_str().unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let status = state.interaction_status(&b).await.unwrap();
    assert_eq!(status["status"], "idle");
    assert_eq!(status["queuedCount"], 0);
    assert_eq!(status["unreadMessageCount"], 1);
    let list = state.inbox_list(&b, &json!({})).unwrap();
    assert!(list["messages"][0].get("text").is_none());
    assert_eq!(
        list["messages"][0]["preview"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        240
    );
    let read = state.inbox_read(&b, &json!({"messageId":id})).unwrap();
    assert_eq!(read["text"].as_str().unwrap().chars().count(), 8192);
    assert_eq!(read["nextTextOffset"], 8192);
    let tail = state
        .inbox_read(&b, &json!({"messageId":id,"textOffset":8192}))
        .unwrap();
    assert_eq!(tail["text"].as_str().unwrap().chars().count(), 3808);
    assert_eq!(state.inbox_unread_count(&b).unwrap(), 1);
    assert!(state.inbox_read(&a, &json!({"messageId":id})).is_err());
    assert!(state
        .inbox_ack(&b, &json!({"messageIds":[id,"missing"]}))
        .is_err());
    assert_eq!(state.inbox_unread_count(&b).unwrap(), 1);
    let db = rusqlite::Connection::open(&state.config.database_url).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM kv WHERE key GLOB 'cli:inbox:*'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    state.inbox_ack(&b, &json!({"messageIds":[id]})).unwrap();
    state.inbox_ack(&b, &json!({"messageIds":[id]})).unwrap();
    assert_eq!(state.inbox_unread_count(&b).unwrap(), 0);
    assert_eq!(
        state.inbox_list(&b, &json!({})).unwrap()["messages"],
        json!([])
    );
    assert_eq!(
        state.inbox_list(&b, &json!({"all":true})).unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn peer_reports_cannot_wake_idle_or_interrupt_running_threads() {
    let (_dir, state) = setup();
    let a = thread(&state, Provider::Codex).await;
    let b = thread(&state, Provider::Codex).await;
    for running in [false, true] {
        if running {
            state.db.with(|c| {
                c.execute("UPDATE threads SET status='running' WHERE id=?1", [&b])?;
                c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES ('active',?1,'inProgress',0)", [&b])?;
                Ok(())
            }).unwrap();
        }
        for kind in [None, Some("status"), Some("result"), Some("question")] {
            for delivery in ["queue", "direct", "steer"] {
                let mut report = send(&a, "batch ready; please read", false, "rejected-report");
                report.kind = kind.map(str::to_owned);
                report.delivery = delivery.into();
                let error = state.send_to_thread(&b, report).unwrap_err().to_string();
                assert!(error.contains("peer "), "{kind:?}/{delivery}: {error}");
            }
        }
        assert_eq!(
            state.interaction_status(&b).await.unwrap()["queuedCount"],
            0
        );
        assert_eq!(state.inbox_unread_count(&b).unwrap(), 0);

        let mut passive = send(
            &a,
            "batch ready at /artifacts/r1",
            false,
            if running {
                "running-result"
            } else {
                "idle-result"
            },
        );
        passive.kind = Some("result".into());
        passive.delivery = "inbox".into();
        let receipt = state.send_to_thread(&b, passive).unwrap();
        assert_eq!(receipt["delivery"], "inbox");
        assert_eq!(
            state.interaction_status(&b).await.unwrap()["queuedCount"],
            0
        );
        assert_eq!(
            state.get_thread(&b).unwrap().status,
            if running { "running" } else { "idle" }
        );
        state
            .inbox_ack(&b, &json!({"messageIds":[receipt["messageId"]]}))
            .unwrap();
    }
}

#[tokio::test]
async fn urgent_peer_corrections_require_a_reason_and_expose_it_in_the_prompt() {
    let (_dir, state) = setup();
    let a = thread(&state, Provider::Codex).await;
    let b = thread(&state, Provider::Codex).await;
    let mut correction = send(&a, "Stop using the invalid inputs", false, "correction");
    correction.delivery = "direct".into();
    for reason in [None, Some(" ".into()), Some("x".repeat(501))] {
        correction.interrupt_reason = reason;
        assert!(state.send_to_thread(&b, correction.clone()).is_err());
        assert_eq!(
            state.interaction_status(&b).await.unwrap()["queuedCount"],
            0
        );
    }
    correction.interrupt_reason =
        Some("The running calculation is based on an invalid dataset".into());
    let receipt = state.send_to_thread(&b, correction.clone()).unwrap();
    assert_eq!(receipt["delivery"], "queued");
    assert_eq!(
        receipt["interruptReason"],
        correction.interrupt_reason.as_deref().unwrap()
    );
    let prompt: String = state
        .db
        .with(|c| {
            Ok(c.query_row(
                "SELECT submitted_prompt FROM thread_pending_steers WHERE id=?1",
                [receipt["pendingSteerId"].as_str().unwrap()],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert!(prompt.contains(correction.interrupt_reason.as_deref().unwrap()));
    assert_eq!(state.send_to_thread(&b, correction).unwrap(), receipt);
    assert_eq!(
        state.interaction_status(&b).await.unwrap()["queuedCount"],
        1
    );
}

#[tokio::test]
async fn pre_policy_retry_receipts_keep_the_original_fingerprint_and_route() {
    use sha2::{Digest, Sha256};
    let (_dir, state) = setup();
    let a = thread(&state, Provider::Codex).await;
    let b = thread(&state, Provider::Codex).await;
    // Exact field order and omitted new fields from the pre-policy DTO. The saved
    // receipt must win even though a NEW report like this is no longer eligible.
    let legacy = format!(
        r#"{{"text":"legacy report","delivery":"direct","notifyDelivery":"inbox","fromThreadId":"{a}","notifyOnComplete":false,"clientRequestId":"old-report","subject":null,"kind":null,"inReplyTo":null}}"#
    );
    let fingerprint = hex::encode(Sha256::digest(legacy.as_bytes()));
    let saved = json!({"threadId":b,"delivery":"steer","messageId":"accepted-before-upgrade","requestedDelivery":"direct"});
    state
        .db
        .with(|c| {
            c.execute(
                "INSERT INTO kv(key,value) VALUES (?1,?2)",
                params![
                    format!("cli:request:{b}:{a}:old-report"),
                    json!({"fingerprint":fingerprint,"receipt":saved}).to_string(),
                ],
            )?;
            Ok(())
        })
        .unwrap();
    let input: SendInput = serde_json::from_str(&legacy).unwrap();
    assert_eq!(state.send_to_thread(&b, input.clone()).unwrap(), saved);
    assert_eq!(
        state.interaction_status(&b).await.unwrap()["queuedCount"],
        0
    );
    assert_eq!(state.inbox_unread_count(&b).unwrap(), 0);
    let mut conflicting = input;
    conflicting.interrupt_reason = Some("different request".into());
    assert!(state
        .send_to_thread(&b, conflicting)
        .unwrap_err()
        .to_string()
        .contains("conflict: clientRequestId"));
}

#[tokio::test]
async fn completion_goes_to_inbox_without_waking_the_sender() {
    let (_dir, state) = setup();
    let a = thread(&state, Provider::Codex).await;
    let b = thread(&state, Provider::Acp).await;
    let mut input = send(&a, "finish task", true, "finish");
    input.delivery = "queue".into();
    input.notify_delivery = "inbox".into();
    let receipt = state.send_to_thread(&b, input.clone()).unwrap();
    assert_eq!(receipt["delivery"], "queued");
    assert_eq!(receipt["requestedDelivery"], "queue");
    state.start_interaction_worker();
    until(|| state.inbox_unread_count(&a).unwrap() == 1).await;
    assert_eq!(state.send_to_thread(&b, input).unwrap(), receipt);
    let pending = state.pending_relay_notifications().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["threadId"], b);
    assert_eq!(state.pending_relay_notifications().unwrap(), pending);
    state
        .acknowledge_relay_notification(pending[0]["turnId"].as_str().unwrap())
        .unwrap();
    assert!(state.pending_relay_notifications().unwrap().is_empty());
    assert_eq!(
        state.interaction_status(&a).await.unwrap()["queuedCount"],
        0
    );
    assert_eq!(state.get_thread(&a).unwrap().status, "idle");
    let list = state.inbox_list(&a, &json!({})).unwrap();
    let id = &list["messages"][0]["id"];
    assert!(
        state.inbox_read(&a, &json!({"messageId":id})).unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("ended with status completed")
    );
    let mut passive = send(&a, "cannot subscribe to a passive message", true, "invalid");
    passive.delivery = "inbox".into();
    assert!(state.send_to_thread(&b, passive).is_err());
    let mut steer = send(&a, "no active turn", false, "no-steer");
    steer.delivery = "steer".into();
    assert!(state.send_to_thread(&b, steer).is_err());
    assert_eq!(
        state.interaction_status(&b).await.unwrap()["queuedCount"],
        0
    );
}

#[tokio::test]
async fn queued_completion_callbacks_are_rejected_before_acceptance() {
    let (_dir, state) = setup();
    let parent = thread(&state, Provider::Codex).await;
    let child = thread(&state, Provider::Acp).await;
    let mut input = send(&parent, "finish task", true, "queued-callback");
    input.notify_delivery = "queue".into();
    let error = state.send_to_thread(&child, input).unwrap_err();
    assert!(error
        .to_string()
        .contains("completion notifications are passive"));
    assert_eq!(
        state.interaction_status(&child).await.unwrap()["queuedCount"],
        0
    );
    assert_eq!(state.inbox_unread_count(&parent).unwrap(), 0);
}

#[tokio::test]
async fn legacy_completion_subscriptions_never_wake_idle_or_running_parents() {
    for running in [false, true] {
        let (_dir, state) = setup();
        let parent = thread(&state, Provider::Codex).await;
        let child = thread(&state, Provider::Acp).await;
        let parent_status = if running { "running" } else { "idle" };
        state.db.with(|conn| {
            conn.execute("UPDATE threads SET status=?1 WHERE id=?2", params![parent_status, parent])?;
            if running {
                conn.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES ('parent-active',?1,'inProgress',0)", [&parent])?;
            }
            for (index, status) in ["completed", "failed", "interrupted"].iter().enumerate() {
                let turn = uuid::Uuid::new_v4().to_string();
                conn.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES (?1,?2,?3,?4)", params![turn,child,status,index as i64])?;
                let target = if index == 0 { parent.clone() } else { json!({"threadId":parent,"delivery":"queue"}).to_string() };
                conn.execute("INSERT INTO kv(key,value) VALUES (?1,?2)", params![format!("cli:notify:turn:{turn}"), target])?;
            }
            Ok(())
        }).unwrap();
        state.start_interaction_worker();
        until(|| state.inbox_unread_count(&parent).unwrap() == 3).await;
        assert_eq!(
            state.interaction_status(&parent).await.unwrap()["queuedCount"],
            0
        );
        let after = state.get_thread(&parent).unwrap();
        assert_eq!(after.status, parent_status);
        assert_eq!(
            after.active_turn_id.as_deref(),
            if running { Some("parent-active") } else { None }
        );
        state
            .db
            .with(|conn| {
                let turns: i64 = conn.query_row(
                    "SELECT count(*) FROM thread_turns WHERE thread_id=?1",
                    [&parent],
                    |r| r.get(0),
                )?;
                assert_eq!(turns, i64::from(running));
                let watchers: i64 = conn.query_row(
                    "SELECT count(*) FROM kv WHERE key GLOB 'cli:notify:turn:*'",
                    [],
                    |r| r.get(0),
                )?;
                assert_eq!(watchers, 0);
                Ok(())
            })
            .unwrap();
    }
}

#[tokio::test]
async fn explicit_steer_uses_active_turn_and_held_input_never_auto_runs() {
    let (_dir, state) = setup();
    let a = thread(&state, Provider::Codex).await;
    let b = thread(&state, Provider::Codex).await;
    state.start_interaction_worker();
    state
        .send_to_thread(&b, send(&a, "perform the original task", false, "task"))
        .unwrap();
    until(|| state.get_thread(&b).unwrap().status == "running").await;
    let turn = state.get_thread(&b).unwrap().active_turn_id.unwrap();
    let mut urgent = send(&a, "urgent correction", true, "urgent");
    urgent.delivery = "direct".into();
    urgent.interrupt_reason = Some("The active task is using invalid inputs".into());
    urgent.notify_delivery = "inbox".into();
    let receipt = state.send_to_thread(&b, urgent.clone()).unwrap();
    assert_eq!(receipt["delivery"], "steer");
    state
        .steer_pending_prompt(&b, receipt["pendingSteerId"].as_str().unwrap())
        .await
        .unwrap();
    until(|| state.inbox_unread_count(&a).unwrap() == 1).await;
    assert_eq!(
        state.send_to_thread(&b, urgent).unwrap(),
        receipt,
        "retry remains valid after the active turn ends"
    );
    let transcript = state
        .transcript(
            &b,
            &TranscriptQuery {
                turn_id: Some(turn),
                view: Some("overview".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(transcript.to_string().contains("urgent correction"));
    state
        .send_to_thread(&b, send(&a, "second task", false, "task2"))
        .unwrap();
    until(|| state.get_thread(&b).unwrap().status == "running").await;
    let mut held = send(&a, "late steering request", false, "held");
    held.delivery = "direct".into();
    held.interrupt_reason = Some("The active task must stop using the invalid inputs".into());
    let receipt = state.send_to_thread(&b, held).unwrap();
    until(|| state.get_thread(&b).unwrap().status != "running").await;
    state
        .send_to_thread(&b, send(&a, "replacement task", false, "task3"))
        .unwrap();
    until(|| state.get_thread(&b).unwrap().status == "running").await;
    assert!(state
        .steer_pending_prompt(&b, receipt["pendingSteerId"].as_str().unwrap())
        .await
        .is_err());
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        state.interaction_status(&b).await.unwrap()["queuedCount"],
        1
    );
}

#[tokio::test]
async fn direct_rejects_unavailable_state_and_unsupported_steering_without_queueing() {
    let (_dir, state) = setup();
    let a = thread(&state, Provider::Codex).await;
    let b = thread(&state, Provider::Acp).await;
    // Use the real ACP capability fallback before a steering extension has
    // been negotiated; the fake runtime advertises steering for every provider.
    let state = Arc::try_unwrap(state).ok().unwrap();
    let state = Supervisor::new(
        state.config,
        state.db,
        vec![Arc::new(pockymoe_runtime::acp::AcpRuntime::catalog(
            None, 1000,
        ))],
    );
    let mut input = send(&a, "act now", false, "direct-rejected");
    input.delivery = "direct".into();
    input.interrupt_reason = Some("Stop the invalid calculation before it wastes more work".into());
    for status in ["recovering", "interrupted", "failed", "running"] {
        state
            .db
            .with(|c| {
                c.execute(
                    "UPDATE threads SET status=?1 WHERE id=?2",
                    params![status, b],
                )?;
                if status == "running" {
                    c.execute("INSERT INTO thread_turns(id,thread_id,status,started_at,ordinal) VALUES ('active',?1,'inProgress','2026-09-11',1)", [&b])?;
                }
                Ok(())
            })
            .unwrap();
        let error = state.send_to_thread(&b, input.clone()).unwrap_err();
        assert!(error.to_string().contains(if status == "running" {
            "does not support steering"
        } else {
            "direct requires an idle or running thread"
        }));
        assert_eq!(
            state.interaction_status(&b).await.unwrap()["queuedCount"],
            0
        );
    }
}

fn reminder_mail(state: &Supervisor, from: &str, to: &str, key: &str) -> String {
    let mut input = send(from, key, false, key);
    input.delivery = "inbox".into();
    input.kind = Some("question".into());
    input.subject = Some(key.into());
    state.send_to_thread(to, input).unwrap()["messageId"]
        .as_str()
        .unwrap()
        .into()
}
fn reminder_time(state: &Supervisor, thread: &str, minutes: i64) -> String {
    let list = state.inbox_list(thread, &json!({"all":true})).unwrap();
    let created = list["messages"][0]["createdAt"].as_str().unwrap();
    (chrono::DateTime::parse_from_rfc3339(created).unwrap()
        + chrono::Duration::minutes(minutes)
        + chrono::Duration::seconds(1))
    .to_rfc3339()
}
fn reminder_queue_count(state: &Supervisor, thread: &str) -> i64 {
    state
        .db
        .with(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM thread_pending_steers WHERE thread_id=?1",
                [thread],
                |r| r.get(0),
            )?)
        })
        .unwrap()
}

#[tokio::test]
async fn inbox_reminder_batches_after_ten_minutes_and_survives_restart_once() {
    let (_dir, state) = setup();
    let a = thread(&state, Provider::Codex).await;
    let b = thread(&state, Provider::Acp).await;
    reminder_mail(&state, &a, &b, "first-question");
    reminder_mail(&state, &a, &b, "second-question");
    state
        .inbox_reminder_tick(&reminder_time(&state, &b, 9))
        .await
        .unwrap();
    assert_eq!(reminder_queue_count(&state, &b), 0);
    state
        .inbox_reminder_tick(&reminder_time(&state, &b, 10))
        .await
        .unwrap();
    assert_eq!(reminder_queue_count(&state, &b), 1);
    let config = state.config.clone();
    drop(state);
    let state = Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![
            Arc::new(FakeRuntime::new(Provider::Codex)),
            Arc::new(FakeRuntime::new(Provider::Acp)),
        ],
    );
    state
        .inbox_reminder_tick(&reminder_time(&state, &b, 20))
        .await
        .unwrap();
    assert_eq!(reminder_queue_count(&state, &b), 1);
    // Simulate consumption of the queued reminder; its persisted batch marker
    // must also prevent a later reminder once the queue is empty.
    state
        .db
        .with(|c| {
            c.execute("DELETE FROM thread_pending_steers WHERE thread_id=?1", [&b])?;
            Ok(())
        })
        .unwrap();
    state
        .inbox_reminder_tick(&reminder_time(&state, &b, 40))
        .await
        .unwrap();
    assert_eq!(reminder_queue_count(&state, &b), 0);
    assert_eq!(
        state.inbox_unread_count(&b).unwrap(),
        2,
        "wake never acknowledges mail"
    );
}

#[tokio::test]
async fn inbox_reminder_read_ack_and_wait_cancel_without_losing_history() {
    let (_dir, state) = setup();
    let a = thread(&state, Provider::Codex).await;
    let b = thread(&state, Provider::Acp).await;
    let read = reminder_mail(&state, &a, &b, "read-question");
    let ack = reminder_mail(&state, &a, &b, "ack-question");
    assert!(state.inbox_read(&b, &json!({"messageId":read})).unwrap()["readAt"].is_string());
    state.inbox_ack(&b, &json!({"messageIds":[ack]})).unwrap();
    let waited = reminder_mail(&state, &a, &b, "wait-question");
    let result = state
        .inbox_wait(&b, &[], &[], false, Duration::from_millis(10))
        .await
        .unwrap();
    assert!(result["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == waited && m["readAt"].is_string()));
    state
        .inbox_reminder_tick(&reminder_time(&state, &b, 20))
        .await
        .unwrap();
    assert_eq!(reminder_queue_count(&state, &b), 0);
    assert_eq!(
        state.inbox_unread_count(&b).unwrap(),
        2,
        "reading is not handling"
    );
    assert_eq!(
        state.inbox_read(&b, &json!({"messageId":read})).unwrap()["text"],
        "read-question"
    );
}

#[tokio::test]
async fn inbox_reminder_defers_active_and_queued_work_and_throttles_new_batches() {
    let (_dir, state) = setup();
    let a = thread(&state, Provider::Codex).await;
    let b = thread(&state, Provider::Codex).await;
    reminder_mail(&state, &a, &b, "deferred-question");
    let now = reminder_time(&state, &b, 20);
    state
        .db
        .with(|c| {
            c.execute(
                "INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('active',?1,'inProgress',0)",
                [&b],
            )?;
            Ok(())
        })
        .unwrap();
    state.inbox_reminder_tick(&now).await.unwrap();
    assert_eq!(reminder_queue_count(&state, &b), 0);
    state
        .db
        .with(|c| {
            c.execute("UPDATE threads SET status='recovering' WHERE id=?1", [&b])?;
            c.execute("DELETE FROM thread_turns WHERE id='active'", [])?;
            Ok(())
        })
        .unwrap();
    state.inbox_reminder_tick(&now).await.unwrap();
    assert_eq!(reminder_queue_count(&state, &b), 0);
    state.db.with(|c| {
        c.execute("UPDATE threads SET status='idle' WHERE id=?1", [&b])?;
        c.execute("INSERT INTO thread_pending_steers(id,thread_id,turn_id,display_prompt,submitted_prompt,delivery,created_at,updated_at) VALUES('held',?1,'','held','held','held',?2,?2)", params![b,now])?;
        Ok(())
    }).unwrap();
    state.inbox_reminder_tick(&now).await.unwrap();
    assert_eq!(reminder_queue_count(&state, &b), 1);
    state
        .db
        .with(|c| {
            c.execute("DELETE FROM thread_pending_steers WHERE thread_id=?1", [&b])?;
            Ok(())
        })
        .unwrap();
    state.inbox_reminder_tick(&now).await.unwrap();
    assert_eq!(reminder_queue_count(&state, &b), 1);
    state
        .db
        .with(|c| {
            c.execute("DELETE FROM thread_pending_steers WHERE thread_id=?1", [&b])?;
            Ok(())
        })
        .unwrap();
    reminder_mail(&state, &a, &b, "new-batch");
    state
        .inbox_reminder_tick(&reminder_time(&state, &b, 25))
        .await
        .unwrap();
    assert_eq!(
        reminder_queue_count(&state, &b),
        0,
        "ten-minute cooldown between batches"
    );
    state
        .inbox_reminder_tick(&reminder_time(&state, &b, 31))
        .await
        .unwrap();
    assert_eq!(reminder_queue_count(&state, &b), 1);
}

#[tokio::test]
async fn inbox_reminder_does_not_reopen_closed_threads_or_replay_legacy_mail() {
    let (_dir, state) = setup();
    let a = thread(&state, Provider::Codex).await;
    let b = thread(&state, Provider::Codex).await;
    let id = reminder_mail(&state, &a, &b, "closed-question");
    let now = reminder_time(&state, &b, 20);
    state
        .db
        .with(|c| {
            c.execute(
                "UPDATE threads SET closed_at=?2 WHERE id=?1",
                params![b, now],
            )?;
            Ok(())
        })
        .unwrap();
    state.inbox_reminder_tick(&now).await.unwrap();
    assert_eq!(reminder_queue_count(&state, &b), 0);
    state
        .db
        .with(|c| {
            c.execute("UPDATE threads SET closed_at=NULL WHERE id=?1", [&b])?;
            c.execute(
                "UPDATE kv SET value=json_remove(value,'$.remindAfter') WHERE key=?1",
                [format!("cli:inbox:{b}:{id}")],
            )?;
            Ok(())
        })
        .unwrap();
    state.inbox_reminder_tick(&now).await.unwrap();
    assert_eq!(reminder_queue_count(&state, &b), 0);
}

#[tokio::test]
async fn inbox_reminder_worker_starts_a_recorded_turn_for_due_mail() {
    let (_dir, state) = setup();
    let a = thread(&state, Provider::Codex).await;
    let b = thread(&state, Provider::Acp).await;
    let id = reminder_mail(&state, &a, &b, "worker-question");
    state.db.with(|c| {
        c.execute("UPDATE kv SET value=json_set(value,'$.remindAfter','2020-01-01T00:00:00Z') WHERE key=?1", [format!("cli:inbox:{b}:{id}")])?;
        Ok(())
    }).unwrap();
    state.start_interaction_worker();
    // FakeRuntime deliberately waits 25 seconds for long prompts.
    tokio::time::timeout(Duration::from_secs(35), async {
        loop {
            let completed = state
                .db
                .with(|c| {
                    Ok(c.query_row(
                "SELECT count(*) FROM thread_turns WHERE thread_id=?1 AND status='completed'",
                [&b], |r| r.get::<_, i64>(0),
            )?)
                })
                .unwrap();
            if completed == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    let history = state.get_thread_detail(&b, None).await.unwrap();
    assert!(history
        .turns
        .iter()
        .flat_map(|t| &t.items)
        .any(|i| i.kind == "userMessage" && i.text.contains("Pockymoe inbox reminder")));
    assert!(history
        .turns
        .iter()
        .flat_map(|t| &t.items)
        .any(|i| i.kind == "agentMessage"));
    assert_eq!(state.inbox_unread_count(&b).unwrap(), 1);
}

#[tokio::test]
async fn treer_identity_rebind_removal_and_children_use_separate_harness_environments() {
    use pockymoe_runtime::{
        acp::AcpRuntime,
        actor::{AgentRuntime, SessionSettings, StartSessionInput},
    };
    let (dir, s) = setup();
    s.configure_cli("http://127.0.0.1:8787".into());
    let root = thread(&s, Provider::Codex).await;
    let child = s
        .create_thread(CreateThreadInput {
            workspace_id: s.get_thread(&root).unwrap().workspace_id,
            title: None,
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: Some(root.clone()),
        })
        .await
        .unwrap()
        .id;
    let identity = vec![
        ("TREER_AGENT_ID".into(), "fixture-agent".into()),
        (
            "TREER_WORKLOAD_CREDENTIAL".into(),
            "fixture-credential".into(),
        ),
    ];
    s.set_thread_cli_identity(&root, identity.clone()).unwrap();
    assert!(s.set_thread_cli_identity(&child, identity.clone()).is_err());
    let script = dir.path().join("identity-agent.py");
    std::fs::write(&script,format!("import os,json\nwith open('identity-context.json','w') as f: json.dump({{'threadId':os.getenv('POCKYMOE_THREAD_ID'),'hasTreerCredential':bool(os.getenv('TREER_WORKLOAD_CREDENTIAL')),'agentId':os.getenv('TREER_AGENT_ID')}},f)\n{}",include_str!("fixtures/fake_acp_agent.py"))).unwrap();
    let python = which::which("python3")
        .or_else(|_| which::which("python"))
        .unwrap();
    let runtime = AcpRuntime::catalog(
        Some(format!("\"{}\" \"{}\"", python.display(), script.display())),
        5000,
    );
    let session = s
        .with_cli_context(
            &root,
            runtime.start_session(StartSessionInput {
                cwd: dir.path().to_string_lossy().into(),
                agent_id: Some("custom".into()),
                model: "default".into(),
                reasoning_effort: None,
                approval_mode: "yolo".into(),
                sandbox_mode: None,
            }),
        )
        .await
        .unwrap()
        .provider_session_id;
    let read = || {
        serde_json::from_str::<Value>(
            &std::fs::read_to_string(dir.path().join("identity-context.json")).unwrap(),
        )
        .unwrap()
    };
    assert_eq!(read()["hasTreerCredential"], true);
    assert_eq!(read()["agentId"], "fixture-agent");
    s.clear_thread_cli_identity(&root);
    s.with_cli_context(
        &root,
        runtime.resume_session(
            &session,
            Some(&dir.path().to_string_lossy()),
            SessionSettings::default(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(read()["threadId"], root);
    assert_eq!(read()["hasTreerCredential"], false);
    s.set_thread_cli_identity(&root, identity).unwrap();
    s.with_cli_context(
        &root,
        runtime.resume_session(
            &session,
            Some(&dir.path().to_string_lossy()),
            SessionSettings::default(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(read()["hasTreerCredential"], true);
    s.with_cli_context(
        &child,
        runtime.resume_session(
            &session,
            Some(&dir.path().to_string_lossy()),
            SessionSettings::default(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(read()["threadId"], child);
    assert_eq!(read()["hasTreerCredential"], false);
    assert_eq!(read()["agentId"], Value::Null);
    runtime.restart("custom").await.unwrap();
}

#[tokio::test]
async fn transcript_summary_activity_counts_only_deferred_operations() {
    let (_dir, supervisor) = setup();
    let thread_id = thread(&supervisor, Provider::Codex).await;
    let operations = vec![
        json!({"id":"command-1","kind":"commandExecution","text":"cargo check","status":"completed"}),
        json!({"id":"command-2","kind":"commandExecution","text":"cargo test","status":"FAILED"}),
        json!({"id":"edit-1","kind":"fileChange","text":"src/a.rs, src/b.rs","changedFiles":2,"addedLines":64,"removedLines":3}),
        json!({"id":"edit-2","kind":"fileChange","text":"src/c.rs","changedFiles":1,"addedLines":2}),
        json!({"id":"read","kind":"fileRead","text":"src/a.rs"}),
        json!({"id":"search","kind":"webSearch","text":"Rust docs"}),
        json!({"id":"tool","kind":"toolCall","text":"lookup","status":"error"}),
        json!({"id":"skill","kind":"skillToolCall","text":"review","status":"declined"}),
        json!({"id":"agent","kind":"agentToolCall","text":"reviewer"}),
        json!({"id":"reason","kind":"reasoning","text":"Consider the result"}),
    ];
    supervisor.db.with(|conn| {
        for (ordinal, turn_id) in [(1, "with-operations"), (2, "conversation-only"), (3, "zero-keys")] {
            conn.execute(
                "INSERT INTO thread_turns(id,thread_id,status,ordinal,started_at,completed_at)
                 VALUES(?1,?2,'completed',?3,'2026-10-10T10:00:00Z','2026-10-10T10:00:51Z')",
                params![turn_id, thread_id, ordinal],
            )?;
            let mut items = vec![json!({"id":"prompt","kind":"userMessage","text":"Inspect the project"})];
            if turn_id == "with-operations" {
                items.extend(operations.iter().cloned());
            } else if turn_id == "zero-keys" {
                items.push(operations[0].clone());
            }
            // Failures on prose are not operation failures.
            items.push(json!({"id":"answer","kind":"agentMessage","text":"Done","status":"failed","responsePhase":"final"}));
            for (index, item) in items.iter().enumerate() {
                conn.execute(
                    "INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at)
                     VALUES(?1,?2,?3,?4,?5,?6,?6)",
                    params![format!("{turn_id}-{index}"), thread_id, turn_id, item["id"].as_str().unwrap(),
                        item.to_string(), format!("2026-10-10T10:00:{index:02}Z")],
                )?;
            }
        }
        Ok(())
    }).unwrap();

    // This is the same summary path selected by HTTP ?view=summary.
    let summary = supervisor
        .get_thread_detail_page(&thread_id, Some(3), None, true)
        .await
        .unwrap();
    let folded = summary
        .turns
        .iter()
        .find(|turn| turn.id == "with-operations")
        .unwrap();
    assert_eq!(folded.has_deferred_items, Some(true));
    assert_eq!(folded.deferred_item_count, Some(operations.len() as u32));
    assert_eq!(folded.items.len(), 2);
    let activity = folded.activity.as_ref().unwrap().as_object().unwrap();
    let expected_counts = json!({
        "commands":2,"fileChanges":2,"changedFiles":3,"addedLines":66,"removedLines":3,
        "reads":1,"searches":1,"tools":2,"agents":1,"reasoning":1,"failed":3,
    });
    for (key, expected) in expected_counts.as_object().unwrap() {
        assert_eq!(activity.get(key), Some(expected), "wrong count for {key}");
    }
    // Every path of a multi-file patch is listed; its line counts stay with the first.
    assert_eq!(
        activity.get("files"),
        Some(&json!([
            {"path":"src/a.rs","itemId":"edit-1","addedLines":64,"removedLines":3},
            {"path":"src/b.rs","itemId":"edit-1","addedLines":0,"removedLines":0},
            {"path":"src/c.rs","itemId":"edit-2","addedLines":2,"removedLines":0},
        ]))
    );
    let conversation = summary
        .turns
        .iter()
        .find(|turn| turn.id == "conversation-only")
        .unwrap();
    assert_ne!(conversation.has_deferred_items, Some(true));
    assert!(conversation.activity.is_none());
    assert!(serde_json::to_value(conversation)
        .unwrap()
        .get("activity")
        .is_none());
    let zero_keys = summary
        .turns
        .iter()
        .find(|turn| turn.id == "zero-keys")
        .unwrap();
    assert_eq!(
        zero_keys.activity,
        Some(json!({"commands":1,"segments":[{
            "before":"answer","commands":1,
            "startedAt":"2026-10-10T10:00:01Z","endedAt":"2026-10-10T10:00:01Z"
        }]}))
    );

    let full = supervisor
        .get_thread_turn_detail(&thread_id, "with-operations")
        .await
        .unwrap();
    assert_eq!(full.items.len(), operations.len() + 2);
    assert!(full.activity.is_none());
}
