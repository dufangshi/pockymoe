//! Relay persistence: the SQLite store plus the one-way migration from the
//! legacy Node data directory. Kept separate from lib.rs so the migration
//! and validation paths can be read without the request handlers.
use super::*;

pub(crate) struct RelayStore {
    pub(crate) conn: Arc<Mutex<Connection>>,
    pub(crate) session_secret: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayMigrationReport {
    pub source_kind: String,
    pub source_path: Option<PathBuf>,
    pub destination_path: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub action: String,
    pub applied: bool,
    pub ready_for_rust: bool,
    pub user_count: i64,
    pub device_count: i64,
    pub share_count: i64,
    pub grant_count: i64,
    pub oauth_identity_count: i64,
    pub pending_registration_count: i64,
    pub active_unsupported_settings: Vec<String>,
    pub unsupported_data_allowed: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RelayMigrationOptions {
    pub allow_unsupported_data: bool,
}

pub fn inspect_relay_migration(data_dir: impl AsRef<FsPath>) -> Result<RelayMigrationReport> {
    let data_dir = data_dir.as_ref();
    let canonical_path = data_dir.join("relay-store.sqlite");
    let legacy_rust_path = data_dir.join("relay.sqlite");
    if canonical_path.exists() {
        let counts = relay_database_counts(&canonical_path, true)?;
        let unsupported = unsupported_relay_data(&canonical_path)?;
        let ready_for_rust = rust_schema_ready(&canonical_path)?;
        return Ok(RelayMigrationReport {
            source_kind: "node".to_string(),
            source_path: Some(canonical_path.clone()),
            destination_path: canonical_path,
            backup_path: Some(data_dir.join("relay-store.pre-rust-0.12.sqlite")),
            action: if ready_for_rust {
                "already-ready".to_string()
            } else {
                "backup-and-validate-canonical".to_string()
            },
            applied: false,
            ready_for_rust,
            user_count: counts.0,
            device_count: counts.1,
            share_count: counts.2,
            grant_count: counts.3,
            oauth_identity_count: unsupported.0,
            pending_registration_count: unsupported.1,
            active_unsupported_settings: unsupported.2,
            unsupported_data_allowed: false,
        });
    }
    if legacy_rust_path.exists() {
        let counts = relay_database_counts(&legacy_rust_path, false)?;
        return Ok(RelayMigrationReport {
            source_kind: "legacy-rust".to_string(),
            source_path: Some(legacy_rust_path),
            destination_path: canonical_path,
            backup_path: None,
            action: "copy-and-import-legacy-rust".to_string(),
            applied: false,
            ready_for_rust: false,
            user_count: counts.0,
            device_count: counts.1,
            share_count: counts.2,
            grant_count: counts.3,
            oauth_identity_count: 0,
            pending_registration_count: 0,
            active_unsupported_settings: Vec::new(),
            unsupported_data_allowed: false,
        });
    }
    Ok(RelayMigrationReport {
        source_kind: "none".to_string(),
        source_path: None,
        destination_path: canonical_path,
        backup_path: None,
        action: "initialize-new-database".to_string(),
        applied: false,
        ready_for_rust: false,
        user_count: 0,
        device_count: 0,
        share_count: 0,
        grant_count: 0,
        oauth_identity_count: 0,
        pending_registration_count: 0,
        active_unsupported_settings: Vec::new(),
        unsupported_data_allowed: false,
    })
}

pub fn migrate_relay_data_dir(data_dir: impl AsRef<FsPath>) -> Result<RelayMigrationReport> {
    migrate_relay_data_dir_with_options(data_dir, RelayMigrationOptions::default())
}

pub fn migrate_relay_data_dir_with_options(
    data_dir: impl AsRef<FsPath>,
    options: RelayMigrationOptions,
) -> Result<RelayMigrationReport> {
    let data_dir = data_dir.as_ref();
    std::fs::create_dir_all(data_dir)?;
    let plan = inspect_relay_migration(data_dir)?;
    if !options.allow_unsupported_data && !plan.active_unsupported_settings.is_empty() {
        bail!(
            "relay migration blocked by unsupported active settings: {:?}; rerun only after resolving them or explicitly pass --allow-unsupported-data",
            plan.active_unsupported_settings
        );
    }
    let created_destination = plan.source_kind == "legacy-rust";
    match plan.source_kind.as_str() {
        "node" => {
            if let Some(backup_path) = plan.backup_path.as_ref() {
                if !backup_path.exists() {
                    backup_database(&plan.destination_path, backup_path)?;
                }
            }
        }
        "legacy-rust" => {
            let source = plan
                .source_path
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("legacy relay migration source is missing"))?;
            if plan.destination_path.exists() {
                bail!(
                    "refusing to overwrite existing {}",
                    plan.destination_path.display()
                );
            }
            backup_database(source, &plan.destination_path)?;
        }
        "none" => bail!("no relay database exists under {}", data_dir.display()),
        other => bail!("unsupported relay migration source: {other}"),
    }

    let migration_result = (|| -> Result<(i64, i64, i64, i64)> {
        let store = RelayStore::open(
            plan.destination_path.clone(),
            "relay-migration-validation-only".to_string(),
        )?;
        drop(store);
        let counts = relay_database_counts(&plan.destination_path, true)?;
        let expected = (
            plan.user_count,
            plan.device_count,
            plan.share_count,
            plan.grant_count,
        );
        if counts != expected {
            bail!(
                "relay migration count mismatch: expected users/devices/shares/grants={expected:?}, got {counts:?}; source database was preserved"
            );
        }
        validate_relay_database(&plan.destination_path)?;
        let conn = Connection::open(&plan.destination_path)?;
        set_relay_setting(&conn, "rustSchemaVersion", "1")?;
        Ok(counts)
    })();
    let counts = match migration_result {
        Ok(counts) => counts,
        Err(error) => {
            if created_destination {
                remove_failed_migration_destination(&plan.destination_path);
            }
            return Err(error);
        }
    };
    Ok(RelayMigrationReport {
        applied: true,
        ready_for_rust: true,
        user_count: counts.0,
        device_count: counts.1,
        share_count: counts.2,
        grant_count: counts.3,
        unsupported_data_allowed: options.allow_unsupported_data,
        ..plan
    })
}

pub(crate) fn unsupported_relay_data(path: &FsPath) -> Result<(i64, i64, Vec<String>)> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let count = |table: &str, condition: &str| -> Result<i64> {
        if !table_exists(&conn, table) {
            return Ok(0);
        }
        Ok(conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE {condition}"),
            [],
            |row| row.get(0),
        )?)
    };
    let mut active_settings = Vec::new();
    if table_exists(&conn, "relay_settings") {
        let key = "emailVerificationEnabled";
        let enabled: Option<String> = conn
            .query_row(
                "SELECT value FROM relay_settings WHERE key=?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        if enabled.as_deref() == Some("true") {
            active_settings.push(key.to_string());
        }
    }
    Ok((
        count("relay_user_identities", "1=1")?,
        count("relay_pending_registrations", "status='pending'")?,
        active_settings,
    ))
}

