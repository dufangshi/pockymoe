use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use pockymoe_protocol::ThreadGoalDto;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use uuid::Uuid;

const RUNTIME_MIGRATION_TABLE: &str = "__remote_codex_runtime_migrations";

struct Migration {
    version: i64,
    name: &'static str,
    apply: fn(&Connection) -> Result<()>,
}

const RUNTIME_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "runtime_schema",
        apply: create_runtime_schema,
    },
    Migration {
        version: 2,
        name: "node_0030_compatibility",
        apply: migrate_node_0030_data,
    },
    Migration {
        version: 3,
        name: "pending_steer_client_request_id",
        apply: add_pending_steer_client_request_id,
    },
    Migration {
        version: 4,
        name: "guard_unsafe_approval_modes",
        apply: guard_unsafe_approval_modes,
    },
    Migration {
        version: 5,
        name: "legacy_turn_metadata_fields",
        apply: add_legacy_turn_metadata_fields,
    },
    Migration {
        version: 6,
        name: "legacy_thread_goals",
        apply: migrate_legacy_thread_goals,
    },
    Migration {
        version: 7,
        name: "thread_context_usage",
        apply: |conn| {
            conn.execute("ALTER TABLE threads ADD COLUMN context_usage_json TEXT", [])?;
            Ok(())
        },
    },
    Migration {
        version: 8,
        name: "durable_prompt_payload",
        apply: |conn| {
            conn.execute(
                "ALTER TABLE thread_pending_steers ADD COLUMN payload_json TEXT",
                [],
            )?;
            Ok(())
        },
    },
    Migration {
        version: 9,
        name: "agent_thread_lineage",
        apply: |conn| {
            // parent = immediate creator; root = top-level ancestor, denormalised so
            // "list the threads a person started" stays one indexed query instead of a
            // recursive walk. Both NULL means user-created, so existing rows need no
            // backfill: they are already top-level by definition.
            conn.execute("ALTER TABLE threads ADD COLUMN parent_thread_id TEXT", [])?;
            conn.execute("ALTER TABLE threads ADD COLUMN root_thread_id TEXT", [])?;
            conn.execute(
                "ALTER TABLE threads ADD COLUMN lineage_depth INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            conn.execute(
                "CREATE INDEX IF NOT EXISTS threads_root_idx
                 ON threads(workspace_id, root_thread_id)",
                [],
            )?;
            conn.execute(
                "CREATE INDEX IF NOT EXISTS threads_parent_idx ON threads(parent_thread_id)",
                [],
            )?;
            Ok(())
        },
    },
    Migration {
        version: 10,
        name: "agent_coordination",
        apply: |conn| {
            // name: a human address unique among the open threads of one lineage.
            // closed_at: the delegate is done and releases its slot and its name;
            // a finished turn alone leaves a thread `idle`, which still holds both.
            // Idempotent: 0.12.55/0.12.56 were cut without this migration, so a
            // device that ran 0.12.53 and then downgraded had its ledger row removed
            // while these columns stayed. Re-running must not fail on them.
            for column in ["agent_name", "agent_role", "worktree_path", "closed_at"] {
                let exists = conn
                    .prepare("SELECT 1 FROM pragma_table_info('threads') WHERE name=?1")?
                    .exists([column])?;
                if !exists {
                    conn.execute(&format!("ALTER TABLE threads ADD COLUMN {column} TEXT"), [])?;
                }
            }
            conn.execute_batch(
                "CREATE UNIQUE INDEX IF NOT EXISTS threads_agent_name_idx
                   ON threads(root_thread_id, agent_name)
                   WHERE agent_name IS NOT NULL AND closed_at IS NULL;
                 CREATE TABLE IF NOT EXISTS agent_tasks(
                   root_thread_id TEXT NOT NULL,
                   number INTEGER NOT NULL,
                   title TEXT NOT NULL,
                   detail TEXT,
                   status TEXT NOT NULL DEFAULT 'pending',
                   owner_thread_id TEXT,
                   created_by TEXT,
                   result TEXT,
                   created_at TEXT NOT NULL,
                   updated_at TEXT NOT NULL,
                   PRIMARY KEY(root_thread_id, number));
                 CREATE TABLE IF NOT EXISTS agent_task_deps(
                   root_thread_id TEXT NOT NULL,
                   number INTEGER NOT NULL,
                   depends_on INTEGER NOT NULL,
                   PRIMARY KEY(root_thread_id, number, depends_on));",
            )?;
            Ok(())
        },
    },
    Migration {
        version: 11,
        name: "device_conversation_search",
        apply: |conn| {
            conn.execute_batch(include_str!("search_index.sql"))?;
            Ok(())
        },
    },
    Migration {
        version: 12,
        name: "file_save_operations",
        apply: |conn| {
            conn.execute_batch(
                "CREATE TABLE file_save_operations (
                actor TEXT NOT NULL, workspace_id TEXT NOT NULL, workspace_revision TEXT NOT NULL,
                operation_id TEXT NOT NULL, input_digest TEXT NOT NULL, intended_hash TEXT,
                before_bytes BLOB, result TEXT NOT NULL, created_at INTEGER NOT NULL,
                PRIMARY KEY(actor,workspace_id,operation_id));",
            )?;
            Ok(())
        },
    },
    Migration {
        version: 13,
        name: "unified_automations",
        apply: |conn| {
            conn.execute_batch(include_str!("automation_schema.sql"))?;
            Ok(())
        },
    },
    Migration {
        version: 14,
        name: "device_owner_trust",
        apply: |conn| {
            conn.execute_batch(include_str!("device_owner_trust.sql"))?;
            Ok(())
        },
    },
    Migration {
        version: 15,
        name: "preview_leases",
        apply: |conn| {
            conn.execute_batch(include_str!("preview_leases.sql"))?;
            Ok(())
        },
    },
    Migration {
        version: 16,
        name: "account_revocation_subject_index",
        apply: |conn| {
            conn.execute("CREATE INDEX IF NOT EXISTS account_revocations_subject_idx ON account_revocations(kind,subject_id)", [])?;
            Ok(())
        },
    },
    Migration {
        version: 17,
        name: "harness_resource_settings",
        apply: |conn| {
            // One row; absent means the defaults. A NULL column disables that rule.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS harness_resource_settings (
                   id INTEGER PRIMARY KEY CHECK (id = 1),
                   idle_release_minutes INTEGER,
                   max_loaded_threads INTEGER,
                   max_harness_memory_bytes INTEGER,
                   updated_at TEXT NOT NULL);",
            )?;
            Ok(())
        },
    },
];

