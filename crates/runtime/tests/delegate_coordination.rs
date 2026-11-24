//! What a coordinator learns about and can do with its delegates beyond
//! settle/block: stalled queues, turn outcomes, `thread ask`, mail wakes, the
//! delegate contract and the failure breaker (docs/thread-interaction.md).
use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Provider};
use pockymoe_runtime::{
    fake::{model_prompts, FakeRuntime},
    interaction::{AgentOptions, CoordinationTimers, MailFilter, SendInput},
    Database, RuntimeConfig, Supervisor,
};
use serde_json::json;
use std::{path::Path, sync::Arc, time::Duration};

fn config(dir: &Path) -> RuntimeConfig {
    RuntimeConfig {
        treer_enabled: false,
        host_mode: Default::default(),
        mode: pockymoe_protocol::Mode::Local,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root: dir.into(),
        database_url: dir.join("db.sqlite"),
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
    }
}

/// Short stall and retry durations so no test waits the production 30 s.
const FAST: CoordinationTimers = CoordinationTimers {
    stall_after: Duration::from_millis(300),
    stall_failures: 3,
    start_retry: Duration::from_millis(50),
};

fn open(dir: &Path, fake: Arc<FakeRuntime>) -> Arc<Supervisor> {
    let config = config(dir);
    let db = Database::open(&config.database_url).unwrap();
    let state = Arc::new(Supervisor::new(config, db, vec![fake]));
    state.interaction.set_timers(FAST);
    state.spawn_live_item_persister();
    state
}

fn setup() -> (tempfile::TempDir, Arc<Supervisor>, Arc<FakeRuntime>) {
    let dir = tempfile::tempdir().unwrap();
    // Wake and question prompts are long, which the fake treats as slow work.
    let fake =
        Arc::new(FakeRuntime::new(Provider::Codex).with_slow_turn(Duration::from_millis(50)));
    let state = open(dir.path(), fake.clone());
    (dir, state, fake)
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

async fn root(s: &Supervisor, ws: &str) -> String {
    s.create_thread(input(ws, None)).await.unwrap().id
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
        subject: Some(format!("{kind} subject")),
        kind: Some(kind.into()),
        in_reply_to: None,
        interrupt_reason: None,
        topic_key: None,
        retry_failed: false,
    }
}

async fn until(mut check: impl FnMut() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    panic!("condition was not reached");
}

