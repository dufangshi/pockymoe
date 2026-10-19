//! Disk use of the Supervisor database and history cleanup.
//!
//! A scan runs only on request, on its own read-only connection, so the shared
//! connection stays free. Cleanup and compaction run as background jobs whose
//! progress the overview reports. Cleanup finds its items on a read-only
//! connection too and rewrites them in transactions of a few milliseconds, so
//! every other database user waits at most that long. Depending on its options
//! it removes command output (and input details), shortens or removes command
//! text, and removes file-change diffs. Every pass also drops the input's copy
//! of the command, which older versions stored twice.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::acp::mapper::{pretty_json, split_command_input};
use chrono::{SecondsFormat, Utc};
use pockymoe_protocol::{
    CommandTextCleanup, StorageCleanupDto, StorageJobDto, StorageOverviewDto, StorageScanDto,
    StorageSettingsDto, StorageThreadDto,
};
use rusqlite::OpenFlags;

pub const STORAGE_PAGE_LIMIT: u64 = 100;
/// Longest a cleanup transaction holds the shared connection.
const PRUNE_SLICE: Duration = Duration::from_millis(20);
/// Pause between slices, so queued database users run first.
const PRUNE_PAUSE: Duration = Duration::from_millis(15);
/// Copy the write-ahead log into the file this often, on a separate connection:
/// a long log would otherwise be copied by an automatic checkpoint inside one
/// of the shared connection's commits, while everyone waits for it.
const CHECKPOINT_INTERVAL: Duration = Duration::from_millis(1_000);
const RETENTION_INTERVAL: Duration = Duration::from_secs(60 * 60);
const MAX_RETENTION_DAYS: u32 = 3650;
const COMMAND_TEXT_CHARS: std::ops::RangeInclusive<u32> = 16..=10_000;
/// Set once the input copies of commands were removed from older history.
const INPUT_DEDUPE_KEY: &str = "storage.commandInputDedupe.v1";
/// Detail sections after the header: the input repeats the command, then the
/// output. Everything before the first one stays.
const BODY_MARKERS: &[&str] = &["\n\nInput:\n", "\n\nOutput:\n", "\n\nResult:\n"];

#[derive(Default)]
pub(crate) struct StorageState {
    scan: std::sync::Mutex<Option<Arc<StorageScanDto>>>,
    gate: tokio::sync::Mutex<()>,
    started: AtomicBool,
    job: std::sync::Mutex<Option<StorageJobDto>>,
    /// The saved cleanup could not start because another job ran: run it after.
    rerun: AtomicBool,
}

enum StorageAction {
    Prune(Cleanup),
    Compact,
}

/// One cleanup pass: what to remove, and from which turns.
#[derive(Clone)]
pub(crate) struct Cleanup {
    /// Turns that ended before this time; `None` for any ended turn.
    cutoff: Option<String>,
    /// The conversation (root thread and its agent threads); `None` for all.
    thread: Option<String>,
    options: StorageCleanupDto,
    /// Only remove the command's copy from inputs (a lossless one-time pass).
    dedupe: bool,
}

impl Cleanup {
    fn dedupe_only() -> Self {
        Self {
            cutoff: None,
            thread: None,
            options: StorageCleanupDto {
                command_output: false,
                command_text: CommandTextCleanup::Keep,
                command_text_chars: pockymoe_protocol::DEFAULT_COMMAND_TEXT_CHARS,
                file_diffs: false,
            },
            dedupe: true,
        }
    }
}

/// Another cleanup or compaction is running.
#[derive(Debug)]
pub struct StorageBusy(pub StorageJobDto);
impl std::fmt::Display for StorageBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Another storage job is running")
    }
}
impl std::error::Error for StorageBusy {}