/// The Node Supervisor's migrations, in the order its migrator applied them
/// (`packages/db/migrations` before the Rust rewrite). The runtime migrations start
/// from the schema after the last one; older Node databases are brought up to it
/// by [`complete_node_migrations`].
const NODE_0030_MIGRATIONS: &[(&str, &str)] = &[
    (
        "0000_initial.sql",
        include_str!("node_migrations/0000_initial.sql"),
    ),
    (
        "0001_thread_runtime_fields.sql",
        include_str!("node_migrations/0001_thread_runtime_fields.sql"),
    ),
    (
        "0002_thread_source.sql",
        include_str!("node_migrations/0002_thread_source.sql"),
    ),
    (
        "0003_thread_runtime_settings.sql",
        include_str!("node_migrations/0003_thread_runtime_settings.sql"),
    ),
    (
        "0004_thread_connection_state.sql",
        include_str!("node_migrations/0004_thread_connection_state.sql"),
    ),
    (
        "0005_thread_turn_metadata.sql",
        include_str!("node_migrations/0005_thread_turn_metadata.sql"),
    ),
    (
        "0006_thread_sandbox_mode.sql",
        include_str!("node_migrations/0006_thread_sandbox_mode.sql"),
    ),
    (
        "0007_thread_pending_steers.sql",
        include_str!("node_migrations/0007_thread_pending_steers.sql"),
    ),
    (
        "0008_thread_fast_mode_and_activity_notes.sql",
        include_str!("node_migrations/0008_thread_fast_mode_and_activity_notes.sql"),
    ),
    (
        "0009_thread_turn_token_usage.sql",
        include_str!("node_migrations/0009_thread_turn_token_usage.sql"),
    ),
    (
        "0010_thread_turn_pricing_snapshot.sql",
        include_str!("node_migrations/0010_thread_turn_pricing_snapshot.sql"),
    ),
    (
        "0011_thread_forks.sql",
        include_str!("node_migrations/0011_thread_forks.sql"),
    ),
    (
        "0012_thread_goals.sql",
        include_str!("node_migrations/0012_thread_goals.sql"),
    ),
    (
        "0013_thread_activity_note_anchor.sql",
        include_str!("node_migrations/0013_thread_activity_note_anchor.sql"),
    ),
    (
        "0014_thread_history_items.sql",
        include_str!("node_migrations/0014_thread_history_items.sql"),
    ),
    (
        "0015_agent_provider_fields.sql",
        include_str!("node_migrations/0015_agent_provider_fields.sql"),
    ),
    (
        "0016_remove_codex_thread_goal_id.sql",
        include_str!("node_migrations/0016_remove_codex_thread_goal_id.sql"),
    ),
    (
        "0017_remove_codex_thread_columns.sql",
        include_str!("node_migrations/0017_remove_codex_thread_columns.sql"),
    ),
    (
        "0018_shell_session_label.sql",
        include_str!("node_migrations/0018_shell_session_label.sql"),
    ),
    (
        "0028_thread_turn_display_prompt.sql",
        include_str!("node_migrations/0028_thread_turn_display_prompt.sql"),
    ),
    (
        "0029_thread_turn_delivery.sql",
        include_str!("node_migrations/0029_thread_turn_delivery.sql"),
    ),
    (
        "0030_thread_agent_id.sql",
        include_str!("node_migrations/0030_thread_agent_id.sql"),
    ),
];

pub struct Database {
    conn: Mutex<Connection>,
    pub host_id: String,
    // Keep the OS lock until all SQLite use has ended; never unlink its inode.
    _owner: std::fs::File,
}

pub fn validate_database_path(path: &Path) -> Result<()> {
    let text = path.to_string_lossy();
    if let Some((scheme, rest)) = text.split_once(':') {
        let drive = scheme.len() == 1
            && scheme.as_bytes()[0].is_ascii_alphabetic()
            && (rest.starts_with('/') || rest.starts_with('\\'));
        let uri = !scheme.is_empty()
            && scheme.as_bytes()[0].is_ascii_alphabetic()
            && scheme
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"+.-".contains(&c));
        anyhow::ensure!(drive || !uri,
            "DATABASE_URL must be a local SQLite file path, not a database URL; restore the original supervisor database path to retain its identity and workspaces");
    }
    Ok(())
}

