//! Coordination primitives on top of the passive inbox: names, close, wait,
//! inbox wait, the task board, wakes, roles and worktrees.
use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Provider};
use pockymoe_runtime::{
    fake::FakeRuntime,
    interaction::{load_role, AgentOptions, SendInput},
    Database, RuntimeConfig, Supervisor,
};
use serde_json::json;
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
        relay_server_url: None,
        web_origin: None,
        relay_agent_token: None,
        enabled_providers: vec![Provider::Codex],
        acp_command: None,
        acp_startup_timeout_ms: 1000,
        fake_runtime: true,
        supervisor_restart_strategy: Default::default(),
    };
    let db = Database::open(&config.database_url).unwrap();
    let state = Arc::new(Supervisor::new(
        config,
        db,
        vec![Arc::new(FakeRuntime::new(Provider::Codex))],
    ));
    state.spawn_live_item_persister();
    (dir, state)
}

fn workspace(s: &Supervisor) -> String {
    s.list_workspaces()
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
        })
        .id
}

fn input(ws: &str, parent: Option<&str>) -> CreateThreadInput {
    CreateThreadInput {
        workspace_id: ws.into(),
        title: None,
        provider: Some(Provider::Codex),
        agent_id: Some("codex".into()),
        model: "ios-e2e-stream".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: parent.map(str::to_owned),
    }
}