pub(crate) fn rust_schema_ready(path: &FsPath) -> Result<bool> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !table_exists(&conn, "relay_settings") {
        return Ok(false);
    }
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM relay_settings WHERE key='rustSchemaVersion'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(version.as_deref() == Some("1"))
}

pub(crate) fn validate_relay_database(path: &FsPath) -> Result<()> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let quick_check: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if quick_check != "ok" {
        bail!("relay database quick_check failed: {quick_check}");
    }
    let foreign_key_errors: i64 =
        conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if foreign_key_errors != 0 {
        bail!("relay database has {foreign_key_errors} foreign key violations");
    }
    Ok(())
}

pub(crate) fn remove_failed_migration_destination(path: &FsPath) {
    let _ = std::fs::remove_file(path);
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(sidecar));
    }
}

pub(crate) fn relay_database_counts(
    path: &FsPath,
    canonical: bool,
) -> Result<(i64, i64, i64, i64)> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let names = if canonical {
        [
            "relay_users",
            "relay_devices",
            "relay_shares",
            "relay_access_grants",
        ]
    } else {
        ["users", "devices", "shares", "grants"]
    };
    let mut counts = [0_i64; 4];
    for (index, table) in names.into_iter().enumerate() {
        if table_exists(&conn, table) {
            counts[index] =
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
        }
    }
    Ok((counts[0], counts[1], counts[2], counts[3]))
}

