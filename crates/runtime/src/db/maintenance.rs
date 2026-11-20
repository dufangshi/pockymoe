//! Small, restartable maintenance steps. No table scan or compaction at open.
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::time::{Duration, Instant};

const CURSOR: &str = "storage.kvRetention.cursor.v1";
const INDEX_DONE: &str = "storage.historyIndex.v1";
const COMPACTED: &str = "storage.compactedAt.v1";
const DAYS_30: i64 = 30 * 86400;

pub(super) fn migrate(c: &Connection) -> Result<()> {
    // No backfill here: legacy entries receive a full retention window when the
    // background cursor first observes them. Triggers cover concurrent writes.
    c.execute_batch("CREATE TABLE IF NOT EXISTS storage_kv_age(key TEXT PRIMARY KEY, touched INTEGER NOT NULL);
        CREATE TRIGGER IF NOT EXISTS storage_kv_insert AFTER INSERT ON kv
        WHEN new.key GLOB 'turn-process:*' OR new.key GLOB 'prompt-receipt:*'
          OR new.key GLOB 'cli:submission:*' OR new.key GLOB 'cli:request:*' OR new.key GLOB 'cli:inbox:*'
        BEGIN INSERT OR REPLACE INTO storage_kv_age VALUES(new.key,unixepoch()); END;
        CREATE TRIGGER IF NOT EXISTS storage_kv_update AFTER UPDATE ON kv
        WHEN new.key GLOB 'turn-process:*' OR new.key GLOB 'prompt-receipt:*'
          OR new.key GLOB 'cli:submission:*' OR new.key GLOB 'cli:request:*' OR new.key GLOB 'cli:inbox:*'
        BEGIN DELETE FROM storage_kv_age WHERE key=old.key;
          INSERT OR REPLACE INTO storage_kv_age VALUES(new.key,unixepoch()); END;
        CREATE TRIGGER IF NOT EXISTS storage_kv_delete AFTER DELETE ON kv
        BEGIN DELETE FROM storage_kv_age WHERE key=old.key; END;")?;
    Ok(())
}

/// Deferred migration: keep the explicit unique index on legacy schemas that
/// lack an equivalent constraint. Never assume an autoindex's numeric suffix.
pub(crate) fn migrate_history_index(c: &Connection) -> Result<()> {
    if c.query_row("SELECT 1 FROM kv WHERE key=?1", [INDEX_DONE], |_| Ok(()))
        .optional()?
        .is_some()
    {
        return Ok(());
    }
    let tx = c.unchecked_transaction()?;
    let indexes = tx.prepare("SELECT name FROM pragma_index_list('thread_history_items') WHERE origin='u' AND \"unique\"=1 AND partial=0")?
        .query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for index in indexes {
        let columns = tx
            .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")?
            .query_map([index], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if columns == ["thread_id", "turn_id", "item_id"] {
            tx.execute_batch("DROP INDEX IF EXISTS thread_history_items_thread_turn_item_idx")?;
            break;
        }
    }
    tx.execute("INSERT INTO kv VALUES(?1,'done')", [INDEX_DONE])?;
    tx.commit()?;
    Ok(())
}

fn tracked(key: &str) -> bool {
    [
        "turn-process:",
        "prompt-receipt:",
        "cli:submission:",
        "cli:request:",
        "cli:inbox:",
    ]
    .iter()
    .any(|p| key.starts_with(p))
}

fn removable(c: &Connection, key: &str, raw: &str, now: i64) -> Result<bool> {
    if let Some(turn) = key.strip_prefix("turn-process:") {
        return Ok(!c
            .query_row(
                "SELECT 1 FROM thread_turns WHERE id=?1 AND status IN ('inProgress','recovering')",
                [turn],
                |_| Ok(()),
            )
            .optional()?
            .is_some());
    }
    if key.starts_with("cli:submission:") {
        let pending = key.rsplit(':').next().unwrap_or("");
        return Ok(!c.query_row("SELECT 1 FROM thread_pending_steers WHERE id=?1 UNION ALL SELECT 1 FROM thread_turns WHERE id=?2 AND status IN ('inProgress','recovering') LIMIT 1", params![pending,raw], |_| Ok(())).optional()?.is_some());
    }
    let thread = key
        .strip_prefix("prompt-receipt:")
        .or_else(|| key.strip_prefix("cli:request:"))
        .or_else(|| key.strip_prefix("cli:inbox:"))
        .and_then(|s| s.split(':').next())
        .unwrap_or("");
    if c.query_row("SELECT 1 FROM thread_pending_steers WHERE thread_id=?1 UNION ALL SELECT 1 FROM thread_turns WHERE thread_id=?1 AND status IN ('inProgress','recovering') LIMIT 1", [thread], |_| Ok(())).optional()?.is_some() {
        return Ok(false);
    }
    if key.starts_with("cli:inbox:") {
        let Ok(v) = serde_json::from_str::<Value>(raw) else {
            return Ok(false);
        };
        return Ok(v["acknowledgedAt"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .is_some_and(|at| at.timestamp() < now - DAYS_30));
    }
    Ok(true)
}

/// At most 128 keys / 10 ms of work, then commit a durable primary-key cursor.
/// The time bound excludes a single SQLite operation and FULL fsync.
pub(crate) fn retain_slice(c: &Connection, now: i64) -> Result<bool> {
    let tx = c.unchecked_transaction()?;
    let cursor: String = tx
        .query_row("SELECT value FROM kv WHERE key=?1", [CURSOR], |r| r.get(0))
        .optional()?
        .unwrap_or_default();
    // Fetch keys first; large unrelated kv values are never loaded.
    let keys = tx
        .prepare("SELECT key FROM kv WHERE key>?1 ORDER BY key LIMIT 128")?
        .query_map([cursor], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let start = Instant::now();
    let mut last = String::new();
    let mut processed = 0;
    for key in &keys {
        if tracked(key) {
            tx.execute(
                "INSERT OR IGNORE INTO storage_kv_age VALUES(?1,?2)",
                params![key, now],
            )?;
            let age: i64 = tx.query_row(
                "SELECT touched FROM storage_kv_age WHERE key=?1",
                [key],
                |r| r.get(0),
            )?;
            if age < now - DAYS_30 {
                let raw: String =
                    tx.query_row("SELECT value FROM kv WHERE key=?1", [key], |r| r.get(0))?;
                if removable(&tx, key, &raw, now)? {
                    tx.execute("DELETE FROM kv WHERE key=?1", [key])?;
                }
            }
        }
        last.clone_from(key);
        processed += 1;
        if start.elapsed() >= Duration::from_millis(10) {
            break;
        }
    }
    let done = processed == keys.len() && keys.len() < 128;
    tx.execute(
        "INSERT OR REPLACE INTO kv VALUES(?1,?2)",
        params![CURSOR, if done { "" } else { &last }],
    )?;
    tx.commit()?;
    Ok(done)
}

/// Guarded occasional VACUUM: no startup conversion, no active or queued work,
/// 30 minutes since thread activity, at least 64 MiB / 20% free, once a week.
/// SQLite commits VACUUM atomically; interruption rolls it back and the next
/// idle pass retries. Only successful compaction advances the durable marker.
pub(crate) fn compact_if_idle(c: &Connection, now: i64) -> Result<bool> {
    let busy: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM thread_turns WHERE status IN ('inProgress','recovering')) OR EXISTS(SELECT 1 FROM thread_pending_steers) OR EXISTS(SELECT 1 FROM command_executions WHERE state IN ('starting','running')) OR EXISTS(SELECT 1 FROM automation_runs WHERE state IN ('due','queued','running')) OR EXISTS(SELECT 1 FROM threads WHERE unixepoch(updated_at)>?1)", [now - 1800], |r| r.get(0))?;
    if busy {
        return Ok(false);
    }
    let last: Option<String> = c
        .query_row("SELECT value FROM kv WHERE key=?1", [COMPACTED], |r| {
            r.get(0)
        })
        .optional()?;
    if last
        .and_then(|v| v.parse::<i64>().ok())
        .is_some_and(|last| now - last < 7 * 86400)
    {
        return Ok(false);
    }
    let pages: i64 = c.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let free: i64 = c.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    let size: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    if free * size < 64 * 1024 * 1024 || free * 5 < pages {
        return Ok(false);
    }
    let start = Instant::now();
    c.execute_batch("VACUUM")?;
    c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    c.execute(
        "INSERT OR REPLACE INTO kv VALUES(?1,?2)",
        params![COMPACTED, now.to_string()],
    )?;
    tracing::info!(
        ms = start.elapsed().as_millis() as u64,
        "idle database compaction finished"
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[test]
    fn retention_resumes_and_preserves_pending_and_unread() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("test.sqlite");
        let now = chrono::Utc::now().timestamp();
        {
            let db = Database::open(&path)?;
            db.with(|c| {
                let tx = c.unchecked_transaction()?;
                for n in 0..300 {
                    tx.execute("INSERT INTO kv VALUES(?1,'fingerprint')", [format!("prompt-receipt:old:{n:04}")])?;
                }
                tx.execute("INSERT INTO kv VALUES('cli:inbox:old:unread',?1)", [r#"{"acknowledgedAt":null}"#])?;
                tx.execute("INSERT INTO kv VALUES('cli:inbox:old:acked',?1)", [r#"{"acknowledgedAt":"2020-01-01T00:00:00Z"}"#])?;
                tx.execute("INSERT INTO kv VALUES('cli:request:pending:r','{}')", [])?;
                tx.execute("INSERT INTO thread_pending_steers(id,thread_id,turn_id,display_prompt,submitted_prompt,delivery,created_at,updated_at) VALUES('p','pending','','','','queue','t','t')", [])?;
                tx.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('active','running','inProgress',1),('ended','running','completed',2)", [])?;
                for (key,value) in [("turn-process:active","pid"),("turn-process:ended","pid"),("cli:submission:turn:old","ended"),("cli:submission:turn:live","active"),("prompt-receipt:legacy:r","fingerprint")] {
                    tx.execute("INSERT INTO kv VALUES(?1,?2)",params![key,value])?;
                }
                tx.execute("UPDATE storage_kv_age SET touched=?1", [now - DAYS_30 - 1])?;
                tx.execute("DELETE FROM storage_kv_age WHERE key='prompt-receipt:legacy:r'", [])?;

                tx.commit()?;
                assert!(!retain_slice(c, now)?);
                Ok(())
            })?;
        }
        let db = Database::reopen(&path)?;
        db.with(|c| {
            while !retain_slice(c, now)? {}
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM kv WHERE key GLOB 'prompt-receipt:old:*'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            assert!(db_key(c, "cli:inbox:old:unread")?);
            assert!(!db_key(c, "cli:inbox:old:acked")?);
            assert!(db_key(c, "cli:request:pending:r")?);
            assert!(db_key(c, "turn-process:active")?);
            assert!(!db_key(c, "turn-process:ended")?);
            assert!(db_key(c, "cli:submission:turn:live")?);
            assert!(!db_key(c, "cli:submission:turn:old")?);
            assert!(db_key(c, "prompt-receipt:legacy:r")?);

            // An update refreshes the full retention window.
            c.execute(
                "UPDATE kv SET value='{}' WHERE key='cli:request:pending:r'",
                [],
            )?;
            c.execute("DELETE FROM thread_pending_steers", [])?;
            while !retain_slice(c, now)? {}
            assert!(db_key(c, "cli:request:pending:r")?);
            Ok(())
        })
    }

    fn db_key(c: &Connection, key: &str) -> Result<bool> {
        Ok(
            c.query_row("SELECT 1 FROM kv WHERE key=?1", [key], |_| Ok(()))
                .optional()?
                .is_some(),
        )
    }

    #[test]
    fn history_migration_preserves_unique_constraint_and_legacy_index() -> Result<()> {
        for constraint in [true, false] {
            let c = Connection::open_in_memory()?;
            c.execute_batch(&format!("CREATE TABLE kv(key TEXT PRIMARY KEY,value TEXT); CREATE TABLE thread_history_items(thread_id TEXT,turn_id TEXT,item_id TEXT{}); CREATE UNIQUE INDEX thread_history_items_thread_turn_item_idx ON thread_history_items(thread_id,turn_id,item_id)", if constraint {", UNIQUE(thread_id,turn_id,item_id)"} else {""}))?;
            migrate_history_index(&c)?;
            migrate_history_index(&c)?;
            let explicit: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='thread_history_items_thread_turn_item_idx')", [], |r|r.get(0))?;
            assert_eq!(explicit, !constraint);
            c.execute("INSERT INTO thread_history_items VALUES('a','b','c')", [])?;
            assert!(c
                .execute("INSERT INTO thread_history_items VALUES('a','b','c')", [])
                .is_err());
        }
        Ok(())
    }

    #[test]
    fn maintenance_pragmas_and_idle_guards() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = Database::open(&dir.path().join("test.sqlite"))?;
        db.with(|c| {
            migrate(c)?; // Reapplying after a downgrade preserves the age ledger.
            for (pragma, expected) in [
                ("journal_size_limit", 67108864),
                ("cache_size", -32768),
                ("wal_autocheckpoint", 1000),
            ] {
                assert_eq!(
                    c.query_row(&format!("PRAGMA {pragma}"), [], |r| r.get::<_, i64>(0))?,
                    expected
                );
            }
            assert!(!compact_if_idle(c, chrono::Utc::now().timestamp())?);
            Ok(())
        })
    }

    #[test]
    fn wal_limit_trims_high_water_on_reuse() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("wal.sqlite");
        let db = Database::open(&path)?;
        db.with(|c| {
            c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_size_limit=8192; PRAGMA wal_autocheckpoint=0; CREATE TABLE wal_test(data BLOB)")?;
            c.execute_batch("BEGIN")?;
            for _ in 0..30 { c.execute("INSERT INTO wal_test VALUES(zeroblob(4096))", [])?; }
            c.execute_batch("COMMIT")?;
            let wal=path.with_file_name("wal.sqlite-wal");
            assert!(std::fs::metadata(&wal)?.len()>8192);
            c.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))?;
            c.execute("INSERT INTO wal_test VALUES('small')", [])?;
            assert!(std::fs::metadata(wal)?.len()<=16384);
            Ok(())
        })
    }

    /// Explicit temporary-only benchmark; never opens the machine's data.
    #[test]
    #[ignore = "670 MB synthetic maintenance measurement"]
    fn realistic_size_measurement() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("large.sqlite");
        let db = Database::open(&path)?;
        db.with(|c| {
            c.execute_batch("CREATE TABLE measurement(id INTEGER PRIMARY KEY,payload BLOB); BEGIN")?;
            for n in 0..143_000 {
                c.execute("INSERT INTO measurement VALUES(?1,zeroblob(4000))",[n])?;
            }
            for n in 0..80_000 {
                let id = format!("{n:012}{}", "x".repeat(64));
                c.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,?2,?3,?1,?4,'t','t')", params![id,"t".repeat(64),"r".repeat(64),r#"{"kind":"commandExecution","text":"echo hi"}"#])?;
            }
            for n in 0..5000 {
                c.execute("INSERT INTO kv VALUES(?1,?2)",params![format!("prompt-receipt:fixture:{n:05}"),"v".repeat(1024)])?;
            }
            c.execute_batch("COMMIT; DELETE FROM measurement WHERE id%3=0; PRAGMA wal_checkpoint(TRUNCATE)")?;
            // Emulate upgrading an existing database with no maintenance schema.
            c.execute_batch("DROP TRIGGER storage_kv_insert; DROP TRIGGER storage_kv_update; DROP TRIGGER storage_kv_delete; DROP TABLE storage_kv_age; DELETE FROM __remote_codex_runtime_migrations WHERE version=23")?;
            let start=Instant::now();
            c.execute_batch("BEGIN")?;
            migrate(c)?;
            c.execute_batch("ROLLBACK")?;
            eprintln!("schema-only migration (rolled back) {:?}",start.elapsed());

            Ok(())
        })?;
        drop(db);
        let start = Instant::now();
        let db = Database::open(&path)?;
        eprintln!(
            "existing open+schema migration {:?}; bytes {}",
            start.elapsed(),
            std::fs::metadata(&path)?.len()
        );
        db.with(|c| {
            let start = Instant::now();
            migrate_history_index(c)?;
            eprintln!("index migration {:?}", start.elapsed());
            let start = Instant::now();
            retain_slice(c, chrono::Utc::now().timestamp())?;
            eprintln!("retention slice {:?}", start.elapsed());
            c.execute("UPDATE storage_kv_age SET touched=?1",[chrono::Utc::now().timestamp()-DAYS_30-1])?;
            c.execute("UPDATE kv SET value='' WHERE key=?1",[CURSOR])?;
            let start=Instant::now();
            retain_slice(c,chrono::Utc::now().timestamp())?;
            eprintln!("expiry slice {:?}",start.elapsed());
            let start=Instant::now();
            c.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))?;
            eprintln!("passive checkpoint {:?}",start.elapsed());

            for kib in [2000, 32768] {
                c.execute_batch(&format!("PRAGMA shrink_memory; PRAGMA cache_size=-{kib}"))?;
                let mut current = 0;
                let mut high = 0;
                unsafe {
                    rusqlite::ffi::sqlite3_db_status(
                        c.handle(),
                        rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS,
                        &mut current,
                        &mut high,
                        1,
                    );
                }
                for _ in 0..5 {
                    for n in 0..770 {
                        let _: i64 = c
                            .query_row(
                                "SELECT length(payload) FROM measurement WHERE id=?1",
                                [n * 170 + 1],
                                |r| r.get(0),
                            )
                            .optional()?
                            .unwrap_or(0);
                    }
                }
                unsafe {
                    rusqlite::ffi::sqlite3_db_status(
                        c.handle(),
                        rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS,
                        &mut current,
                        &mut high,
                        0,
                    );
                }
                eprintln!("cache {kib} KiB: {current} page misses / 5 repeated reads");
            }
            let now=chrono::Utc::now().timestamp();
            c.execute("INSERT INTO thread_pending_steers(id,thread_id,turn_id,display_prompt,submitted_prompt,delivery,created_at,updated_at) VALUES('guard','t','','','','queue','t','t')", [])?;
            let start=Instant::now();
            assert!(!compact_if_idle(c,now)?);
            eprintln!("queued-work guard {:?}",start.elapsed());
            c.execute("DELETE FROM thread_pending_steers WHERE id='guard'", [])?;
            c.execute("INSERT INTO kv VALUES(?1,?2)",params![COMPACTED,now.to_string()])?;
            assert!(!compact_if_idle(c,now)?);
            c.execute("DELETE FROM kv WHERE key=?1",[COMPACTED])?;
            let interrupt = c.get_interrupt_handle();
            let start = Instant::now();
            let interrupted = std::thread::scope(|scope| {
                scope.spawn(move || {
                    std::thread::sleep(Duration::from_millis(5));
                    interrupt.interrupt();
                });
                c.execute_batch("VACUUM")
            });
            assert!(interrupted.is_err());
            assert!(!db_key(c, COMPACTED)?);
            eprintln!("interrupted vacuum rollback {:?}", start.elapsed());
            let check: String = c.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
            assert_eq!(check, "ok");
            assert_eq!(
                c.query_row("SELECT count(*) FROM thread_history_items", [], |r| r
                    .get::<_, i64>(0))?,
                80_000
            );
            let before = std::fs::metadata(&path)?.len();
            let start = Instant::now();
            assert!(compact_if_idle(c, chrono::Utc::now().timestamp())?);
            eprintln!(
                "guarded vacuum+truncate {:?}; bytes {} -> {}",
                start.elapsed(),
                before,
                std::fs::metadata(&path)?.len()
            );
            let start = Instant::now();
            assert!(!compact_if_idle(c, chrono::Utc::now().timestamp())?);
            eprintln!("repeat guard {:?}", start.elapsed());
            Ok(())
        })
    }
}
