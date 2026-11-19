//! History paging cursors, turn selection for exports and publications, and
//! in-thread search over the conversation projection.
use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Provider};
use pockymoe_runtime::{
    fake::FakeRuntime, service::TurnSelection, Database, RuntimeConfig, Supervisor,
};
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::Arc;

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
        enabled_providers: vec![Provider::Claude],
        acp_command: None,
        acp_startup_timeout_ms: 1000,
        fake_runtime: true,
        supervisor_restart_strategy: Default::default(),
    };
    let db = Database::open(&config.database_url).unwrap();
    let state = Arc::new(Supervisor::new(
        config,
        db,
        vec![Arc::new(FakeRuntime::new(Provider::Claude))],
    ));
    (dir, state)
}

async fn thread(s: &Supervisor) -> String {
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
        provider: Some(Provider::Claude),
        agent_id: None,
        model: "ios-e2e-stream".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: None,
    })
    .await
    .unwrap()
    .id
}

fn turn(s: &Supervisor, thread: &str, id: &str, ordinal: i64) {
    s.db.with(|c| {
        c.execute(
            "INSERT INTO thread_turns(id,thread_id,status,ordinal,started_at) VALUES(?1,?2,'completed',?3,'2030-01-01T00:00:00Z')",
            params![id, thread, ordinal],
        )?;
        Ok(())
    })
    .unwrap();
}

fn item(s: &Supervisor, thread: &str, turn: &str, item: &str, kind: &str, text: &str) {
    s.db.with(|c| {
        c.execute(
            "INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,'2030-01-01T00:00:01Z','2030-01-01T00:00:01Z')",
            params![
                format!("{turn}:{item}"),
                thread,
                turn,
                item,
                json!({"id":item,"kind":kind,"text":text}).to_string()
            ],
        )?;
        Ok(())
    })
    .unwrap();
}

/// Five turns, `{prefix}turn-1` to `-5`, each with a prompt, a tool call and a reply.
async fn history(s: &Supervisor, prefix: &str) -> String {
    let id = thread(s).await;
    for n in 1..=5 {
        let t = format!("{prefix}turn-{n}");
        turn(s, &id, &t, n);
        item(s, &id, &t, "prompt", "userMessage", &format!("prompt {n}"));
        item(
            s,
            &id,
            &t,
            "tool",
            "commandExecution",
            &format!("tool output {n}"),
        );
        item(s, &id, &t, "reply", "agentMessage", &format!("answer {n}"));
    }
    id
}

fn ids(turns: &[pockymoe_protocol::ThreadTurnDto]) -> Vec<&str> {
    turns.iter().map(|turn| turn.id.as_str()).collect()
}

#[tokio::test]
async fn a_cursor_outside_the_thread_is_a_conflict_not_the_newest_page() {
    let (_dir, s) = setup();
    let a = history(&s, "").await;
    let b = thread(&s).await;
    turn(&s, &b, "other-turn", 1);
    for summary in [false, true] {
        let page = s
            .get_thread_detail_page(&a, Some(10), Some("turn-3"), summary)
            .await
            .unwrap();
        assert_eq!(ids(&page.turns), ["turn-1", "turn-2"]);
        let page = s
            .get_thread_detail_page(&a, Some(10), Some("turn-1"), summary)
            .await
            .unwrap();
        assert!(page.turns.is_empty(), "the first turn ends the history");
        let error = s
            .get_thread_detail_page(&a, Some(10), Some("other-turn"), summary)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("conflict: "), "{error}");
    }
    // A turn deleted while the client was paging.
    s.db.with(|c| {
        c.execute("DELETE FROM thread_turns WHERE id='turn-3'", [])?;
        Ok(())
    })
    .unwrap();
    for summary in [false, true] {
        let error = s
            .get_thread_detail_page(&a, Some(10), Some("turn-3"), summary)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("conflict: "), "{error}");
    }
}

#[tokio::test]
async fn exports_read_only_the_selected_turns_numbered_in_the_thread() {
    let (_dir, s) = setup();
    let a = history(&s, "").await;
    let b = history(&s, "b-").await;
    let latest = s
        .get_thread_turns(&a, TurnSelection::Latest(2))
        .await
        .unwrap();
    assert_eq!(
        latest
            .turns
            .iter()
            .map(|t| (t.number, t.turn.id.as_str()))
            .collect::<Vec<_>>(),
        [(4, "turn-4"), (5, "turn-5")]
    );
    let texts = latest.turns[1]
        .turn
        .items
        .iter()
        .map(|item| item.text.as_str())
        .collect::<Vec<_>>();
    assert_eq!(texts, ["prompt 5", "tool output 5", "answer 5"]);
    assert_eq!(latest.thread.id, a);

    let wanted = ["turn-4".to_string(), "turn-2".into(), "missing".into()];
    let selected = s
        .get_thread_turns(&a, TurnSelection::Ids(&wanted))
        .await
        .unwrap();
    assert_eq!(
        selected
            .turns
            .iter()
            .map(|t| (t.number, t.turn.id.as_str()))
            .collect::<Vec<_>>(),
        [(2, "turn-2"), (4, "turn-4")]
    );
    // Turn IDs are global; another thread's turns never enter this export.
    let foreign = ["b-turn-5".to_string()];
    let own = s
        .get_thread_turns(&b, TurnSelection::Ids(&foreign))
        .await
        .unwrap();
    assert_eq!(own.turns[0].number, 5);
    assert!(s
        .get_thread_turns(&a, TurnSelection::Ids(&foreign))
        .await
        .unwrap()
        .turns
        .is_empty());
    assert_eq!(
        s.get_thread_turns(&a, TurnSelection::Latest(100))
            .await
            .unwrap()
            .turns
            .len(),
        5
    );

    let options = s.export_turn_options(&a).unwrap();
    assert_eq!(options.len(), 5);
    assert_eq!(options[2].number, 3);
    assert_eq!(options[2].id, "turn-3");
    assert_eq!(options[2].user_prompt_preview, "prompt 3");
    turn(&s, &a, "empty-turn", 6);
    let options = s.export_turn_options(&a).unwrap();
    assert_eq!(options.len(), 6);
    assert_eq!(options[5].user_prompt_preview, "");
}