/// Prepare only the additive trust schema for native setup/offline binding. This
/// deliberately does not acquire Supervisor ownership: setup may bind version 13
/// before restarting it into the new runtime. Older schemas still need an upgrade.
pub fn ensure_device_owner_trust(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    validate_database_before_migration(&tx)?;
    let version: i64 = tx.query_row(
        "SELECT COALESCE(max(version),0) FROM __remote_codex_runtime_migrations",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        version >= 13,
        "Update the Supervisor before binding this database"
    );
    for migration in RUNTIME_MIGRATIONS.iter().filter(|m| m.version > version) {
        (migration.apply)(&tx)?;
        tx.execute("INSERT INTO __remote_codex_runtime_migrations(version,name,applied_at) VALUES(?1,?2,?3)", params![migration.version,migration.name,pockymoe_protocol::now_rfc3339()])?;
    }
    tx.commit()?;
    Ok(())
}

/// Restrict both the database and journals left by an earlier installation.
pub fn secure_database_file(path: &Path) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        for suffix in ["-wal", "-shm"] {
            let mut journal_path = path.as_os_str().to_os_string();
            journal_path.push(suffix);
            match std::fs::File::open(std::path::PathBuf::from(journal_path)) {
                Ok(journal) => journal.set_permissions(std::fs::Permissions::from_mode(0o600))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_as_owner(path, true)?.context("database owner lock was not acquired")
    }

    /// Create or migrate the database unless a running Supervisor owns it (that
    /// owner has migrated it already). Returns whether this call opened it.
    pub fn prepare_unowned(path: &Path) -> Result<bool> {
        Ok(Self::open_as_owner(path, false)?.is_some())
    }

    fn open_as_owner(path: &Path, required: bool) -> Result<Option<Self>> {
        // DATABASE_URL is a file path. Never create a new SQLite database from
        // another application's connection URI (which may also contain secrets).
        validate_database_path(path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let absolute = if path.exists() {
            std::fs::canonicalize(path)?
        } else {
            std::fs::canonicalize(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?
            .join(path.file_name().context("database filename missing")?)
        };
        let mut lock_path = absolute.as_os_str().to_os_string();
        lock_path.push(".supervisor.lock");
        let owner = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(std::path::PathBuf::from(lock_path))?;
        let mut attempts = 0;
        loop {
            match owner.try_lock() {
                Ok(()) => break,
                // A running Supervisor holds the lock for its lifetime. A process
                // forked meanwhile can briefly hold an inherited copy until it execs,
                // so only a lock that stays held means the database is owned.
                Err(std::fs::TryLockError::WouldBlock) if !required => {
                    attempts += 1;
                    if attempts == 20 {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => bail!(
                    "This database is already owned by another Supervisor. Use an isolated database or stop its owner before starting another instance."
                ),
            }
        }
        secure_database_file(path)?;
        let mut conn =
            Connection::open(path).with_context(|| format!("open sqlite {}", path.display()))?;

        register_search_functions(&conn)?;
        create_pre_rust_backup(path, &conn)
            .with_context(|| format!("back up Node sqlite {}", path.display()))?;
        complete_node_migrations(&mut conn)
            .with_context(|| format!("upgrade Node sqlite {}", path.display()))?;
        validate_database_before_migration(&conn)
            .with_context(|| format!("validate sqlite {}", path.display()))?;
        conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=FULL;
            PRAGMA foreign_keys=ON;
            ",
        )?;
        run_migrations(&mut conn).with_context(|| format!("migrate sqlite {}", path.display()))?;
        validate_runtime_schema(&conn)
            .with_context(|| format!("validate migrated sqlite {}", path.display()))?;

        let host_id = {
            let existing: Option<String> = conn
                .query_row("SELECT id FROM hosts LIMIT 1", [], |row| row.get(0))
                .optional()?;
            if let Some(id) = existing {
                id
            } else {
                let id = Uuid::new_v4().to_string();
                let now = pockymoe_protocol::now_rfc3339();
                let hostname = hostname::get()
                    .map(|h| h.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "localhost".into());
                conn.execute(
                    "INSERT INTO hosts(id, hostname, platform, created_at, last_seen_at) VALUES (?1,?2,?3,?4,?5)",
                    params![id, hostname, std::env::consts::OS, now, now],
                )?;
                id
            }
        };
        Ok(Some(Self {
            conn: Mutex::new(conn),
            _owner: owner,
            host_id,
        }))
    }

    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().expect("db mutex");
        f(&conn)
    }

    pub fn get_kv(&self, key: &str) -> Result<Option<String>> {
        self.with(|conn| {
            Ok(conn
                .query_row("SELECT value FROM kv WHERE key=?1", params![key], |row| {
                    row.get(0)
                })
                .optional()?)
        })
    }

    pub fn set_kv(&self, key: &str, value: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
    }
}

fn create_pre_rust_backup(path: &Path, conn: &Connection) -> Result<()> {
    if !table_exists(conn, "__migrations")? || table_exists(conn, RUNTIME_MIGRATION_TABLE)? {
        return Ok(());
    }
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("supervisor");
    let backup_path = path.with_file_name(format!("{stem}.pre-rust-0.12.sqlite"));
    if backup_path.exists() {
        return Ok(());
    }
    let temporary_path =
        backup_path.with_extension(format!("sqlite.tmp-{}", Uuid::new_v4().simple()));
    let result = (|| -> Result<()> {
        let mut destination = Connection::open(&temporary_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temporary_path, std::fs::Permissions::from_mode(0o600))?;
        }
        {
            let backup = rusqlite::backup::Backup::new(conn, &mut destination)?;
            backup.run_to_completion(128, Duration::from_millis(5), None)?;
        }
        let quick_check: String =
            destination.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        ensure!(
            quick_check == "ok",
            "pre-Rust SQLite backup integrity check failed: {quick_check}"
        );
        drop(destination);
        match std::fs::hard_link(&temporary_path, &backup_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error.into()),
        }
    })();
    let _ = std::fs::remove_file(&temporary_path);
    result
}