async fn named(s: &Supervisor, ws: &str, parent: &str, name: &str) -> String {
    s.create_thread_with(
        input(ws, Some(parent)),
        AgentOptions {
            name: Some(name.into()),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .id
}

fn mail(from: &str, delivery: &str, kind: &str, text: &str) -> SendInput {
    SendInput {
        delivery: delivery.into(),
        notify_delivery: "inbox".into(),
        text: text.into(),
        from_thread_id: Some(from.into()),
        notify_on_complete: false,
        client_request_id: None,
        subject: None,
        kind: Some(kind.into()),
        in_reply_to: None,
        interrupt_reason: None,
        topic_key: None,
    }
}

async fn until(mut check: impl FnMut() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("condition was not reached");
}

#[tokio::test]
async fn names_resolve_within_a_lineage_and_closing_frees_name_and_slot() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let root = s.create_thread(input(&ws, None)).await.unwrap().id;
    let reviewer = named(&s, &ws, &root, "reviewer").await;
    let worker = named(&s, &ws, &root, "worker").await;

    assert_eq!(
        s.resolve_agent(Some(&worker), "reviewer").unwrap(),
        reviewer
    );
    assert_eq!(s.resolve_agent(Some(&worker), "parent").unwrap(), root);
    assert_eq!(s.resolve_agent(Some(&worker), "root").unwrap(), root);
    let err = s
        .resolve_agent(Some(&root), "nobody")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("reviewer") && err.contains("worker"),
        "lists names: {err}"
    );

    let dup = s
        .create_thread_with(
            input(&ws, Some(&root)),
            AgentOptions {
                name: Some("reviewer".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(dup.contains("already named"), "got: {dup}");
    for bad in ["Reviewer", "parent", "1st", ""] {
        assert!(s
            .create_thread_with(
                input(&ws, Some(&root)),
                AgentOptions {
                    name: Some(bad.into()),
                    ..Default::default()
                },
            )
            .await
            .is_err());
    }

    // Only an ancestor may close, and the root itself cannot be closed.
    assert!(s
        .close_agent_thread(Some(&worker), &reviewer, false)
        .is_err());
    assert!(s.close_agent_thread(None, &root, false).is_err());
    let closed = s.close_agent_thread(Some(&root), &reviewer, false).unwrap();
    assert!(closed["closedAt"].is_string());
    assert!(s.resolve_agent(Some(&root), "reviewer").is_err());
    let again = named(&s, &ws, &root, "reviewer").await;
    assert_ne!(again, reviewer, "a closed name is reusable");
}

#[tokio::test]
async fn idle_delegates_hold_their_slot_until_closed() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let root = s.create_thread(input(&ws, None)).await.unwrap().id;
    let mut made = Vec::new();
    for _ in 0..pockymoe_runtime::service::MAX_OPEN_AGENT_THREADS {
        made.push(s.create_thread(input(&ws, Some(&root))).await.unwrap().id);
    }
    let refused = s
        .create_thread(input(&ws, Some(&root)))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("thread close"),
        "points at the fix: {refused}"
    );
    s.close_agent_thread(Some(&root), &made[0], false).unwrap();
    s.create_thread(input(&ws, Some(&root))).await.unwrap();

    // Prompting a closed delegate reopens it, which must respect the cap.
    let reopen = s
        .send_to_thread(&made[0], mail(&root, "queue", "task", "more work"))
        .unwrap_err()
        .to_string();
    assert!(reopen.contains("limit"), "got: {reopen}");
}

#[tokio::test]
async fn wait_returns_when_delegates_settle_with_their_closing_message() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let root = s.create_thread(input(&ws, None)).await.unwrap().id;
    let a = named(&s, &ws, &root, "a").await;
    let b = named(&s, &ws, &root, "b").await;

    // Fresh, never prompted: already settled.
    let idle = s
        .wait_threads(&[a.clone()], false, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(idle["done"], true);

    s.send_to_thread(&a, mail(&root, "queue", "task", "do a"))
        .unwrap();
    s.send_to_thread(&b, mail(&root, "queue", "task", "do b"))
        .unwrap();
    // Queued but undispatched work is not settled.
    let pending = s
        .wait_threads(&[a.clone(), b.clone()], false, Duration::from_millis(300))
        .await
        .unwrap();
    assert_eq!(pending["timedOut"], true);
    assert_eq!(pending["threads"][0]["state"], "queued");

    s.start_interaction_worker();
    let done = s
        .wait_threads(&[a.clone(), b.clone()], false, Duration::from_secs(20))
        .await
        .unwrap();
    assert_eq!(done["done"], true, "{done}");
    for t in done["threads"].as_array().unwrap() {
        assert_eq!(t["state"], "idle");
        assert_eq!(t["lastTurn"]["status"], "completed");
    }
}

#[tokio::test]
async fn inbox_wait_blocks_until_matching_mail_arrives() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let root = s.create_thread(input(&ws, None)).await.unwrap().id;
    let a = named(&s, &ws, &root, "a").await;
    let b = named(&s, &ws, &root, "b").await;

    let empty = s
        .inbox_wait(&root, &[], &[], false, Duration::from_millis(200))
        .await
        .unwrap();
    assert_eq!(empty["timedOut"], true);

    let sender = s.clone();
    let (root2, a2, b2) = (root.clone(), a.clone(), b.clone());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        sender
            .send_to_thread(&root2, mail(&b2, "inbox", "status", "b progress"))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        sender
            .send_to_thread(&root2, mail(&a2, "inbox", "result", "a result"))
            .unwrap();
    });
    // Filtered on sender: b's status does not end a wait on a.
    let got = s
        .inbox_wait(&root, &[a.clone()], &[], false, Duration::from_secs(10))
        .await
        .unwrap();
    let messages = got["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "{got}");
    assert_eq!(messages[0]["text"], "a result");

    // Unacknowledged mail satisfies the next wait at once, unless only new mail counts.
    let again = s
        .inbox_wait(&root, &[], &[], false, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(again["messages"].as_array().unwrap().len(), 2);
    let fresh = s
        .inbox_wait(&root, &[], &[], true, Duration::from_millis(200))
        .await
        .unwrap();
    assert_eq!(fresh["timedOut"], true, "{fresh}");
    let kinds = s
        .inbox_wait(
            &root,
            &[],
            &["question".into()],
            false,
            Duration::from_millis(200),
        )
        .await
        .unwrap();
    assert_eq!(kinds["timedOut"], true);
}

#[tokio::test]
async fn inbox_filters_and_pagination_reach_results_beyond_a_noisy_backlog() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let root = s.create_thread(input(&ws, None)).await.unwrap().id;
    let a = named(&s, &ws, &root, "a").await;
    let b = named(&s, &ws, &root, "b").await;
    for n in 0..220 {
        let (sender, kind) = if n % 2 == 0 {
            (&a, "status")
        } else {
            (&b, "result")
        };
        s.send_to_thread(
            &root,
            mail(sender, "inbox", kind, "unrelated progress/result"),
        )
        .unwrap();
    }
    // Make the desired results strictly later than the first 200 unread records,
    // independent of clock resolution and UUID tie-breaking.
    s.db.with(|c| {
        c.execute("UPDATE kv SET value=json_set(value,'$.createdAt','2026-01-01T00:00:00Z') WHERE key GLOB 'cli:inbox:*'", [])?;
        Ok(())
    }).unwrap();
    let mut ids = Vec::new();
    for n in 1..=3 {
        let receipt = s
            .send_to_thread(
                &root,
                mail(&a, "inbox", "result", &format!("usable batch {n}")),
            )
            .unwrap();
        ids.push(receipt["messageId"].clone());
    }
    let got = s
        .inbox_wait(
            &root,
            &[a.clone()],
            &["result".into()],
            false,
            Duration::from_millis(50),
        )
        .await
        .unwrap();
    assert_eq!(got["timedOut"], false, "{got}");
    assert_eq!(got["messages"].as_array().unwrap().len(), 3);
    assert!(got["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["text"].as_str().unwrap().starts_with("usable batch")));

    let first = s
        .inbox_list(
            &root,
            &json!({"limit":2,"fromThreadIds":[a],"kinds":["result"]}),
        )
        .unwrap();
    assert_eq!(first["messages"].as_array().unwrap().len(), 2);
    assert!(first["nextBefore"].is_string(), "{first}");
    let older = s
        .inbox_list(
            &root,
            &json!({"limit":2,"fromThreadIds":[a],"kinds":["result"],"before":first["nextBefore"]}),
        )
        .unwrap();
    assert_eq!(older["messages"].as_array().unwrap().len(), 1);
    let listed = first["messages"]
        .as_array()
        .unwrap()
        .iter()
        .chain(older["messages"].as_array().unwrap())
        .map(|m| m["id"].clone())
        .collect::<Vec<_>>();
    assert!(ids.iter().all(|id| listed.contains(id)));
    assert!(older["nextBefore"].is_null());
    assert!(s.inbox_list(&root, &json!({"kinds":["unknown"]})).is_err());
    assert!(s
        .inbox_list(&root, &json!({"fromThreadIds":"not-an-array"}))
        .is_err());
}

#[tokio::test]
async fn inbox_wait_new_ignores_the_entire_backlog_without_hiding_new_mail() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let root = s.create_thread(input(&ws, None)).await.unwrap().id;
    let a = named(&s, &ws, &root, "a").await;
    for _ in 0..220 {
        s.send_to_thread(&root, mail(&a, "inbox", "result", "old batch"))
            .unwrap();
    }
    s.db.with(|c| {
        c.execute("UPDATE kv SET value=json_set(value,'$.createdAt','2026-01-01T00:00:00Z') WHERE key GLOB 'cli:inbox:*'", [])?;
        Ok(())
    }).unwrap();
    let empty = s
        .inbox_wait(&root, &[], &[], true, Duration::from_millis(30))
        .await
        .unwrap();
    assert_eq!(empty["timedOut"], true);

    let sender = s.clone();
    let (root2, a2) = (root.clone(), a.clone());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        sender
            .send_to_thread(
                &root2,
                mail(&a2, "inbox", "question", "need a decision to finish"),
            )
            .unwrap();
    });
    let got = s
        .inbox_wait(
            &root,
            &[a],
            &["result".into(), "question".into()],
            true,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
    assert_eq!(got["timedOut"], false, "{got}");
    assert_eq!(got["messages"].as_array().unwrap().len(), 1);
    assert_eq!(got["messages"][0]["kind"], "question");
    assert_eq!(
        s.inbox_unread_count(&root).unwrap(),
        221,
        "wait/read never ack old mail"
    );
}

