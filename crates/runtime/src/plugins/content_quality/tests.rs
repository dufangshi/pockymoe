use super::*;
use std::{
    net::TcpListener,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

fn settings(value: Value) -> Settings {
    Settings::parse(&value).unwrap()
}
#[test]
fn typed_settings_and_inputs_reject_unknown_or_invalid_values() {
    let p = ContentQualityPlugin;
    assert!(!p.enabled_by_default());
    assert!(p.validate_settings(&p.default_settings()).is_ok());
    for bad in [
        json!({"apiKey":"not-allowed"}),
        json!({"preset":"invented"}),
        json!({"documentMode":"write"}),
        json!({"minReplyChars":-1}),
        json!({"minReplyChars":1.2}),
        json!({"autoReplies":"yes"}),
        json!({"language":""}),
    ] {
        assert!(p.validate_settings(&bad).is_err());
    }
    assert!(p
        .validate_action("testConnection", &json!({"apiKey":"secret"}))
        .is_err());
    assert!(p
        .validate_action("rewriteReply", &json!({"turnId":"t","reasoning":"private"}))
        .is_err());
    assert!(p
        .validate_action(
            "reviewDocument",
            &json!({"path":"readme.md","mode":"rewrite"})
        )
        .is_ok());
    assert_eq!(settings(json!({})).min_reply_chars, 800);
}
#[test]
fn final_reply_prefers_final_phase_and_falls_back_to_unknown_phases() {
    let values = json!([{"kind":"agentMessage","phase":"final_answer","text":"final"},{"kind":"reasoning","text":"private"},{"kind":"agentMessage","phase":"commentary","text":"after"}]);
    assert_eq!(
        final_reply(values.as_array().unwrap()).unwrap()["text"],
        "final"
    );
    let values = json!([{"kind":"agentMessage","phase":"future","text":"new"},{"kind":"reasoning","text":"private"},{"kind":"agentMessage","text":" "}]);
    assert_eq!(
        final_reply(values.as_array().unwrap()).unwrap()["text"],
        "new"
    );
    assert!(!terminal("recovering"));
    assert!(!terminal("unknown"));
    assert!(terminal("failed"));
}

#[test]
fn final_reply_private_analysis_does_not_hide_public_unknown_phase_fallback() {
    let items = json!([
        {"id":"public","kind":"agentMessage","phase":"future","text":"Public answer"},
        {"id":"private-channel","kind":"agentMessage","channel":"analysis","text":"Private analysis"},
        {"id":"private-phase","kind":"agentMessage","phase":"analysis","text":"Private analysis"}
    ]);
    assert_eq!(
        final_reply(items.as_array().unwrap()).unwrap()["id"],
        "public"
    );
    assert!(final_reply(&items.as_array().unwrap()[1..]).is_none());
}

#[test]
fn final_reply_private_final_does_not_hide_public_final() {
    let items = json!([
        {"id":"public","kind":"agentMessage","responsePhase":"final","text":"Public answer"},
        {"id":"private-channel","kind":"agentMessage","channel":"analysis","phase":"final","text":"Private final"},
        {"id":"private-phase","kind":"agentMessage","phase":"private","responsePhase":"final","text":"Private final"},
        {"id":"private-role","kind":"agentMessage","role":"developer","phase":"final_answer","text":"Private final"},
        {"id":"private-origin","kind":"agentMessage","origin":{"kind":"systemInjection"},"phase":"final","text":"Private final"}
    ]);
    assert_eq!(
        final_reply(items.as_array().unwrap()).unwrap()["id"],
        "public"
    );
    assert!(final_reply(&items.as_array().unwrap()[1..]).is_none());
}

#[tokio::test]
async fn final_reply_candidates_agree_with_public_summary_visibility_and_precedence() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    reply_thread(&s);
    let items = json!([
        {"id":"public","kind":"agentMessage","phase":"future","text":"Public answer"},
        {"id":"channel","kind":"agentMessage","channel":"internal","text":"Hidden"},
        {"id":"phase","kind":"agentMessage","phase":"reasoning","responsePhase":"final","text":"Hidden"},
        {"id":"response","kind":"agentMessage","phase":null,"responsePhase":"private","text":"Hidden"},
        {"id":"status","kind":"agentMessage","status":"analysis","text":"Hidden"},
        {"id":"role","kind":"agentMessage","role":"tool","text":"Hidden"},
        {"id":"origin","kind":"agentMessage","origin":"automation","text":"Hidden"},
        {"id":"origin-kind","kind":"agentMessage","origin":{"kind":"nativeBackgroundWait"},"text":"Hidden"},
        {"id":"public-phase","kind":"agentMessage","phase":"future","responsePhase":"analysis","status":"private","text":"Public phase takes precedence"},
        {"id":"public-empty-phase","kind":"agentMessage","phase":"","responsePhase":"analysis","text":"Empty phase takes precedence"},
        {"id":"public-nonstring-phase","kind":"agentMessage","phase":0,"responsePhase":"analysis","text":"Non-null phase takes precedence"}
    ]);
    s.db.with(|c| {
        c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('turn','thread','completed',0)", [])?;
        for item in items.as_array().unwrap() {
            c.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,'thread','turn',?1,?2,'now','now')", params![item["id"].as_str().unwrap(),item.to_string()])?;
        }
        Ok(())
    }).unwrap();
    for item in items.as_array().unwrap() {
        let candidate = final_reply(std::slice::from_ref(item));
        let summary = s.thread_summary_for_reply("thread", "turn", item["id"].as_str().unwrap());
        assert_eq!(candidate.is_some(), summary.is_ok(), "{}", item["id"]);
        if let Ok((summary, target)) = summary {
            assert_eq!(
                summary.messages[target].text,
                item["text"].as_str().unwrap()
            );
        }
    }
}

