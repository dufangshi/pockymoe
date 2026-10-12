use std::sync::Arc;

use pockymoe_protocol::{Mode, Provider, WorkspaceSettingsDto};
use pockymoe_runtime::actor::SharedRuntime;
use pockymoe_runtime::config::RuntimeConfig;
use pockymoe_runtime::db::Database;
use pockymoe_runtime::fake::FakeRuntime;
use pockymoe_runtime::Supervisor;
use rusqlite::{params, Connection, OptionalExtension};
use tempfile::tempdir;

// Final schema produced by main's packages/db migrations through 0030_thread_agent_id.sql.
const NODE_0030_SCHEMA: &str = r#"
CREATE TABLE __migrations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL UNIQUE,
  applied_at TEXT NOT NULL
);

CREATE TABLE hosts (
  id TEXT PRIMARY KEY NOT NULL,
  hostname TEXT NOT NULL,
  platform TEXT NOT NULL,
  tailscale_name TEXT,
  created_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL
);

CREATE TABLE workspaces (
  id TEXT PRIMARY KEY NOT NULL,
  host_id TEXT NOT NULL,
  label TEXT NOT NULL,
  abs_path TEXT NOT NULL UNIQUE,
  is_favorite INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  last_opened_at TEXT
);

CREATE TABLE threads (
  id TEXT PRIMARY KEY NOT NULL,
  workspace_id TEXT NOT NULL,
  provider TEXT DEFAULT 'codex' NOT NULL,
  provider_session_id TEXT,
  provider_turn_id TEXT,
  source TEXT DEFAULT 'supervisor' NOT NULL,
  title TEXT NOT NULL,
  model TEXT,
  reasoning_effort TEXT,
  fast_mode INTEGER DEFAULT false NOT NULL,
  fast_base_model TEXT,
  fast_base_reasoning_effort TEXT,
  collaboration_mode TEXT DEFAULT 'default' NOT NULL,
  active_turn_collaboration_mode TEXT,
  approval_mode TEXT,
  sandbox_mode TEXT,
  status TEXT,
  summary_text TEXT,
  last_error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  last_turn_started_at TEXT,
  last_turn_completed_at TEXT,
  last_viewed_at TEXT,
  is_pinned INTEGER DEFAULT false NOT NULL,
  is_connected INTEGER DEFAULT true NOT NULL,
  agent_id TEXT
);

CREATE TABLE shell_sessions (
  id TEXT PRIMARY KEY NOT NULL,
  workspace_id TEXT NOT NULL,
  thread_id TEXT,
  tmux_session_name TEXT,
  cwd TEXT NOT NULL,
  status TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  last_activity_at TEXT,
  label TEXT
);

CREATE TABLE viewer_sessions (
  id TEXT PRIMARY KEY NOT NULL,
  thread_id TEXT,
  shell_id TEXT,
  connected_at TEXT NOT NULL,
  last_heartbeat_at TEXT,
  active_tab TEXT
);

