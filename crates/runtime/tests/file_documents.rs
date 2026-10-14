#![cfg(target_os = "linux")]
use pockymoe_protocol::{CreateWorkspaceInput, Provider};
use pockymoe_runtime::file_documents::{Document, SaveDocument, MAX_BYTES};
use pockymoe_runtime::{fake::FakeRuntime, Database, RuntimeConfig, Supervisor};
use serde_json::json;
use std::{fs, sync::Arc};
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

fn request(doc: &Document, content: &str) -> SaveDocument {
    SaveDocument {
        path: doc.path.clone(),
        workspace_revision: doc.workspace_revision.clone(),
        file_identity: doc.file_identity.clone(),
        expected_hash: doc.content_hash.clone().unwrap(),
        content: content.into(),
        draft_revision: 3,
        operation_id: uuid::Uuid::new_v4().to_string(),
        operation_created_at: chrono::Utc::now().timestamp_millis() as u64,
    }
}
#[test]
fn conditional_save_preserves_bom_crlf_and_mode_and_replays_receipt_without_writing() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, s) = setup();
    let ws = workspace(&s);
    let path = dir.path().join("notes.txt");
    fs::write(&path, b"\xef\xbb\xbfhello\r\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let doc = s.file_document(&ws, "notes.txt").unwrap();
    assert_eq!(doc.content.as_deref(), Some("hello\n"));
    assert!(doc.bom);
    assert_eq!(doc.eol, "crlf");
    let input = request(&doc, "你好\nnext");
    let result = s.file_save("a", &ws, input.clone()).unwrap();
    assert_eq!(result["status"], "saved");
    assert_eq!(fs::read(&path).unwrap(), "\u{feff}你好\r\nnext".as_bytes());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    fs::write(&path, "later external write").unwrap();
    assert_eq!(s.file_save("a", &ws, input.clone()).unwrap(), result);
    assert_eq!(fs::read_to_string(&path).unwrap(), "later external write");
    assert_eq!(
        s.file_operation("a", &ws, &input.operation_id).unwrap(),
        result
    );
    assert!(s.file_operation("other", &ws, &input.operation_id).is_err());
    let mut reused = input;
    reused.content = "different".into();
    assert!(s
        .file_save("a", &ws, reused)
        .unwrap_err()
        .to_string()
        .contains("operationIdReuse"));
}
#[test]
fn conflict_snapshot_is_fixed_missing_does_not_recreate_and_newer_snapshot_conflicts_again() {
    let (dir, s) = setup();
    let ws = workspace(&s);
    let path = dir.path().join("file.txt");
    fs::write(&path, "base").unwrap();
    let original = s.file_document(&ws, "file.txt").unwrap();
    fs::write(&path, "agent version").unwrap();
    let input = request(&original, "mine");
    let result = s.file_save("a", &ws, input.clone()).unwrap();
    assert_eq!(result["status"], "conflict");
    assert_eq!(result["snapshot"]["content"], "agent version");
    let snapshot: Document = serde_json::from_value(result["snapshot"].clone()).unwrap();
    fs::write(&path, "second agent version").unwrap();
    assert_eq!(
        s.file_operation("a", &ws, &input.operation_id).unwrap()["snapshot"]["content"],
        "agent version"
    );
    assert_eq!(
        s.file_save("a", &ws, request(&snapshot, "mine")).unwrap()["status"],
        "conflict"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "second agent version");
    fs::remove_file(&path).unwrap();
    assert_eq!(
        s.file_save("a", &ws, request(&original, "mine")).unwrap()["status"],
        "conflict"
    );
    assert!(!path.exists());
}
#[test]
fn rejects_unsupported_encoding_eol_size_metadata_paths_and_replaced_identity() {
    use std::os::unix::fs::symlink;
    let (dir, s) = setup();
    let ws = workspace(&s);
    for (name, bytes, reason) in [
        ("gbk.txt", vec![0xff, 0xfe], "unsupportedEncoding"),
        ("mixed.txt", b"a\r\nb\n".to_vec(), "unsupportedEol"),
        ("cr.txt", b"a\rb".to_vec(), "unsupportedEol"),
        ("binary", vec![0], "binaryFile"),
        ("large", vec![b'x'; MAX_BYTES + 1], "fileTooLarge"),
        ("lines", vec![b'\n'; 1000], "tooManyLines"),
    ] {
        fs::write(dir.path().join(name), bytes).unwrap();
        let doc = s.file_document(&ws, name).unwrap();
        assert_eq!(doc.read_only_reason.as_deref(), Some(reason));
        if doc.content_hash.is_some() {
            assert_eq!(
                s.file_save("a", &ws, request(&doc, "replace")).unwrap()["status"],
                "failedBeforeWrite"
            );
        }
    }
    fs::write(dir.path().join("a.txt"), "same").unwrap();
    let doc = s.file_document(&ws, "a.txt").unwrap();
    fs::rename(dir.path().join("a.txt"), dir.path().join("old.txt")).unwrap();
    fs::write(dir.path().join("a.txt"), "same").unwrap();
    assert_eq!(
        s.file_save("a", &ws, request(&doc, "mine")).unwrap()["status"],
        "conflict"
    );
    symlink(dir.path().join("a.txt"), dir.path().join("link")).unwrap();
    assert!(s.file_document(&ws, "link").is_err());
    fs::hard_link(dir.path().join("a.txt"), dir.path().join("hard")).unwrap();
    assert_eq!(
        s.file_document(&ws, "a.txt")
            .unwrap()
            .read_only_reason
            .as_deref(),
        Some("unsupportedMetadata")
    );
    symlink(dir.path(), dir.path().join("parent-link")).unwrap();
    assert!(s.file_document(&ws, "parent-link/a.txt").is_err());
    for path in ["../a.txt", "/etc/passwd", "a/../a.txt", "."] {
        assert!(s.file_document(&ws, path).is_err());
    }
}
#[test]
fn interrupted_intent_remains_uncertain_after_reopen_and_is_never_replayed() {
    let (dir, s) = setup();
    let ws = workspace(&s);
    fs::write(dir.path().join("a.txt"), "base").unwrap();
    let doc = s.file_document(&ws, "a.txt").unwrap();
    let input = request(&doc, "mine");
    let digest = pockymoe_runtime::file_documents::hash(&serde_json::to_vec(&input).unwrap());
    let uncertain = json!({"status":"uncertain","operationId":input.operation_id,"draftRevision":3,"path":"a.txt"});
    s.db.with(|conn| {conn.execute("INSERT INTO file_save_operations(actor,workspace_id,workspace_revision,operation_id,input_digest,result,created_at) VALUES(?1,?2,?3,?4,?5,?6,unixepoch())",rusqlite::params!["a",ws,doc.workspace_revision,input.operation_id,digest,uncertain.to_string()])?;Ok(())}).unwrap();
    let live = s.track_file_save("a", &ws, &input);
    assert_eq!(
        s.file_operation("a", &ws, &input.operation_id).unwrap()["status"],
        "pending"
    );
    assert!(s.file_operation("other", &ws, &input.operation_id).is_err());
    drop(live);
    assert_eq!(
        s.file_operation("a", &ws, &input.operation_id).unwrap()["status"],
        "uncertain"
    );
    let config = s.config.clone();
    drop(s);
    let db = Database::open(&config.database_url).unwrap();
    let s = Supervisor::new(config, db, vec![]);
    assert_eq!(s.file_save("a", &ws, input).unwrap()["status"], "uncertain");
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "base"
    );
}
#[test]
fn concurrent_managed_saves_have_one_winner_and_preview_read_is_bounded_strict_utf8() {
    let (dir, s) = setup();
    let ws = workspace(&s);
    fs::write(dir.path().join("a.txt"), "base").unwrap();
    let doc = s.file_document(&ws, "a.txt").unwrap();
    let a = request(&doc, "first");
    let b = request(&doc, "second");
    let s1 = s.clone();
    let ws1 = ws.clone();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let b1 = barrier.clone();
    let worker = std::thread::spawn(move || {
        b1.wait();
        s1.file_save("a", &ws1, a).unwrap()
    });
    barrier.wait();
    let r2 = s.file_save("b", &ws, b).unwrap();
    let r1 = worker.join().unwrap();
    assert_eq!(
        [
            r1["status"].as_str().unwrap(),
            r2["status"].as_str().unwrap()
        ]
        .into_iter()
        .filter(|v| *v == "saved")
        .count(),
        1
    );
    let large = dir.path().join("big.txt");
    let file = fs::File::create(&large).unwrap();
    file.set_len(1_000_000_000).unwrap();
    assert!(pockymoe_runtime::files::preview_file(dir.path(), "big.txt", 64 * 1024).is_err());
    fs::write(&large, "你好世界").unwrap();
    let p = pockymoe_runtime::files::preview_file(dir.path(), "big.txt", 4).unwrap();
    assert_eq!(p.content, "你");
    assert_eq!(p.next_offset, 3);
    assert!(p.truncated);
    fs::write(&large, [0xff]).unwrap();
    assert!(pockymoe_runtime::files::preview_file(dir.path(), "big.txt", 4).is_err());
}