fn turns(s: &Supervisor, thread: &str) -> Vec<(String, String, String)> {
    s.db.with(|c| {
        let mut stmt = c.prepare(
            "SELECT id,status,COALESCE(display_prompt,'') FROM thread_turns WHERE thread_id=?1 ORDER BY ordinal",
        )?;
        let rows = stmt
            .query_map([thread], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .unwrap()
}

fn settled_turns(s: &Supervisor, thread: &str) -> usize {
    turns(s, thread)
        .iter()
        .filter(|t| t.1 != "inProgress")
        .count()
}

fn origin(s: &Supervisor, turn: &str) -> String {
    s.db.with(|c| {
        Ok(c.query_row(
            "SELECT json_extract(item_json,'$.origin') FROM thread_history_items WHERE item_id=?1||':user'",
            [turn],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}

fn add_turn(s: &Supervisor, thread: &str, ordinal: i64, status: &str, error: Option<&str>) {
    s.db.with(|c| {
        c.execute(
            "INSERT INTO thread_turns(id,thread_id,status,ordinal,error,completed_at) VALUES(?1,?2,?3,?4,?5,'2026-11-24T00:00:00Z')",
            rusqlite::params![format!("{thread}-turn-{ordinal}"), thread, status, ordinal, error],
        )?;
        Ok(())
    })
    .unwrap();
}

fn wake_count(s: &Supervisor) -> i64 {
    s.db.with(|c| {
        Ok(c.query_row(
            "SELECT COUNT(*) FROM kv WHERE key GLOB 'cli:wake:*'",
            [],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}

#[tokio::test]
async fn a_delegate_whose_queue_cannot_start_stalls_and_reports_it_once() {
    let (_dir, s, _) = setup();
    let ws = workspace(&s);
    let lead = root(&s, &ws).await;
    let a = named(&s, &ws, &lead, "builder").await;
    // Its harness is not available on this Supervisor.
    s.db.with(|c| {
        c.execute("UPDATE threads SET provider='claude' WHERE id=?1", [&a])?;
        Ok(())
    })
    .unwrap();
    s.send_to_thread(&a, mail(&lead, "queue", "task", "build it"))
        .unwrap();
    s.register_wake(&lead, &[a.clone()]).unwrap();
    s.start_interaction_worker();

    let started = std::time::Instant::now();
    let waited = s
        .wait_threads(&[a.clone()], false, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(waited["stalled"], true, "{waited}");
    assert_eq!(waited["timedOut"], false);
    let member = &waited["threads"][0];
    assert_eq!(member["state"], "stalled");
    assert_eq!(member["settled"], false);
    assert_eq!(member["outcome"]["kind"], "stalled");
    assert!(member["outcome"]["failures"].as_u64().unwrap() >= 3);
    assert!(
        member["outcome"]["error"]
            .as_str()
            .unwrap()
            .contains("claude is not enabled"),
        "{member}"
    );
    assert!(waited["next"].as_str().unwrap().contains("--clear-queue"));
    assert!(started.elapsed() < Duration::from_secs(3));

    // The registered wake fires once, as stalled, with the error.
    until(|| settled_turns(&s, &lead) == 1).await;
    let prompt = &turns(&s, &lead)[0].2;
    assert!(
        prompt.starts_with("[Pockymoe wake: a delegate you registered a wake for is stalled]\n"),
        "{prompt}"
    );
    assert!(prompt.contains("builder (") && prompt.contains("): stalled"));
    assert!(prompt.contains("outcome: stalled") && prompt.contains("claude is not enabled"));
    // `thread status` shows the same.
    let status = s.interaction_status(&a).await.unwrap();
    assert_eq!(status["state"], "stalled");
    assert_eq!(status["outcome"]["kind"], "stalled");

    // Registering again for the same stall does not fire again.
    s.register_wake(&lead, &[a.clone()]).unwrap();
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(wake_count(&s), 1, "the same stall is reported once");
    assert_eq!(turns(&s, &lead).len(), 1);

    // Once the harness works, the turn starts, the stall clears and the
    // waiting wake fires as settled.
    s.db.with(|c| {
        c.execute("UPDATE threads SET provider='codex' WHERE id=?1", [&a])?;
        Ok(())
    })
    .unwrap();
    until(|| settled_turns(&s, &lead) == 2).await;
    assert!(turns(&s, &lead)[1]
        .2
        .starts_with("[Pockymoe wake: the delegates you registered a wake for have settled]\n"));
    let after = s.interaction_status(&a).await.unwrap();
    assert_eq!(after["state"], "idle");
    assert_eq!(after["outcome"]["kind"], "completed");
    let stall: i64 =
        s.db.with(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM kv WHERE key=?1",
                [format!("cli:stall:{a}")],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(stall, 0, "a started turn clears the stall");
}

#[tokio::test]
async fn a_withdrawn_stalled_queue_does_not_mark_new_work_stalled() {
    let (_dir, s, _) = setup();
    let ws = workspace(&s);
    let lead = root(&s, &ws).await;
    let a = named(&s, &ws, &lead, "builder").await;
    s.db.with(|c| {
        c.execute("UPDATE threads SET provider='claude' WHERE id=?1", [&a])?;
        Ok(())
    })
    .unwrap();
    s.send_to_thread(&a, mail(&lead, "queue", "task", "build it"))
        .unwrap();
    s.start_interaction_worker();
    let waited = s
        .wait_threads(&[a.clone()], false, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(waited["threads"][0]["state"], "stalled");
    s.remove_queued_messages(&a, &[], true, None).await.unwrap();
    assert_eq!(s.interaction_status(&a).await.unwrap()["state"], "idle");
    // New work starts a new period; the old failures do not count for it.
    s.send_to_thread(&a, mail(&lead, "queue", "task", "build it again"))
        .unwrap();
    assert_eq!(s.interaction_status(&a).await.unwrap()["state"], "queued");
}

#[tokio::test]
async fn outcomes_say_how_the_last_turn_ended_and_who_stopped_it() {
    let (_dir, s, fake) = setup();
    fake.set_slow_turn(Duration::from_secs(20));
    let ws = workspace(&s);
    let lead = root(&s, &ws).await;
    let w = named(&s, &ws, &lead, "worker").await;
    s.start_interaction_worker();
    let state = |s: Arc<Supervisor>, id: String| async move {
        s.wait_threads(&[id], false, Duration::from_secs(5))
            .await
            .unwrap()["threads"][0]
            .clone()
    };

    s.send_to_thread(&w, mail(&lead, "queue", "task", "hello there"))
        .unwrap();
    until(|| settled_turns(&s, &w) == 1).await;
    let done = state(s.clone(), w.clone()).await;
    assert_eq!(done["outcome"], json!({"kind":"completed"}));
    assert_eq!(done["lastTurn"]["outcome"]["kind"], "completed");

    // The caller's `thread stop` with its reason.
    s.send_to_thread(
        &w,
        mail(&lead, "queue", "task", "inspect this repository slowly"),
    )
    .unwrap();
    until(|| s.get_thread(&w).unwrap().status == "running").await;
    s.stop_thread_for_agent(&w, Some("wrong branch"), false, Some(&lead))
        .await
        .unwrap();
    until(|| settled_turns(&s, &w) == 2).await;
    let stopped = state(s.clone(), w.clone()).await;
    assert_eq!(stopped["outcome"]["kind"], "interrupted", "{stopped}");
    assert_eq!(stopped["outcome"]["by"], "thread");
    assert_eq!(stopped["outcome"]["byThreadId"], lead.as_str());
    assert_eq!(stopped["outcome"]["reason"], "wrong branch");

    // The owner's Stop.
    s.send_to_thread(
        &w,
        mail(&lead, "queue", "task", "inspect this repository again"),
    )
    .unwrap();
    until(|| s.get_thread(&w).unwrap().status == "running").await;
    s.interrupt(&w).await.unwrap();
    until(|| settled_turns(&s, &w) == 3).await;
    let owner = state(s.clone(), w.clone()).await;
    assert_eq!(owner["outcome"]["by"], "owner", "{owner}");

    // A restart, recorded only as the lost-connection error; then a failure.
    add_turn(
        &s,
        &w,
        10,
        "interrupted",
        Some("Backend completion was not confirmed before the Supervisor connection ended. Reconnect to verify before continuing."),
    );
    assert_eq!(
        state(s.clone(), w.clone()).await["outcome"]["by"],
        "restart"
    );
    add_turn(&s, &w, 11, "failed", Some("model refused: quota exhausted"));
    let failed = state(s.clone(), w.clone()).await;
    assert_eq!(
        failed["outcome"],
        json!({"kind":"failed","error":"model refused: quota exhausted"})
    );
    assert_eq!(failed["consecutiveFailures"], 1);
    let status = s.interaction_status(&w).await.unwrap();
    assert_eq!(status["outcome"]["kind"], "failed");
    assert_eq!(status["consecutiveFailures"], 1);
    // Older fields stay.
    assert_eq!(status["status"], json!(s.get_thread(&w).unwrap().status));
    assert!(failed["lastTurn"]["status"] == "failed");
}

#[tokio::test]
async fn the_breaker_refuses_more_work_after_three_failed_turns_unless_retry_is_intended() {
    let (_dir, s, _) = setup();
    let ws = workspace(&s);
    let lead = root(&s, &ws).await;
    let d = named(&s, &ws, &lead, "flaky").await;
    let sibling = named(&s, &ws, &lead, "sibling").await;
    add_turn(&s, &d, 1, "failed", Some("first"));
    add_turn(&s, &d, 2, "failed", Some("second"));
    // Two failures are not yet a pattern.
    s.send_to_thread_as_agent(&d, mail(&lead, "queue", "task", "retry 1"))
        .unwrap();
    add_turn(&s, &d, 3, "failed", Some("linker: cannot find -lssl"));
    assert_eq!(
        s.interaction_status(&d).await.unwrap()["consecutiveFailures"],
        3
    );

    let refused = s
        .send_to_thread_as_agent(&d, mail(&lead, "queue", "task", "retry 2"))
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("last 3 turns") && refused.contains("cannot find -lssl"),
        "{refused}"
    );
    assert!(refused.contains("thread status") && refused.contains("--retry-failed"));
    let mut direct = mail(&lead, "direct", "task", "fix it");
    direct.interrupt_reason = Some("wrong flags".into());
    assert!(s.send_to_thread_as_agent(&d, direct).is_err());

    // Mail, questions and the owner are never refused; nor is a non-delegate.
    s.send_to_thread_as_agent(&d, mail(&lead, "inbox", "status", "fyi"))
        .unwrap();
    s.send_to_thread(&d, mail(&lead, "queue", "task", "owner retry"))
        .unwrap();
    s.send_to_thread_as_agent(&d, mail(&sibling, "queue", "task", "sibling work"))
        .unwrap();

    // An intended retry passes, and its request id still deduplicates without it.
    let mut intended = mail(&lead, "queue", "task", "retry with the fix");
    intended.retry_failed = true;
    intended.client_request_id = Some("retry-once".into());
    let receipt = s.send_to_thread_as_agent(&d, intended.clone()).unwrap();
    intended.retry_failed = false;
    let again = s.send_to_thread_as_agent(&d, intended).unwrap();
    assert_eq!(receipt["messageId"], again["messageId"]);

    // An interrupted turn ends the run of failures.
    add_turn(&s, &d, 4, "interrupted", None);
    assert_eq!(
        s.interaction_status(&d).await.unwrap()["consecutiveFailures"],
        0
    );
    s.send_to_thread_as_agent(&d, mail(&lead, "queue", "task", "fresh"))
        .unwrap();
}

#[tokio::test]
async fn the_first_ancestor_task_carries_the_delegate_contract_once_and_only_to_the_harness() {
    let (_dir, s, _) = setup();
    let ws = workspace(&s);
    let lead = root(&s, &ws).await;
    let c = named(&s, &ws, &lead, "worker").await;
    let peer = named(&s, &ws, &lead, "peer").await;

    // A sibling's task is not from an ancestor: no contract.
    s.send_to_thread(&peer, mail(&c, "queue", "task", "sibling task"))
        .unwrap();
    s.send_to_thread(&c, mail(&lead, "queue", "task", "first task"))
        .unwrap();
    // The queued message is the sender's own; the contract is not in it.
    let queued = s.queued_messages(&c, Some(&lead)).unwrap();
    assert_eq!(queued["messages"][0]["fromThreadId"], lead.as_str());
    assert_eq!(queued["messages"][0]["mine"], true);
    assert!(!queued["messages"][0]["preview"]
        .as_str()
        .unwrap()
        .contains("contract"));
    s.start_interaction_worker();
    until(|| settled_turns(&s, &c) == 1 && settled_turns(&s, &peer) == 1).await;

    let sent = model_prompts(&c);
    let first = &sent[0];
    let contract = first
        .split("\n\n[Pockymoe task from")
        .next()
        .unwrap()
        .rsplit("[Pockymoe delegate contract]")
        .next()
        .unwrap();
    assert!(
        first.contains("[Pockymoe delegate contract]\nYou work for thread "),
        "{first}"
    );
    assert!(first.contains(&lead) && first.contains("pockymoe thread ask parent"));
    assert!(
        contract.chars().count() < 600,
        "{}",
        contract.chars().count()
    );
    // The stored prompt is what the sender wrote, with its header first.
    let stored = &turns(&s, &c)[0].2;
    assert!(stored.starts_with(&format!(
        "[Pockymoe task from thread {lead} | task subject]\n"
    )));
    assert!(!stored.contains("delegate contract"));
    assert!(!model_prompts(&peer)[0].contains("delegate contract"));

    // Later tasks start without it.
    s.send_to_thread(&c, mail(&lead, "queue", "task", "second task"))
        .unwrap();
    until(|| settled_turns(&s, &c) == 2).await;
    assert!(!model_prompts(&c)[1].contains("delegate contract"));
}

#[tokio::test]
async fn ask_delivers_the_question_to_an_idle_parent_and_returns_the_answer() {
    let (_dir, s, _) = setup();
    let ws = workspace(&s);
    let lead = root(&s, &ws).await;
    let q = named(&s, &ws, &lead, "asker").await;
    s.start_interaction_worker();

    let asking = {
        let s = s.clone();
        let (lead, q) = (lead.clone(), q.clone());
        tokio::spawn(async move {
            s.ask_thread(
                &lead,
                &q,
                "Which key?",
                "May I use the staging key?".into(),
                Some("ask-1".into()),
                Duration::from_secs(5),
                true,
            )
            .await
            .unwrap()
        })
    };
    // The idle parent gets the question as a turn right away.
    until(|| settled_turns(&s, &lead) == 1).await;
    let (turn, _, prompt) = turns(&s, &lead)[0].clone();
    assert!(
        prompt.starts_with(&format!(
            "[Pockymoe question from thread {q} | Which key?]\nImmediate handling needed: asker is blocked in `pockymoe thread ask`"
        )),
        "{prompt}"
    );
    assert!(prompt.ends_with("\nMay I use the staging key?"));
    assert_eq!(origin(&s, &turn), "peer");
    let inbox = s.inbox_list(&lead, &json!({})).unwrap();
    let question = inbox["messages"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(inbox["messages"][0]["kind"], "question");
    let mut answer = mail(&lead, "inbox", "result", "Yes, use staging.");
    answer.in_reply_to = Some(question.clone());
    s.send_to_thread(&q, answer).unwrap();

    let answered = asking.await.unwrap();
    assert_eq!(answered["answered"], true, "{answered}");
    assert_eq!(answered["questionId"], question.as_str());
    assert_eq!(answered["promptDelivery"], "prompt");
    assert_eq!(answered["reply"]["text"], "Yes, use staging.");
    assert!(answered["reply"]["readAt"].is_string(), "marked read");
    assert!(answered["reply"]["acknowledgedAt"].is_null());

    // A retry with the same request id is the same question, answered at once.
    let retried = s
        .ask_thread(
            &lead,
            &q,
            "Which key?",
            "May I use the staging key?".into(),
            Some("ask-1".into()),
            Duration::from_millis(200),
            true,
        )
        .await
        .unwrap();
    assert_eq!(retried["questionId"], question.as_str());
    assert_eq!(retried["answered"], true);
    let questions = s
        .inbox_list(&lead, &json!({"all":true,"kinds":["question"]}))
        .unwrap();
    assert_eq!(questions["messages"].as_array().unwrap().len(), 1);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(turns(&s, &lead).len(), 1, "a retry is not delivered again");
}

#[tokio::test]
async fn an_ask_to_a_parent_with_a_wake_is_told_once_in_the_wake_with_the_question() {
    let (_dir, s, _) = setup();
    let ws = workspace(&s);
    let lead = root(&s, &ws).await;
    let q = named(&s, &ws, &lead, "asker").await;
    s.register_wake(&lead, std::slice::from_ref(&q)).unwrap();
    // The asker works on a turn while it asks, as a real delegate does.
    add_turn(&s, &q, 1, "inProgress", None);
    s.db.with(|c| {
        c.execute("UPDATE threads SET status='running' WHERE id=?1", [&q])?;
        Ok(())
    })
    .unwrap();
    s.start_interaction_worker();

    let asking = {
        let s = s.clone();
        let (lead, q) = (lead.clone(), q.clone());
        tokio::spawn(async move {
            s.ask_thread(
                &lead,
                &q,
                "Which port?",
                "8080 or 9090?".into(),
                None,
                Duration::from_secs(5),
                true,
            )
            .await
            .unwrap()
        })
    };
    until(|| settled_turns(&s, &lead) == 1).await;
    let (_, _, prompt) = turns(&s, &lead)[0].clone();
    assert!(
        prompt.starts_with("[Pockymoe wake: a delegate you registered a wake for is blocked]"),
        "{prompt}"
    );
    let inbox = s.inbox_list(&lead, &json!({})).unwrap();
    let question = inbox["messages"][0]["id"].as_str().unwrap().to_owned();
    // The wake quotes the question and how to answer it, in plain text.
    assert!(
        prompt.contains(&format!(
            "question {question} | Which port?:\n  8080 or 9090?"
        )),
        "{prompt}"
    );
    assert!(
        prompt.contains(&format!(
            "pockymoe thread send asker --kind result --in-reply-to {question}"
        )),
        "{prompt}"
    );
    assert!(!prompt.contains("String("), "{prompt}");
    // Quoted in the wake, the question counts as read: no reminder turn later.
    let listed = s.inbox_list(&lead, &json!({})).unwrap();
    assert!(listed["messages"][0]["readAt"].is_string(), "{listed}");
    // No second turn repeats it as a prompt of its own.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(turns(&s, &lead).len(), 1, "{:?}", turns(&s, &lead));

    let mut answer = mail(&lead, "inbox", "result", "Use 9090");
    answer.in_reply_to = Some(question);
    s.send_to_thread(&q, answer).unwrap();
    let answered = asking.await.unwrap();
    assert_eq!(answered["answered"], true, "{answered}");
    assert_eq!(answered["promptDelivery"], "wake");
    assert_eq!(answered["reply"]["text"], "Use 9090");
}

#[tokio::test]
async fn an_unanswered_ask_times_out_open_and_limits_further_prompt_questions() {
    let (_dir, s, _) = setup();
    let ws = workspace(&s);
    let lead = root(&s, &ws).await;
    let q = named(&s, &ws, &lead, "asker").await;
    let other = named(&s, &ws, &lead, "other").await;
    s.start_interaction_worker();

    let timed_out = s
        .ask_thread(
            &lead,
            &q,
            "Unanswered",
            "Anyone?".into(),
            None,
            Duration::from_millis(300),
            true,
        )
        .await
        .unwrap();
    assert_eq!(timed_out["answered"], false);
    assert_eq!(timed_out["timedOut"], true);
    let question = timed_out["questionId"].as_str().unwrap();
    assert!(timed_out["next"]
        .as_str()
        .unwrap()
        .contains(&format!("inbox wait --in-reply-to {question}")));
    until(|| settled_turns(&s, &lead) == 1).await;

    // While that one is open, the next goes to the inbox only.
    let second = s
        .ask_thread(
            &lead,
            &q,
            "Second",
            "Still there?".into(),
            None,
            Duration::from_millis(100),
            true,
        )
        .await
        .unwrap();
    assert_eq!(second["promptDelivery"], "passive", "{second}");
    assert!(second["promptDeliveryNote"]
        .as_str()
        .unwrap()
        .contains("still unanswered"));
    // A thread that is not the asker's ancestor reads it from its inbox.
    let sideways = s
        .ask_thread(
            &other,
            &q,
            "Sideways",
            "Sibling question".into(),
            None,
            Duration::from_millis(100),
            true,
        )
        .await
        .unwrap();
    assert_eq!(sideways["promptDelivery"], "passive");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(turns(&s, &lead).len(), 1);
    assert!(turns(&s, &other).is_empty());

    // `inbox wait --in-reply-to` resumes waiting for the first question.
    let mut late = mail(&root_of(&s, &q), "inbox", "result", "Late answer");
    late.in_reply_to = Some(question.into());
    s.send_to_thread(&q, late).unwrap();
    let resumed = s
        .inbox_wait_matching(
            &q,
            &MailFilter {
                in_reply_to: Some(question.into()),
                ..Default::default()
            },
            false,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(resumed["messages"][0]["text"], "Late answer");
    assert_eq!(resumed["messages"].as_array().unwrap().len(), 1);
}

fn root_of(s: &Supervisor, id: &str) -> String {
    s.get_thread(id).unwrap().parent_thread_id.unwrap()
}

#[tokio::test]
async fn a_mail_wake_fires_once_on_matching_new_mail_and_shares_the_cap() {
    let (_dir, s, _) = setup();
    let ws = workspace(&s);
    let lead = root(&s, &ws).await;
    let a = named(&s, &ws, &lead, "producer").await;
    let b = named(&s, &ws, &lead, "noise").await;
    let old = s
        .send_to_thread(&lead, mail(&a, "inbox", "result", "old result"))
        .unwrap();
    let filter = MailFilter {
        from_thread_ids: vec![a.clone()],
        kinds: vec!["result".into(), "question".into()],
        in_reply_to: None,
    };
    let registered = s.register_mail_wake(&lead, &filter, None).unwrap();
    assert_eq!(registered["kind"], "mail");
    let pending = s
        .get_thread_detail(&lead, None)
        .await
        .unwrap()
        .pending_wakes;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].kind, "mail");
    let wanted = pending[0].mail.as_ref().unwrap();
    assert_eq!(wanted.from[0].name.as_deref(), Some("producer"));
    assert_eq!(wanted.kinds, ["result", "question"]);
    assert_eq!(
        pending[0].members[0].thread_id, a,
        "older clients list the sender"
    );
    let raw = serde_json::to_value(&pending[0]).unwrap();
    assert_eq!(raw["kind"], "mail");

    // The cap counts both kinds.
    s.register_wake(&lead, &[a.clone()]).unwrap();
    s.register_mail_wake(&lead, &MailFilter::default(), None)
        .unwrap();
    let capped = s
        .register_mail_wake(&lead, &filter, None)
        .unwrap_err()
        .to_string();
    assert!(capped.contains("limit"), "{capped}");
    assert!(s.register_wake(&lead, &[b.clone()]).is_err());
    // Keep only the filtered mail wake for the rest of the test.
    s.db.with(|c| {
        c.execute(
            "DELETE FROM kv WHERE key GLOB ?1 AND json_extract(value,'$.filter.fromThreadIds[0]') IS NOT ?2",
            rusqlite::params![format!("cli:wake:{lead}:*"), a],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(wake_count(&s), 1);

    s.start_interaction_worker();
    // Mail already waiting, another sender and another kind do not fire it.
    s.send_to_thread(&lead, mail(&b, "inbox", "result", "noise result"))
        .unwrap();
    s.send_to_thread(&lead, mail(&a, "inbox", "status", "progress"))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(turns(&s, &lead).is_empty());
    assert_eq!(wake_count(&s), 1);

    let mut wanted_mail = mail(&a, "inbox", "result", "the build passed\nsee artifacts");
    wanted_mail.subject = Some("Build done".into());
    let arrived = s.send_to_thread(&lead, wanted_mail).unwrap();
    until(|| settled_turns(&s, &lead) == 1).await;
    let (turn, _, prompt) = turns(&s, &lead)[0].clone();
    assert!(
        prompt.starts_with("[Pockymoe wake: mail you registered a wake for has arrived]\n"),
        "{prompt}"
    );
    assert!(prompt.contains("result \"Build done\" from producer"));
    assert!(prompt.contains(arrived["messageId"].as_str().unwrap()));
    assert!(prompt.contains("the build passed see artifacts"));
    assert!(!prompt.contains(old["messageId"].as_str().unwrap()));
    assert!(prompt.contains("pockymoe inbox ack"));
    assert_eq!(origin(&s, &turn), "system");
    assert_eq!(wake_count(&s), 0, "one-shot");
}

#[tokio::test]
async fn a_mail_wake_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (lead, a) = {
        let s = open(
            dir.path(),
            Arc::new(FakeRuntime::new(Provider::Codex).with_slow_turn(Duration::from_millis(50))),
        );
        let ws = workspace(&s);
        let lead = root(&s, &ws).await;
        let a = named(&s, &ws, &lead, "remote-ish").await;
        s.register_mail_wake(
            &lead,
            &MailFilter {
                from_thread_ids: vec![a.clone()],
                ..Default::default()
            },
            None,
        )
        .unwrap();
        (lead, a)
    };
    let s = open(
        dir.path(),
        Arc::new(FakeRuntime::new(Provider::Codex).with_slow_turn(Duration::from_millis(50))),
    );
    assert_eq!(s.pending_wakes(&lead).unwrap()[0].kind, "mail");
    s.send_to_thread(&lead, mail(&a, "inbox", "result", "after restart"))
        .unwrap();
    s.start_interaction_worker();
    until(|| settled_turns(&s, &lead) == 1).await;
    assert!(turns(&s, &lead)[0].2.contains("after restart"));
}

#[tokio::test]
async fn a_mail_wake_expires_with_a_status_note_and_wakes_can_be_listed_and_cancelled() {
    let (_dir, s, _) = setup();
    let ws = workspace(&s);
    let lead = root(&s, &ws).await;
    let helper = named(&s, &ws, &lead, "helper").await;
    let filter = MailFilter {
        kinds: vec!["result".into()],
        ..MailFilter::default()
    };
    // One that expires at once, one long-lived, one on a delegate.
    let short = s
        .register_mail_wake(&lead, &filter, Some(Duration::from_secs(1)))
        .unwrap();
    assert!(short["expiresAt"].is_string(), "{short}");
    let long = s.register_mail_wake(&lead, &filter, None).unwrap();
    // The delegate is still working, so its wake stays pending.
    add_turn(&s, &helper, 1, "inProgress", None);
    s.db.with(|c| {
        c.execute("UPDATE threads SET status='running' WHERE id=?1", [&helper])?;
        Ok(())
    })
    .unwrap();
    s.register_wake(&lead, std::slice::from_ref(&helper))
        .unwrap();
    let listed = s.manage_wakes(&lead, &[], false).unwrap();
    assert_eq!(listed["pendingWakes"].as_array().unwrap().len(), 3);
    assert_eq!(wake_count(&s), 3);

    s.start_interaction_worker();
    until(|| wake_count(&s) == 2).await;
    // Expiry is passive: a status note, no turn of its own.
    let inbox = s.inbox_list(&lead, &json!({})).unwrap();
    let note = &inbox["messages"][0];
    assert_eq!(note["kind"], "status");
    assert_eq!(note["subject"], "Mail wake expired");
    assert!(turns(&s, &lead).is_empty(), "{:?}", turns(&s, &lead));

    // A thread cancels its own wakes by id, or all of them; cancelled wakes never fire.
    let id = long["wakeId"].as_str().unwrap().to_owned();
    let cancelled = s
        .manage_wakes(&lead, std::slice::from_ref(&id), false)
        .unwrap();
    assert_eq!(cancelled["cancelled"], json!([id]));
    assert_eq!(wake_count(&s), 1);
    assert_eq!(
        s.manage_wakes(&helper, &[id], false).unwrap()["cancelled"],
        json!([]),
        "another thread's id cancels nothing"
    );
    let rest = s.manage_wakes(&lead, &[], true).unwrap();
    assert_eq!(rest["cancelled"].as_array().unwrap().len(), 1);
    assert_eq!(wake_count(&s), 0);
    assert!(s.manage_wakes(&lead, &["../x".into()], false).is_err());
}

#[tokio::test]
async fn a_wake_on_settled_delegates_reports_now_and_names_a_mailed_task() {
    let (_dir, s, _) = setup();
    let ws = workspace(&s);
    let lead = root(&s, &ws).await;
    let helper = named(&s, &ws, &lead, "helper").await;
    // The usual mistake: the task is mailed (inbox), so the idle delegate never runs it.
    s.send_to_thread(&helper, mail(&lead, "inbox", "task", "Create c.txt"))
        .unwrap();
    let report = s
        .wake_or_report(&lead, std::slice::from_ref(&helper))
        .await
        .unwrap();
    assert_eq!(report["registered"], false, "{report}");
    assert_eq!(report["allSettled"], true);
    assert_eq!(report["threads"][0]["threadId"], helper.as_str());
    assert_eq!(
        report["unstartedTasks"],
        json!([{"threadId": helper, "unreadTasksFromYou": 1}])
    );
    assert!(report["next"]
        .as_str()
        .unwrap()
        .contains("--delivery queue"));
    assert_eq!(
        wake_count(&s),
        0,
        "nothing registered, so nothing fires later"
    );

    // While the delegate works, the wake is registered as before.
    add_turn(&s, &helper, 1, "inProgress", None);
    s.db.with(|c| {
        c.execute("UPDATE threads SET status='running' WHERE id=?1", [&helper])?;
        Ok(())
    })
    .unwrap();
    let registered = s
        .wake_or_report(&lead, std::slice::from_ref(&helper))
        .await
        .unwrap();
    assert_eq!(registered["registered"], true, "{registered}");
    assert_eq!(wake_count(&s), 1);
    // Only one's own delegates.
    assert!(s
        .wake_or_report(&helper, std::slice::from_ref(&lead))
        .await
        .is_err());
}