#[tokio::test]
async fn the_export_picker_shows_the_visible_request_of_an_imported_turn() {
    let (_dir, s) = setup();
    let a = thread(&s).await;
    turn(&s, &a, "import-turn", 1);
    item(
        &s,
        &a,
        "import-turn",
        "context",
        "userMessage",
        "# AGENTS.md instructions\nhidden context",
    );
    item(
        &s,
        &a,
        "import-turn",
        "request",
        "userMessage",
        "visible request",
    );
    s.db.with(|c| {
        c.execute(
            "UPDATE threads SET source='local_codex_import' WHERE id=?1",
            [&a],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.export_turn_options(&a).unwrap()[0].user_prompt_preview,
        "visible request"
    );
}

fn found(s: &Supervisor, thread: &str, q: &str, limit: usize) -> Vec<(String, String)> {
    s.search_thread_messages(thread, q, limit).unwrap()["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["turnId"].as_str().unwrap().to_owned(),
                m["itemId"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(t, i)| (t.to_string(), i.to_string()))
        .collect()
}

#[tokio::test]
async fn thread_search_reads_the_projection_in_transcript_order() {
    let (_dir, s) = setup();
    let a = history(&s, "").await;
    let b = history(&s, "b-").await;
    item(
        &s,
        &a,
        "turn-2",
        "extra",
        "agentMessage",
        "第二轮 中文 answer",
    );
    // Newest turn first, items in the order they were written.
    assert_eq!(
        found(&s, &a, "answer", 100),
        pairs(&[
            ("turn-5", "reply"),
            ("turn-4", "reply"),
            ("turn-3", "reply"),
            ("turn-2", "reply"),
            ("turn-2", "extra"),
            ("turn-1", "reply"),
        ])
    );
    // A streaming update moves the projection row, not the item's place.
    s.db.with(|c| {
        c.execute(
            "UPDATE thread_history_items SET item_json=?1 WHERE id='turn-2:reply'",
            [json!({"id":"reply","kind":"agentMessage","text":"answer 2 updated"}).to_string()],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        found(&s, &a, "answer", 100)[3..5],
        pairs(&[("turn-2", "reply"), ("turn-2", "extra")])
    );
    // Short and non-Latin queries skip FTS but keep literal matching.
    assert_eq!(found(&s, &a, "中", 100), pairs(&[("turn-2", "extra")]));
    assert_eq!(found(&s, &a, "2 UPD", 100), pairs(&[("turn-2", "reply")]));
    // Tool output is never searched; other threads stay out.
    assert!(found(&s, &a, "tool output", 100).is_empty());
    assert_eq!(found(&s, &b, "中文", 100), pairs(&[]));
    let page = s.search_thread_messages(&a, "prompt", 2).unwrap();
    assert_eq!(page["matches"].as_array().unwrap().len(), 2);
    assert_eq!(page["hasMore"], true);
    assert_eq!(page["matches"][0]["role"], "You");
    assert_eq!(page["matches"][0]["text"], "prompt 5");
    assert_eq!(page["matches"][0]["createdAt"], "2030-01-01T00:00:01Z");
    assert_eq!(
        s.search_thread_messages(&a, "prompt", 5).unwrap()["hasMore"],
        false
    );
    s.db.with(|c| {
        c.execute("DELETE FROM thread_turns WHERE id='turn-5'", [])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(found(&s, &a, "answer", 1), pairs(&[("turn-4", "reply")]));
    for (q, limit) in [("", 1), ("x", 0), ("x", 101)] {
        assert!(s.search_thread_messages(&a, q, limit).is_err());
    }
}

#[tokio::test]
async fn thread_search_hides_injected_context_and_imported_duplicates() {
    let (_dir, s) = setup();
    let a = thread(&s).await;
    turn(&s, &a, "import-turn", 1);
    item(
        &s,
        &a,
        "import-turn",
        "injection",
        "userMessage",
        "# AGENTS.md instructions\nhidden context",
    );
    item(
        &s,
        &a,
        "import-turn",
        "request-first",
        "userMessage",
        "# AGENTS.md instructions\nhidden context\n## My request:\nvisible request",
    );
    item(
        &s,
        &a,
        "import-turn",
        "request-second",
        "userMessage",
        "visible request ",
    );
    item(
        &s,
        &a,
        "import-turn",
        "reply",
        "agentMessage",
        "visible answer",
    );
    item(
        &s,
        &a,
        "import-turn",
        "reply-again",
        "agentMessage",
        "visible answer",
    );
    s.db.with(|c| {
        c.execute(
            "UPDATE threads SET source='local_codex_import' WHERE id=?1",
            [&a],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(found(&s, &a, "hidden context", 100).is_empty());
    assert_eq!(
        found(&s, &a, "visible", 100),
        pairs(&[("import-turn", "request-first"), ("import-turn", "reply")])
    );
    let first: Value = s.search_thread_messages(&a, "request", 100).unwrap();
    assert_eq!(first["matches"][0]["text"], "visible request");
}