fn cutoff(days: u32) -> String {
    (Utc::now() - chrono::Duration::days(i64::from(days)))
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn validate_cleanup(options: &StorageCleanupDto) -> Result<()> {
    if !COMMAND_TEXT_CHARS.contains(&options.command_text_chars) {
        return Err(InvalidSettings(format!(
            "commandTextChars must be {}-{}",
            COMMAND_TEXT_CHARS.start(),
            COMMAND_TEXT_CHARS.end()
        ))
        .into());
    }
    Ok(())
}

impl Supervisor {
    pub fn storage_settings(&self) -> Result<StorageSettingsDto> {
        self.db.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT command_output_retention_days, clean_command_output, command_text,
                            command_text_chars, clean_file_diffs
                     FROM storage_settings WHERE id=1",
                    [],
                    |row| {
                        Ok(StorageSettingsDto {
                            retention_days: row
                                .get::<_, Option<i64>>(0)?
                                .and_then(|days| u32::try_from(days).ok()),
                            cleanup: StorageCleanupDto {
                                command_output: row.get::<_, i64>(1)? != 0,
                                command_text: match row.get::<_, String>(2)?.as_str() {
                                    "truncate" => CommandTextCleanup::Truncate,
                                    "remove" => CommandTextCleanup::Remove,
                                    _ => CommandTextCleanup::Keep,
                                },
                                command_text_chars: u32::try_from(row.get::<_, i64>(3)?)
                                    .unwrap_or(pockymoe_protocol::DEFAULT_COMMAND_TEXT_CHARS),
                                file_diffs: row.get::<_, i64>(4)? != 0,
                            },
                        })
                    },
                )
                .optional()?
                .unwrap_or_default())
        })
    }

    /// Replace the policy; a set age cleans up at once in the background.
    pub fn update_storage_settings(
        self: &Arc<Self>,
        settings: StorageSettingsDto,
    ) -> Result<StorageSettingsDto> {
        if let Some(days) = settings.retention_days {
            if !(1..=MAX_RETENTION_DAYS).contains(&days) {
                return Err(InvalidSettings(format!(
                    "retentionDays must be 1-{MAX_RETENTION_DAYS}"
                ))
                .into());
            }
        }
        validate_cleanup(&settings.cleanup)?;
        let text = match settings.cleanup.command_text {
            CommandTextCleanup::Keep => "keep",
            CommandTextCleanup::Truncate => "truncate",
            CommandTextCleanup::Remove => "remove",
        };
        self.db.with(|conn| {
            conn.execute(
                "INSERT INTO storage_settings(id, command_output_retention_days, clean_command_output,
                   command_text, command_text_chars, clean_file_diffs, updated_at)
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                   command_output_retention_days=excluded.command_output_retention_days,
                   clean_command_output=excluded.clean_command_output,
                   command_text=excluded.command_text,
                   command_text_chars=excluded.command_text_chars,
                   clean_file_diffs=excluded.clean_file_diffs,
                   updated_at=excluded.updated_at",
                params![
                    settings.retention_days,
                    settings.cleanup.command_output,
                    text,
                    settings.cleanup.command_text_chars,
                    settings.cleanup.file_diffs,
                    now_rfc3339()
                ],
            )?;
            Ok(())
        })?;
        let saved = self.storage_settings()?;
        self.start_saved_cleanup();
        Ok(saved)
    }

    /// Hourly pass: first the one-time input deduplication, then the saved
    /// automatic cleanup. The first one runs a few minutes after start.
    pub fn spawn_storage_retention(self: &Arc<Self>) {
        if self.storage.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(180)).await;
            loop {
                let Some(state) = weak.upgrade() else { break };
                if !matches!(state.db.get_kv(INPUT_DEDUPE_KEY), Ok(Some(_))) {
                    if let Ok(job) =
                        state.start_storage_job(StorageAction::Prune(Cleanup::dedupe_only()))
                    {
                        state.wait_for_job(&job.started_at).await;
                    }
                }
                state.start_saved_cleanup();
                drop(state);
                tokio::time::sleep(RETENTION_INTERVAL).await;
            }
        });
    }

    async fn wait_for_job(&self, started_at: &str) {
        loop {
            let running = self
                .storage
                .job
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|job| job.started_at == started_at && job.state == "running");
            if !running {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Measure the database now and keep the result for paging.
    pub async fn storage_scan(&self) -> Result<StorageScanDto> {
        let _gate = self.storage.gate.lock().await;
        let path = self
            .db
            .with(|conn| Ok(conn.path().map(PathBuf::from)))?
            .ok_or_else(|| anyhow!("The database has no file to measure"))?;
        let host = self.db.host_id.clone();
        let scan =
            Arc::new(tokio::task::spawn_blocking(move || scan_database(&path, &host)).await??);
        *self.storage.scan.lock().unwrap() = Some(scan.clone());
        Ok(page(&scan, 0, STORAGE_PAGE_LIMIT))
    }

    /// The device name and a page of the last scan, if one ran.
    pub fn storage_overview(&self, offset: u64, limit: u64) -> Result<StorageOverviewDto> {
        let device_name = self.db.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT hostname FROM hosts WHERE id=?1",
                    [&self.db.host_id],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or_default())
        })?;
        let scan = self.storage.scan.lock().unwrap().clone();
        Ok(StorageOverviewDto {
            device_name,
            scan: scan.map(|scan| page(&scan, offset, limit.clamp(1, STORAGE_PAGE_LIMIT))),
            job: self.storage.job.lock().unwrap().clone(),
        })
    }

    /// Start the saved automatic cleanup, or run it once the running job ends.
    fn start_saved_cleanup(self: &Arc<Self>) {
        match self.start_storage_prune() {
            Ok(_) => {}
            Err(error) if error.is::<StorageBusy>() => {
                self.storage.rerun.store(true, Ordering::SeqCst);
            }
            Err(error) => tracing::warn!(%error, "starting the storage cleanup failed"),
        }
    }

    /// Clean up now with the saved settings (`None` when no age is saved).
    pub fn start_storage_prune(self: &Arc<Self>) -> Result<Option<StorageJobDto>> {
        let settings = self.storage_settings()?;
        let Some(days) = settings.retention_days else {
            return Ok(None);
        };
        self.start_storage_job(StorageAction::Prune(Cleanup {
            cutoff: Some(cutoff(days)),
            thread: None,
            options: settings.cleanup,
            dedupe: false,
        }))
        .map(Some)
    }

    /// Clean up one conversation now, whatever the age of its ended turns:
    /// the thread a scan row names and the agent threads under it.
    pub fn start_thread_cleanup(
        self: &Arc<Self>,
        thread_id: &str,
        options: StorageCleanupDto,
    ) -> Result<StorageJobDto> {
        validate_cleanup(&options)?;
        let known = self.db.with(|conn| {
            Ok(conn
                .query_row("SELECT 1 FROM threads WHERE id=?1", [thread_id], |_| Ok(()))
                .optional()?
                .is_some())
        })?;
        if !known {
            return Err(anyhow!("thread not found"));
        }
        self.start_storage_job(StorageAction::Prune(Cleanup {
            cutoff: None,
            thread: Some(thread_id.to_owned()),
            options,
            dedupe: false,
        }))
    }

    /// Start rewriting the file without its free pages.
    pub fn start_storage_compact(self: &Arc<Self>) -> Result<StorageJobDto> {
        self.start_storage_job(StorageAction::Compact)
    }

    /// Start a job, or fail with [`StorageBusy`] naming the running one.
    fn start_storage_job(self: &Arc<Self>, action: StorageAction) -> Result<StorageJobDto> {
        let mut slot = self.storage.job.lock().unwrap();
        if let Some(job) = slot.as_ref().filter(|job| job.state == "running") {
            return Err(StorageBusy(job.clone()).into());
        }
        let (name, thread_id) = match &action {
            StorageAction::Prune(cleanup) if cleanup.dedupe => ("dedupe", None),
            StorageAction::Prune(cleanup) => ("prune", cleanup.thread.clone()),
            StorageAction::Compact => ("compact", None),
        };
        let job = StorageJobDto {
            action: name.into(),
            state: "running".into(),
            total: 0,
            processed: 0,
            items: 0,
            bytes: 0,
            before_bytes: None,
            after_bytes: None,
            started_at: now_rfc3339(),
            finished_at: None,
            error: None,
            thread_id,
        };
        match &action {
            StorageAction::Prune(cleanup) => tracing::info!(
                action = name,
                cutoff = cleanup.cutoff.as_deref().unwrap_or("-"),
                thread = cleanup.thread.as_deref().unwrap_or("-"),
                options = ?cleanup.options,
                "storage job started"
            ),
            StorageAction::Compact => tracing::info!(action = name, "storage job started"),
        }
        *slot = Some(job.clone());
        drop(slot);
        let state = self.clone();
        tokio::spawn(async move {
            let dedupe = matches!(&action, StorageAction::Prune(cleanup) if cleanup.dedupe);
            let result = match action {
                StorageAction::Prune(cleanup) => state.run_prune(cleanup).await,
                StorageAction::Compact => state.run_compact().await,
            };
            if dedupe && result.is_ok() {
                if let Err(error) = state.db.set_kv(INPUT_DEDUPE_KEY, &now_rfc3339()) {
                    tracing::warn!(%error, "recording the input deduplication failed");
                }
            }
            *state.storage.scan.lock().unwrap() = None;
            state.update_job(|job| {
                job.finished_at = Some(now_rfc3339());
                match &result {
                    Ok(()) => job.state = "done".into(),
                    Err(error) => {
                        job.state = "failed".into();
                        job.error = Some(format!("{error:#}"));
                    }
                }
            });
            match result {
                Ok(()) => {
                    if let Some(job) = state.storage.job.lock().unwrap().clone() {
                        tracing::info!(action = %job.action, items = job.items, bytes = job.bytes, "storage job finished");
                    }
                }
                Err(error) => tracing::warn!(%error, "storage job failed"),
            }
            if state.storage.rerun.swap(false, Ordering::SeqCst) {
                state.start_saved_cleanup();
            }
        });
        Ok(job)
    }

    fn update_job(&self, change: impl FnOnce(&mut StorageJobDto)) {
        if let Some(job) = self.storage.job.lock().unwrap().as_mut() {
            change(job);
        }
    }

    fn database_path(&self) -> Result<PathBuf> {
        self.db
            .with(|conn| Ok(conn.path().map(PathBuf::from)))?
            .ok_or_else(|| anyhow!("The database has no file to clean up"))
    }

    /// Rewrite the history items the cleanup selects.
    async fn run_prune(self: &Arc<Self>, cleanup: Cleanup) -> Result<()> {
        let _gate = self.storage.gate.lock().await;
        let path = self.database_path()?;
        // Finding candidates reads every history item: never on the shared connection.
        let counting = std::time::Instant::now();
        let candidates_path = path.clone();
        let selection = cleanup.clone();
        let rows =
            tokio::task::spawn_blocking(move || prune_candidates(&candidates_path, &selection))
                .await??;
        tracing::info!(
            items = rows.len(),
            ms = counting.elapsed().as_millis() as u64,
            "storage cleanup candidates"
        );
        self.update_job(|job| job.total = rows.len() as u64);
        let pruned_at = now_rfc3339();
        let rows = Arc::new(rows);
        let cleanup = Arc::new(cleanup);
        // The shared connection would otherwise checkpoint inside one of its
        // commits; the side checkpoints below do it outside its lock.
        let _autocheckpoint = AutocheckpointPause::new(self.clone())?;
        let mut done = 0_usize;
        let mut checkpointed = std::time::Instant::now();
        while done < rows.len() {
            let state = self.clone();
            let (rows_ref, at, options) = (rows.clone(), pruned_at.clone(), cleanup.clone());
            let timer = std::time::Instant::now();
            let slice = tokio::task::spawn_blocking(move || {
                state
                    .db
                    .with(|conn| prune_rows(conn, &rows_ref[done..], &at, &options))
            })
            .await??;
            if timer.elapsed() > Duration::from_millis(200) {
                tracing::warn!(
                    ms = timer.elapsed().as_millis() as u64,
                    rows = slice.processed,
                    "slow storage cleanup slice"
                );
            }
            done += slice.processed;
            self.update_job(|job| {
                job.processed = done as u64;
                job.items += slice.items;
                job.bytes += slice.bytes;
            });
            if checkpointed.elapsed() >= CHECKPOINT_INTERVAL || done == rows.len() {
                let checkpoint_path = path.clone();
                tokio::task::spawn_blocking(move || side_checkpoint(&checkpoint_path)).await??;
                checkpointed = std::time::Instant::now();
            }
            tokio::time::sleep(PRUNE_PAUSE).await;
        }
        Ok(())
    }

    /// Rewrite the file without its free pages. Every database use waits meanwhile.
    async fn run_compact(self: &Arc<Self>) -> Result<()> {
        let _gate = self.storage.gate.lock().await;
        let state = self.clone();
        let (before, after) = tokio::task::spawn_blocking(move || {
            state.db.with(|conn| {
                let before = file_bytes(conn)?;
                conn.execute_batch("VACUUM;")?;
                conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
                Ok((before, file_bytes(conn)?))
            })
        })
        .await??;
        self.update_job(|job| {
            job.before_bytes = Some(before);
            job.after_bytes = Some(after);
        });
        Ok(())
    }
}