#[tokio::test]
async fn status_snapshots_coalesce_only_the_same_sender_topic_and_recipient() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let root = s.create_thread(input(&ws, None)).await.unwrap().id;
    let a = named(&s, &ws, &root, "a").await;
    let b = named(&s, &ws, &root, "b").await;
    let snapshot = |sender: &str, topic: &str, text: &str| {
        let mut input = mail(sender, "inbox", "status", text);
        input.topic_key = Some(topic.into());
        input.client_request_id = Some(text.into());
        input
    };
    let first = snapshot(&a, "simulation", "r1: 10 of 100");
    let old = s.send_to_thread(&root, first.clone()).unwrap();
    s.send_to_thread(&root, snapshot(&b, "simulation", "b: 30 of 100"))
        .unwrap();
    s.send_to_thread(&root, snapshot(&a, "other-stage", "other: 5 of 10"))
        .unwrap();
    s.send_to_thread(&b, snapshot(&a, "simulation", "r1: 10 of 100"))
        .unwrap();
    s.send_to_thread(
        &root,
        mail(&a, "inbox", "question", "simulation: which inputs?"),
    )
    .unwrap();
    s.send_to_thread(
        &root,
        mail(&a, "inbox", "result", "simulation batch 1 ready"),
    )
    .unwrap();
    let latest = s
        .send_to_thread(&root, snapshot(&a, "simulation", "r2: 20 of 100"))
        .unwrap();
    assert_eq!(latest["supersededMessageCount"], 1);
    assert_eq!(s.inbox_unread_count(&root).unwrap(), 5);
    assert_eq!(s.inbox_unread_count(&b).unwrap(), 1);
    let history = s
        .inbox_read(&root, &json!({"messageId":old["messageId"]}))
        .unwrap();
    assert_eq!(history["supersededBy"], latest["messageId"]);
    assert!(
        history["acknowledgedAt"].is_null(),
        "replacement does not imply handling"
    );
    assert_eq!(history["text"], "r1: 10 of 100");
    assert_eq!(
        s.inbox_list(&root, &json!({"all":true})).unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    let active = s.inbox_list(&root, &json!({})).unwrap();
    assert!(!active["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == old["messageId"]));
    let waited = s
        .inbox_wait(
            &root,
            &[a.clone()],
            &["status".into()],
            false,
            Duration::from_millis(50),
        )
        .await
        .unwrap();
    assert_eq!(waited["messages"].as_array().unwrap().len(), 2);
    assert!(!waited.to_string().contains("r1: 10 of 100"));
    assert_eq!(
        s.send_to_thread(&root, first).unwrap(),
        old,
        "retry cannot resurrect an obsolete snapshot"
    );

    for kind in ["question", "result", "task"] {
        let mut invalid = snapshot(&a, "simulation", kind);
        invalid.kind = Some(kind.into());
        assert!(s.send_to_thread(&root, invalid).is_err());
    }
    let mut reply = snapshot(&a, "simulation", "reply cannot replace a status");
    reply.in_reply_to = Some("message".into());
    assert!(s.send_to_thread(&root, reply).is_err());
}

#[tokio::test]
async fn tasks_claim_atomically_respect_dependencies_and_report_back() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let lead = s.create_thread(input(&ws, None)).await.unwrap().id;
    let a = named(&s, &ws, &lead, "a").await;
    let b = named(&s, &ws, &lead, "b").await;

    s.task_add(&lead, "design schema", Some("tables for x"), &[], None)
        .unwrap();
    s.task_add(&lead, "write migration", None, &[1], Some(&b))
        .unwrap();
    s.task_add(&lead, "docs", None, &[], None).unwrap();
    assert!(s.task_add(&lead, "bad dep", None, &[9], None).is_err());
    // b was told about its assignment, passively.
    assert_eq!(s.inbox_unread_count(&b).unwrap(), 1);

    let list = s.task_list(&a, false).unwrap();
    let tasks = list["tasks"].as_array().unwrap();
    assert_eq!(tasks[1]["blockedBy"], json!([1]));
    assert_eq!(tasks[1]["ready"], false);

    // A blocked task cannot be claimed, nor one assigned elsewhere.
    assert!(s.task_claim(&b, Some(2)).is_err());
    assert!(s.task_claim(&a, Some(2)).is_err());
    let first = s.task_claim(&a, None).unwrap();
    assert_eq!(first["claimed"]["number"], 1);
    assert_eq!(first["claimed"]["detail"], "tables for x");
    // Next free task for b skips #1 (taken) and #2 (blocked) - it gets #3.
    assert_eq!(s.task_claim(&b, None).unwrap()["claimed"]["number"], 3);
    assert!(s.task_claim(&b, Some(1)).is_err(), "already in progress");
    assert!(s.task_done(&b, 1, None, false).is_err(), "not the owner");

    let done = s
        .task_done(&a, 1, Some("schema in schema.sql"), false)
        .unwrap();
    assert_eq!(done["unblocked"], json!([2]));
    // The lead hears about the result; b hears #2 is ready.
    let lead_mail = s.inbox_list(&lead, &json!({})).unwrap();
    assert!(lead_mail.to_string().contains("Task #1 completed"));
    assert_eq!(s.inbox_unread_count(&b).unwrap(), 2);
    assert_eq!(s.task_claim(&b, Some(2)).unwrap()["claimed"]["number"], 2);
    s.task_release(&b, 2).unwrap();
    assert_eq!(s.task_show(&lead, 2).unwrap()["status"], "pending");

    let tree = s.lineage_tree(&a, false).await.unwrap();
    assert_eq!(tree["tasks"]["completed"], 1);
    let b_row = tree["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "b")
        .unwrap();
    assert_eq!(b_row["currentTask"]["number"], 3);
}

#[tokio::test]
async fn concurrent_claims_never_hand_out_the_same_task() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let lead = s.create_thread(input(&ws, None)).await.unwrap().id;
    let mut workers = Vec::new();
    for i in 0..6 {
        workers.push(named(&s, &ws, &lead, &format!("w{i}")).await);
    }
    for i in 0..4 {
        s.task_add(&lead, &format!("t{i}"), None, &[], None)
            .unwrap();
    }
    let mut handles = Vec::new();
    for w in workers {
        let s = s.clone();
        handles.push(tokio::task::spawn_blocking(move || {
            s.task_claim(&w, None)
                .ok()
                .and_then(|v| v["claimed"]["number"].as_i64())
        }));
    }
    let mut claimed = Vec::new();
    for h in handles {
        if let Some(n) = h.await.unwrap() {
            claimed.push(n);
        }
    }
    claimed.sort();
    let before = claimed.len();
    claimed.dedup();
    assert_eq!(
        before,
        claimed.len(),
        "a task was claimed twice: {claimed:?}"
    );
    assert!(claimed.len() <= 4);
}

#[tokio::test]
async fn a_wake_queues_one_turn_on_the_parent_once_delegates_settle() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let lead = s.create_thread(input(&ws, None)).await.unwrap().id;
    let a = named(&s, &ws, &lead, "a").await;
    let other_root = s.create_thread(input(&ws, None)).await.unwrap().id;
    assert!(
        s.register_wake(&other_root, &[a.clone()]).is_err(),
        "only a delegate's ancestors can wake on it"
    );
    s.send_to_thread(&a, mail(&lead, "queue", "task", "work"))
        .unwrap();
    s.register_wake(&lead, &[a.clone()]).unwrap();
    // Until it fires, the parent's detail names what it waits for.
    let detail = s.get_thread_detail(&lead, None).await.unwrap();
    assert_eq!(detail.pending_wakes.len(), 1);
    assert_eq!(detail.pending_wakes[0].members[0].thread_id, a);
    assert_eq!(
        detail.pending_wakes[0].members[0].name.as_deref(),
        Some("a")
    );
    s.start_interaction_worker();
    // The lead starts exactly one turn, which carries a's outcome.
    until(|| {
        s.db.with(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM thread_turns WHERE thread_id=?1",
                [&lead],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap()
            == 1
    })
    .await;
    let prompt: String =
        s.db.with(|c| {
            Ok(c.query_row(
                "SELECT display_prompt FROM thread_turns WHERE thread_id=?1",
                [&lead],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert!(
        prompt.starts_with("[Pockymoe wake: the delegates you registered a wake for have settled]\nThis continues the task")
            && prompt.contains("a ("),
        "{prompt}"
    );
    let origin: String = s
        .db
        .with(|c| {
            Ok(c.query_row(
                "SELECT json_extract(h.item_json,'$.origin') FROM thread_history_items h JOIN thread_turns t ON t.id=h.turn_id WHERE t.thread_id=?1 AND h.item_id=t.id||':user'",
                [&lead],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(origin, "system", "a wake is not the user's speech");
    assert!(s
        .get_thread_detail(&lead, None)
        .await
        .unwrap()
        .pending_wakes
        .is_empty());
    tokio::time::sleep(Duration::from_millis(600)).await;
    let wakes: i64 =
        s.db.with(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM kv WHERE key GLOB 'cli:wake:*'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(wakes, 0, "a wake fires once");
}

#[tokio::test]
async fn peer_work_wake_persists_the_question_and_causal_task_through_queue_acceptance() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let lead = s.create_thread(input(&ws, None)).await.unwrap().id;
    let asker = named(&s, &ws, &lead, "reviewer").await;
    // Register while the parent owns a task, then end that foreground task.
    s.db.with(|c| {
        c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal,display_prompt) VALUES('original-task',?1,'inProgress',1,'Implement pairing')", [&lead])?;
        c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('asker-task',?1,'inProgress',1)", [&asker])?;
        Ok(())
    }).unwrap();
    let mut question = mail(
        &asker,
        "inbox",
        "question",
        "May a registration expire tomorrow?\n\nWhich scope should I use?",
    );
    question.subject = Some("Session lifetime".into());
    let receipt = s.send_to_thread(&lead, question).unwrap();
    s.register_wake(&lead, &[asker.clone()]).unwrap();
    s.db.with(|c| {
        c.execute(
            "UPDATE thread_turns SET status='completed' WHERE id='original-task'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    s.start_interaction_worker();
    until(|| {
        s.db.with(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM thread_turns WHERE thread_id=?1",
                [&lead],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap()
            == 2
    })
    .await;
    let turn: String =
        s.db.with(|c| {
            Ok(c.query_row(
                "SELECT id FROM thread_turns WHERE thread_id=?1 AND ordinal=2",
                [&lead],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    let metadata: serde_json::Value =
        s.db.with(|c| {
            let raw: String = c.query_row(
                "SELECT value FROM kv WHERE key=?1",
                [format!("cli:peer-work:turn:{turn}")],
                |r| r.get(0),
            )?;
            Ok(serde_json::from_str(&raw)?)
        })
        .unwrap();
    let prompt: String =
        s.db.with(|c| {
            Ok(c.query_row(
                "SELECT display_prompt FROM thread_turns WHERE id=?1",
                [&turn],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert!(
        prompt.starts_with("[Pockymoe wake: a delegate you registered a wake for is blocked]\n"),
        "the header names why it fired: {prompt}"
    );
    assert_eq!(metadata["taskTurnId"], "original-task");
    assert_eq!(metadata["messages"][0]["id"], receipt["messageId"]);
    assert_eq!(metadata["messages"][0]["name"], "reviewer");
    let summary = s.get_thread_detail_view(&lead, None, true).await.unwrap();
    let item = summary
        .turns
        .iter()
        .find(|t| t.id == turn)
        .unwrap()
        .items
        .iter()
        .find(|i| i.kind == "userMessage")
        .unwrap();
    assert!(item.extra["peerWork"]["messages"][0].get("text").is_none());
    let detail = s.get_thread_turn_detail(&lead, &turn).await.unwrap();
    let item = detail
        .items
        .iter()
        .find(|i| i.kind == "userMessage")
        .unwrap();
    assert_eq!(
        item.extra["peerWork"]["messages"][0]["text"],
        "May a registration expire tomorrow?\n\nWhich scope should I use?"
    );
    // Reading the browser history never answers/acknowledges the agent's question.
    let acknowledged: Option<String> =
        s.db.with(|c| {
            Ok(c.query_row(
                "SELECT json_extract(value,'$.acknowledgedAt') FROM kv WHERE key=?1",
                [format!(
                    "cli:inbox:{lead}:{}",
                    receipt["messageId"].as_str().unwrap()
                )],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert!(acknowledged.is_none());
}

#[tokio::test]
async fn a_wake_joins_the_running_turn_of_a_parent_that_takes_steers() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let lead = s.create_thread(input(&ws, None)).await.unwrap().id;
    let a = named(&s, &ws, &lead, "a").await;
    s.start_interaction_worker();
    // A long turn, like Claude waiting for a native subagent.
    s.send_to_thread(&lead, mail(&a, "queue", "task", "inspect this repository"))
        .unwrap();
    until(|| s.get_thread(&lead).unwrap().status == "running").await;
    let turn = s.get_thread(&lead).unwrap().active_turn_id.unwrap();
    s.register_wake(&lead, &[a.clone()]).unwrap();
    let steered = || {
        s.db.with(|c| {
            Ok(c
                .query_row(
                    "SELECT turn_id,item_json FROM thread_history_items WHERE thread_id=?1 AND item_id LIKE 'steer:%'",
                    [&lead],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .ok())
        })
        .unwrap()
    };
    until(|| steered().is_some()).await;
    let (steer_turn, item) = steered().unwrap();
    let item: serde_json::Value = serde_json::from_str(&item).unwrap();
    assert_eq!(steer_turn, turn, "the report joins the running turn");
    assert_eq!(item["origin"], "system");
    assert!(item["text"]
        .as_str()
        .unwrap()
        .starts_with("[Pockymoe wake: the delegates you registered a wake for have settled]\n"));
    let (turns, queued): (i64, i64) = s
        .db
        .with(|c| {
            Ok(c.query_row(
                "SELECT (SELECT COUNT(*) FROM thread_turns WHERE thread_id=?1),
                        (SELECT COUNT(*) FROM thread_pending_steers WHERE thread_id=?1 AND delivery IN ('continuation','cli-steer'))",
                [&lead],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!((turns, queued), (1, 0), "nothing waits for the turn to end");
    s.interrupt(&lead).await.unwrap();
}

#[tokio::test]
async fn roles_supply_defaults_and_prefix_only_the_first_turn() {
    let (dir, s) = setup();
    let agents = dir.path().join(".pockymoe/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("reviewer.md"),
        "---\ndescription: Finds bugs\nmodel: gpt-x\neffort: high\n---\nReport only actionable findings.\n",
    )
    .unwrap();
    let role = load_role(dir.path(), "reviewer").unwrap();
    assert_eq!(role.model.as_deref(), Some("gpt-x"));
    assert_eq!(role.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(role.instructions, "Report only actionable findings.");
    assert!(load_role(dir.path(), "missing").is_err());
    assert!(load_role(dir.path(), "../etc").is_err());

    let ws = workspace(&s);
    let lead = s.create_thread(input(&ws, None)).await.unwrap().id;
    let r = s
        .create_thread_with(
            input(&ws, Some(&lead)),
            AgentOptions {
                name: Some("rev".into()),
                role: Some(role),
                worktree: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(r.agent_role.as_deref(), Some("reviewer"));
    let staged = |s: &Supervisor| -> Option<String> {
        s.db.with(|c| {
            use rusqlite::OptionalExtension;
            Ok(c.query_row(
                "SELECT value FROM kv WHERE key=?1",
                [format!("cli:role:{}", r.id)],
                |row| row.get(0),
            )
            .optional()?)
        })
        .unwrap()
    };
    assert!(staged(&s)
        .unwrap()
        .contains("Report only actionable findings."));
    s.start_interaction_worker();
    s.send_to_thread(&r.id, mail(&lead, "queue", "task", "first"))
        .unwrap();
    s.wait_threads(&[r.id.clone()], false, Duration::from_secs(20))
        .await
        .unwrap();
    assert_eq!(staged(&s), None, "the role applies to the first turn only");
}

#[tokio::test]
async fn worktree_delegates_run_in_their_own_checkout() {
    let (dir, s) = setup();
    let repo = dir.path();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "init",
    ]);
    let ws = workspace(&s);
    let lead = s.create_thread(input(&ws, None)).await.unwrap().id;
    let t = s
        .create_thread_with(
            input(&ws, Some(&lead)),
            AgentOptions {
                name: Some("iso".into()),
                worktree: Some(None),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let path = std::path::PathBuf::from(t.worktree_path.clone().unwrap());
    assert!(
        path.join(".git").exists(),
        "worktree checked out at {path:?}"
    );
    assert!(path.ends_with("iso"));
    assert_eq!(s.session_cwd(&t).as_deref(), t.worktree_path.as_deref());

    // A dirty worktree refuses removal and the thread stays open.
    std::fs::write(path.join("scratch.txt"), "x").unwrap();
    assert!(s.close_agent_thread(Some(&lead), &t.id, true).is_err());
    assert!(s.get_thread(&t.id).unwrap().closed_at.is_none());
    std::fs::remove_file(path.join("scratch.txt")).unwrap();
    let closed = s.close_agent_thread(Some(&lead), &t.id, true).unwrap();
    assert_eq!(closed["worktreeRemoved"], true);
    assert!(!path.exists());
}

#[tokio::test]
async fn an_unanswered_question_blocks_a_wait_until_it_is_answered() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let lead = s.create_thread(input(&ws, None)).await.unwrap().id;
    let asker = named(&s, &ws, &lead, "asker").await;
    // Hold `asker` in a running turn so it is otherwise unsettled.
    s.db.with(|c| {
        c.execute(
            "INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('t-run',?1,'inProgress',1)",
            [&asker],
        )?;
        Ok(())
    })
    .unwrap();
    let receipt = s
        .send_to_thread(&lead, mail(&asker, "inbox", "question", "Which ellipsis?"))
        .unwrap();
    let waited = s
        .wait_threads(&[asker.clone()], false, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(waited["blocked"], true, "{waited}");
    assert!(waited["threads"][0]["waitingOn"][0]
        .as_str()
        .unwrap()
        .contains("Which ellipsis?"));

    // A correlated reply resolves it, without the parent having to acknowledge.
    let mut reply = mail(&lead, "inbox", "status", "Use U+2026");
    reply.in_reply_to = receipt["messageId"].as_str().map(str::to_owned);
    s.send_to_thread(&asker, reply).unwrap();
    let after = s
        .wait_threads(&[asker.clone()], false, Duration::from_millis(300))
        .await
        .unwrap();
    assert_eq!(after["blocked"], false, "{after}");
    assert_eq!(after["threads"][0]["state"], "running");
}

#[tokio::test]
async fn claim_wait_holds_a_worker_across_a_dependency_barrier() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let lead = s.create_thread(input(&ws, None)).await.unwrap().id;
    let a = named(&s, &ws, &lead, "a").await;
    let b = named(&s, &ws, &lead, "b").await;
    s.task_add(&lead, "base", None, &[], None).unwrap();
    s.task_add(&lead, "dependent", None, &[1], None).unwrap();
    assert_eq!(s.task_claim(&a, None).unwrap()["claimed"]["number"], 1);

    let blocked = s.task_claim(&b, None).unwrap();
    assert_eq!(blocked["claimed"], serde_json::Value::Null);
    assert_eq!(
        blocked["finished"], false,
        "work is blocked, not finished: {blocked}"
    );

    let finisher = s.clone();
    let a2 = a.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        finisher.task_done(&a2, 1, Some("ok"), false).unwrap();
    });
    let got = s
        .task_claim_wait(&b, None, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(got["claimed"]["number"], 2, "{got}");

    s.task_done(&b, 2, None, false).unwrap();
    let done = s
        .task_claim_wait(&b, None, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(done["finished"], true, "{done}");
    assert_eq!(done["boardComplete"], true);
    assert_eq!(done["timedOut"], false);
}

#[test]
fn a_reused_name_gets_a_fresh_default_worktree_branch() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap()
            .status
            .success())
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "init",
    ]);
    let first = pockymoe_runtime::interaction::create_worktree(&repo, "w", None).unwrap();
    assert_eq!(first.branch, "agent/w");
    pockymoe_runtime::interaction::remove_worktree(&repo, &first.path).unwrap();
    let second = pockymoe_runtime::interaction::create_worktree(&repo, "w", None).unwrap();
    assert_eq!(
        second.branch, "agent/w-2",
        "must not silently reuse stale agent/w"
    );
}