impl RelayStore {
    pub(crate) fn open_data_dir(
        data_dir: &FsPath,
        session_secret: String,
        allow_legacy_migration: bool,
    ) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let canonical_path = data_dir.join("relay-store.sqlite");
        let legacy_rust_path = data_dir.join("relay.sqlite");
        let creating_new = !canonical_path.exists() && !legacy_rust_path.exists();

        if canonical_path.exists() {
            if !rust_schema_ready(&canonical_path)? {
                if !allow_legacy_migration {
                    bail!(
                        "relay-store.sqlite has not been approved for Rust; run `pockymoe relay-migrate --data-dir {}` first, or explicitly set POCKYMOE_RELAY_AUTO_MIGRATE=1",
                        data_dir.display()
                    );
                }
                migrate_relay_data_dir(data_dir)?;
            }
        } else if legacy_rust_path.exists() {
            if !allow_legacy_migration {
                bail!(
                    "legacy relay database found at {}; run `pockymoe relay-migrate --data-dir {}` first, or explicitly set POCKYMOE_RELAY_AUTO_MIGRATE=1",
                    legacy_rust_path.display(),
                    data_dir.display()
                );
            }
            migrate_relay_data_dir(data_dir)?;
        }

        let store = Self::open(canonical_path.clone(), session_secret)?;
        if creating_new {
            let conn = store
                .conn
                .try_lock()
                .map_err(|_| anyhow::anyhow!("new relay database is unexpectedly busy"))?;
            set_relay_setting(&conn, "rustSchemaVersion", "1")?;
        }
        Ok(store)
    }

    pub(crate) fn open(path: PathBuf, session_secret: String) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS relay_settings (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS relay_users (
              id TEXT PRIMARY KEY,
              email TEXT NOT NULL UNIQUE,
              username TEXT NOT NULL UNIQUE,
              role TEXT NOT NULL CHECK (role IN ('admin', 'user')),
              enabled INTEGER NOT NULL DEFAULT 1,
              last_seen_at TEXT,
              created_at TEXT NOT NULL,
              password_salt TEXT NOT NULL,
              password_hash TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS relay_devices (
              id TEXT PRIMARY KEY,
              owner_user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
              name TEXT NOT NULL,
              token TEXT,
              token_hash TEXT NOT NULL UNIQUE,
              token_preview TEXT NOT NULL,
              created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS relay_devices_owner_idx
              ON relay_devices(owner_user_id);
            CREATE TABLE IF NOT EXISTS relay_device_certificates (
              device_id TEXT PRIMARY KEY REFERENCES relay_devices(id) ON DELETE CASCADE,
              certificate_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS relay_public_links (
              id TEXT PRIMARY KEY,
              owner_user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
              device_id TEXT NOT NULL REFERENCES relay_devices(id) ON DELETE CASCADE,
              thread_id TEXT NOT NULL,
              snapshot_json TEXT NOT NULL,
              created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS relay_public_links_owner_idx ON relay_public_links(owner_user_id,device_id,thread_id);
            CREATE TABLE IF NOT EXISTS relay_public_link_sources (
              link_id TEXT PRIMARY KEY REFERENCES relay_public_links(id) ON DELETE CASCADE,
              publication_token TEXT NOT NULL,
              refreshed_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS relay_shares (
              id TEXT PRIMARY KEY,
              owner_user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
              owner_username TEXT,
              target_user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
              target_username TEXT,
              device_id TEXT NOT NULL REFERENCES relay_devices(id) ON DELETE CASCADE,
              device_name TEXT,
              thread_id TEXT NOT NULL,
              thread_title TEXT,
              workspace_id TEXT,
              workspace_label TEXT,
              label TEXT,
              thread_access TEXT NOT NULL DEFAULT 'control',
              workspace_access TEXT NOT NULL DEFAULT 'none',
              created_at TEXT NOT NULL,
              revoked_at TEXT,
              expires_at TEXT
            );
            CREATE INDEX IF NOT EXISTS relay_shares_owner_idx ON relay_shares(owner_user_id);
            CREATE INDEX IF NOT EXISTS relay_shares_target_idx ON relay_shares(target_user_id);
            CREATE INDEX IF NOT EXISTS relay_shares_device_thread_idx ON relay_shares(device_id, thread_id);
            CREATE TABLE IF NOT EXISTS relay_access_grants (
              id TEXT PRIMARY KEY,
              owner_user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
              owner_username TEXT,
              target_user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
              target_username TEXT,
              device_id TEXT NOT NULL REFERENCES relay_devices(id) ON DELETE CASCADE,
              device_name TEXT,
              scope TEXT NOT NULL CHECK (scope IN ('thread', 'workspace', 'device')),
              thread_id TEXT,
              thread_title TEXT,
              workspace_id TEXT,
              workspace_label TEXT,
              workspace_scope TEXT NOT NULL DEFAULT 'all',
              workspace_ids TEXT NOT NULL DEFAULT '[]',
              label TEXT,
              thread_access TEXT NOT NULL DEFAULT 'control',
              workspace_access TEXT NOT NULL DEFAULT 'none',
              can_create_threads INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL,
              revoked_at TEXT,
              expires_at TEXT
            );
            CREATE INDEX IF NOT EXISTS relay_access_grants_owner_idx ON relay_access_grants(owner_user_id);
            CREATE INDEX IF NOT EXISTS relay_access_grants_target_idx ON relay_access_grants(target_user_id);
            CREATE INDEX IF NOT EXISTS relay_access_grants_device_scope_idx ON relay_access_grants(device_id, scope);
            ",
        )?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS relay_user_profiles (
            user_id TEXT PRIMARY KEY REFERENCES relay_users(id) ON DELETE CASCADE,
            avatar_data_url TEXT
        );",
        )?;
        ensure_registration_schema(&conn)?;
        drop_retired_hosted_tables(&conn)?;
        migrate_legacy_rust_tables(&mut conn)?;
        security::ensure_schema(&conn)?;
        device_tokens::migrate(&conn, &session_secret)?;
        auth_factors::ensure_schema(&conn)?;
        share_activity::ensure_schema(&conn)?;
        notifications::ensure_schema(&conn)?;
        workbench::ensure_schema(&conn)?;
        preferences::ensure_schema(&conn)?;
        thread_groups::ensure_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            session_secret,
        })
    }
}