#[test]
fn protected_content_roundtrips_and_rejects_missing_reordered_or_duplicate_markers() {
    let text = "---\ntitle: Keep 42\n---\nOriginal prose `cmd --flag` and [link](./docs/v2.md).\n```rust\nlet n = 3;\n```\nhttps://example.com/a?q=2\n";
    let p = Protected::new(text).unwrap();
    assert!(!p.masked.contains("let n"));
    assert!(!p.masked.contains("title:"));
    assert_eq!(p.restore(&p.masked).unwrap(), text);
    assert_eq!(
        p.restore(&p.masked.replace("Original prose", "Clear prose"))
            .unwrap(),
        text.replace("Original prose", "Clear prose")
    );
    assert!(p.restore(&p.masked.replace(&p.marker(0), "")).is_err());
    assert!(p.restore(&format!("{}{}", p.masked, p.marker(0))).is_err());
    let swapped = p
        .masked
        .replace(&p.marker(0), "SWAP")
        .replace(&p.marker(1), &p.marker(0))
        .replace("SWAP", &p.marker(1));
    assert!(p.restore(&swapped).is_err());
    assert!(Protected::new("```rust\nunclosed").is_err());
}
#[test]
fn numeric_and_technical_facts_are_checked_as_multisets() {
    assert!(preserve_facts(
        "Use HTTP 42 with sourceHash.",
        "Use sourceHash with HTTP 42."
    )
    .is_ok());
    for changed in [
        "Use HTTP 43 with sourceHash.",
        "Use HTTP 42 with source_hash.",
        "Use HTTP 42 with sourceHash twice 42.",
    ] {
        assert!(preserve_facts("Use HTTP 42 with sourceHash.", changed).is_err());
    }
}
#[test]
fn document_path_restrictions_and_optimistic_atomic_write() {
    let dir = tempfile::tempdir().unwrap();
    for path in [
        "/etc/passwd",
        "../a.md",
        "a/../a.md",
        "AGENTS.md",
        "nested/SKILL.md",
        "vendor/a.md",
        "target/a.txt",
        ".local/a.md",
        "main.rs",
    ] {
        assert!(document::resolve(dir.path(), path).is_err(), "{path}");
    }
    let path = dir.path().join("guide.md");
    std::fs::write(&path, "Original prose 42").unwrap();
    let resolved = document::resolve(dir.path(), "guide.md").unwrap();
    let original_hash = hash(&document::read(&resolved).unwrap());
    document::atomic_write(&resolved, &original_hash, "Clear prose 42").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Clear prose 42");
    assert!(document::atomic_write(&resolved, &original_hash, "Overwrite edits").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Clear prose 42");
    std::fs::write(&path, b"\xff\xfe").unwrap();
    assert!(document::read(&path).is_err());
    std::fs::write(&path, "x".repeat(SOURCE_LIMIT + 1)).unwrap();
    assert!(document::read(&path).is_err());
    std::fs::write(&path, "<!-- AUTO-GENERATED -->").unwrap();
    assert!(document::read(&path).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/tmp", dir.path().join("escape")).unwrap();
        assert!(document::resolve(dir.path(), "escape/guide.md").is_err());
        std::os::unix::fs::symlink(&path, dir.path().join("alias.md")).unwrap();
        assert!(document::resolve(dir.path(), "alias.md").is_err());
    }
    assert!(document::unsupported("plain", "a.mdx"));
    assert!(document::unsupported("<Component />", "a.md"));
    assert!(document::unsupported(".. code::", "a.rst"));
}

// A real HTTP socket verifies the OpenAI-compatible wire shape; no network or credential fixtures.
fn mock(
    status: u16,
    respond: impl Fn(Value) -> Value + Send + 'static,
) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let handle = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut data = Vec::new();
        let header_end;
        loop {
            let mut one = [0];
            socket.read_exact(&mut one).unwrap();
            data.push(one[0]);
            if data.ends_with(b"\r\n\r\n") {
                header_end = data.len();
                break;
            }
        }
        let headers = String::from_utf8_lossy(&data).to_ascii_lowercase();
        assert!(headers.starts_with("post /v1/chat/completions "));
        assert!(headers.contains("authorization: bearer mock-credential"));
        let len: usize = headers
            .lines()
            .find_map(|l| l.strip_prefix("content-length:").map(str::trim))
            .unwrap()
            .parse()
            .unwrap();
        data.resize(header_end + len, 0);
        socket.read_exact(&mut data[header_end..]).unwrap();
        let body: Value = serde_json::from_slice(&data[header_end..]).unwrap();
        assert!(!body.to_string().contains("mock-credential"));
        assert_eq!(body["model"], "mock-model");
        seen.fetch_add(1, Ordering::SeqCst);
        let response = respond(body).to_string();
        write!(socket, "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
    });
    (base, count, handle)
}
fn connection(dir: &Path, base: &str) -> PathBuf {
    let path = dir.join("connection.json");
    std::fs::write(
        &path,
        json!({"baseUrl":base,"apiKey":"mock-credential","model":"mock-model"}).to_string(),
    )
    .unwrap();
    path
}
fn completion(text: &str) -> Value {
    json!({"choices":[{"finish_reason":"stop","message":{"content":text}}]})
}
#[tokio::test]
async fn upstream_validates_wire_shape_and_redacts_errors_and_echoed_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let (base, count, handle) = mock(200, |_| completion("OK"));
    let path = connection(dir.path(), &base);
    let u = Upstream::from_file(&path, "").unwrap();
    assert_eq!(u.complete("system", "user", 32).await.unwrap(), "OK");
    handle.join().unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(u
        .complete("system", "mock-credential", 32)
        .await
        .unwrap_err()
        .to_string()
        .contains("source contains"));
    let (base, _, handle) = mock(
        401,
        |_| json!({"error":"mock-credential raw upstream private detail"}),
    );
    let u = Upstream::from_file(&connection(dir.path(), &base), "").unwrap();
    let error = u
        .complete("system", "user", 32)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(error, "content quality upstream returned HTTP 401");
    handle.join().unwrap();
    let (base, _, handle) = mock(200, |_| completion("mock-credential"));
    let u = Upstream::from_file(&connection(dir.path(), &base), "").unwrap();
    assert_eq!(
        u.complete("system", "user", 32)
            .await
            .unwrap_err()
            .to_string(),
        "upstream returned sensitive content"
    );
    handle.join().unwrap();
}
fn supervisor(dir: &Path) -> Arc<Supervisor> {
    let config = crate::RuntimeConfig {
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
        enabled_providers: vec![],
        acp_command: None,
        acp_startup_timeout_ms: 1000,
        fake_runtime: true,
        supervisor_restart_strategy: Default::default(),
    };
    let db = crate::db::Database::open(&config.database_url).unwrap();
    Arc::new(Supervisor::new(config, db, vec![]))
}
fn context(s: Arc<Supervisor>, settings: Value, input: Value) -> PluginActionContext {
    PluginActionContext {
        supervisor: s,
        run_id: "run".into(),
        thread_id: "thread".into(),
        settings: super::super::PluginSettings {
            revision: 1,
            values: settings,
        },
        input,
        event: json!({}),
    }
}
#[tokio::test]
async fn rewrite_persists_only_metadata_emits_live_event_and_skips_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    reply_thread(&s);
    let (base, count, handle) = mock(200, |body| {
        let request: Value =
            serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert!(!request.to_string().contains("PRIVATE_REASONING"));
        assert!(!request.to_string().contains("PRIVATE_COMMAND_OUTPUT"));
        assert_eq!(request["turnStatus"], "failed");
        completion(&json!({"text":request["messages"][request["target"].as_u64().unwrap() as usize]["text"].as_str().unwrap().replace("Original prose", "Clear prose"),"findings":["Clearer wording; limitations retained."]}).to_string())
    });
    let path = connection(dir.path(), &base);
    let items = [
        json!({"id":"u","kind":"userMessage","text":"Explain the failed attempt"}),
        json!({"id":"r","kind":"reasoning","text":"PRIVATE_REASONING"}),
        json!({"id":"c","kind":"commandExecution","text":"PRIVATE_COMMAND_OUTPUT","status":"failed"}),
        json!({"id":"a","kind":"agentMessage","phase":"future_phase","text":"Original prose: failed, 42 tasks remain. `cargo check`"}),
    ];
    s.db.with(|c| { c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('turn','thread','failed',0)", [])?; for item in &items { c.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,'thread','turn',?1,?2,'now','now')", params![item["id"].as_str().unwrap(),item.to_string()])?; } Ok(()) }).unwrap();
    let settings = json!({"connectionFile":path,"minReplyChars":0});
    let mut events = s.bus.subscribe();
    let result = ContentQualityPlugin
        .invoke(
            "rewriteReply",
            context(s.clone(), settings.clone(), json!({"turnId":"turn"})),
        )
        .await
        .unwrap();
    assert_eq!(result["status"], "completed");
    handle.join().unwrap();
    let event = events.recv().await.unwrap();
    assert_eq!(event.event_type.as_str(), "thread.item.completed");
    assert_eq!(event.payload["item"]["text"], items[3]["text"]);
    assert!(event.payload["item"]["polishedReply"]["text"]
        .as_str()
        .unwrap()
        .starts_with("Clear prose"));
    let result = ContentQualityPlugin
        .invoke(
            "rewriteReply",
            context(s.clone(), settings, json!({"turnId":"turn"})),
        )
        .await
        .unwrap();
    assert_eq!(result["reason"], "alreadyPolished");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    s.db.with(|c| {
        c.execute("UPDATE thread_turns SET status='inProgress'", [])?;
        Ok(())
    })
    .unwrap();
    assert!(ContentQualityPlugin
        .invoke(
            "rewriteReply",
            context(s, json!({"minReplyChars":0}), json!({"turnId":"turn"}))
        )
        .await
        .is_err());
}

