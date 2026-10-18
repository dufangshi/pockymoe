//! Disk use of the Supervisor database and the command-output retention policy.
//!
//! A scan runs only on request, on its own read-only connection, so the shared
//! connection stays free. Pruning keeps each command, its status and its input
//! (watches read the input) and removes the output that follows.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use pockymoe_protocol::{
    StorageCompactResultDto, StorageOverviewDto, StoragePruneResultDto, StorageScanDto,
    StorageSettingsDto, StorageThreadDto,
};
use rusqlite::OpenFlags;

pub const STORAGE_PAGE_LIMIT: u64 = 100;
const PRUNE_BATCH: usize = 200;
const RETENTION_INTERVAL: Duration = Duration::from_secs(60 * 60);
const MAX_RETENTION_DAYS: u32 = 3650;
/// Detail sections holding a tool's output; everything before the first stays.
const OUTPUT_MARKERS: &[&str] = &["\n\nOutput:\n", "\n\nResult:\n"];

#[derive(Default)]
pub(crate) struct StorageState {
    scan: std::sync::Mutex<Option<Arc<StorageScanDto>>>,
    gate: tokio::sync::Mutex<()>,
    started: AtomicBool,
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
            let state = self.clone();
            tokio::spawn(async move {
                if let Err(error) = state.prune_command_outputs(days).await {
                    tracing::warn!(%error, "command output retention failed");
                }
            });
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
                        if let Err(error) = state.prune_command_outputs(days).await {
                            tracing::warn!(%error, "command output retention failed");
                        }
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
        })
    }

    /// Remove command output from turns that ended at least `days` ago.
    pub async fn prune_command_outputs(&self, days: u32) -> Result<StoragePruneResultDto> {
        let _gate = self.storage.gate.lock().await;
        let cutoff = (Utc::now() - chrono::Duration::days(i64::from(days)))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let pruned_at = now_rfc3339();
        let mut result = StoragePruneResultDto { items: 0, bytes: 0 };
        let mut after = 0_i64;
        loop {
            let batch = self
                .db
                .with(|conn| prune_batch(conn, &cutoff, &pruned_at, after))?;
            result.items += batch.items;
            result.bytes += batch.bytes;
            let Some(last) = batch.last_rowid else { break };
            after = last;
            // Each batch is one short transaction; let other writers in between.
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if result.items > 0 {
            *self.storage.scan.lock().unwrap() = None;
            tracing::info!(
                items = result.items,
                bytes = result.bytes,
                days,
                "pruned command output"
            );
        }
        Ok(result)
    }

    /// Rewrite the file without its free pages. Every database use waits meanwhile.
    pub async fn storage_compact(self: &Arc<Self>) -> Result<StorageCompactResultDto> {
        let _gate = self.storage.gate.lock().await;
        let state = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            state.db.with(|conn| {
                let before = file_bytes(conn)?;
                conn.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
                Ok(StorageCompactResultDto {
                    before_bytes: before,
                    after_bytes: file_bytes(conn)?,
                })
            })
        })
        .await??;
        *self.storage.scan.lock().unwrap() = None;
        Ok(result)
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

struct PruneBatch {
    items: u64,
    bytes: u64,
    last_rowid: Option<i64>,
}

fn prune_batch(conn: &Connection, cutoff: &str, pruned_at: &str, after: i64) -> Result<PruneBatch> {
    let tx = conn.unchecked_transaction()?;
    let candidates = {
        let mut statement = tx.prepare(
            "SELECT h.rowid, h.item_json FROM thread_history_items h
             JOIN thread_turns t ON t.id = h.turn_id
             WHERE h.rowid > ?1
               AND t.status NOT IN ('inProgress', 'running')
               AND COALESCE(t.completed_at, t.started_at) < ?2
               AND json_extract(h.item_json, '$.kind') = 'commandExecution'
               AND json_extract(h.item_json, '$.outputPrunedAt') IS NULL
               AND length(COALESCE(json_extract(h.item_json, '$.detailText'), '')) > 0
             ORDER BY h.rowid LIMIT ?3",
        )?;
        let rows = statement.query_map(params![after, cutoff, PRUNE_BATCH as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut batch = PruneBatch {
        items: 0,
        bytes: 0,
        last_rowid: candidates.last().map(|row| row.0),
    };
    for (rowid, json) in &candidates {
        let Some((pruned, removed)) = prune_item(json, pruned_at) else {
            continue;
        };
        tx.execute(
            "UPDATE thread_history_items SET item_json=?1 WHERE rowid=?2",
            params![pruned, rowid],
        )?;
        batch.items += 1;
        batch.bytes += removed;
    }
    tx.commit()?;
    Ok(batch)
}

/// The item without its command output, and the bytes that went.
pub(crate) fn prune_item(json: &str, pruned_at: &str) -> Option<(String, u64)> {
    let mut value: Value = serde_json::from_str(json).ok()?;
    let item = value.as_object_mut()?;
    let detail = item.get("detailText")?.as_str()?.to_owned();
    let kept = OUTPUT_MARKERS
        .iter()
        .filter_map(|marker| detail.find(marker))
        .min()
        .map(|end| detail[..end].to_owned())
        .unwrap_or_default();
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
    item.insert("outputPrunedAt".into(), pruned_at.into());
    item.insert("outputPrunedBytes".into(), removed.into());
    Some((serde_json::to_string(&value).ok()?, removed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_command_input_and_marks_the_item() {
        let item = json!({
            "id": "exec-1", "kind": "commandExecution", "text": "ls", "previewText": "ls",
            "detailText": "Tool: exec\n\nStatus: completed\n\nInput:\n{\n  \"command\": \"ls\"\n}\n\nOutput:\nfile-a\nfile-b",
            "status": "completed"
        });
        let (pruned, removed) = prune_item(&item.to_string(), "2026-10-18T00:00:00.000Z").unwrap();
        let pruned: Value = serde_json::from_str(&pruned).unwrap();
        assert_eq!(
            pruned["detailText"],
            "Tool: exec\n\nStatus: completed\n\nInput:\n{\n  \"command\": \"ls\"\n}"
        );
        assert_eq!(pruned["text"], "ls");
        assert!(pruned.get("previewText").is_none());
        assert_eq!(pruned["outputPrunedAt"], "2026-10-18T00:00:00.000Z");
        assert_eq!(pruned["outputPrunedBytes"], removed);
        assert_eq!(removed, ("\n\nOutput:\nfile-a\nfile-b".len() + 2) as u64);
        // Without a known section the whole detail is the output.
        let plain = json!({"kind": "commandExecution", "text": "ls", "detailText": "raw"});
        let (pruned, _) = prune_item(&plain.to_string(), "t").unwrap();
        assert!(serde_json::from_str::<Value>(&pruned)
            .unwrap()
            .get("detailText")
            .is_none());
        let result = json!({"kind": "commandExecution", "text": "x", "detailText": "Input:\n{}\n\nResult:\nok"});
        let (pruned, _) = prune_item(&result.to_string(), "t").unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&pruned).unwrap()["detailText"],
            "Input:\n{}"
        );
    }
}