fn file_bytes(conn: &Connection) -> Result<u64> {
    let path = conn
        .path()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("The database has no file to measure"))?;
    Ok(database_file_bytes(&path))
}

fn database_file_bytes(path: &Path) -> u64 {
    let size = |path: &Path| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    size(path) + size(Path::new(&wal))
}

struct ThreadRow {
    id: String,
    title: String,
    workspace_label: Option<String>,
    root: String,
    updated_at: Option<String>,
}

fn scan_database(path: &Path, host_id: &str) -> Result<StorageScanDto> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(10))?;
    let pragma = |name: &str| -> Result<u64> {
        Ok(conn.query_row(&format!("PRAGMA {name}"), [], |row| row.get::<_, i64>(0))? as u64)
    };
    let page_size = pragma("page_size")?;
    let free_bytes = pragma("freelist_count")? * page_size;
    let device_name = conn
        .query_row("SELECT hostname FROM hosts WHERE id=?1", [host_id], |row| {
            row.get(0)
        })
        .optional()?
        .unwrap_or_default();
    let mut threads = HashMap::new();
    {
        let mut statement = conn.prepare(
            "SELECT t.id, t.title, w.label, COALESCE(t.root_thread_id, t.id), t.updated_at
             FROM threads t LEFT JOIN workspaces w ON w.id = t.workspace_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(ThreadRow {
                id: row.get(0)?,
                title: row.get(1)?,
                workspace_label: row.get(2)?,
                root: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        for row in rows {
            let row = row?;
            threads.insert(row.id.clone(), row);
        }
    }
    // (bytes, command bytes, items, pruned items, pruned bytes, file-change bytes) per thread.
    let mut usage: Vec<(String, [u64; 6])> = Vec::new();
    {
        let mut statement = conn.prepare(
            "SELECT thread_id,
               SUM(length(CAST(item_json AS BLOB))),
               SUM(CASE WHEN json_extract(item_json, '$.kind') = 'commandExecution'
                   THEN length(CAST(item_json AS BLOB)) ELSE 0 END),
               COUNT(*),
               SUM(json_extract(item_json, '$.outputPrunedAt') IS NOT NULL),
               SUM(COALESCE(json_extract(item_json, '$.outputPrunedBytes'), 0)),
               SUM(CASE WHEN json_extract(item_json, '$.kind') = 'fileChange'
                   THEN length(CAST(item_json AS BLOB)) ELSE 0 END)
             FROM thread_history_items GROUP BY thread_id",
        )?;
        let rows = statement.query_map([], |row| {
            let number = |index: usize| {
                row.get::<_, Option<i64>>(index)
                    .map(|v| v.unwrap_or(0).max(0) as u64)
            };
            Ok((
                row.get::<_, String>(0)?,
                [
                    number(1)?,
                    number(2)?,
                    number(3)?,
                    number(4)?,
                    number(5)?,
                    number(6)?,
                ],
            ))
        })?;
        for row in rows {
            usage.push(row?);
        }
    }
    let mut totals = [0_u64; 6];
    let mut conversations: HashMap<String, StorageThreadDto> = HashMap::new();
    for (thread_id, values) in &usage {
        for (total, value) in totals.iter_mut().zip(values) {
            *total += value;
        }
        let thread = threads.get(thread_id);
        // An agent thread whose root is gone stands for its own conversation.
        let root_id = thread
            .map(|thread| thread.root.clone())
            .filter(|root| threads.contains_key(root))
            .unwrap_or_else(|| thread_id.clone());
        let root = threads.get(&root_id);
        let entry = conversations
            .entry(root_id.clone())
            .or_insert_with(|| StorageThreadDto {
                thread_id: root_id.clone(),
                title: root.map(|root| root.title.clone()).unwrap_or_default(),
                workspace_label: root.and_then(|root| root.workspace_label.clone()),
                bytes: 0,
                command_bytes: 0,
                file_change_bytes: 0,
                items: 0,
                agent_threads: 0,
                last_activity_at: None,
            });
        entry.bytes += values[0];
        entry.command_bytes += values[1];
        entry.file_change_bytes += values[5];
        entry.items += values[2];
        if *thread_id != root_id {
            entry.agent_threads += 1;
        }
        let updated = thread.and_then(|thread| thread.updated_at.clone());
        if updated > entry.last_activity_at {
            entry.last_activity_at = updated;
        }
    }
    let mut rows: Vec<_> = conversations.into_values().collect();
    rows.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then_with(|| a.thread_id.cmp(&b.thread_id))
    });
    Ok(StorageScanDto {
        scanned_at: now_rfc3339(),
        device_name,
        database_bytes: database_file_bytes(path),
        free_bytes,
        history_bytes: totals[0],
        command_bytes: totals[1],
        file_change_bytes: totals[5],
        pruned_items: totals[3],
        pruned_bytes: totals[4],
        thread_count: threads.len() as u64,
        conversation_count: rows.len() as u64,
        threads: rows,
        offset: 0,
        next_offset: None,
    })
}

