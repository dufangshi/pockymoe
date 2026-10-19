//! Disk use of the Supervisor database and the command-output retention policy.
//!
//! A scan runs only on request, on its own read-only connection, so the shared
//! connection stays free. Cleanup and compaction run as background jobs whose
//! progress the overview reports. Cleanup finds its items on a read-only
//! connection too and rewrites them in transactions of a few milliseconds, so
//! every other database user waits at most that long. It keeps each command's
//! text and the detail header (tool, kind, status) and removes the input
//! (the command again) and the output.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use pockymoe_protocol::{
    StorageJobDto, StorageOverviewDto, StorageScanDto, StorageSettingsDto, StorageThreadDto,
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
/// Detail sections after the header: the input repeats the command, then the
/// output. Everything before the first one stays.
const BODY_MARKERS: &[&str] = &["\n\nInput:\n", "\n\nOutput:\n", "\n\nResult:\n"];

#[derive(Default)]
pub(crate) struct StorageState {
    scan: std::sync::Mutex<Option<Arc<StorageScanDto>>>,
    gate: tokio::sync::Mutex<()>,
    started: AtomicBool,
    job: std::sync::Mutex<Option<StorageJobDto>>,
}

enum StorageAction {
    Prune(u32),
    Compact,
}

impl Supervisor {
    pub fn storage_settings(&self) -> Result<StorageSettingsDto> {
        self.db.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT command_output_retention_days FROM storage_settings WHERE id=1",
                    [],
                    |row| {
                        Ok(StorageSettingsDto {
                            command_output_retention_days: row
                                .get::<_, Option<i64>>(0)?
                                .and_then(|days| u32::try_from(days).ok()),
                        })
                    },
                )
                .optional()?
                .unwrap_or_default())
        })
    }

    /// Replace the policy; a set retention prunes at once in the background.
    pub fn update_storage_settings(
        self: &Arc<Self>,
        settings: StorageSettingsDto,
    ) -> Result<StorageSettingsDto> {
        if let Some(days) = settings.command_output_retention_days {
            if !(1..=MAX_RETENTION_DAYS).contains(&days) {
                return Err(InvalidSettings(format!(
                    "commandOutputRetentionDays must be 1-{MAX_RETENTION_DAYS}"
                ))
                .into());
            }
        }
        self.db.with(|conn| {
            conn.execute(
                "INSERT INTO storage_settings(id, command_output_retention_days, updated_at)
                 VALUES (1, ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET
                   command_output_retention_days=excluded.command_output_retention_days,
                   updated_at=excluded.updated_at",
                params![settings.command_output_retention_days, now_rfc3339()],
            )?;
            Ok(())
        })?;
        if let Some(days) = settings.command_output_retention_days {
            self.start_storage_job(StorageAction::Prune(days));
        }
        self.storage_settings()
    }

    /// Hourly retention pass; the first one runs a few minutes after start.
    pub fn spawn_storage_retention(self: &Arc<Self>) {
        if self.storage.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(180)).await;
            loop {
                let Some(state) = weak.upgrade() else { break };
                match state.storage_settings() {
                    Ok(StorageSettingsDto {
                        command_output_retention_days: Some(days),
                    }) => {
                        state.start_storage_job(StorageAction::Prune(days));
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!(%error, "reading storage settings failed"),
                }
                drop(state);
                tokio::time::sleep(RETENTION_INTERVAL).await;
            }
        });
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

    /// Start a cleanup with the given retention; returns the running job
    /// (an already running one is returned as is).
    pub fn start_storage_prune(self: &Arc<Self>, days: u32) -> StorageJobDto {
        self.start_storage_job(StorageAction::Prune(days))
    }

    /// Start rewriting the file without its free pages.
    pub fn start_storage_compact(self: &Arc<Self>) -> StorageJobDto {
        self.start_storage_job(StorageAction::Compact)
    }

    fn start_storage_job(self: &Arc<Self>, action: StorageAction) -> StorageJobDto {
        let mut slot = self.storage.job.lock().unwrap();
        if let Some(job) = slot.as_ref().filter(|job| job.state == "running") {
            return job.clone();
        }
        let job = StorageJobDto {
            action: match action {
                StorageAction::Prune(_) => "prune",
                StorageAction::Compact => "compact",
            }
            .into(),
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
        };
        *slot = Some(job.clone());
        drop(slot);
        let state = self.clone();
        tokio::spawn(async move {
            let result = match action {
                StorageAction::Prune(days) => state.run_prune(days).await,
                StorageAction::Compact => state.run_compact().await,
            };
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
        });
        job
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

    /// Remove command output from turns that ended at least `days` ago.
    async fn run_prune(self: &Arc<Self>, days: u32) -> Result<()> {
        let _gate = self.storage.gate.lock().await;
        let cutoff = (Utc::now() - chrono::Duration::days(i64::from(days)))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let path = self.database_path()?;
        // Finding candidates reads every history item: never on the shared connection.
        let counting = std::time::Instant::now();
        let candidates_path = path.clone();
        let rows = tokio::task::spawn_blocking(move || prune_candidates(&candidates_path, &cutoff))
            .await??;
        tracing::info!(
            items = rows.len(),
            ms = counting.elapsed().as_millis() as u64,
            "storage cleanup candidates"
        );
        self.update_job(|job| job.total = rows.len() as u64);
        let pruned_at = now_rfc3339();
        let rows = Arc::new(rows);
        // The shared connection would otherwise checkpoint inside one of its
        // commits; the side checkpoints below do it outside its lock.
        let _autocheckpoint = AutocheckpointPause::new(self.clone())?;
        let mut done = 0_usize;
        let mut checkpointed = std::time::Instant::now();
        while done < rows.len() {
            let state = self.clone();
            let (rows_ref, at) = (rows.clone(), pruned_at.clone());
            let timer = std::time::Instant::now();
            let slice = tokio::task::spawn_blocking(move || {
                state
                    .db
                    .with(|conn| prune_rows(conn, &rows_ref[done..], &at))
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
    // (bytes, command bytes, items, pruned items, pruned bytes) per thread.
    let mut usage: Vec<(String, [u64; 5])> = Vec::new();
    {
        let mut statement = conn.prepare(
            "SELECT thread_id,
               SUM(length(CAST(item_json AS BLOB))),
               SUM(CASE WHEN json_extract(item_json, '$.kind') = 'commandExecution'
                   THEN length(CAST(item_json AS BLOB)) ELSE 0 END),
               COUNT(*),
               SUM(json_extract(item_json, '$.outputPrunedAt') IS NOT NULL),
               SUM(COALESCE(json_extract(item_json, '$.outputPrunedBytes'), 0))
             FROM thread_history_items GROUP BY thread_id",
        )?;
        let rows = statement.query_map([], |row| {
            let number = |index: usize| {
                row.get::<_, Option<i64>>(index)
                    .map(|v| v.unwrap_or(0).max(0) as u64)
            };
            Ok((
                row.get::<_, String>(0)?,
                [number(1)?, number(2)?, number(3)?, number(4)?, number(5)?],
            ))
        })?;
        for row in rows {
            usage.push(row?);
        }
    }
    let mut totals = [0_u64; 5];
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
                items: 0,
                agent_threads: 0,
                last_activity_at: None,
            });
        entry.bytes += values[0];
        entry.command_bytes += values[1];
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

/// Command executions of ended turns before the cutoff that still hold more
/// than their header, in storage order.
fn prune_candidates(path: &Path, cutoff: &str) -> Result<Vec<i64>> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(10))?;
    let mut statement = conn.prepare(
        "SELECT h.rowid FROM thread_history_items h
         JOIN thread_turns t ON t.id = h.turn_id
         WHERE t.status NOT IN ('inProgress', 'running')
           AND COALESCE(t.completed_at, t.started_at) < ?1
           AND json_extract(h.item_json, '$.kind') = 'commandExecution'
           AND (json_extract(h.item_json, '$.outputPrunedAt') IS NULL
                OR instr(COALESCE(json_extract(h.item_json, '$.detailText'), ''), 'Input:' || char(10)) > 0)
         ORDER BY h.rowid",
    )?;
    let rows = statement.query_map([cutoff], |row| row.get::<_, i64>(0))?;
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
fn prune_rows(conn: &Connection, rows: &[i64], pruned_at: &str) -> Result<PruneSlice> {
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
        let Some((pruned, removed)) = prune_item(&json, pruned_at) else {
            continue;
        };
        tx.execute(
            "UPDATE thread_history_items SET item_json=?1 WHERE rowid=?2",
            params![pruned, rowid],
        )?;
        slice.items += 1;
        slice.bytes += removed;
    }
    tx.commit()?;
    Ok(slice)
}

/// The command execution reduced to its text and detail header, and the bytes
/// that went. `None` when there is nothing left to remove.
pub(crate) fn prune_item(json: &str, pruned_at: &str) -> Option<(String, u64)> {
    let mut value: Value = serde_json::from_str(json).ok()?;
    let item = value.as_object_mut()?;
    if item.get("kind").and_then(Value::as_str) != Some("commandExecution") {
        return None;
    }
    let detail = item
        .get("detailText")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let earlier = item.get("outputPrunedBytes").and_then(Value::as_u64);
    // A detail may also start with its input or output, without a header. One
    // with no known section is all output, unless an earlier cleanup left it.
    let kept = if ["Input:\n", "Output:\n", "Result:\n"]
        .iter()
        .any(|start| detail.starts_with(start))
    {
        String::new()
    } else {
        match BODY_MARKERS
            .iter()
            .filter_map(|marker| detail.find(marker))
            .min()
        {
            Some(end) => detail[..end].to_owned(),
            None if earlier.is_some() => detail.clone(),
            None => String::new(),
        }
    };
    let mut removed = (detail.len() - kept.len()) as u64;
    if kept.is_empty() {
        item.remove("detailText");
    } else {
        item.insert("detailText".into(), kept.into());
    }
    // Readers of command executions treat a missing previewText as the text.
    if item.get("previewText").is_some() && item.get("previewText") == item.get("text") {
        removed += item
            .remove("previewText")
            .and_then(|text| text.as_str().map(str::len))
            .unwrap_or(0) as u64;
    }
    if removed == 0 && earlier.is_some() {
        return None;
    }
    if earlier.is_none() {
        item.insert("outputPrunedAt".into(), pruned_at.into());
    }
    item.insert(
        "outputPrunedBytes".into(),
        (earlier.unwrap_or(0) + removed).into(),
    );
    Some((serde_json::to_string(&value).ok()?, removed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_the_command_and_header_only() {
        let item = json!({
            "id": "exec-1", "kind": "commandExecution", "text": "ls", "previewText": "ls",
            "detailText": "Tool: exec\n\nStatus: completed\n\nInput:\n{\n  \"command\": \"ls\"\n}\n\nOutput:\nfile-a\nfile-b",
            "status": "completed"
        });
        let (pruned, removed) = prune_item(&item.to_string(), "2026-10-18T00:00:00.000Z").unwrap();
        let pruned: Value = serde_json::from_str(&pruned).unwrap();
        assert_eq!(pruned["detailText"], "Tool: exec\n\nStatus: completed");
        assert_eq!(pruned["text"], "ls");
        assert!(pruned.get("previewText").is_none());
        assert_eq!(pruned["outputPrunedAt"], "2026-10-18T00:00:00.000Z");
        assert_eq!(pruned["outputPrunedBytes"], removed);
        // Nothing more to remove: no rewrite.
        assert!(prune_item(&pruned.to_string(), "later").is_none());
        // An item cleaned by an earlier version still held its input.
        let earlier = json!({"kind": "commandExecution", "text": "ls", "outputPrunedAt": "first", "outputPrunedBytes": 10,
            "detailText": "Tool: exec\n\nInput:\n{\"command\":\"ls\"}"});
        let (slim, removed) = prune_item(&earlier.to_string(), "later").unwrap();
        let slim: Value = serde_json::from_str(&slim).unwrap();
        assert_eq!(slim["detailText"], "Tool: exec");
        assert_eq!(slim["outputPrunedAt"], "first");
        assert_eq!(slim["outputPrunedBytes"], 10 + removed);
        // Without a header, or without known sections, the whole detail goes.
        for detail in ["raw", "Input:\n{}\n\nResult:\nok"] {
            let plain = json!({"kind": "commandExecution", "text": "x", "detailText": detail});
            let (pruned, _) = prune_item(&plain.to_string(), "t").unwrap();
            assert!(serde_json::from_str::<Value>(&pruned)
                .unwrap()
                .get("detailText")
                .is_none());
        }
        // Other kinds are never touched.
        let tool = json!({"kind": "toolCall", "text": "CronCreate", "detailText": "Input:\n{}\n\nResult:\nok"});
        assert!(prune_item(&tool.to_string(), "t").is_none());
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
        let started = s.start_storage_prune(3);
        assert_eq!(
            (started.action.as_str(), started.state.as_str()),
            ("prune", "running")
        );
        // A second request while it runs returns the same job.
        assert_eq!(s.start_storage_prune(3).started_at, started.started_at);
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
        let slice = prune_rows(&conn, &[1, 99, 2, 3], "t").unwrap();
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