pub(crate) fn backup_database(source_path: &FsPath, destination_path: &FsPath) -> Result<()> {
    let temporary_path =
        destination_path.with_extension(format!("sqlite.tmp-{}", Uuid::new_v4().simple()));
    let backup_result = (|| -> Result<()> {
        let source = Connection::open_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut destination = Connection::open(&temporary_path)?;
        {
            let backup = Backup::new(&source, &mut destination)?;
            backup.run_to_completion(128, Duration::from_millis(10), None)?;
        }
        destination.close().map_err(|(_, error)| error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temporary_path, std::fs::Permissions::from_mode(0o600))?;
        }
        if destination_path.exists() {
            bail!(
                "refusing to overwrite existing database backup {}",
                destination_path.display()
            );
        }
        std::fs::rename(&temporary_path, destination_path)?;
        Ok(())
    })();
    if backup_result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    backup_result
}

fn ensure_registration_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS relay_pending_registrations (
          id TEXT PRIMARY KEY,
          email TEXT NOT NULL,
          username TEXT NOT NULL,
          password_salt TEXT NOT NULL,
          password_hash TEXT NOT NULL,
          created_at TEXT NOT NULL,
          status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','approved','rejected')),
          reviewed_at TEXT,
          reviewed_by_user_id TEXT,
          provider TEXT NOT NULL DEFAULT 'password',
          provider_subject TEXT
        );
        CREATE INDEX IF NOT EXISTS relay_pending_registrations_status_idx
          ON relay_pending_registrations(status,created_at DESC);
        CREATE TABLE IF NOT EXISTS relay_user_identities (
          id TEXT PRIMARY KEY,
          user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE,
          provider TEXT NOT NULL CHECK (provider IN ('google','github')),
          provider_subject TEXT NOT NULL,
          provider_email TEXT NOT NULL,
          created_at TEXT NOT NULL,
          UNIQUE(provider,provider_subject)
        );
        ",
    )?;
    for (column, definition) in [
        ("provider", "TEXT NOT NULL DEFAULT 'password'"),
        ("provider_subject", "TEXT"),
    ] {
        let exists = conn
            .prepare("PRAGMA table_info(relay_pending_registrations)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .flatten()
            .any(|value| value == column);
        if !exists {
            conn.execute_batch(&format!(
                "ALTER TABLE relay_pending_registrations ADD COLUMN {column} {definition}"
            ))?;
        }
    }
    Ok(())
}