fn page(scan: &StorageScanDto, offset: u64, limit: u64) -> StorageScanDto {
    let start = (offset as usize).min(scan.threads.len());
    let end = (start + limit as usize).min(scan.threads.len());
    StorageScanDto {
        threads: scan.threads[start..end].to_vec(),
        offset: start as u64,
        next_offset: (end < scan.threads.len()).then_some(end as u64),
        ..scan.clone()
    }
}

/// History items of ended turns (before the cutoff, in the conversation) that
/// this cleanup would change, in storage order.
fn prune_candidates(path: &Path, cleanup: &Cleanup) -> Result<Vec<i64>> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(10))?;
    let text_mode = match cleanup.options.command_text {
        CommandTextCleanup::Keep => "keep",
        CommandTextCleanup::Truncate => "truncate",
        CommandTextCleanup::Remove => "remove",
    };
    let mut statement = conn.prepare(
        "SELECT h.rowid FROM thread_history_items h
         JOIN thread_turns t ON t.id = h.turn_id
         WHERE t.status NOT IN ('inProgress', 'running')
           AND (?1 IS NULL OR COALESCE(t.completed_at, t.started_at) < ?1)
           AND (?2 IS NULL OR h.thread_id IN (SELECT id FROM threads WHERE id = ?2 OR root_thread_id = ?2))
           AND CASE json_extract(h.item_json, '$.kind')
             WHEN 'commandExecution' THEN
               (?3 AND ((json_extract(h.item_json, '$.outputPrunedAt') IS NULL
                         AND length(COALESCE(json_extract(h.item_json, '$.detailText'), '')) > 0)
                        OR instr(COALESCE(json_extract(h.item_json, '$.detailText'), ''), 'Input:' || char(10)) > 0))
               OR (?4 = 'truncate' AND json_extract(h.item_json, '$.textPruned') IS NULL
                   AND length(COALESCE(json_extract(h.item_json, '$.text'), '')) > ?5)
               OR (?4 = 'remove' AND COALESCE(json_extract(h.item_json, '$.textPruned'), '') != 'removed'
                   AND length(COALESCE(json_extract(h.item_json, '$.text'), '')) > 0)
               OR (?6 AND (instr(COALESCE(json_extract(h.item_json, '$.detailText'), ''), 'Input:' || char(10)) > 0
                   OR json_extract(h.item_json, '$.previewText') = json_extract(h.item_json, '$.text')
                   OR substr(COALESCE(json_extract(h.item_json, '$.detailText'), ''), 1,
                             length(json_extract(h.item_json, '$.text')) + 2)
                      = json_extract(h.item_json, '$.text') || char(10) || char(10)))
             WHEN 'fileChange' THEN
               ?7 AND json_extract(h.item_json, '$.outputPrunedAt') IS NULL
               AND length(COALESCE(json_extract(h.item_json, '$.detailText'), '')) > 0
             ELSE 0 END
         ORDER BY h.rowid",
    )?;
    let rows = statement.query_map(
        params![
            cleanup.cutoff,
            cleanup.thread,
            cleanup.options.command_output,
            text_mode,
            cleanup.options.command_text_chars,
            cleanup.dedupe,
            cleanup.options.file_diffs
        ],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Turns the shared connection's automatic checkpoint off until dropped.
struct AutocheckpointPause {
    state: Arc<Supervisor>,
    pages: i64,
}

impl AutocheckpointPause {
    fn new(state: Arc<Supervisor>) -> Result<Self> {
        let pages = state.db.with(|conn| {
            let pages =
                conn.query_row("PRAGMA wal_autocheckpoint", [], |row| row.get::<_, i64>(0))?;
            conn.query_row("PRAGMA wal_autocheckpoint=0", [], |_| Ok(()))?;
            Ok(pages)
        })?;
        Ok(Self { state, pages })
    }
}

impl Drop for AutocheckpointPause {
    fn drop(&mut self) {
        let pages = self.pages;
        let restored = self.state.db.with(|conn| {
            conn.query_row(
                &format!("PRAGMA wal_autocheckpoint={pages}"),
                [],
                |_| Ok(()),
            )?;
            Ok(())
        });
        if let Err(error) = restored {
            tracing::warn!(%error, "restoring the automatic checkpoint failed");
        }
    }
}

/// A passive checkpoint on its own connection: it copies what it can while
/// the shared connection keeps reading and writing.
fn side_checkpoint(path: &Path) -> Result<()> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))?;
    Ok(())
}