fn reply_thread(s: &Supervisor) {
    s.db.with(|c| {
        c.execute("INSERT INTO workspaces(id,host_id,label,abs_path,created_at) VALUES('workspace','host','fixture',?1,'now')", [s.config.workspace_root.to_string_lossy().as_ref()])?;
        c.execute("INSERT INTO threads(id,workspace_id,provider,title,created_at,updated_at,status) VALUES('thread','workspace','codex','fixture','now','now','idle')", [])?;
        Ok(())
    }).unwrap();
}

#[tokio::test]
async fn reader_rewrite_can_remove_process_code_links_and_terms_or_add_an_analogy() {
    for (reader, original, rewritten) in [
        (
            "只要结果，不要过程。",
            "# Work log\nRan 17 HTTP checks using sourceHash and opened README.md.\n`cargo check`\n[Docs](https://example.test/docs)\n| Check | Count |\n| done | 17 |\nThe deployment failed; the site is not live.",
            "**还没上线。** 部署失败，需要修好后再试。",
        ),
        (
            "纯新手，解释陌生概念，可以打比方。",
            "The update now uses atomic replacement.\n```sh\nexample of an unfinished code fence",
            "更新现在会先准备好新文件，再一次换掉旧文件。\n\n可以把它想成换门牌：先把新门牌做好，最后直接替换，避免让别人看见只写到一半的门牌。",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let s = supervisor(dir.path());
        reply_thread(&s);
        let (base, count, server) = mock(200, move |body| {
            let system = body["messages"][0]["content"].as_str().unwrap();
            assert_eq!(system, reply_instruction());
            let request: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
            let target = request["target"].as_u64().unwrap() as usize;
            assert_eq!(request["messages"][target]["text"], original);
            assert_eq!(request["preferences"]["readerBackground"], reader);
            assert!(request.get("source").is_none(), "do not duplicate the target");
            assert!(request.get("publicOperations").is_none());
            completion(&json!({"text":rewritten,"findings":[]}).to_string())
        });
        s.db.with(|c| {
            c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('turn','thread','completed',0)", [])?;
            let item = json!({"id":"answer","kind":"agentMessage","phase":"final_answer","text":original,"polishedReply":{"requestHash":"old-copyedit-policy","text":"old lightly edited answer"}});
            c.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES('answer','thread','turn','answer',?1,'now','now')", [item.to_string()])?;
            Ok(())
        }).unwrap();
        let result = ContentQualityPlugin.invoke("rewriteReply", context(
            s.clone(),
            json!({"connectionFile":connection(dir.path(), &base),"minReplyChars":0,"readerBackground":reader}),
            json!({"turnId":"turn"}),
        )).await.unwrap();
        server.join().unwrap();
        assert_eq!(result["status"], "completed");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let stored = stored_turn(&s, "thread", "turn").unwrap();
        assert_eq!(stored.items[0]["text"], original);
        assert_eq!(stored.items[0]["polishedReply"]["text"], rewritten);
    }
}

#[tokio::test]
async fn reply_request_redacts_other_saved_connection_opaque_key_from_history() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    reply_thread(&s);
    let active = "fixtureActiveConnectionCredential723508";
    let historical = "fixtureOpaqueConnectionCredential907241";
    let directory = crate::upstreams::directory(&s.config.database_url);
    for (name, key) in [
        ("Active fixture", active),
        ("Historical fixture", historical),
    ] {
        let connection = serde_json::from_value(json!({
            "name":name,"protocol":"openai_chat","baseUrl":"https://example.test/v1",
            "apiKey":key,"model":"fixture-model"
        }))
        .unwrap();
        crate::upstreams::connections::upsert(&directory, connection).unwrap();
    }
    let original = "The check failed; no deployment happened.";
    s.db.with(|c| {
        c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('old','thread','completed',0),('turn','thread','completed',1)", [])?;
        for (turn, item) in [
            ("old", json!({"id":"old:user","kind":"userMessage","text":format!("Old connection key: {historical}"),"createdAt":"2030-01-01T00:00:00Z"})),
            ("old", json!({"id":"old:reply","kind":"agentMessage","text":"Already checked.","createdAt":"2030-01-01T00:00:01Z"})),
            ("turn", json!({"id":"turn:user","kind":"userMessage","text":"Summarize the result.","createdAt":"2030-01-01T00:00:02Z"})),
            ("turn", json!({"id":"answer","kind":"agentMessage","phase":"final","text":original,"createdAt":"2030-01-01T00:00:03Z"})),
        ] {
            c.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,'thread',?2,?1,?3,'now','now')", params![item["id"].as_str().unwrap(),turn,item.to_string()])?;
        }
        Ok(())
    }).unwrap();
    let request = reply::request(
        &s,
        "thread",
        "turn",
        "answer",
        "completed",
        &settings(json!({"readerBackground":format!("Reader note: {historical}")})),
        active,
    )
    .unwrap();
    let payload = request.to_string();
    assert!(!contains_credential(&payload, historical));
    assert!(!contains_credential(&payload, active));
    assert_eq!(request["messages"].as_array().unwrap().len(), 4);
    assert_eq!(request["target"], 3);
    assert_eq!(
        request["messages"][0]["text"],
        "Old connection key: [credential omitted]"
    );
    assert_eq!(request["messages"][3]["text"], original);
    assert_eq!(
        request["preferences"]["readerBackground"],
        "Reader note: [credential omitted]"
    );
    assert!(request.get("source").is_none());
    // Sanitizing the outgoing request never changes the saved public history.
    let (summary, target) = s
        .thread_summary_for_reply("thread", "turn", "answer")
        .unwrap();
    assert!(summary.messages[0].text.contains(historical));
    assert_eq!(summary.messages[target].text, original);
}