// Hosted VMs were removed. Their tables restricted user deletion, so drop them
// (children first) instead of leaving orphaned constraints behind.
fn drop_retired_hosted_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        DROP TABLE IF EXISTS relay_hosted_active_turns;
        DROP TABLE IF EXISTS relay_hosted_operations;
        DROP TABLE IF EXISTS relay_hosted_user_threads;
        DROP TABLE IF EXISTS relay_hosted_user_workspaces;
        DROP TABLE IF EXISTS relay_hosted_sandbox_members;
        DROP TABLE IF EXISTS relay_hosted_sandboxes;
        ",
    )?;
    Ok(())
}

pub(crate) fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        params![table],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

pub(crate) const LEGACY_SHA256_SALT: &str = "__remote_codex_legacy_sha256__";

pub(crate) fn migrate_legacy_rust_tables(conn: &mut Connection) -> Result<()> {
    if !table_exists(conn, "users") {
        return Ok(());
    }
    let already_migrated: Option<String> = conn
        .query_row(
            "SELECT value FROM relay_settings WHERE key='rustLegacyTablesImported'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if already_migrated.as_deref() == Some("true") {
        return Ok(());
    }

    let tx = conn.transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO relay_users
         (id,email,username,role,enabled,last_seen_at,created_at,password_salt,password_hash)
         SELECT id,lower(email),lower(username),role,enabled,NULL,created_at,?1,password_hash FROM users",
        params![LEGACY_SHA256_SALT],
    )?;

    if table_exists(&tx, "devices") {
        let devices = {
            let mut stmt = tx.prepare(
                "SELECT id,user_id,name,token,token_hash,token_preview,created_at FROM devices",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for (id, owner, name, token, legacy_hash, preview, created_at) in devices {
            let token_hash = token
                .as_deref()
                .map(hash_device_token)
                .unwrap_or(legacy_hash);
            let token_preview = preview
                .filter(|value| !value.is_empty())
                .or_else(|| token.as_deref().map(preview_token))
                .unwrap_or_else(|| "unknown".to_string());
            tx.execute(
                "INSERT OR IGNORE INTO relay_devices
                 (id,owner_user_id,name,token,token_hash,token_preview,created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    id,
                    owner,
                    name,
                    token,
                    token_hash,
                    token_preview,
                    created_at
                ],
            )?;
        }
    }

    if table_exists(&tx, "shares") {
        tx.execute(
            "INSERT OR IGNORE INTO relay_shares
             (id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,
              thread_id,workspace_id,thread_access,workspace_access,created_at,revoked_at)
             SELECT s.id,s.owner_user_id,owner.username,target.id,target.username,s.device_id,d.name,
                    s.thread_id,s.workspace_id,s.thread_access,s.workspace_access,s.created_at,s.revoked_at
             FROM shares s
             JOIN relay_users owner ON owner.id=s.owner_user_id
             JOIN relay_users target ON target.username=lower(s.target_username)
             JOIN relay_devices d ON d.id=s.device_id
             WHERE s.thread_id IS NOT NULL",
            [],
        )?;
    }

    if table_exists(&tx, "grants") {
        tx.execute(
            "INSERT OR IGNORE INTO relay_access_grants
             (id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,
              scope,thread_id,workspace_id,thread_access,workspace_access,can_create_threads,created_at,revoked_at)
             SELECT g.id,g.owner_user_id,owner.username,target.id,target.username,g.device_id,d.name,
                    g.scope,g.thread_id,g.workspace_id,g.thread_access,g.workspace_access,
                    g.can_create_threads,g.created_at,g.revoked_at
             FROM grants g
             JOIN relay_users owner ON owner.id=g.owner_user_id
             JOIN relay_users target ON target.username=lower(g.target_username)
             JOIN relay_devices d ON d.id=g.device_id",
            [],
        )?;
    }

    tx.execute(
        "INSERT INTO relay_settings(key,value) VALUES ('rustLegacyTablesImported','true')
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_drops_retired_hosted_tables_and_unblocks_user_deletion() {
        let path = std::env::temp_dir()
            .join(format!("relay-hosted-removal-{}", Uuid::new_v4()))
            .join("relay-store.sqlite");
        {
            let store = RelayStore::open(path.clone(), "secret".into()).unwrap();
            let conn = store.conn.try_lock().unwrap();
            for user in ["admin", "member"] {
                conn.execute(
                    "INSERT INTO relay_users VALUES (?1,?1,?1,'user',1,NULL,'now','salt','hash')",
                    [user],
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO relay_devices VALUES ('vm','admin','VM',NULL,'hash','rcd_x','now')",
                [],
            )
            .unwrap();
            conn.execute_batch(
                "CREATE TABLE relay_hosted_sandboxes (
                   id TEXT PRIMARY KEY,
                   device_id TEXT NOT NULL REFERENCES relay_devices(id) ON DELETE CASCADE,
                   assigned_user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE RESTRICT
                 );
                 CREATE TABLE relay_hosted_sandbox_members (
                   sandbox_id TEXT NOT NULL REFERENCES relay_hosted_sandboxes(id) ON DELETE CASCADE,
                   user_id TEXT NOT NULL REFERENCES relay_users(id) ON DELETE CASCADE
                 );
                 INSERT INTO relay_hosted_sandboxes VALUES ('sandbox','vm','member');
                 INSERT INTO relay_hosted_sandbox_members VALUES ('sandbox','member');",
            )
            .unwrap();
            assert!(conn
                .execute("DELETE FROM relay_users WHERE id='member'", [])
                .is_err());
        }

        let store = RelayStore::open(path, "secret".into()).unwrap();
        let conn = store.conn.try_lock().unwrap();
        assert!(!table_exists(&conn, "relay_hosted_sandboxes"));
        assert!(!table_exists(&conn, "relay_hosted_sandbox_members"));
        assert!(table_exists(&conn, "relay_pending_registrations"));
        assert!(table_exists(&conn, "relay_user_identities"));
        conn.execute("DELETE FROM relay_users WHERE id='member'", [])
            .unwrap();
        let devices: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM relay_devices WHERE id='vm'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(devices, 1);
    }
}