struct PruneSlice {
    processed: usize,
    items: u64,
    bytes: u64,
}

/// Rewrite rows from the front of `rows` for at most [`PRUNE_SLICE`].
fn prune_rows(
    conn: &Connection,
    rows: &[i64],
    pruned_at: &str,
    cleanup: &Cleanup,
) -> Result<PruneSlice> {
    let started = std::time::Instant::now();
    let tx = conn.unchecked_transaction()?;
    let mut slice = PruneSlice {
        processed: 0,
        items: 0,
        bytes: 0,
    };
    for rowid in rows {
        if slice.processed > 0 && started.elapsed() >= PRUNE_SLICE {
            break;
        }
        slice.processed += 1;
        // The row may have changed or gone since the candidates were read.
        let Some(json) = tx
            .query_row(
                "SELECT item_json FROM thread_history_items WHERE rowid=?1",
                [rowid],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        else {
            continue;
        };
        let Some(pruned) = prune_item(&json, pruned_at, &cleanup.options) else {
            continue;
        };
        tx.execute(
            "UPDATE thread_history_items SET item_json=?1 WHERE rowid=?2",
            params![pruned, rowid],
        )?;
        slice.items += 1;
        slice.bytes += json.len().saturating_sub(pruned.len()) as u64;
    }
    tx.commit()?;
    Ok(slice)
}

/// The item after this cleanup; `None` when it would not change.
///
/// Every pass also stores the command once: it drops the command from the
/// input and the preview, where older versions copied it. Removed output and
/// diffs are recorded in `outputPrunedAt` and `outputPrunedBytes`; a shortened
/// or removed command in `textPruned`.
pub(crate) fn prune_item(
    json: &str,
    pruned_at: &str,
    options: &StorageCleanupDto,
) -> Option<String> {
    let mut value: Value = serde_json::from_str(json).ok()?;
    let item = value.as_object_mut()?;
    let detail = item
        .get("detailText")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let earlier = item.get("outputPrunedBytes").and_then(Value::as_u64);
    let mut kept = detail.clone();
    let mut changed = false;
    match item.get("kind").and_then(Value::as_str) {
        Some("commandExecution") => {
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if options.command_output {
                kept = detail_header(&detail, earlier.is_some());
            }
            if let Some(slim) = without_command(&kept, &text, item.contains_key("textPruned")) {
                kept = slim;
            }
            let shortened = match options.command_text {
                CommandTextCleanup::Truncate
                    if !item.contains_key("textPruned")
                        && text.chars().count() > options.command_text_chars as usize =>
                {
                    let mut short: String = text
                        .chars()
                        .take(options.command_text_chars as usize)
                        .collect();
                    short.push('…');
                    Some((short, "truncated"))
                }
                CommandTextCleanup::Remove
                    if !text.is_empty()
                        && item.get("textPruned").and_then(Value::as_str) != Some("removed") =>
                {
                    Some((String::new(), "removed"))
                }
                _ => None,
            };
            // Readers of command executions show the text when there is no preview.
            if item.get("previewText").is_some()
                && (shortened.is_some() || item.get("previewText") == item.get("text"))
            {
                item.remove("previewText");
                changed = true;
            }
            if let Some((short, mark)) = shortened {
                item.insert("text".into(), short.into());
                item.insert("textPruned".into(), mark.into());
                changed = true;
            }
        }
        Some("fileChange") if options.file_diffs && earlier.is_none() => kept.clear(),
        _ => return None,
    }
    if kept != detail {
        changed = true;
        if kept.is_empty() {
            item.remove("detailText");
        } else {
            item.insert("detailText".into(), kept.clone().into());
        }
        // Without the cleanup asking, only the input's copy of the command went.
        let removed_output = options.command_output
            || (options.file_diffs
                && item.get("kind").and_then(Value::as_str) == Some("fileChange"));
        if removed_output {
            if earlier.is_none() {
                item.insert("outputPrunedAt".into(), pruned_at.into());
            }
            item.insert(
                "outputPrunedBytes".into(),
                (earlier.unwrap_or(0) + (detail.len() - kept.len()) as u64).into(),
            );
        }
    }
    if !changed {
        return None;
    }
    serde_json::to_string(&value).ok()
}

/// The detail's header (tool, kind, status): everything before its input,
/// output or result. A detail that starts with one of those, or has none of
/// them, is all output, unless an earlier cleanup already left only a header.
fn detail_header(detail: &str, pruned_before: bool) -> String {
    if ["Input:\n", "Output:\n", "Result:\n"]
        .iter()
        .any(|start| detail.starts_with(start))
    {
        return String::new();
    }
    match BODY_MARKERS
        .iter()
        .filter_map(|marker| detail.find(marker))
        .min()
    {
        Some(end) => detail[..end].to_owned(),
        None if pruned_before => detail.to_owned(),
        None => String::new(),
    }
}

/// The detail without its copy of the command `text`, or `None` when it holds
/// none. A shortened or removed text no longer matches; its copy goes anyway.
fn without_command(detail: &str, text: &str, text_pruned: bool) -> Option<String> {
    // Imported sessions put the command above the output.
    if !text.is_empty() && !detail.starts_with("Tool: ") {
        if let Some(output) = detail
            .strip_prefix(text)
            .and_then(|rest| rest.strip_prefix("\n\n"))
        {
            return Some(format!("Output:\n{output}"));
        }
    }
    let start = if detail.starts_with("Input:\n") {
        0
    } else {
        detail.find("\n\nInput:\n")? + 2
    };
    let body = start + "Input:\n".len();
    let end = ["\n\nOutput:\n", "\n\nResult:\n"]
        .iter()
        .filter_map(|marker| detail[body..].find(marker))
        .min()
        .map_or(detail.len(), |end| body + end);
    let input: Value = serde_json::from_str(&detail[body..end]).ok()?;
    let (command, rest) = split_command_input(&input)?;
    if command != text && !text_pruned {
        return None;
    }
    let before = detail[..start].trim_end_matches('\n');
    let after = &detail[end..];
    let mut slim = before.to_owned();
    if let Some(rest) = rest {
        if !slim.is_empty() {
            slim.push_str("\n\n");
        }
        slim.push_str("Input:\n");
        slim.push_str(&pretty_json(&rest));
    }
    if slim.is_empty() {
        slim.push_str(after.trim_start_matches('\n'));
    } else {
        slim.push_str(after);
    }
    Some(slim)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output_only() -> StorageCleanupDto {
        StorageCleanupDto::default()
    }

    fn prune(item: &Value, options: &StorageCleanupDto) -> Option<Value> {
        prune_item(&item.to_string(), "2026-10-18T00:00:00.000Z", options)
            .map(|json| serde_json::from_str(&json).unwrap())
    }

    #[test]
    fn prune_keeps_the_command_and_header_only() {
        let item = json!({
            "id": "exec-1", "kind": "commandExecution", "text": "ls", "previewText": "ls",
            "detailText": "Tool: exec\n\nStatus: completed\n\nInput:\n{\n  \"command\": \"ls\"\n}\n\nOutput:\nfile-a\nfile-b",
            "status": "completed"
        });
        let pruned = prune(&item, &output_only()).unwrap();
        assert_eq!(pruned["detailText"], "Tool: exec\n\nStatus: completed");
        assert_eq!(pruned["text"], "ls");
        assert!(pruned.get("previewText").is_none());
        assert_eq!(pruned["outputPrunedAt"], "2026-10-18T00:00:00.000Z");
        let removed =
            item["detailText"].as_str().unwrap().len() - "Tool: exec\n\nStatus: completed".len();
        assert_eq!(pruned["outputPrunedBytes"], removed);
        // Nothing more to remove: no rewrite.
        assert!(prune(&pruned, &output_only()).is_none());
        // An item cleaned by an earlier version still held its input.
        let earlier = json!({"kind": "commandExecution", "text": "ls", "outputPrunedAt": "first", "outputPrunedBytes": 10,
            "detailText": "Tool: exec\n\nInput:\n{\"command\":\"ls\"}"});
        let slim = prune(&earlier, &output_only()).unwrap();
        assert_eq!(slim["detailText"], "Tool: exec");
        assert_eq!(slim["outputPrunedAt"], "first");
        assert_eq!(
            slim["outputPrunedBytes"],
            10 + "\n\nInput:\n{\"command\":\"ls\"}".len()
        );
        // Without a header, or without known sections, the whole detail goes.
        for detail in ["raw", "Input:\n{}\n\nResult:\nok"] {
            let plain = json!({"kind": "commandExecution", "text": "x", "detailText": detail});
            assert!(prune(&plain, &output_only())
                .unwrap()
                .get("detailText")
                .is_none());
        }
        // Other kinds are never touched.
        let tool = json!({"kind": "toolCall", "text": "CronCreate", "detailText": "Input:\n{}\n\nResult:\nok"});
        assert!(prune(&tool, &output_only()).is_none());
    }

    #[test]
    fn every_pass_stores_the_command_once() {
        let keep = StorageCleanupDto {
            command_output: false,
            ..StorageCleanupDto::default()
        };
        // The input keeps its other fields; the output stays.
        let item = json!({"kind": "commandExecution", "text": "ls -la", "previewText": "ls -la",
            "detailText": "Tool: Bash\n\nInput:\n{\n  \"command\": \"ls -la\",\n  \"timeout\": 5\n}\n\nOutput:\nfile"});
        let slim = prune(&item, &keep).unwrap();
        assert_eq!(
            slim["detailText"],
            "Tool: Bash\n\nInput:\n{\n  \"timeout\": 5\n}\n\nOutput:\nfile"
        );
        assert!(slim.get("previewText").is_none());
        assert!(slim.get("outputPrunedAt").is_none());
        assert!(prune(&slim, &keep).is_none());
        // An input of only the command goes; so does an argv that spells it.
        let argv = json!({"kind": "commandExecution", "text": "git status",
            "detailText": "Tool: exec\n\nInput:\n{\"argv\": [\"git\", \"status\"]}\n\nOutput:\nclean"});
        assert_eq!(
            prune(&argv, &keep).unwrap()["detailText"],
            "Tool: exec\n\nOutput:\nclean"
        );
        // An input naming another command is not a copy.
        let other = json!({"kind": "commandExecution", "text": "Run tests",
            "detailText": "Tool: exec\n\nInput:\n{\"command\": \"cargo test\"}"});
        assert!(prune(&other, &keep).is_none());
        // Imported sessions put the command above the output.
        let imported = json!({"kind": "commandExecution", "text": "make", "previewText": "make",
            "detailText": "make\n\nbuilt"});
        let slim = prune(&imported, &keep).unwrap();
        assert_eq!(slim["detailText"], "Output:\nbuilt");
        assert!(slim.get("previewText").is_none());
    }

    #[test]
    fn prune_shortens_or_removes_the_command_and_removes_diffs() {
        let long = "x".repeat(40);
        let item = json!({"kind": "commandExecution", "text": long, "previewText": long,
            "detailText": format!("Tool: exec\n\nInput:\n{{\"command\": \"{long}\"}}\n\nOutput:\nok")});
        let truncate = StorageCleanupDto {
            command_output: false,
            command_text: CommandTextCleanup::Truncate,
            command_text_chars: 16,
            file_diffs: false,
        };
        let short = prune(&item, &truncate).unwrap();
        assert_eq!(short["text"], format!("{}…", "x".repeat(16)));
        assert_eq!(short["textPruned"], "truncated");
        assert_eq!(short["detailText"], "Tool: exec\n\nOutput:\nok");
        assert!(short.get("previewText").is_none());
        assert!(prune(&short, &truncate).is_none());
        // Removing goes further, also after a truncation.
        let remove = StorageCleanupDto {
            command_text: CommandTextCleanup::Remove,
            ..truncate.clone()
        };
        let gone = prune(&short, &remove).unwrap();
        assert_eq!(
            (gone["text"].as_str(), gone["textPruned"].as_str()),
            (Some(""), Some("removed"))
        );
        assert!(prune(&gone, &remove).is_none());
        // Multibyte text is cut on characters.
        let wide = json!({"kind": "commandExecution", "text": "echo 你好你好你好你好你好你好你好"});
        assert_eq!(
            prune(&wide, &truncate).unwrap()["text"],
            "echo 你好你好你好你好你好你…"
        );
        // Diffs go only when asked, once.
        let diffs = StorageCleanupDto {
            command_output: false,
            file_diffs: true,
            ..StorageCleanupDto::default()
        };
        let change = json!({"kind": "fileChange", "text": "a.rs", "detailText": "--- a/a.rs\n+++ b/a.rs\n+x", "addedLines": 1});
        assert!(prune(&change, &output_only()).is_none());
        let pruned = prune(&change, &diffs).unwrap();
        assert!(pruned.get("detailText").is_none());
        assert_eq!(pruned["outputPrunedBytes"], 24);
        assert_eq!(pruned["addedLines"], 1);
        assert!(prune(&pruned, &diffs).is_none());
    }

    #[tokio::test]
    async fn prune_job_reports_progress_and_restores_the_automatic_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let s = crate::service::automation::tests::supervisor(dir.path());
        let old = "2026-01-01T00:00:00.000Z";
        let output = json!({"id": "exec-1", "kind": "commandExecution", "text": "ls",
            "detailText": "Tool: exec\n\nInput:\n{\"command\":\"ls\"}\n\nOutput:\nfile"})
        .to_string();
        s.db.with(|c| {
            c.execute_batch(&format!(
                "INSERT INTO hosts(id, hostname, platform, created_at, last_seen_at) VALUES ('h', 'h', 'linux', '{old}', '{old}');
                 INSERT INTO workspaces(id, host_id, label, abs_path, created_at, updated_at) VALUES ('w', 'h', 'w', '/w', '{old}', '{old}');"
            )).ok();
            c.execute(
                "INSERT INTO thread_turns(id, thread_id, status, started_at, completed_at, ordinal)
                 VALUES ('turn-old', 'thread', 'completed', ?1, ?1, 0)",
                [old],
            )?;
            c.execute(
                "INSERT INTO thread_history_items(id, thread_id, turn_id, item_id, item_json, created_at, updated_at)
                 VALUES ('row-1', 'thread', 'turn-old', 'exec-1', ?1, ?2, ?2)",
                params![output, old],
            )?;
            Ok(())
        })
        .unwrap();
        s.db.with(|c| {
            c.execute(
                "INSERT INTO storage_settings(id, command_output_retention_days, updated_at) VALUES (1, 3, ?1)",
                [old],
            )?;
            Ok(())
        })
        .unwrap();
        let started = s.start_storage_prune().unwrap().unwrap();
        assert_eq!(
            (started.action.as_str(), started.state.as_str()),
            ("prune", "running")
        );
        // A second request while it runs is refused, naming the running job.
        let busy = s.start_storage_prune().unwrap_err();
        assert_eq!(
            busy.downcast_ref::<StorageBusy>().unwrap().0.started_at,
            started.started_at
        );
        let mut job = started;
        for _ in 0..200 {
            job = s.storage_overview(0, 10).unwrap().job.unwrap();
            if job.state != "running" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            (job.state.as_str(), job.total, job.processed, job.items),
            ("done", 1, 1, 1)
        );
        let (item, pages): (String, i64) =
            s.db.with(|c| {
                Ok((
                    c.query_row(
                        "SELECT item_json FROM thread_history_items WHERE id='row-1'",
                        [],
                        |r| r.get(0),
                    )?,
                    c.query_row("PRAGMA wal_autocheckpoint", [], |r| r.get(0))?,
                ))
            })
            .unwrap();
        let item: Value = serde_json::from_str(&item).unwrap();
        assert_eq!(item["detailText"], "Tool: exec");
        assert!(item["outputPrunedAt"].is_string());
        assert_eq!(pages, 1000);
    }

    #[test]
    fn prune_rows_stops_after_its_slice_and_skips_missing_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE thread_history_items(item_json TEXT NOT NULL);")
            .unwrap();
        let item = json!({"kind": "commandExecution", "text": "ls", "detailText": "Tool: t\n\nOutput:\nx"}).to_string();
        for _ in 0..3 {
            conn.execute(
                "INSERT INTO thread_history_items(item_json) VALUES (?1)",
                [&item],
            )
            .unwrap();
        }
        let cleanup = Cleanup {
            cutoff: None,
            thread: None,
            options: StorageCleanupDto::default(),
            dedupe: false,
        };
        let slice = prune_rows(&conn, &[1, 99, 2, 3], "t", &cleanup).unwrap();
        assert_eq!((slice.processed, slice.items), (4, 3));
        let left: i64 = conn
            .query_row(
                "SELECT count(*) FROM thread_history_items WHERE instr(item_json, 'Output') > 0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
    }
}