// Explicitly opt in to the user's private connection file; prints no responses or settings.
#[tokio::test]
#[ignore = "requires authorized private upstream and network"]
async fn live_connection_and_editorial_protocol() {
    let path = std::env::var_os("POCKYMOE_CONTENT_QUALITY_LIVE_CONNECTION")
        .expect("set a connection file path");
    let model = std::env::var("POCKYMOE_CONTENT_QUALITY_LIVE_MODEL").unwrap_or_default();
    let u = Upstream::from_file(Path::new(&path), &model).unwrap();
    // The private file supplies fixture credentials only. Exercise the saved
    // connection transport used by the current UI, without applying a harness.
    // Legacy file transport remains covered by deterministic HTTP fixtures.
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    let c: crate::upstreams::connections::Connection = serde_json::from_value(json!({
        "name":"Private live fixture", "protocol":"openai_chat",
        "baseUrl":u.endpoint.as_str(), "apiKey":u.api_key, "model":u.model
    }))
    .unwrap();
    let saved = crate::upstreams::connections::upsert(
        &crate::upstreams::directory(&s.config.database_url),
        c,
    )
    .unwrap();
    let u = Upstream::load(&s, &settings(json!({"connectionId":saved["id"]}))).unwrap();
    assert!(!u
        .complete(
            "Reply only OK.",
            "Connectivity check.",
            crate::upstreams::connections::CONNECTION_PROBE_MAX_TOKENS,
        )
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        crate::upstreams::inventory(&crate::upstreams::directory(&s.config.database_url)).unwrap()
            ["active"],
        json!({})
    );
    let original = "Original prose: the task failed. There are 42 unfinished checks. `cargo check` was not run. https://example.com/docs";
    let p = Protected::new(original).unwrap();
    let result = edited(
        &u.complete(
            &document_instruction(&settings(json!({"language":"en","preset":"copyedit"}))),
            &json!({"turnStatus":"failed","source":p.masked}).to_string(),
            4000,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let revised = p.restore(&result.text).unwrap();
    preserve_facts(original, &revised).unwrap();
}

// Diagnostic seam: the caller supplies an isolated database snapshot, never the
// live database. Only sanitized request text and public editorial output are saved.
#[tokio::test]
#[ignore = "requires an isolated thread snapshot and authorized private upstream"]
async fn live_reader_rewrite_context_and_input_usage() {
    let snapshot = PathBuf::from(
        std::env::var_os("POCKYMOE_CONTENT_QUALITY_SNAPSHOT").expect("isolated snapshot path"),
    );
    let output = PathBuf::from(
        std::env::var_os("POCKYMOE_CONTENT_QUALITY_OUTPUT").expect("diagnostic output directory"),
    );
    let thread = std::env::var("POCKYMOE_CONTENT_QUALITY_THREAD").expect("thread id");
    let turn = std::env::var("POCKYMOE_CONTENT_QUALITY_TURN").expect("target turn id");
    let path = std::env::var_os("POCKYMOE_CONTENT_QUALITY_LIVE_CONNECTION")
        .expect("private connection path");
    let connections_directory = crate::upstreams::directory(&snapshot);
    assert!(
        connections_directory.join("connections.json").is_file(),
        "isolated snapshot requires the complete private connection registry"
    );
    let isolated = tempfile::tempdir().unwrap();
    let mut config = supervisor(isolated.path()).config.clone();
    config.database_url = snapshot;
    let db = crate::db::Database::open(&config.database_url).unwrap();
    let s = Supervisor::new(config, db, vec![]);
    let mut preferences = Settings::parse(&s.plugin_settings(PLUGIN_ID).unwrap().values).unwrap();
    if let Ok(reader) = std::env::var("POCKYMOE_CONTENT_QUALITY_READER_OVERRIDE") {
        preferences.reader_background = reader;
    }
    let upstream = Upstream::from_file(Path::new(&path), &preferences.model).unwrap();
    let original = stored_turn(&s, &thread, &turn).unwrap();
    let item = final_reply(&original.items).unwrap();
    let input = reply::request(
        &s,
        &thread,
        &turn,
        item["id"].as_str().unwrap(),
        &original.status,
        &preferences,
        &upstream.api_key,
    )
    .unwrap();
    let system = reply_instruction();
    let user = input.to_string();
    assert!(!contains_credential(system, &upstream.api_key));
    assert!(!contains_credential(&user, &upstream.api_key));
    for metadata in crate::upstreams::connections::metadata(&connections_directory)
        .expect("read isolated connection metadata")
        .as_array()
        .expect("isolated connection metadata must be an array")
    {
        let connection = crate::upstreams::connections::get(
            &connections_directory,
            metadata["id"].as_str().expect("isolated connection id"),
        )
        .expect("read isolated connection credential");
        if !connection.api_key.is_empty() {
            assert!(
                !contains_credential(&user, &connection.api_key),
                "sanitized request contains a saved connection credential"
            );
        }
    }
    let body = json!({"model":upstream.model,"stream":false,"temperature":0.2,"max_tokens":16000,"messages":[{"role":"system","content":system},{"role":"user","content":user}]});
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(
        output.join("request.json"),
        serde_json::to_vec_pretty(&body).unwrap(),
    )
    .unwrap();
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .post(upstream.endpoint.clone())
        .bearer_auth(&upstream.api_key)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    assert!(
        status.is_success(),
        "live upstream HTTP {}",
        status.as_u16()
    );
    let value: Value = response.json().await.unwrap();
    assert_eq!(
        value["choices"][0]["finish_reason"], "stop",
        "live response incomplete"
    );
    let revised = upstream
        .edit(value["choices"][0]["message"]["content"].as_str().unwrap())
        .unwrap();
    let usage = &value["usage"];
    assert!(
        usage["prompt_tokens"].as_u64().is_some(),
        "upstream did not provide input token usage"
    );
    let target = input["target"].as_u64().unwrap() as usize;
    let messages = input["messages"].as_array().unwrap();
    let evidence = json!({
        "model":upstream.model,"messages":messages.len(),"target":target,
        "contextChars":messages.iter().map(|m|m["text"].as_str().unwrap().chars().count()).sum::<usize>(),
        "systemChars":system.chars().count(),"userPayloadChars":user.chars().count(),
        "originalChars":item["text"].as_str().unwrap().chars().count(),
        "rewrittenChars":revised.text.chars().count(),"usage":usage,
    });
    std::fs::write(
        output.join("usage.json"),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
    std::fs::write(output.join("rewritten.md"), revised.text).unwrap();
}

#[test]
fn automatic_hook_is_data_only_and_respects_opt_out() {
    let plugin = ContentQualityPlugin;
    let definition = plugin
        .turn_automation("thread", &json!({"minReplyChars":900}))
        .unwrap()
        .unwrap();
    let value = serde_json::to_value(definition).unwrap();
    assert_eq!(
        value["trigger"],
        json!({"kind":"threadEnded","sourceThreadId":"thread"})
    );
    assert_eq!(value["action"]["input"], json!({}));
    assert_eq!(
        value["condition"]["conditions"][1],
        json!({"kind":"replyLengthAtLeast","chars":900})
    );
    assert!(plugin
        .turn_automation("thread", &json!({"autoReplies":false}))
        .unwrap()
        .is_none());
    assert!(plugin.validate_action("rewriteReply", &json!({})).is_ok());
    assert!(plugin.validate_settings(&json!({"detail":"short"})).is_ok());
    assert!(plugin
        .validate_settings(&json!({"detail":"brief"}))
        .is_err());
}

#[tokio::test]
async fn hook_fallback_checks_thread_ownership_and_uses_event_turn() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    s.db.with(|c| { c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('turn','thread','completed',0)", [])?; Ok(()) }).unwrap();
    let mut ctx = context(s.clone(), json!({}), json!({}));
    ctx.event = json!({"sourceThreadId":"another-thread","turnId":"turn"});
    let error = ContentQualityPlugin
        .invoke("rewriteReply", ctx)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "reply event must belong to this thread");
    let mut ctx = context(s, json!({}), json!({}));
    ctx.event = json!({"sourceThreadId":"thread","turnId":"turn"});
    assert_eq!(
        ContentQualityPlugin
            .invoke("rewriteReply", ctx)
            .await
            .unwrap()["reason"],
        "noReply"
    );
}

#[tokio::test]
async fn review_and_explicit_rewrite_actions_preserve_code_and_refuse_concurrent_edits() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    s.db.with(|c| {
        c.execute("INSERT INTO workspaces(id,host_id,label,abs_path,created_at) VALUES('workspace','host','fixture',?1,'now')", [dir.path().to_string_lossy().as_ref()])?;
        c.execute("INSERT INTO threads(id,workspace_id,provider,title,created_at,updated_at,status) VALUES('thread','workspace','codex','fixture','now','now','idle')", [])?; Ok(())
    }).unwrap();
    let path = dir.path().join("guide.md");
    let original = "Original prose with 42 checks and `cargo check`.\n";
    std::fs::write(&path, original).unwrap();
    for mode in ["review", "rewrite", "concurrent"] {
        std::fs::write(&path, original).unwrap();
        let path_for_edit = path.clone();
        let concurrent = mode == "concurrent";
        let (base, _, handle) = mock(200, move |body| {
            if concurrent {
                std::fs::write(&path_for_edit, "New user edits 99").unwrap();
            }
            let request: Value =
                serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
            completion(&json!({"text":request["document"].as_str().unwrap().replace("Original prose", "Clear prose"),"findings":["Clearer wording."]}).to_string())
        });
        let conn = connection(dir.path(), &base);
        let result = ContentQualityPlugin
            .invoke(
                "reviewDocument",
                context(
                    s.clone(),
                    json!({"connectionFile":conn}),
                    json!({"path":"guide.md","mode":if mode=="review" {"review"} else {"rewrite"}}),
                ),
            )
            .await;
        handle.join().unwrap();
        if mode == "concurrent" {
            assert!(result.unwrap_err().to_string().contains("document changed"));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "New user edits 99");
        } else {
            let result = result.unwrap();
            assert!(result["proposedRewrite"]
                .as_str()
                .unwrap()
                .contains("`cargo check`"));
            assert_eq!(result["written"], mode == "rewrite");
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                if mode == "review" {
                    original.into()
                } else {
                    original.replace("Original prose", "Clear prose")
                }
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires authorized private upstream and network"]
async fn live_reply_and_document_actions() {
    let connection_path = PathBuf::from(
        std::env::var_os("POCKYMOE_CONTENT_QUALITY_LIVE_CONNECTION")
            .expect("set an absolute connection file path"),
    );
    let model = std::env::var("POCKYMOE_CONTENT_QUALITY_LIVE_MODEL").unwrap_or_default();
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    let original = "This attempt failed. There are 42 unfinished checks. The command `cargo check` was not run. The result is uncertain, and verification remains incomplete.";
    s.db.with(|c| {
        c.execute("INSERT INTO workspaces(id,host_id,label,abs_path,created_at) VALUES('workspace','host','fixture',?1,'now')", [dir.path().to_string_lossy().as_ref()])?;
        c.execute("INSERT INTO threads(id,workspace_id,provider,title,created_at,updated_at,status) VALUES('thread','workspace','codex','fixture','now','now','idle')", [])?;
        c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('turn','thread','failed',0)", [])?;
        let item = json!({"id":"a","kind":"agentMessage","phase":"final_answer","text":original});
        c.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES('a','thread','turn','a',?1,'now','now')", [item.to_string()])?;
        Ok(())
    }).unwrap();
    let settings = json!({"connectionFile":connection_path,"model":model,"language":"en","preset":"copyedit","minReplyChars":0});
    let plugin = ContentQualityPlugin;
    let result = plugin
        .invoke(
            "rewriteReply",
            context(s.clone(), settings.clone(), json!({"turnId":"turn"})),
        )
        .await
        .unwrap();
    assert_eq!(result["status"], "completed");
    let stored = stored_turn(&s, "thread", "turn").unwrap();
    assert_eq!(stored.status, "failed");
    assert_eq!(stored.items[0]["text"], original);
    let polished = stored.items[0]["polishedReply"]["text"].as_str().unwrap();
    assert!(polished.contains("`cargo check`"));
    preserve_facts(original, polished).unwrap();
    assert_eq!(
        plugin
            .invoke(
                "rewriteReply",
                context(s.clone(), settings.clone(), json!({"turnId":"turn"}))
            )
            .await
            .unwrap()["reason"],
        "alreadyPolished"
    );
    let document = "The task failed. There are 42 unfinished checks. Verification remains incomplete.\n\n```sh\ncargo check\n```\n\nSee https://example.com/docs for reference.\n";
    let path = dir.path().join("guide.md");
    std::fs::write(&path, document).unwrap();
    let report = plugin
        .invoke(
            "reviewDocument",
            context(s.clone(), settings.clone(), json!({"path":"guide.md"})),
        )
        .await
        .unwrap();
    assert_eq!(report["status"], "reviewed");
    assert_eq!(report["written"], false);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), document);
    let report = plugin
        .invoke(
            "reviewDocument",
            context(s, settings, json!({"path":"guide.md","mode":"rewrite"})),
        )
        .await
        .unwrap();
    assert_eq!(report["status"], "completed");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        report["proposedRewrite"].as_str().unwrap()
    );
}

#[test]
fn credentials_are_detected_inside_json_escaped_content() {
    let secret = "fake-quote\"-credential";
    let encoded = json!({"source":secret}).to_string();
    assert!(!encoded.contains(secret));
    assert!(contains_credential(&encoded, secret));
    assert!(contains_credential(secret, secret));
    assert!(!contains_credential("ordinary source", secret));
}

#[test]
fn normalized_final_reply_wins_over_same_timestamp_commentary() {
    let items = json!([
        {"kind":"agentMessage","createdAt":"same","responsePhase":"commentary","text":"before"},
        {"kind":"agentMessage","createdAt":"same","responsePhase":"final","text":"final"},
        {"kind":"agentMessage","createdAt":"same","responsePhase":"commentary","text":"after"}
    ]);
    assert_eq!(
        final_reply(items.as_array().unwrap()).unwrap()["text"],
        "final"
    );
}

fn git_fixture(root: &Path, args: &[&str]) {
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success());
}
#[tokio::test]
async fn dirty_document_scan_is_bounded_and_respects_nested_workspaces_and_exclusions() {
    let dir = tempfile::tempdir().unwrap();
    git_fixture(dir.path(), &["init", "--quiet"]);
    std::fs::create_dir(dir.path().join("nested")).unwrap();
    std::fs::create_dir(dir.path().join("nested/vendor")).unwrap();
    for i in 0..7 {
        std::fs::write(
            dir.path().join(format!("nested/guide-{i}.md")),
            "Human documentation 42",
        )
        .unwrap();
    }
    std::fs::write(dir.path().join("outside.md"), "Sibling excluded").unwrap();
    std::fs::write(dir.path().join("nested/AGENTS.md"), "Instructions").unwrap();
    std::fs::write(dir.path().join("nested/vendor/lib.md"), "Vendor").unwrap();
    std::fs::write(
        dir.path().join("nested/generated.md"),
        "@generated do not edit",
    )
    .unwrap();
    std::fs::write(dir.path().join("nested/component.mdx"), "<Component />").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/tmp", dir.path().join("nested/escape")).unwrap();
    git_fixture(dir.path(), &["add", "nested/guide-0.md"]);
    std::fs::write(
        dir.path().join("nested/guide-0.md"),
        "Modified human documentation 42",
    )
    .unwrap();
    let (paths, truncated) = after_turn::documents(&dir.path().join("nested"))
        .await
        .unwrap();
    assert_eq!(paths.len(), 5);
    assert!(truncated);
    assert!(paths
        .iter()
        .all(|p| p.starts_with("guide-") && !p.contains('/')));
    assert_eq!(paths[0], "guide-0.md");
}
#[tokio::test]
async fn after_turn_skips_short_reply_but_reviews_docs_and_isolates_failures_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    git_fixture(dir.path(), &["init", "--quiet"]);
    let s = supervisor(dir.path());
    s.db.with(|c| {
        c.execute("INSERT INTO workspaces(id,host_id,label,abs_path,created_at) VALUES('workspace','host','fixture',?1,'now')", [dir.path().to_string_lossy().as_ref()])?;
        c.execute("INSERT INTO threads(id,workspace_id,provider,title,created_at,updated_at,status) VALUES('thread','workspace','codex','fixture','now','now','idle')", [])?;
        c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('turn','thread','completed',0)", [])?;
        let item = json!({"id":"a","kind":"agentMessage","responsePhase":"final","text":"Short reply."});
        c.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES('a','thread','turn','a',?1,'now','now')", [item.to_string()])?;
        Ok(())
    }).unwrap();
    let original = "Original prose with 42 checks and `cargo check`.\n";
    for p in ["a.md", "b.md"] {
        std::fs::write(dir.path().join(p), original).unwrap();
    }
    let (base, count, handle) = mock(200, |body| {
        let request: Value =
            serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        completion(&json!({"text":request["document"].as_str().unwrap().replace("Original prose","Clear prose"),"findings":["Clearer wording."]}).to_string())
    });
    let conn = connection(dir.path(), &base);
    let settings = json!({"connectionFile":conn,"autoDocuments":true,"documentMode":"rewrite"});
    let report = ContentQualityPlugin
        .invoke(
            "afterTurn",
            context(s, settings.clone(), json!({"turnId":"turn"})),
        )
        .await
        .unwrap();
    handle.join().unwrap();
    assert_eq!(report["reply"]["reason"], "belowMinReplyChars");
    assert_eq!(report["documents"].as_array().unwrap().len(), 2);
    assert_eq!(report["documents"][0]["status"], "reviewed");
    assert_eq!(report["documents"][1]["status"], "failed");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(serde_json::to_vec(&report).unwrap().len() < 256 * 1024);
    for p in ["a.md", "b.md"] {
        assert_eq!(
            std::fs::read_to_string(dir.path().join(p)).unwrap(),
            original
        );
    }
    let hook = serde_json::to_value(
        ContentQualityPlugin
            .turn_automation("thread", &settings)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(hook["action"]["action"], "afterTurn");
    assert_eq!(hook["condition"]["kind"], "statusIn");
    assert!(ContentQualityPlugin
        .turn_automation("thread", &json!({"autoReplies":false,"autoDocuments":true}))
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn manual_reply_threshold_excludes_code_using_shared_hook_counter() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    let item = json!({"id":"a","kind":"agentMessage","responsePhase":"final","text":format!("Short.\n```sh\n{}\n```", "command ".repeat(200))});
    s.db.with(|c| {
        c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('turn','thread','completed',0)", [])?;
        c.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES('a','thread','turn','a',?1,'now','now')", [item.to_string()])?; Ok(())
    }).unwrap();
    assert_eq!(
        ContentQualityPlugin
            .invoke(
                "rewriteReply",
                context(s, json!({"minReplyChars":20}), json!({"turnId":"turn"}))
            )
            .await
            .unwrap()["reason"],
        "belowMinReplyChars"
    );
}

#[tokio::test]
#[ignore = "requires authorized private upstream and network"]
async fn live_after_turn_reviews_git_document_without_writing() {
    let connection_path = PathBuf::from(
        std::env::var_os("POCKYMOE_CONTENT_QUALITY_LIVE_CONNECTION")
            .expect("set an absolute connection file path"),
    );
    let model = std::env::var("POCKYMOE_CONTENT_QUALITY_LIVE_MODEL").unwrap_or_default();
    let dir = tempfile::tempdir().unwrap();
    git_fixture(dir.path(), &["init", "--quiet"]);
    let s = supervisor(dir.path());
    s.db.with(|c| {
        c.execute("INSERT INTO workspaces(id,host_id,label,abs_path,created_at) VALUES('workspace','host','fixture',?1,'now')", [dir.path().to_string_lossy().as_ref()])?;
        c.execute("INSERT INTO threads(id,workspace_id,provider,title,created_at,updated_at,status) VALUES('thread','workspace','codex','fixture','now','now','idle')", [])?;
        c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('turn','thread','completed',0)", [])?; Ok(())
    }).unwrap();
    let original = "The task failed. There are 42 unfinished checks. Verification remains incomplete.\n\n```sh\ncargo check\n```\n\nSee https://example.com/docs for reference.\n";
    let path = dir.path().join("guide.md");
    std::fs::write(&path, original).unwrap();
    let settings = json!({"connectionFile":connection_path,"model":model,"language":"en","preset":"copyedit","autoReplies":false,"autoDocuments":true,"documentMode":"rewrite"});
    let report = ContentQualityPlugin
        .invoke("afterTurn", context(s, settings, json!({"turnId":"turn"})))
        .await
        .unwrap();
    assert_eq!(report["reply"]["reason"], "autoRepliesDisabled");
    assert_eq!(report["documents"].as_array().unwrap().len(), 1);
    assert_eq!(report["documents"][0]["status"], "reviewed");
    assert_eq!(report["documents"][0]["written"], false);
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
}

#[test]
fn complete_markdown_links_and_images_are_atomic_and_bracket_corruption_is_rejected() {
    let source = "请看[事件](https://example.com/issues/42)和![图片](./images/a(b).png)。";
    let p = Protected::new(source).unwrap();
    assert!(!p.masked.contains("事件"));
    assert!(!p.masked.contains("图片"));
    assert!(!p.masked.contains("]("));
    assert_eq!(p.restore(&p.masked).unwrap(), source);
    assert!(p.restore(&format!("{}]", p.masked)).is_err());
    assert!(p.restore(&format!("[{}", p.masked)).is_err());
    for bad in [
        "[outer [nested]](https://example.com)",
        r"[escaped\]](https://example.com)",
        r"[text](https://example.com/a\(b\))",
        r"\[escaped](url)",
        "missing ](https://example.com)",
    ] {
        assert!(Protected::new(bad).is_err(), "{bad}");
    }
}
#[test]
fn git_scan_command_explicit_environment_is_whitelisted_and_fsmonitor_disabled() {
    let command = after_turn::command(
        Path::new("workspace"),
        &["status", "--porcelain=v1", "-z", "--", "."],
    );
    let command = command.as_std();
    let env = command.get_envs().collect::<Vec<_>>();
    assert!(env
        .iter()
        .all(|(name, _)| ["PATH", "SYSTEMROOT", "TEMP", "TMP"]
            .iter()
            .any(|allowed| name == allowed)));
    let args = command
        .get_args()
        .map(|v| v.to_string_lossy())
        .collect::<Vec<_>>();
    assert!(args.iter().any(|v| v == "core.fsmonitor=false"));
    assert!(args.iter().any(|v| v == "core.untrackedCache=false"));
}
#[tokio::test]
async fn fenced_editorial_json_cannot_smuggle_unicode_escaped_credentials() {
    let dir = tempfile::tempdir().unwrap();
    for content in [
        r#"{"text":"\u006dock-credential","findings":[]}"#,
        r#"{"text":"Clear prose","findings":["\u006dock-credential"]}"#,
    ] {
        let (base, _, handle) = mock(200, move |_| {
            completion(&format!("```json\n{content}\n```"))
        });
        let u = Upstream::from_file(&connection(dir.path(), &base), "").unwrap();
        let raw = u.complete("system", "source", 100).await.unwrap();
        let error = u.edit(&raw).err().unwrap().to_string();
        assert_eq!(error, "upstream returned sensitive editorial content");
        handle.join().unwrap();
    }
}

#[tokio::test]
async fn committed_recorded_docs_are_prioritized_and_survive_git_scan_failure() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    let outside = dir.path().join("outside.md");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("guide.md"), "Committed guide 42").unwrap();
    std::fs::write(&outside, "Outside guide 77").unwrap();
    let (paths, truncated, warning) = after_turn::candidates(
        &root,
        &[json!({"kind":"fileChange","status":"completed","text":"guide.md"})],
    )
    .await
    .unwrap();
    assert_eq!(paths, vec!["guide.md"]);
    assert!(!truncated);
    assert!(warning.is_some());
    let (paths, _, _) = after_turn::candidates(
        &root,
        &[json!({"kind":"fileChange","status":"completed","text":outside.to_string_lossy()})],
    )
    .await
    .unwrap();
    assert!(paths.is_empty());
    let (paths, _, _) = after_turn::candidates(
        &root,
        &[
            json!({"kind":"fileChange","status":"failed","text":"guide.md"}),
            json!({"kind":"fileChange","status":"completed","deleted":true,"text":"guide.md"}),
        ],
    )
    .await
    .unwrap();
    assert!(paths.is_empty());
}
#[test]
fn recorded_structural_paths_are_bounded_and_reject_failed_deleted_and_shell_content() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for i in 0..7 {
        std::fs::write(root.join(format!("guide-{i}.md")), "guide").unwrap();
    }
    std::fs::write(root.join("AGENTS.md"), "instructions").unwrap();
    let changes = (0..7)
        .map(|i| json!({"filePath":format!("guide-{i}.md"),"status":"completed"}))
        .collect::<Vec<_>>();
    let (paths, truncated) = after_turn::recorded_documents(
        root,
        &[json!({"kind":"fileChange","status":"completed","changes":changes})],
    )
    .unwrap();
    assert_eq!(paths.len(), 5);
    assert!(truncated);
    for raw in ["AGENTS.md", "echo hello > guide-0.md", "../outside.md"] {
        assert!(
            after_turn::recorded_documents(
                root,
                &[json!({"kind":"fileChange","status":"completed","text":raw})]
            )
            .unwrap()
            .0
            .is_empty(),
            "{raw}"
        );
    }
    let (paths,_)=after_turn::recorded_documents(root,&[json!({"kind":"fileChange","status":"completed","changes":[{"path":"guide-0.md","status":"failed"},{"path":"guide-1.md","type":"delete"},{"path":"guide-2.md","status":"completed"}]})]).unwrap();
    assert_eq!(paths, vec!["guide-2.md"]);
}
#[tokio::test]
async fn committed_guide_is_reviewed_after_turn_and_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    s.db.with(|c| {
        c.execute("INSERT INTO workspaces(id,host_id,label,abs_path,created_at) VALUES('workspace','host','fixture',?1,'now')",[dir.path().to_string_lossy().as_ref()])?;
        c.execute("INSERT INTO threads(id,workspace_id,provider,title,created_at,updated_at,status) VALUES('thread','workspace','codex','fixture','now','now','idle')",[])?;
        c.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('turn','thread','completed',0)",[])?;
        let change=json!({"id":"file-1","kind":"fileChange","status":"completed","text":"guide.md"});
        let reply=json!({"id":"a","kind":"agentMessage","responsePhase":"final","text":"Completed this change."});
        for item in [change,reply] { c.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,'thread','turn',?1,?2,'now','now')",params![item["id"].as_str().unwrap(),item.to_string()])?; }
        Ok(())
    }).unwrap();
    let original = "Committed guide has 42 checks and `cargo test`.";
    let path = dir.path().join("guide.md");
    std::fs::write(&path, original).unwrap();
    let git = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["init", "--quiet"])
        .status()
        .unwrap();
    assert!(git.success());
    git_fixture(dir.path(), &["add", "guide.md"]);
    git_fixture(
        dir.path(),
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "committed guide",
        ],
    );
    let (base, _, handle) = mock(200, |body| {
        let req: Value =
            serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        completion(&json!({"text":req["document"].as_str().unwrap().replace("Committed guide","Reviewed guide"),"findings":["Review suggestion."]}).to_string())
    });
    let conn = connection(dir.path(), &base);
    let result = ContentQualityPlugin
        .invoke(
            "afterTurn",
            context(
                s,
                json!({"connectionFile":conn,"autoReplies":false,"autoDocuments":true}),
                json!({"turnId":"turn"}),
            ),
        )
        .await
        .unwrap();
    handle.join().unwrap();
    assert_eq!(result["documents"].as_array().unwrap().len(), 1);
    assert_eq!(result["documents"][0]["path"], "guide.md");
    assert_eq!(result["documents"][0]["status"], "reviewed");
    assert_eq!(result["documentScanError"], Value::Null);
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
}