/// Bring a database last written by an older Node Supervisor up to Node migration
/// 0030, the schema the runtime migrations start from. The Node migrator applied
/// these files in order and recorded each name, so its ledger tells which ones are
/// missing. Runs after the pre-Rust backup; a partial upgrade rolls back.
fn complete_node_migrations(conn: &mut Connection) -> Result<()> {
    if !table_exists(conn, "__migrations")? || table_exists(conn, RUNTIME_MIGRATION_TABLE)? {
        return Ok(());
    }
    ensure_columns(conn, "__migrations", &["id", "name", "applied_at"])?;
    let applied: Vec<String> = conn
        .prepare("SELECT name FROM __migrations ORDER BY id")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let known: HashSet<&str> = NODE_0030_MIGRATIONS.iter().map(|(name, _)| *name).collect();
    if applied.len() == known.len() && applied.iter().all(|name| known.contains(name.as_str())) {
        return Ok(());
    }
    // Only an unbroken prefix can be continued; anything else is another schema.
    for (index, name) in applied.iter().enumerate() {
        ensure!(
            NODE_0030_MIGRATIONS
                .get(index)
                .is_some_and(|(expected, _)| expected == name),
            "database was created by an unsupported Node migration `{name}`"
        );
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (name, sql) in &NODE_0030_MIGRATIONS[applied.len()..] {
        tx.execute_batch(sql)
            .with_context(|| format!("apply Node migration {name}"))?;
        tx.execute(
            "INSERT INTO __migrations(name, applied_at) VALUES(?1, ?2)",
            params![name, pockymoe_protocol::now_rfc3339()],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn validate_database_before_migration(conn: &Connection) -> Result<()> {
    let quick_check: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    ensure!(
        quick_check == "ok",
        "SQLite integrity check failed: {quick_check}"
    );

    let tables = table_names(conn)?;
    let application_tables: HashSet<_> = tables
        .iter()
        .filter(|name| name.as_str() != "__migrations" && name.as_str() != RUNTIME_MIGRATION_TABLE)
        .cloned()
        .collect();
    if application_tables.is_empty() {
        validate_migration_ledgers(conn, &tables)?;
        return Ok(());
    }

    for anchor in ["hosts", "workspaces", "threads"] {
        ensure!(
            application_tables.contains(anchor),
            "unsupported SQLite schema: found application tables but `{anchor}` is missing"
        );
    }

    ensure_columns(
        conn,
        "hosts",
        &["id", "hostname", "platform", "created_at", "last_seen_at"],
    )?;
    ensure_columns(
        conn,
        "workspaces",
        &[
            "id",
            "host_id",
            "label",
            "abs_path",
            "is_favorite",
            "created_at",
            "last_opened_at",
        ],
    )?;
    ensure_columns(
        conn,
        "threads",
        &[
            "id",
            "workspace_id",
            "provider",
            "agent_id",
            "provider_session_id",
            "source",
            "title",
            "model",
            "reasoning_effort",
            "fast_mode",
            "collaboration_mode",
            "approval_mode",
            "sandbox_mode",
            "status",
            "summary_text",
            "last_error",
            "created_at",
            "updated_at",
            "last_turn_started_at",
            "last_turn_completed_at",
            "is_pinned",
            "is_connected",
        ],
    )?;

    validate_optional_table(
        conn,
        &tables,
        "thread_turns",
        &[
            "id",
            "thread_id",
            "status",
            "error",
            "model",
            "reasoning_effort",
            "started_at",
            "completed_at",
            "ordinal",
        ],
    )?;
    validate_optional_table(
        conn,
        &tables,
        "thread_turn_metadata",
        &[
            "id",
            "thread_id",
            "turn_id",
            "model",
            "reasoning_effort",
            "token_usage_json",
            "pricing_model_key",
            "pricing_tier_key",
            "display_prompt",
            "created_at",
            "updated_at",
        ],
    )?;
    validate_optional_table(
        conn,
        &tables,
        "thread_history_items",
        &[
            "id",
            "thread_id",
            "turn_id",
            "item_id",
            "item_json",
            "created_at",
            "updated_at",
        ],
    )?;
    validate_optional_table(
        conn,
        &tables,
        "thread_pending_steers",
        &[
            "id",
            "thread_id",
            "turn_id",
            "display_prompt",
            "submitted_prompt",
            "delivery",
            "created_at",
        ],
    )?;
    validate_optional_table(
        conn,
        &tables,
        "auth_sessions",
        &["token", "username", "expires_at"],
    )?;
    validate_optional_table(conn, &tables, "kv", &["key", "value"])?;
    validate_optional_table(conn, &tables, "policies", &["key", "value_json"])?;
    validate_optional_table(
        conn,
        &tables,
        "thread_goals",
        &[
            "id",
            "thread_id",
            "provider_session_id",
            "objective",
            "status",
            "token_budget",
            "tokens_used",
            "time_used_seconds",
            "started_at",
            "completed_at",
            "created_at",
            "updated_at",
        ],
    )?;
    validate_migration_ledgers(conn, &tables)
}

fn validate_migration_ledgers(conn: &Connection, tables: &HashSet<String>) -> Result<()> {
    if tables.contains("__migrations") {
        ensure_columns(conn, "__migrations", &["id", "name", "applied_at"])?;
        let mut stmt = conn.prepare("SELECT name FROM __migrations ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            let name = row?;
            ensure!(
                NODE_0030_MIGRATIONS.iter().any(|(known, _)| *known == name),
                "database was created by an unsupported Node migration `{name}`"
            );
        }
    }

    if tables.contains(RUNTIME_MIGRATION_TABLE) {
        ensure_columns(
            conn,
            RUNTIME_MIGRATION_TABLE,
            &["version", "name", "applied_at"],
        )?;
        let mut stmt = conn.prepare(&format!(
            "SELECT version, name FROM {RUNTIME_MIGRATION_TABLE} ORDER BY version"
        ))?;
        let records: Vec<(i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (index, (version, name)) in records.iter().enumerate() {
            let expected_version = index as i64 + 1;
            ensure!(
                *version == expected_version,
                "runtime migration history has a gap before version {version}"
            );
            let Some(expected) = RUNTIME_MIGRATIONS
                .iter()
                .find(|item| item.version == *version)
            else {
                bail!("database uses unsupported runtime migration version {version}");
            };
            ensure!(
                name == expected.name,
                "runtime migration {version} is named `{name}`, expected `{}`",
                expected.name
            );
        }
    }
    Ok(())
}

fn validate_runtime_schema(conn: &Connection) -> Result<()> {
    for (table, columns) in [
        (
            "hosts",
            &["id", "hostname", "platform", "created_at", "last_seen_at"][..],
        ),
        (
            "workspaces",
            &[
                "id",
                "host_id",
                "label",
                "abs_path",
                "is_favorite",
                "created_at",
                "last_opened_at",
            ][..],
        ),
        (
            "threads",
            &[
                "id",
                "workspace_id",
                "provider",
                "agent_id",
                "provider_session_id",
                "source",
                "title",
                "model",
                "reasoning_effort",
                "fast_mode",
                "collaboration_mode",
                "approval_mode",
                "sandbox_mode",
                "status",
                "summary_text",
                "last_error",
                "created_at",
                "updated_at",
                "last_turn_started_at",
                "last_turn_completed_at",
                "is_pinned",
                "is_connected",
            ][..],
        ),
        (
            "thread_turns",
            &[
                "id",
                "thread_id",
                "status",
                "error",
                "model",
                "reasoning_effort",
                "token_usage_json",
                "pricing_model_key",
                "pricing_tier_key",
                "display_prompt",
                "started_at",
                "completed_at",
                "ordinal",
            ][..],
        ),
        (
            "thread_history_items",
            &[
                "id",
                "thread_id",
                "turn_id",
                "item_id",
                "item_json",
                "created_at",
                "updated_at",
            ][..],
        ),
        (
            "thread_pending_steers",
            &[
                "id",
                "thread_id",
                "turn_id",
                "client_request_id",
                "display_prompt",
                "submitted_prompt",
                "delivery",
                "created_at",
                "updated_at",
            ][..],
        ),
        ("auth_sessions", &["token", "username", "expires_at"][..]),
        ("kv", &["key", "value"][..]),
    ] {
        ensure_columns(conn, table, columns)?;
    }
    Ok(())
}

fn validate_optional_table(
    conn: &Connection,
    tables: &HashSet<String>,
    table: &str,
    columns: &[&str],
) -> Result<()> {
    if tables.contains(table) {
        ensure_columns(conn, table, columns)?;
    }
    Ok(())
}

fn ensure_columns(conn: &Connection, table: &str, required: &[&str]) -> Result<()> {
    let columns = table_columns(conn, table)?;
    ensure!(!columns.is_empty(), "required table `{table}` is missing");
    for column in required {
        ensure!(
            columns.contains(*column),
            "unsupported `{table}` schema: required column `{column}` is missing"
        );
    }
    Ok(())
}

fn table_names(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(names)
}

fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let columns = stmt
        .query_map(params![table], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(columns)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1",
            params![table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn run_migrations(conn: &mut Connection) -> Result<()> {
    // Keep this ledger separate so the legacy Node migrator can still inspect its own history.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(&format!(
        "
        CREATE TABLE IF NOT EXISTS {RUNTIME_MIGRATION_TABLE} (
          version INTEGER PRIMARY KEY,
          name TEXT NOT NULL UNIQUE,
          applied_at TEXT NOT NULL
        );
        "
    ))?;

    let mut applied = HashMap::new();
    {
        let mut stmt = tx.prepare(&format!(
            "SELECT version, name FROM {RUNTIME_MIGRATION_TABLE} ORDER BY version"
        ))?;
        for row in stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))? {
            let (version, name): (i64, String) = row?;
            applied.insert(version, name);
        }
    }

    for migration in RUNTIME_MIGRATIONS {
        if let Some(name) = applied.get(&migration.version) {
            ensure!(
                name == migration.name,
                "runtime migration {} is named `{name}`, expected `{}`",
                migration.version,
                migration.name
            );
            continue;
        }
        (migration.apply)(&tx).with_context(|| {
            format!(
                "apply runtime migration {} ({})",
                migration.version, migration.name
            )
        })?;
        tx.execute(
            &format!(
                "INSERT INTO {RUNTIME_MIGRATION_TABLE}(version, name, applied_at) VALUES (?1,?2,?3)"
            ),
            params![
                migration.version,
                migration.name,
                pockymoe_protocol::now_rfc3339()
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn create_runtime_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS hosts (
          id TEXT PRIMARY KEY,
          hostname TEXT NOT NULL,
          platform TEXT NOT NULL,
          created_at TEXT NOT NULL,
          last_seen_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS workspaces (
          id TEXT PRIMARY KEY,
          host_id TEXT NOT NULL,
          label TEXT NOT NULL,
          abs_path TEXT NOT NULL UNIQUE,
          is_favorite INTEGER NOT NULL DEFAULT 0,
          created_at TEXT NOT NULL,
          last_opened_at TEXT
        );
        CREATE TABLE IF NOT EXISTS threads (
          id TEXT PRIMARY KEY,
          workspace_id TEXT NOT NULL,
          provider TEXT NOT NULL,
          agent_id TEXT,
          provider_session_id TEXT,
          source TEXT NOT NULL DEFAULT 'supervisor',
          title TEXT NOT NULL,
          model TEXT,
          reasoning_effort TEXT,
          fast_mode INTEGER NOT NULL DEFAULT 0,
          collaboration_mode TEXT NOT NULL DEFAULT 'default',
          approval_mode TEXT NOT NULL DEFAULT 'guarded',
          sandbox_mode TEXT,
          status TEXT,
          summary_text TEXT,
          last_error TEXT,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          last_turn_started_at TEXT,
          last_turn_completed_at TEXT,
          is_pinned INTEGER NOT NULL DEFAULT 0,
          is_connected INTEGER NOT NULL DEFAULT 1
        );
        CREATE TABLE IF NOT EXISTS thread_turns (
          id TEXT PRIMARY KEY,
          thread_id TEXT NOT NULL,
          status TEXT NOT NULL,
          error TEXT,
          model TEXT,
          reasoning_effort TEXT,
          token_usage_json TEXT,
          pricing_model_key TEXT,
          pricing_tier_key TEXT,
          display_prompt TEXT,
          started_at TEXT,
          completed_at TEXT,
          ordinal INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS thread_turns_thread_ordinal_idx
          ON thread_turns(thread_id, ordinal);
        CREATE TABLE IF NOT EXISTS thread_history_items (
          id TEXT PRIMARY KEY,
          thread_id TEXT NOT NULL,
          turn_id TEXT NOT NULL,
          item_id TEXT NOT NULL,
          item_json TEXT NOT NULL,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          UNIQUE(thread_id, turn_id, item_id)
        );
        CREATE UNIQUE INDEX IF NOT EXISTS thread_history_items_thread_turn_item_idx
          ON thread_history_items(thread_id, turn_id, item_id);
        -- Summary reads must not scan/decode large command and tool payloads.
        CREATE INDEX IF NOT EXISTS thread_history_conversation_idx
          ON thread_history_items(thread_id, turn_id, created_at)
          WHERE json_extract(item_json, '$.kind') IN ('userMessage', 'agentMessage');
        CREATE TABLE IF NOT EXISTS thread_pending_steers (
          id TEXT PRIMARY KEY,
          thread_id TEXT NOT NULL,
          turn_id TEXT NOT NULL,
          client_request_id TEXT,
          display_prompt TEXT NOT NULL,
          submitted_prompt TEXT NOT NULL,
          delivery TEXT NOT NULL DEFAULT 'steer',
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS thread_pending_steers_thread_created_idx
          ON thread_pending_steers(thread_id, created_at);
        CREATE TABLE IF NOT EXISTS auth_sessions (
          token TEXT PRIMARY KEY,
          username TEXT NOT NULL,
          expires_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS kv (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL
        );
        ",
    )?;
    Ok(())
}

fn migrate_node_0030_data(conn: &Connection) -> Result<()> {
    let pending_columns = table_columns(conn, "thread_pending_steers")?;
    if !pending_columns.contains("updated_at") {
        conn.execute_batch(
            "
            ALTER TABLE thread_pending_steers
              ADD COLUMN updated_at TEXT NOT NULL DEFAULT '';
            UPDATE thread_pending_steers
              SET updated_at=created_at
              WHERE updated_at='';
            ",
        )?;
    } else {
        conn.execute(
            "UPDATE thread_pending_steers SET updated_at=created_at WHERE updated_at IS NULL OR updated_at=''",
            [],
        )?;
    }

    backfill_legacy_turns(conn)?;
    migrate_legacy_policies(conn)?;
    Ok(())
}

fn add_pending_steer_client_request_id(conn: &Connection) -> Result<()> {
    if !table_columns(conn, "thread_pending_steers")?.contains("client_request_id") {
        conn.execute(
            "ALTER TABLE thread_pending_steers ADD COLUMN client_request_id TEXT",
            [],
        )?;
    }
    Ok(())
}

fn guard_unsafe_approval_modes(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE threads
         SET approval_mode='guarded'
         WHERE approval_mode IS NULL
            OR TRIM(approval_mode)=''
            OR approval_mode NOT IN ('yolo','guarded')",
        [],
    )?;
    Ok(())
}

fn add_legacy_turn_metadata_fields(conn: &Connection) -> Result<()> {
    let mut columns = table_columns(conn, "thread_turns")?;
    for column in [
        "token_usage_json",
        "pricing_model_key",
        "pricing_tier_key",
        "display_prompt",
    ] {
        if !columns.contains(column) {
            conn.execute(
                &format!("ALTER TABLE thread_turns ADD COLUMN {column} TEXT"),
                [],
            )?;
            columns.insert(column.to_string());
        }
    }
    if table_exists(conn, "thread_turn_metadata")? {
        conn.execute_batch(
            "
            UPDATE thread_turns
            SET token_usage_json=COALESCE(
                  token_usage_json,
                  (SELECT metadata.token_usage_json
                   FROM thread_turn_metadata AS metadata
                   WHERE metadata.thread_id=thread_turns.thread_id
                     AND metadata.turn_id=thread_turns.id)
                ),
                pricing_model_key=COALESCE(
                  pricing_model_key,
                  (SELECT metadata.pricing_model_key
                   FROM thread_turn_metadata AS metadata
                   WHERE metadata.thread_id=thread_turns.thread_id
                     AND metadata.turn_id=thread_turns.id)
                ),
                pricing_tier_key=COALESCE(
                  pricing_tier_key,
                  (SELECT metadata.pricing_tier_key
                   FROM thread_turn_metadata AS metadata
                   WHERE metadata.thread_id=thread_turns.thread_id
                     AND metadata.turn_id=thread_turns.id)
                ),
                display_prompt=COALESCE(
                  display_prompt,
                  (SELECT metadata.display_prompt
                   FROM thread_turn_metadata AS metadata
                   WHERE metadata.thread_id=thread_turns.thread_id
                     AND metadata.turn_id=thread_turns.id)
                )
            WHERE EXISTS (
              SELECT 1
              FROM thread_turn_metadata AS metadata
              WHERE metadata.thread_id=thread_turns.thread_id
                AND metadata.turn_id=thread_turns.id
            );
            ",
        )?;
    }
    Ok(())
}

fn migrate_legacy_thread_goals(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "thread_goals")? {
        return Ok(());
    }

    // Node keeps goal history, while the Rust runtime currently keeps one local snapshot.
    // Match Node's fallback selection: newest active state, otherwise newest termination.
    let goals = {
        let mut stmt = conn.prepare(
            "SELECT goals.id, goals.thread_id, goals.objective, goals.status,
                    goals.token_budget, goals.tokens_used, goals.time_used_seconds,
                    goals.created_at, goals.updated_at, goals.completed_at
             FROM thread_goals AS goals
             JOIN threads ON threads.id=goals.thread_id
             WHERE goals.status IN ('active','paused','budgetLimited','terminated')
               AND TRIM(goals.objective)<>''
               AND (goals.token_budget IS NULL OR goals.token_budget>0)
               AND goals.tokens_used>=0
               AND goals.time_used_seconds>=0
               AND TRIM(goals.created_at)<>''
               AND TRIM(goals.updated_at)<>''
             ORDER BY goals.thread_id,
                      CASE WHEN goals.status IN ('active','paused','budgetLimited')
                           THEN 0 ELSE 1 END,
                      goals.updated_at DESC,
                      goals.created_at DESC,
                      goals.id DESC",
        )?;
        let records = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        records
    };

    let mut migrated_threads = HashSet::new();
    for (
        id,
        thread_id,
        objective,
        status,
        token_budget,
        tokens_used,
        time_used_seconds,
        created_at,
        updated_at,
        completed_at,
    ) in goals
    {
        if !migrated_threads.insert(thread_id.clone()) {
            continue;
        }
        let terminal_completed_at = if matches!(status.as_str(), "complete" | "terminated") {
            completed_at.or_else(|| Some(updated_at.clone()))
        } else {
            None
        };
        let goal = ThreadGoalDto {
            // Rust routes identify the goal by the local thread id. The legacy table retains
            // provider_session_id independently for a future Node rollback.
            thread_id: thread_id.clone(),
            local_goal_id: Some(id),
            objective,
            status,
            token_budget: token_budget.map(u64::try_from).transpose()?,
            tokens_used: u64::try_from(tokens_used)?,
            time_used_seconds: u64::try_from(time_used_seconds)?,
            created_at,
            updated_at,
            completed_at: terminal_completed_at,
        };
        conn.execute(
            "INSERT INTO kv(key,value) VALUES (?1,?2) ON CONFLICT(key) DO NOTHING",
            params![
                format!("thread_goal:{thread_id}"),
                serde_json::to_string(&goal)?
            ],
        )?;
    }
    Ok(())
}

#[derive(Clone)]
struct LegacyTurn {
    thread_id: String,
    turn_id: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    started_at: String,
    completed_at: String,
}

fn backfill_legacy_turns(conn: &Connection) -> Result<()> {
    // Metadata has richer turn settings; history supplies turns that were persisted without it.
    let mut legacy = BTreeMap::<(String, String), LegacyTurn>::new();
    if table_exists(conn, "thread_turn_metadata")? {
        let records = {
            let mut stmt = conn.prepare(
                "SELECT thread_id, turn_id, model, reasoning_effort, created_at, updated_at
                 FROM thread_turn_metadata",
            )?;
            let records = stmt
                .query_map([], |row| {
                    Ok(LegacyTurn {
                        thread_id: row.get(0)?,
                        turn_id: row.get(1)?,
                        model: row.get(2)?,
                        reasoning_effort: row.get(3)?,
                        started_at: row.get(4)?,
                        completed_at: row.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            records
        };
        for turn in records {
            legacy.insert((turn.thread_id.clone(), turn.turn_id.clone()), turn);
        }
    }

    if table_exists(conn, "thread_history_items")? {
        let records = {
            let mut stmt = conn.prepare(
                "SELECT thread_id, turn_id, MIN(created_at), MAX(updated_at)
                 FROM thread_history_items
                 GROUP BY thread_id, turn_id",
            )?;
            let records = stmt
                .query_map([], |row| {
                    Ok(LegacyTurn {
                        thread_id: row.get(0)?,
                        turn_id: row.get(1)?,
                        model: None,
                        reasoning_effort: None,
                        started_at: row.get(2)?,
                        completed_at: row.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            records
        };
        for turn in records {
            legacy
                .entry((turn.thread_id.clone(), turn.turn_id.clone()))
                .or_insert(turn);
        }
    }

    let mut legacy_owners = HashMap::<String, String>::new();
    for turn in legacy.values() {
        ensure!(
            !turn.thread_id.trim().is_empty() && !turn.turn_id.trim().is_empty(),
            "legacy turn has an empty thread or turn id"
        );
        if let Some(owner) = legacy_owners.insert(turn.turn_id.clone(), turn.thread_id.clone()) {
            ensure!(
                owner == turn.thread_id,
                "legacy turn id `{}` is shared by threads `{owner}` and `{}`; runtime turn ids must be globally unique",
                turn.turn_id,
                turn.thread_id
            );
        }
    }

    let existing_owners: HashMap<String, String> = {
        let mut stmt = conn.prepare("SELECT id, thread_id FROM thread_turns")?;
        let owners = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        owners
    };
    for turn in legacy.values() {
        if let Some(owner) = existing_owners.get(&turn.turn_id) {
            ensure!(
                owner == &turn.thread_id,
                "runtime turn id `{}` belongs to thread `{owner}`, but legacy data assigns it to `{}`",
                turn.turn_id,
                turn.thread_id
            );
        }
    }

    let active_turns: HashMap<String, String> =
        if table_columns(conn, "threads")?.contains("provider_turn_id") {
            let mut stmt = conn.prepare(
                "SELECT id, provider_turn_id FROM threads
                 WHERE status='running' AND provider_turn_id IS NOT NULL",
            )?;
            let active = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            active
        } else {
            HashMap::new()
        };

    let mut by_thread = BTreeMap::<String, Vec<LegacyTurn>>::new();
    for turn in legacy.into_values() {
        if !existing_owners.contains_key(&turn.turn_id) {
            by_thread
                .entry(turn.thread_id.clone())
                .or_default()
                .push(turn);
        }
    }
    for (thread_id, turns) in &mut by_thread {
        turns.sort_by(|left, right| {
            left.started_at
                .cmp(&right.started_at)
                .then_with(|| left.turn_id.cmp(&right.turn_id))
        });
        let mut ordinal: i64 = conn.query_row(
            "SELECT COALESCE(MAX(ordinal), 0) FROM thread_turns WHERE thread_id=?1",
            params![thread_id],
            |row| row.get(0),
        )?;
        for turn in turns {
            ordinal += 1;
            let in_progress = active_turns.get(thread_id) == Some(&turn.turn_id);
            let status = if in_progress {
                "inProgress"
            } else {
                "completed"
            };
            let completed_at = (!in_progress).then_some(turn.completed_at.as_str());
            conn.execute(
                "INSERT INTO thread_turns(
                   id, thread_id, status, error, model, reasoning_effort,
                   started_at, completed_at, ordinal
                 ) VALUES (?1,?2,?3,NULL,?4,?5,?6,?7,?8)",
                params![
                    turn.turn_id,
                    turn.thread_id,
                    status,
                    turn.model,
                    turn.reasoning_effort,
                    turn.started_at,
                    completed_at,
                    ordinal
                ],
            )?;
        }
    }
    Ok(())
}

fn migrate_legacy_policies(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "policies")? {
        return Ok(());
    }
    let policies: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT key, value_json FROM policies")?;
        let policies = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        policies
    };
    let policy_map: HashMap<&str, &str> = policies
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();

    for (key, value) in &policies {
        // Unknown policies remain recoverable even when Rust has no equivalent setting yet.
        conn.execute(
            "INSERT INTO kv(key, value) VALUES (?1,?2) ON CONFLICT(key) DO NOTHING",
            params![format!("legacy-policy:{key}"), value],
        )?;
    }

    let dev_home = policy_map
        .get("dev_home")
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| {
            value
                .get("absPath")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    let default_backend = policy_map
        .get("default_backend")
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| {
            value
                .get("provider")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .filter(|provider| matches!(provider.as_str(), "codex" | "claude" | "opencode" | "acp"));
    if dev_home.is_some() || default_backend.is_some() {
        let workspace_settings = json!({
            "workspaceRoot": "",
            "devHome": dev_home.unwrap_or_default(),
            "defaultBackend": default_backend.unwrap_or_else(|| "codex".to_string()),
        });
        conn.execute(
            "INSERT INTO kv(key, value) VALUES ('workspace_settings',?1) ON CONFLICT(key) DO NOTHING",
            params![serde_json::to_string(&workspace_settings)?],
        )?;
    }

    if let Some(settings) = policy_map
        .get("plugins")
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
    {
        if let Some(enabled) = settings.get("enabled").and_then(Value::as_object) {
            for (plugin_id, setting) in enabled {
                let enabled = setting
                    .as_bool()
                    .or_else(|| setting.get("enabled").and_then(Value::as_bool));
                if let Some(enabled) = enabled {
                    conn.execute(
                        "INSERT INTO kv(key, value) VALUES (?1,?2) ON CONFLICT(key) DO NOTHING",
                        params![format!("plugin:{plugin_id}:enabled"), enabled.to_string()],
                    )?;
                }
            }
        }
    }
    Ok(())
}

// Deterministic functions are shared by migration backfill and transactional
// triggers. Rust folding handles Unicode beyond SQLite's ASCII-only lower().
fn register_search_functions(conn: &Connection) -> Result<()> {
    use rusqlite::functions::FunctionFlags;
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    conn.create_scalar_function("search_fold", 1, flags, |ctx| {
        Ok(ctx.get::<String>(0)?.to_lowercase())
    })?;
    conn.create_scalar_function("search_body", 3, flags, |ctx| {
        let text = ctx.get::<Option<String>>(0)?.unwrap_or_default();
        let kind = ctx.get::<String>(1)?;
        let source = ctx.get::<Option<String>>(2)?;
        Ok(
            if source.as_deref() == Some("local_codex_import") && kind == "userMessage" {
                crate::local_sessions::sanitize_codex_user_text(&text)
            } else {
                Some(text)
            },
        )
    })?;
    Ok(())
}