CREATE TABLE notifications (
  id TEXT PRIMARY KEY NOT NULL,
  thread_id TEXT,
  kind TEXT NOT NULL,
  severity TEXT NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  is_read INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE TABLE policies (
  id TEXT PRIMARY KEY NOT NULL,
  key TEXT NOT NULL UNIQUE,
  value_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE thread_turn_metadata (
  id TEXT PRIMARY KEY NOT NULL,
  thread_id TEXT NOT NULL,
  turn_id TEXT NOT NULL,
  model TEXT,
  reasoning_effort TEXT,
  reasoning_effort_available INTEGER,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  token_usage_json TEXT,
  pricing_model_key TEXT,
  pricing_tier_key TEXT,
  display_prompt TEXT
);
CREATE UNIQUE INDEX thread_turn_metadata_thread_turn_idx
  ON thread_turn_metadata(thread_id, turn_id);

CREATE TABLE thread_pending_steers (
  id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  turn_id TEXT NOT NULL,
  client_request_id TEXT,
  display_prompt TEXT NOT NULL,
  submitted_prompt TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  delivery TEXT NOT NULL DEFAULT 'steer',
  turn_config_json TEXT
);
CREATE INDEX thread_pending_steers_thread_created_idx
  ON thread_pending_steers(thread_id, created_at);

CREATE TABLE thread_prompt_requests (
  id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  client_request_id TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX thread_prompt_requests_thread_client_request_idx
  ON thread_prompt_requests(thread_id, client_request_id);

CREATE TABLE thread_history_items (
  id TEXT PRIMARY KEY NOT NULL,
  thread_id TEXT NOT NULL,
  turn_id TEXT NOT NULL,
  item_id TEXT NOT NULL,
  item_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX thread_history_items_thread_turn_item_idx
  ON thread_history_items(thread_id, turn_id, item_id);

CREATE TABLE thread_activity_notes (
  id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  text TEXT NOT NULL,
  created_at TEXT NOT NULL,
  anchor_turn_id TEXT
);
CREATE INDEX thread_activity_notes_thread_created_idx
  ON thread_activity_notes(thread_id, created_at);

CREATE TABLE thread_forks (
  id TEXT PRIMARY KEY NOT NULL,
  source_thread_id TEXT NOT NULL,
  source_turn_id TEXT,
  source_turn_index INTEGER,
  forked_thread_id TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE thread_goals (
  id TEXT PRIMARY KEY NOT NULL,
  thread_id TEXT NOT NULL,
  provider_session_id TEXT NOT NULL,
  objective TEXT NOT NULL,
  status TEXT NOT NULL,
  token_budget INTEGER,
  tokens_used INTEGER DEFAULT 0 NOT NULL,
  time_used_seconds INTEGER DEFAULT 0 NOT NULL,
  started_at TEXT NOT NULL,
  completed_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
"#;

const NODE_MIGRATIONS: &[&str] = &[
    "0000_initial.sql",
    "0001_thread_runtime_fields.sql",
    "0002_thread_source.sql",
    "0003_thread_runtime_settings.sql",
    "0004_thread_connection_state.sql",
    "0005_thread_turn_metadata.sql",
    "0006_thread_sandbox_mode.sql",
    "0007_thread_pending_steers.sql",
    "0008_thread_fast_mode_and_activity_notes.sql",
    "0009_thread_turn_token_usage.sql",
    "0010_thread_turn_pricing_snapshot.sql",
    "0011_thread_forks.sql",
    "0012_thread_goals.sql",
    "0013_thread_activity_note_anchor.sql",
    "0014_thread_history_items.sql",
    "0015_agent_provider_fields.sql",
    "0016_remove_codex_thread_goal_id.sql",
    "0017_remove_codex_thread_columns.sql",
    "0018_shell_session_label.sql",
    "0028_thread_turn_display_prompt.sql",
    "0029_thread_turn_delivery.sql",
    "0030_thread_agent_id.sql",
];

fn seed_node_0030(path: &std::path::Path, workspace_path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(NODE_0030_SCHEMA).unwrap();
    for migration in NODE_MIGRATIONS {
        conn.execute(
            "INSERT INTO __migrations(name, applied_at) VALUES (?1,'2026-01-01T00:00:00Z')",
            params![migration],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO hosts(id,hostname,platform,created_at,last_seen_at)
         VALUES ('node-host','node-hostname','darwin','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO workspaces(id,host_id,label,abs_path,is_favorite,created_at,last_opened_at)
         VALUES ('node-workspace','node-host','Node workspace',?1,1,'2026-01-01T00:00:00Z',NULL)",
        params![workspace_path.to_string_lossy()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO threads(
           id,workspace_id,provider,agent_id,provider_session_id,provider_turn_id,source,title,
           model,reasoning_effort,collaboration_mode,approval_mode,sandbox_mode,status,
           created_at,updated_at,is_pinned,is_connected
         ) VALUES (
           'node-thread','node-workspace','codex',NULL,'node-session',NULL,'supervisor','Old thread',
           'gpt-5.4','medium','default','yolo','danger-full-access','idle',
           '2026-01-01T00:00:00Z','2026-01-01T00:03:00Z',0,1
         )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO threads(
           id,workspace_id,provider,agent_id,provider_session_id,provider_turn_id,source,title,
           model,reasoning_effort,collaboration_mode,approval_mode,sandbox_mode,status,
           created_at,updated_at,is_pinned,is_connected
         ) VALUES (
           'live-thread','node-workspace','codex',NULL,'live-session','live-turn','supervisor','Live thread',
           'gpt-5.4','high','default','yolo','danger-full-access','running',
           '2026-01-01T00:00:00Z','2026-01-01T00:04:00Z',0,1
         )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO thread_turn_metadata(
           id,thread_id,turn_id,model,reasoning_effort,created_at,updated_at,
           token_usage_json,pricing_model_key,pricing_tier_key,display_prompt
         ) VALUES (
           'metadata-1','node-thread','node-turn-1','gpt-5.4','medium',
           '2026-01-01T00:01:00Z','2026-01-01T00:01:30Z',
           '{\"inputTokens\":12,\"outputTokens\":7}','gpt-5.4','standard','hello'
         )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO thread_turn_metadata(
           id,thread_id,turn_id,model,reasoning_effort,created_at,updated_at,display_prompt
         ) VALUES (
           'metadata-live','live-thread','live-turn','gpt-5.4','high',
           '2026-01-01T00:04:00Z','2026-01-01T00:04:30Z','still running'
         )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO thread_history_items(
           id,thread_id,turn_id,item_id,item_json,created_at,updated_at
         ) VALUES (
           'history-1','node-thread','node-turn-1','node-item-1',
           '{\"id\":\"node-item-1\",\"kind\":\"userMessage\",\"text\":\"hello from Node\",\"createdAt\":\"2026-01-01T00:01:00Z\"}',
           '2026-01-01T00:01:00Z','2026-01-01T00:01:00Z'
         )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO thread_history_items(
           id,thread_id,turn_id,item_id,item_json,created_at,updated_at
         ) VALUES (
           'history-only','node-thread','node-turn-2','node-item-2',
           '{\"id\":\"node-item-2\",\"kind\":\"agentMessage\",\"text\":\"history-only turn\",\"createdAt\":\"2026-01-01T00:02:00Z\"}',
           '2026-01-01T00:02:00Z','2026-01-01T00:02:30Z'
         )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO thread_pending_steers(
           id,thread_id,turn_id,client_request_id,display_prompt,submitted_prompt,
           created_at,updated_at,delivery,turn_config_json
         ) VALUES (
           'node-steer','node-thread','node-turn-1','client-1','follow up','follow up',
           '2026-01-01T00:03:00Z','2026-01-01T00:03:01Z','steer',NULL
         )",
        [],
    )
    .unwrap();
    for (id, key, value) in [
        (
            "policy-dev-home",
            "dev_home",
            r#"{"absPath":"/tmp/node-dev"}"#,
        ),
        (
            "policy-backend",
            "default_backend",
            r#"{"provider":"claude"}"#,
        ),
        (
            "policy-plugins",
            "plugins",
            r#"{"enabled":{"remote-codex.terminal":false,"nested":{"enabled":true}},"imported":[]}"#,
        ),
        ("policy-unknown", "future_setting", r#"{"keep":true}"#),
    ] {
        conn.execute(
            "INSERT INTO policies(id,key,value_json,created_at,updated_at)
             VALUES (?1,?2,?3,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
            params![id, key, value],
        )
        .unwrap();
    }
}

fn test_config(root: &std::path::Path, database_url: &std::path::Path) -> RuntimeConfig {
    RuntimeConfig {
        mode: Mode::Local,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root: root.join("workspaces"),
        database_url: database_url.to_path_buf(),
        app_name: "migration-test".into(),
        app_version: "0.12.0".into(),
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
        acp_startup_timeout_ms: 1_000,
        fake_runtime: true,
    }
}

#[tokio::test]
async fn migrates_node_0030_history_and_policies_idempotently() {
    let dir = tempdir().unwrap();
    let database_path = dir.path().join("node.sqlite");
    let workspace_path = dir.path().join("node-workspace");
    std::fs::create_dir(&workspace_path).unwrap();
    seed_node_0030(&database_path, &workspace_path);
    let conn = Connection::open(&database_path).unwrap();
    conn.execute(
        "UPDATE threads SET approval_mode=NULL WHERE id='node-thread'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE threads SET approval_mode='unexpected' WHERE id='live-thread'",
        [],
    )
    .unwrap();
    drop(conn);

    let database = Database::open(&database_path).unwrap();
    database
        .with(|conn| {
            let status: String = conn.query_row(
                "SELECT status FROM thread_turns WHERE id='live-turn'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(status, "inProgress");
            Ok(())
        })
        .unwrap();
    let runtime: SharedRuntime = Arc::new(FakeRuntime::new(Provider::Codex));
    let supervisor = Supervisor::new(
        test_config(dir.path(), &database_path),
        database,
        vec![runtime],
    );

    let backfilled: i64 = supervisor
        .db
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT count(*) FROM search_documents WHERE thread_id='node-thread'",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert!(
        backfilled > 1,
        "title and pre-existing conversational history must be searchable immediately"
    );
    supervisor.db.with(|conn| {
        conn.execute("INSERT INTO search_documents_fts(search_documents_fts, rank) VALUES('integrity-check', 1)", [])?;
        Ok(())
    }).unwrap();

    let detail = supervisor
        .get_thread_detail("node-thread", None)
        .await
        .unwrap();
    assert_eq!(detail.total_turn_count, Some(2));
    assert_eq!(detail.turns[0].id, "node-turn-1");
    assert_eq!(detail.turns[0].status, "completed");
    assert_eq!(detail.turns[0].model.as_deref(), Some("gpt-5.4"));
    assert_eq!(detail.turns[0].reasoning_effort.as_deref(), Some("medium"));
    assert_eq!(
        detail.turns[0].token_usage.as_ref().unwrap()["inputTokens"],
        12
    );
    assert_eq!(
        detail.turns[0].token_usage.as_ref().unwrap()["outputTokens"],
        7
    );
    assert_eq!(detail.turns[0].items[0].text, "hello from Node");
    assert_eq!(detail.turns[1].id, "node-turn-2");
    assert_eq!(detail.turns[1].items[0].text, "history-only turn");
    assert_eq!(detail.pending_steers.len(), 1);
    assert_eq!(detail.pending_steers[0].id, "node-steer");
    assert_eq!(detail.thread.approval_mode, "guarded");

    let live_detail = supervisor
        .get_thread_detail("live-thread", None)
        .await
        .unwrap();
    assert_eq!(live_detail.thread.active_turn_id, None);
    assert_eq!(live_detail.turns[0].status, "recovering");
    assert_eq!(live_detail.thread.approval_mode, "guarded");

    let settings: WorkspaceSettingsDto =
        serde_json::from_str(&supervisor.db.get_kv("workspace_settings").unwrap().unwrap())
            .unwrap();
    assert_eq!(settings.dev_home, "/tmp/node-dev");
    assert_eq!(settings.default_backend, Provider::Claude);
    assert_eq!(
        supervisor
            .db
            .get_kv("plugin:remote-codex.terminal:enabled")
            .unwrap()
            .as_deref(),
        Some("false")
    );
    assert_eq!(
        supervisor
            .db
            .get_kv("plugin:nested:enabled")
            .unwrap()
            .as_deref(),
        Some("true")
    );
    assert_eq!(
        supervisor
            .db
            .get_kv("legacy-policy:future_setting")
            .unwrap()
            .as_deref(),
        Some(r#"{"keep":true}"#)
    );

    supervisor
        .db
        .with(|conn| {
            let node_migration_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM __migrations", [], |row| row.get(0))?;
            let runtime_migration_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM __remote_codex_runtime_migrations",
                [],
                |row| row.get(0),
            )?;
            let legacy_metadata_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM thread_turn_metadata", [], |row| {
                    row.get(0)
                })?;
            assert_eq!(node_migration_count, NODE_MIGRATIONS.len() as i64);
            let migrated_metadata: (String, String, String, String) = conn.query_row(
                "SELECT token_usage_json, pricing_model_key, pricing_tier_key, display_prompt
                 FROM thread_turns WHERE id='node-turn-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            assert_eq!(runtime_migration_count, 16);
            assert_eq!(legacy_metadata_count, 2);
            assert_eq!(
                migrated_metadata.0,
                r#"{"inputTokens":12,"outputTokens":7}"#
            );
            assert_eq!(migrated_metadata.1, "gpt-5.4");
            assert_eq!(migrated_metadata.2, "standard");
            assert_eq!(migrated_metadata.3, "hello");
            Ok(())
        })
        .unwrap();
    drop(supervisor);

    let reopened = Database::open(&database_path).unwrap();
    reopened
        .with(|conn| {
            let turns: i64 =
                conn.query_row("SELECT COUNT(*) FROM thread_turns", [], |row| row.get(0))?;
            let migrations: i64 = conn.query_row(
                "SELECT COUNT(*) FROM __remote_codex_runtime_migrations",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(turns, 3);
            assert_eq!(migrations, 16);
            Ok(())
        })
        .unwrap();
}

#[test]
fn pre_rust_backup_includes_uncheckpointed_wal_content() {
    let dir = tempdir().unwrap();
    let database_path = dir.path().join("node-wal.sqlite");
    let workspace_path = dir.path().join("node-workspace");
    std::fs::create_dir(&workspace_path).unwrap();
    seed_node_0030(&database_path, &workspace_path);

    let writer = Connection::open(&database_path).unwrap();
    writer
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    writer
        .execute(
            "INSERT INTO policies(id,key,value_json,created_at,updated_at)
             VALUES ('wal-policy','wal-only','{\"present\":true}',
                     '2026-01-03T00:00:00Z','2026-01-03T00:00:00Z')",
            [],
        )
        .unwrap();
    let wal_path = std::path::PathBuf::from(format!("{}-wal", database_path.display()));
    assert!(wal_path.is_file());

    let migrated = Database::open(&database_path).unwrap();
    let backup_path = dir.path().join("node-wal.pre-rust-0.12.sqlite");
    assert!(backup_path.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&backup_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let backup =
        Connection::open_with_flags(&backup_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let wal_value: String = backup
        .query_row(
            "SELECT value_json FROM policies WHERE key='wal-only'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(wal_value, r#"{"present":true}"#);
    let rust_table: Option<String> = backup
        .query_row(
            "SELECT name FROM sqlite_schema
             WHERE type='table' AND name='__remote_codex_runtime_migrations'",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert!(
        rust_table.is_none(),
        "backup must be taken before migration"
    );
    drop(backup);
    drop(migrated);
    drop(writer);
}

#[test]
fn rejects_future_node_migration_before_runtime_migration() {
    let dir = tempdir().unwrap();
    let database_path = dir.path().join("future.sqlite");
    let workspace_path = dir.path().join("node-workspace");
    std::fs::create_dir(&workspace_path).unwrap();
    seed_node_0030(&database_path, &workspace_path);
    let conn = Connection::open(&database_path).unwrap();
    conn.execute(
        "INSERT INTO __migrations(name, applied_at)
         VALUES ('0031_unknown_future.sql','2026-01-02T00:00:00Z')",
        [],
    )
    .unwrap();
    drop(conn);

    let error = Database::open(&database_path).err().unwrap();
    assert!(
        format!("{error:#}").contains("unsupported Node migration `0031_unknown_future.sql`"),
        "unexpected error: {error:#}"
    );
    let conn = Connection::open(&database_path).unwrap();
    let runtime_table: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_schema
             WHERE type='table' AND name='__remote_codex_runtime_migrations'",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert!(runtime_table.is_none());
}

#[test]
fn rolls_back_the_whole_upgrade_when_legacy_turn_ids_conflict() {
    let dir = tempdir().unwrap();
    let database_path = dir.path().join("conflicting.sqlite");
    let workspace_path = dir.path().join("node-workspace");
    std::fs::create_dir(&workspace_path).unwrap();
    seed_node_0030(&database_path, &workspace_path);
    let conn = Connection::open(&database_path).unwrap();
    conn.execute(
        "INSERT INTO threads(
           id,workspace_id,provider,provider_session_id,source,title,collaboration_mode,
           approval_mode,status,created_at,updated_at,is_pinned,is_connected
         ) VALUES (
           'other-thread','node-workspace','codex','other-session','supervisor','Other',
           'default','yolo','idle','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z',0,1
         )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO thread_history_items(
           id,thread_id,turn_id,item_id,item_json,created_at,updated_at
         ) VALUES (
           'conflicting-history','other-thread','node-turn-1','other-item',
           '{\"id\":\"other-item\",\"kind\":\"userMessage\",\"text\":\"conflict\"}',
           '2026-01-01T00:05:00Z','2026-01-01T00:05:00Z'
         )",
        [],
    )
    .unwrap();
    drop(conn);

    let error = Database::open(&database_path).err().unwrap();
    assert!(
        format!("{error:#}").contains("runtime turn ids must be globally unique"),
        "unexpected error: {error:#}"
    );
    let conn = Connection::open(&database_path).unwrap();
    for table in ["__remote_codex_runtime_migrations", "thread_turns", "kv"] {
        let created: Option<String> = conn
            .query_row(
                "SELECT name FROM sqlite_schema WHERE type='table' AND name=?1",
                params![table],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert!(created.is_none(), "migration left `{table}` behind");
    }
    let legacy_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM thread_history_items", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(legacy_rows, 3);
}

/// 0.12.55/0.12.56 shipped without migration 10. A device that had run 0.12.53
/// was repaired by dropping the ledger row while the columns stayed; reapplying
/// migration 10 on top of that state must succeed rather than fail on duplicates.
#[test]
fn migration_10_reapplies_over_columns_left_by_a_downgrade() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("downgraded.sqlite");
    drop(Database::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "DELETE FROM __remote_codex_runtime_migrations WHERE version=10",
        [],
    )
    .unwrap();
    conn.execute_batch("DELETE FROM __remote_codex_runtime_migrations WHERE version>=11;
        DROP TRIGGER search_history_insert; DROP TRIGGER search_history_update; DROP TRIGGER search_history_delete;
        DROP TRIGGER search_turn_delete; DROP TRIGGER search_thread_delete; DROP TRIGGER search_thread_insert;
        DROP TRIGGER search_thread_title; DROP TRIGGER search_thread_source;
        DROP TABLE search_documents_fts; DROP TABLE search_documents; DROP TABLE file_save_operations;").unwrap();
    drop(conn);

    drop(Database::open(&path).expect("reapplying migration 10 must be idempotent"));
    let conn = Connection::open(&path).unwrap();
    let name: String = conn
        .query_row(
            "SELECT name FROM __remote_codex_runtime_migrations WHERE version=10",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "agent_coordination");
    let columns: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('threads') WHERE name IN ('agent_name','agent_role','worktree_path','closed_at')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(columns, 4);
}

#[test]
fn device_owner_trust_v2_migration_defaults_and_persists_account_registrations() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("trust.sqlite");
    let db = Database::open(&path).unwrap();
    db.with(|c| {
        assert_eq!(
            c.query_row(
                "SELECT name FROM __remote_codex_runtime_migrations WHERE version=14",
                [],
                |r| r.get::<_, String>(0)
            )?,
            "device_owner_trust"
        );
        assert_eq!(
            c.query_row("SELECT strict FROM device_owner_trust", [], |r| r
                .get::<_, i32>(0))?,
            0
        );
        c.execute(
            "UPDATE device_owner_trust SET relay_device_id='relay-device', strict=1",
            [],
        )?;
        c.execute("UPDATE device_owner_trust SET account_key_id='account',account_public_key='spki',enroll_secret=?1",[vec![7u8;32]])?;
        c.execute("INSERT INTO browser_registrations VALUES('browser','public-key','owner','account',NULL,42,43)",[])?;
        c.execute("INSERT INTO account_revocations VALUES('account','browser','revoked','{}')",[])?;
        for old in ["owner_credentials","owner_sessions","grantee_sessions","pairing_offers"] {
            assert_eq!(c.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",[old],|r|r.get::<_,u32>(0))?,0);
        }
        Ok(())
    })
    .unwrap();
    drop(db);
    let db = Database::open(&path).unwrap();
    db.with(|c| {
        assert_eq!(
            c.query_row(
                "SELECT relay_device_id,strict FROM device_owner_trust",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
            )?,
            ("relay-device".to_owned(), true)
        );
        assert_eq!(
            c.query_row("SELECT account_key_id FROM device_owner_trust", [], |r| {
                r.get::<_, String>(0)
            })?,
            "account"
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM browser_registrations", [], |r| r
                .get::<_, u32>(0))?,
            1
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM account_revocations", [], |r| r
                .get::<_, u32>(0))?,
            1
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn offline_trust_preparation_accepts_current_and_later_additive_migrations() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("offline.sqlite");
    let database = Database::open(&path).unwrap();
    database
        .with(|c| {
            pockymoe_runtime::db::ensure_device_owner_trust(c)?;
            pockymoe_runtime::db::ensure_device_owner_trust(c)?;
            Ok(())
        })
        .unwrap();
}
#[cfg(unix)]
#[test]
fn newly_created_database_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let path = dir.path().join("private.sqlite");
    let _database = Database::open(&path).unwrap();
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
#[test]
fn second_review_existing_trust_schema_gains_subject_index_without_losing_tombstones() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("index.sqlite");
    let db = Database::open(&path).unwrap();
    db.with(|c| {
        c.execute_batch("DROP INDEX IF EXISTS account_revocations_subject_idx; DELETE FROM __remote_codex_runtime_migrations WHERE version>=16; INSERT INTO account_revocations VALUES('account','browser','subject','{}');")?;
        Ok(())
    }).unwrap();
    drop(db);
    let db = Database::open(&path).unwrap();
    db.with(|c| {
        let columns = c.prepare("SELECT name FROM pragma_index_info('account_revocations_subject_idx') ORDER BY seqno")?
            .query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        assert_eq!(columns, vec!["kind", "subject_id"]);
        assert_eq!(c.query_row("SELECT count(*) FROM account_revocations",[],|r|r.get::<_,u64>(0))?,1);
        Ok(())
    }).unwrap();
}
#[cfg(unix)]
#[test]
fn second_review_open_restricts_existing_wal_and_shm_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let path = dir.path().join("journal.sqlite");
    drop(Database::open(&path).unwrap());
    let keeper = Connection::open(&path).unwrap();
    keeper
        .execute_batch(
            "PRAGMA journal_mode=WAL; INSERT INTO kv(key,value) VALUES('synthetic','preserved');",
        )
        .unwrap();
    let files = [
        path.clone(),
        std::path::PathBuf::from(format!("{}-wal", path.display())),
        std::path::PathBuf::from(format!("{}-shm", path.display())),
    ];
    for file in &files {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let db = Database::open(&path).unwrap();
    for file in &files {
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        db.get_kv("synthetic").unwrap().as_deref(),
        Some("preserved")
    );
    drop(keeper);
}