#[tokio::test]
async fn recorded_turn_paths_take_priority_over_alphabetically_earlier_dirty_files() {
    let dir = tempfile::tempdir().unwrap();
    git_fixture(dir.path(), &["init", "--quiet"]);
    for name in ["a.md", "b.md", "c.md", "d.md", "e.md", "f.md", "z-turn.md"] {
        std::fs::write(dir.path().join(name), "documentation").unwrap();
    }
    let (paths, truncated, _) = after_turn::candidates(
        dir.path(),
        &[json!({"kind":"fileChange","status":"completed","text":"z-turn.md"})],
    )
    .await
    .unwrap();
    assert_eq!(paths[0], "z-turn.md");
    assert_eq!(paths.len(), 5);
    assert!(truncated);
}

#[tokio::test]
async fn saved_connection_overrides_legacy_path_without_applying_a_harness() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    let (base, _, server) = mock(200, |body| {
        assert_eq!(body["model"], "mock-model");
        // A reasoning model can exhaust a tiny probe budget before completing
        // its visible answer. Retain incomplete-response rejection.
        if body["max_tokens"].as_u64().unwrap() < 128 {
            json!({"choices":[{"finish_reason":"length","message":{"content":""}}]})
        } else {
            completion("OK")
        }
    });
    let c:crate::upstreams::connections::Connection=serde_json::from_value(json!({"name":"Writing model","protocol":"openai_chat","baseUrl":base,"apiKey":"mock-credential","model":"default-model"})).unwrap();
    let saved = crate::upstreams::connections::upsert(
        &crate::upstreams::directory(&s.config.database_url),
        c,
    )
    .unwrap();
    let settings = settings(
        json!({"connectionId":saved["id"],"connectionFile":"/nonexistent/legacy.json","model":"mock-model"}),
    );
    assert_eq!(
        ContentQualityPlugin
            .invoke(
                "testConnection",
                context(
                    s.clone(),
                    serde_json::to_value(&settings).unwrap(),
                    json!({})
                ),
            )
            .await
            .unwrap()["status"],
        "connected"
    );
    server.join().unwrap();
    assert!(
        crate::upstreams::inventory(&crate::upstreams::directory(&s.config.database_url)).unwrap()
            ["profiles"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mut missing = settings;
    missing.connection_id = uuid::Uuid::new_v4().to_string();
    assert!(Upstream::load(&s, &missing).is_err());
    assert!(Settings::parse(&json!({"connectionId":"not-an-id"})).is_err());
}
