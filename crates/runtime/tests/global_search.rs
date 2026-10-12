//! Device-local search indexing and scope regression.
use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Provider};
use pockymoe_runtime::{fake::FakeRuntime, Database, RuntimeConfig, Supervisor};
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
        relay_server_url: None,
        web_origin: None,
        relay_agent_token: None,
        enabled_providers: vec![Provider::Claude],
        acp_command: None,
        acp_startup_timeout_ms: 1000,
        fake_runtime: true,
    };
    let db = Database::open(&config.database_url).unwrap();
    let state = Arc::new(Supervisor::new(
        config,
        db,
        vec![Arc::new(FakeRuntime::new(Provider::Claude))],
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
        provider: Some(Provider::Claude),
        agent_id: None,
        model: "ios-e2e-stream".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: parent.map(str::to_owned),
    })
    .await
    .unwrap()
}

fn insert_message(s: &Supervisor, thread: &str, turn: &str, item: &str, kind: &str, text: &str) {
    s.db.with(|conn| {
        conn.execute("INSERT OR IGNORE INTO thread_turns(id,thread_id,status,ordinal) VALUES(?1,?2,'completed',1)", (turn, thread))?;
        conn.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at)
            VALUES(?1,?2,?3,?1,?4,'2030-01-01T00:00:00Z','2030-01-01T00:00:00Z')",
            (item, thread, turn, json!({"id":item,"kind":kind,"text":text}).to_string()))?;
        Ok(())
    }).unwrap();
}

#[tokio::test]
async fn global_search_indexes_literal_unicode_updates_forks_and_deletions() {
    let (dir, s) = setup();
    let ws = workspace(&s);
    let other_path = dir.path().join("other");
    std::fs::create_dir(&other_path).unwrap();
    let other_ws = s
        .create_workspace(CreateWorkspaceInput {
            abs_path: Some(other_path.to_string_lossy().into()),
            git_url: None,
            label: Some("other".into()),
        })
        .unwrap();
    let first = spawn(&s, &ws, None).await;
    let other = spawn(&s, &other_ws.id, None).await;
    s.rename_thread(&first.id, "搜索 中文标题").unwrap();
    insert_message(
        &s,
        &first.id,
        "first-turn",
        "first-message",
        "agentMessage",
        "🙂 İSTANBUL 中文全局搜索 100% _ quotes \"a\" OR x",
    );
    insert_message(
        &s,
        &other.id,
        "other-turn",
        "other-message",
        "userMessage",
        "中文全局搜索 other",
    );
    insert_message(
        &s,
        &first.id,
        "first-turn",
        "tool-secret",
        "commandExecution",
        "private tool payload",
    );
    for q in [
        "中",
        "中文",
        "中文全局",
        "🙂",
        "100% _",
        "\"a\"",
        "OR x",
        "i\u{307}stanbul",
    ] {
        let found = s.search_conversations(q, Some(&ws), 50, 0).unwrap();
        assert!(
            !found["matches"].as_array().unwrap().is_empty(),
            "query: {q}: {found}"
        );
        assert!(found["matches"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["threadId"] == first.id));
    }
    assert_eq!(
        s.search_conversations("private tool", None, 50, 0).unwrap()["matches"],
        json!([])
    );
    let page = s.search_conversations("中文全局", None, 1, 0).unwrap();
    assert_eq!(page["matches"].as_array().unwrap().len(), 1);
    assert_eq!(page["nextOffset"], 1);
    let next = s.search_conversations("中文全局", None, 1, 1).unwrap();
    assert_ne!(
        page["matches"][0]["threadId"],
        next["matches"][0]["threadId"]
    );
    assert_eq!(next["hasMore"], false);
    s.db.with(|conn| {
        conn.execute(
            "UPDATE thread_history_items SET item_json=?1 WHERE id='first-message'",
            [
                json!({"id":"first-message","kind":"agentMessage","text":"updated streaming 文本"})
                    .to_string(),
            ],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.search_conversations("中文全局", Some(&ws), 50, 0)
            .unwrap()["matches"],
        json!([])
    );
    assert_eq!(
        s.search_conversations("streaming", Some(&ws), 50, 0)
            .unwrap()["matches"][0]["itemId"],
        "first-message"
    );
    // Use the production fork path; copied history is indexed without a rebuild.
    let fork = s.fork_thread(&first.id).await.unwrap();
    let forked = s
        .search_conversations("streaming", Some(&ws), 50, 0)
        .unwrap();
    assert_eq!(forked["matches"].as_array().unwrap().len(), 2, "{forked}");
    let destination = forked["matches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["threadId"] == fork.id)
        .unwrap();
    assert_ne!(destination["turnId"], "first-turn");
    s.delete_thread(&fork.id).unwrap();
    s.db.with(|conn| {
        conn.execute(
            "DELETE FROM thread_history_items WHERE id='first-message'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.search_conversations("streaming", None, 50, 0).unwrap()["matches"],
        json!([])
    );
    s.rename_thread(&first.id, "Renamed").unwrap();
    assert_eq!(
        s.search_conversations("中文标题", None, 50, 0).unwrap()["matches"],
        json!([])
    );
    s.db.with(|conn| {
        conn.execute("INSERT INTO search_documents_fts(search_documents_fts, rank) VALUES('integrity-check',1)", [])?;
        Ok(())
    }).unwrap();
    for (q, limit, offset) in [("", 1, 0), ("x", 0, 0), ("x", 101, 0), ("x", 1, 10001)] {
        assert!(s.search_conversations(q, None, limit, offset).is_err());
    }
}

#[tokio::test]
async fn global_search_imports_only_visible_requests_and_deduplicates() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let thread = spawn(&s, &ws, None).await;
    insert_message(
        &s,
        &thread.id,
        "import-turn",
        "injection",
        "userMessage",
        "# AGENTS.md instructions\nhidden context",
    );
    insert_message(
        &s,
        &thread.id,
        "import-turn",
        "request-first",
        "userMessage",
        "# AGENTS.md instructions\nhidden context\n## My request:\nvisible request",
    );
    insert_message(
        &s,
        &thread.id,
        "import-turn",
        "request-second",
        "userMessage",
        "visible request",
    );
    s.db.with(|conn| {
        conn.execute(
            "UPDATE threads SET source='local_codex_import' WHERE id=?1",
            [&thread.id],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.search_conversations("hidden context", None, 50, 0)
            .unwrap()["matches"],
        json!([])
    );
    let found = s
        .search_conversations("visible request", None, 50, 0)
        .unwrap();
    assert_eq!(found["matches"].as_array().unwrap().len(), 1);
    assert_eq!(found["matches"][0]["itemId"], "request-first");
    s.db.with(|conn| {
        conn.execute("DELETE FROM thread_turns WHERE id='import-turn'", [])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.search_conversations("visible request", None, 50, 0)
            .unwrap()["matches"],
        json!([])
    );
}
