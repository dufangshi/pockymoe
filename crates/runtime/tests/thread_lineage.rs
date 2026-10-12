//! Agent-spawned threads must group under the thread a person started, so a
//! fan-out cannot flood a workspace listing.
use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Provider};
use pockymoe_runtime::{
    fake::FakeRuntime, interaction::SendInput, Database, RuntimeConfig, Supervisor,
};
use serde_json::json;
use std::sync::Arc;

fn setup() -> (tempfile::TempDir, Arc<Supervisor>) {
    let dir = tempfile::tempdir().unwrap();
    let config = RuntimeConfig {
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
    };
    let db = Database::open(&config.database_url).unwrap();
    let state = Arc::new(Supervisor::new(
        config,
        db,
        vec![Arc::new(FakeRuntime::new(Provider::Codex))],
    ));
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

async fn spawn(s: &Supervisor, ws: &str, parent: Option<&str>) -> pockymoe_protocol::ThreadDto {
    s.create_thread(CreateThreadInput {
        workspace_id: ws.to_string(),
        title: None,
        provider: Some(Provider::Codex),
        agent_id: Some("codex".into()),
        model: "ios-e2e-stream".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: parent.map(str::to_owned),
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn descendants_anchor_on_the_root_a_person_started() {
    let (_dir, s) = setup();
    let ws = workspace(&s);

    let root = spawn(&s, &ws, None).await;
    assert_eq!(root.parent_thread_id, None, "a user thread has no parent");
    assert_eq!(root.root_thread_id, None, "a root does not point at itself");
    assert_eq!(root.lineage_depth, 0);

    let child = spawn(&s, &ws, Some(&root.id)).await;
    assert_eq!(child.parent_thread_id.as_deref(), Some(root.id.as_str()));
    assert_eq!(child.root_thread_id.as_deref(), Some(root.id.as_str()));
    assert_eq!(child.lineage_depth, 1);

    // The grandchild must anchor on the ORIGINAL root, not on its own parent -
    // otherwise a chain splinters into several groups in the listing.
    let grandchild = spawn(&s, &ws, Some(&child.id)).await;
    assert_eq!(
        grandchild.parent_thread_id.as_deref(),
        Some(child.id.as_str())
    );
    assert_eq!(
        grandchild.root_thread_id.as_deref(),
        Some(root.id.as_str()),
        "the whole chain groups under the thread the person started"
    );
    assert_eq!(grandchild.lineage_depth, 2);
}

#[tokio::test]
async fn nesting_is_bounded_and_an_unknown_parent_is_refused() {
    let (_dir, s) = setup();
    let ws = workspace(&s);

    let mut current = spawn(&s, &ws, None).await.id;
    // depth 0 root, then 1, 2, 3 are allowed; the fourth generation is refused.
    for _ in 0..pockymoe_runtime::service::MAX_LINEAGE_DEPTH {
        current = spawn(&s, &ws, Some(&current)).await.id;
    }
    let too_deep = s
        .create_thread(CreateThreadInput {
            workspace_id: ws.clone(),
            title: None,
            provider: Some(Provider::Codex),
            agent_id: Some("codex".into()),
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: Some(current),
        })
        .await;
    let message = too_deep
        .err()
        .expect("nesting past the cap must fail")
        .to_string();
    assert!(
        message.contains("nesting is limited"),
        "error should explain the cap, got: {message}"
    );

    let orphan = s
        .create_thread(CreateThreadInput {
            workspace_id: ws,
            title: None,
            provider: Some(Provider::Codex),
            agent_id: Some("codex".into()),
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: Some("00000000-0000-0000-0000-000000000000".into()),
        })
        .await;
    assert!(
        orphan.is_err(),
        "an unknown parent must not create a thread"
    );
}

#[tokio::test]
async fn a_workspace_listing_shows_only_threads_a_person_started() {
    let (_dir, s) = setup();
    let ws = workspace(&s);

    let root = spawn(&s, &ws, None).await;
    let other_root = spawn(&s, &ws, None).await;
    for _ in 0..5 {
        spawn(&s, &ws, Some(&root.id)).await;
    }

    let visible = s.list_threads(Some(&ws), false).unwrap();
    assert_eq!(
        visible.len(),
        2,
        "five agent threads must not appear beside the two the person started"
    );
    assert!(visible.iter().all(|t| t.parent_thread_id.is_none()));

    let listed_root = visible.iter().find(|t| t.id == root.id).unwrap();
    assert_eq!(
        listed_root.descendant_count,
        Some(5),
        "the root should advertise how many agent threads it owns"
    );
    let quiet = visible.iter().find(|t| t.id == other_root.id).unwrap();
    assert_eq!(
        quiet.descendant_count, None,
        "a root with no descendants reports nothing rather than zero"
    );

    let everything = s.list_threads(Some(&ws), true).unwrap();
    assert_eq!(everything.len(), 7, "opting in returns the full tree");
}

#[tokio::test]
async fn parent_can_delete_finished_direct_children_only_and_preserve_the_result() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let root = spawn(&s, &ws, None).await;
    let other = spawn(&s, &ws, None).await;
    let child = spawn(&s, &ws, Some(&root.id)).await;
    let grandchild = spawn(&s, &ws, Some(&child.id)).await;
    assert!(s
        .delete_child_thread(&other.id, &child.id)
        .await
        .unwrap_err()
        .to_string()
        .starts_with("forbidden:"));
    assert!(s.delete_child_thread(&root.id, &root.id).await.is_err());
    assert!(s.delete_child_thread(&child.id, &root.id).await.is_err());
    assert!(s
        .delete_child_thread(&root.id, &grandchild.id)
        .await
        .is_err());
    assert!(s
        .delete_child_thread(&root.id, &child.id)
        .await
        .unwrap_err()
        .to_string()
        .contains("still owns child"));
    s.delete_child_thread(&child.id, &grandchild.id)
        .await
        .unwrap();
    let token = s.cli_thread_token(&child.id);
    let turn = uuid::Uuid::new_v4().to_string();
    s.db.with(|conn| {
        conn.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES (?1,?2,'completed',1)", rusqlite::params![turn,child.id])?;
        let item = json!({"id":"final","kind":"agentMessage","text":"CHILD_RESULT_RETAINED"});
        conn.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES ('item',?1,?2,'final',?3,'2030','2030')", rusqlite::params![child.id,turn,item.to_string()])?;
        conn.execute("INSERT INTO kv(key,value) VALUES (?1,?2)",rusqlite::params![format!("cli:notify:turn:{turn}:pending"),json!({"threadId":root.id,"delivery":"inbox"}).to_string()])?;
        Ok(())
    }).unwrap();
    let mut events = s.bus.subscribe();
    let receipt = s.delete_child_thread(&root.id, &child.id).await.unwrap();
    assert_eq!(receipt["deleted"], true);
    assert!(s.get_thread(&child.id).is_err());
    assert!(s.get_thread(&root.id).is_ok());
    assert!(s.get_thread(&other.id).is_ok());
    assert_eq!(s.cli_token_thread(&token), None);
    let mail = s.inbox_list(&root.id, &json!({})).unwrap();
    let message = &mail["messages"][0]["id"];
    assert!(s
        .inbox_read(&root.id, &json!({"messageId":message}))
        .unwrap()
        .to_string()
        .contains("CHILD_RESULT_RETAINED"));
    assert_eq!(
        events.try_recv().unwrap().payload["deletedThreadId"],
        child.id
    );
    assert_eq!(
        s.list_threads(Some(&ws), false)
            .unwrap()
            .iter()
            .find(|t| t.id == root.id)
            .unwrap()
            .descendant_count,
        None
    );
    s.db.with(|conn| {
        for table in ["threads", "thread_turns", "thread_history_items"] {
            let column = if table == "threads" {
                "id"
            } else {
                "thread_id"
            };
            let count: i64 = conn.query_row(
                &format!("SELECT count(*) FROM {table} WHERE {column}=?1"),
                [&child.id],
                |r| r.get(0),
            )?;
            assert_eq!(count, 0);
        }
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
async fn deletion_refuses_running_recovering_and_queued_children_without_removing_work() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let root = spawn(&s, &ws, None).await;
    let child = spawn(&s, &ws, Some(&root.id)).await;
    for status in ["running", "recovering"] {
        s.db.with(|c| {
            c.execute(
                "UPDATE threads SET status=?1 WHERE id=?2",
                rusqlite::params![status, child.id],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(s.delete_child_thread(&root.id, &child.id).await.is_err());
    }
    s.db.with(|c| {c.execute("UPDATE threads SET status='idle' WHERE id=?1",[&child.id])?;c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES ('active',?1,'inProgress',1)",[&child.id])?;Ok(())}).unwrap();
    assert!(s.delete_child_thread(&root.id, &child.id).await.is_err());
    s.db.with(|c| {
        c.execute(
            "UPDATE thread_turns SET status='completed' WHERE id='active'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    s.send_to_thread(
        &child.id,
        SendInput {
            delivery: "queue".into(),
            notify_delivery: "inbox".into(),
            text: "work still waiting".into(),
            from_thread_id: Some(root.id.clone()),
            notify_on_complete: false,
            client_request_id: None,
            subject: None,
            kind: Some("task".into()),
            in_reply_to: None,
            interrupt_reason: None,
            topic_key: None,
        },
    )
    .unwrap();
    assert!(s
        .delete_child_thread(&root.id, &child.id)
        .await
        .unwrap_err()
        .to_string()
        .contains("queued work"));
    assert!(s.get_thread(&child.id).is_ok());
    assert_eq!(
        s.interaction_status(&child.id).await.unwrap()["queuedCount"],
        1
    );
}

#[tokio::test]
async fn a_burst_of_idle_agent_threads_is_capped_and_finished_ones_free_slots() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let root = spawn(&s, &ws, None).await;

    // Freshly created threads sit at `idle`, never having been prompted. A cap that
    // only counted `running` threads would let this burst through unbounded.
    let mut made = Vec::new();
    for _ in 0..pockymoe_runtime::service::MAX_OPEN_AGENT_THREADS {
        made.push(spawn(&s, &ws, Some(&root.id)).await.id);
    }
    let refused = s
        .create_thread(CreateThreadInput {
            workspace_id: ws.clone(),
            title: None,
            provider: Some(Provider::Codex),
            agent_id: Some("codex".into()),
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: Some(root.id.clone()),
        })
        .await;
    let message = refused.err().expect("past the cap must fail").to_string();
    assert!(
        message.contains("Do not retry"),
        "refusal must tell the agent not to retry, got: {message}"
    );

    // Completed work must not hold a slot, or a long sequential fan-out would wedge.
    s.db.with(|conn| {
        conn.execute(
            "UPDATE threads SET status='completed' WHERE id=?1",
            rusqlite::params![made[0]],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(
        s.create_thread(CreateThreadInput {
            workspace_id: ws,
            title: None,
            provider: Some(Provider::Codex),
            agent_id: Some("codex".into()),
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: Some(root.id),
        })
        .await
        .is_ok(),
        "finishing a thread should free a slot"
    );
}

#[tokio::test]
async fn waiting_peer_mail_is_announced_to_the_agent_and_stays_passive() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let sender = spawn(&s, &ws, None).await;
    let receiver = spawn(&s, &ws, None).await;

    // Without CLI credentials the agent could not act on a notice, so we stay quiet.
    assert_eq!(
        s.pending_mail_notice(&receiver.id),
        None,
        "no notice before the CLI is configured"
    );
    s.configure_cli("http://127.0.0.1:1".into());
    assert_eq!(
        s.pending_mail_notice(&receiver.id),
        None,
        "an empty inbox produces no notice"
    );

    s.send_to_thread(
        &receiver.id,
        SendInput {
            delivery: "inbox".into(),
            notify_delivery: "inbox".into(),
            text: "build finished, artifact at /tmp/out".into(),
            from_thread_id: Some(sender.id.clone()),
            notify_on_complete: false,
            client_request_id: Some("mail-1".into()),
            subject: None,
            kind: None,
            in_reply_to: None,
            interrupt_reason: None,
            topic_key: None,
        },
    )
    .unwrap();

    let notice = s
        .pending_mail_notice(&receiver.id)
        .expect("waiting mail must be announced");
    assert!(notice.contains("1 unread peer message"), "got: {notice}");
    assert!(
        notice.contains("Passive"),
        "the notice must say it is passive so the agent does not abandon its task          or acknowledge mail just to silence the reminder: {notice}"
    );
    assert!(notice.contains("pockymoe inbox"), "got: {notice}");

    // Pluralisation, and the sender's own inbox stays untouched.
    s.send_to_thread(
        &receiver.id,
        SendInput {
            delivery: "inbox".into(),
            notify_delivery: "inbox".into(),
            text: "second".into(),
            from_thread_id: Some(sender.id.clone()),
            notify_on_complete: false,
            client_request_id: Some("mail-2".into()),
            subject: None,
            kind: None,
            in_reply_to: None,
            interrupt_reason: None,
            topic_key: None,
        },
    )
    .unwrap();
    assert!(s
        .pending_mail_notice(&receiver.id)
        .unwrap()
        .contains("2 unread peer messages"));
    assert_eq!(
        s.pending_mail_notice(&sender.id),
        None,
        "sending mail must not announce anything to the sender"
    );
}

fn mail(from: &str, subject: &str, kind: &str, text: &str, key: &str) -> SendInput {
    SendInput {
        delivery: "inbox".into(),
        notify_delivery: "inbox".into(),
        text: text.into(),
        from_thread_id: Some(from.into()),
        notify_on_complete: false,
        client_request_id: Some(key.into()),
        subject: Some(subject.into()),
        kind: Some(kind.into()),
        in_reply_to: None,
        interrupt_reason: None,
        topic_key: None,
    }
}

#[tokio::test]
async fn the_unread_notice_names_what_is_waiting_and_flags_questions() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let sender = spawn(&s, &ws, None).await;
    let receiver = spawn(&s, &ws, None).await;
    s.configure_cli("http://127.0.0.1:1".into());

    s.send_to_thread(
        &receiver.id,
        mail(
            &sender.id,
            "crate builds clean",
            "result",
            "cargo check passed",
            "m1",
        ),
    )
    .unwrap();
    let notice = s.pending_mail_notice(&receiver.id).unwrap();
    assert!(
        notice.contains("result: crate builds clean"),
        "got: {notice}"
    );
    assert!(
        !notice.contains("waiting on you"),
        "a result does not imply anyone is blocked: {notice}"
    );

    s.send_to_thread(
        &receiver.id,
        mail(
            &sender.id,
            "which API key?",
            "question",
            "need the staging key",
            "m2",
        ),
    )
    .unwrap();
    let notice = s.pending_mail_notice(&receiver.id).unwrap();
    assert!(notice.contains("question: which API key?"), "got: {notice}");
    assert!(
        notice.contains("waiting on you"),
        "a question must signal that a peer is blocked: {notice}"
    );
    for n in 0..10 {
        s.send_to_thread(
            &receiver.id,
            mail(
                &sender.id,
                "newer progress",
                "status",
                "running",
                &format!("status-{n}"),
            ),
        )
        .unwrap();
    }
    let notice = s.pending_mail_notice(&receiver.id).unwrap();
    assert!(
        notice.contains("question: which API key?"),
        "new status must not bury a question: {notice}"
    );
    assert!(
        notice.contains("result: crate builds clean"),
        "usable results precede progress: {notice}"
    );
}

#[tokio::test]
async fn messages_without_a_subject_still_summarise_and_bad_kinds_are_refused() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let sender = spawn(&s, &ws, None).await;
    let receiver = spawn(&s, &ws, None).await;
    s.configure_cli("http://127.0.0.1:1".into());

    // Senders that skip --subject, and mail predating the envelope, must still be
    // triageable - fall back to the first non-empty line.
    let mut bare = mail(
        &sender.id,
        "x",
        "status",
        "\n\nDeploy finished on staging\nmore detail",
        "m1",
    );
    bare.subject = None;
    s.send_to_thread(&receiver.id, bare).unwrap();
    let notice = s.pending_mail_notice(&receiver.id).unwrap();
    assert!(
        notice.contains("Deploy finished on staging"),
        "a missing subject should fall back to the first line: {notice}"
    );

    let mut bogus = mail(&sender.id, "s", "urgent", "text", "m2");
    bogus.kind = Some("urgent".into());
    let err = s
        .send_to_thread(&receiver.id, bogus)
        .unwrap_err()
        .to_string();
    assert!(err.contains("kind must be one of"), "got: {err}");

    let mut long = mail(&sender.id, &"x".repeat(200), "status", "text", "m3");
    long.subject = Some("x".repeat(200));
    assert!(
        s.send_to_thread(&receiver.id, long).is_err(),
        "an oversized subject defeats the point of a one-line summary"
    );
}
