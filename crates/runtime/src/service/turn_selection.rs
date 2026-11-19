//! Exports and publications read only the turns they include: SQL picks the
//! turns from metadata, then one query loads just those turns' items.
use super::*;

/// The turns an export or publication includes.
pub enum TurnSelection<'a> {
    /// The newest `n` turns.
    Latest(u32),
    /// These turn IDs. IDs that are not turns of the thread are left out, so
    /// callers compare the result length when every ID must exist.
    Ids(&'a [String]),
}

pub struct NumberedTurn {
    /// 1-based position of the turn in the whole thread.
    pub number: u32,
    pub turn: ThreadTurnDto,
}

/// One row of the export turn picker.
pub struct ExportTurnOption {
    pub number: u32,
    pub id: String,
    pub started_at: Option<String>,
    pub status: String,
    pub user_prompt_preview: String,
}

pub struct SelectedTurns {
    pub thread: ThreadDto,
    pub workspace: WorkspaceDto,
    /// In thread order, with full items, prepared like `?view=full`.
    pub turns: Vec<NumberedTurn>,
}

impl Supervisor {
    pub async fn get_thread_turns(
        &self,
        id: &str,
        selection: TurnSelection<'_>,
    ) -> Result<SelectedTurns> {
        self.sync_claude_scheduled_history(id).await?;
        self.observe_execution(id).await?;
        let thread = self.get_thread(id)?;
        let workspace = self.get_workspace(&thread.workspace_id)?;
        let (numbers, mut turns) = self.load_selected_turns(id, selection)?;
        self.hydrate_missing_harness_usage(&thread, &mut turns)
            .await?;
        self.hydrate_legacy_claude_reply_phases(&thread, &mut turns)
            .await;
        if thread.source == "local_codex_import" {
            normalize_imported_turns(&mut turns);
        }
        Ok(SelectedTurns {
            thread,
            workspace,
            turns: numbers
                .into_iter()
                .zip(turns)
                .map(|(number, turn)| NumberedTurn { number, turn })
                .collect(),
        })
    }

    fn load_selected_turns(
        &self,
        thread_id: &str,
        selection: TurnSelection<'_>,
    ) -> Result<(Vec<u32>, Vec<ThreadTurnDto>)> {
        let catalog = self.model_pricing();
        let (ids, limit) = match selection {
            TurnSelection::Latest(count) => (None, i64::from(count)),
            TurnSelection::Ids(ids) => (Some(serde_json::to_string(ids)?), -1),
        };
        self.db.with(|conn| {
            // Numbering reads only (id, ordinal) for the thread; the selected
            // rows alone are joined back for their metadata.
            let mut stmt = conn.prepare(&format!(
                "SELECT {TURN_META_COLUMNS}, n.number FROM (
                   SELECT id AS turn_id, ROW_NUMBER() OVER (ORDER BY ordinal ASC, rowid ASC) AS number
                   FROM thread_turns WHERE thread_id=?1) n
                 JOIN thread_turns t ON t.id=n.turn_id
                 WHERE ?2 IS NULL OR n.turn_id IN (SELECT value FROM json_each(?2))
                 ORDER BY n.number DESC LIMIT ?3"
            ))?;
            let mut rows = stmt
                .query_map(params![thread_id, ids, limit], |row| {
                    Ok((
                        u32::try_from(row.get::<_, i64>(11)?).unwrap_or(u32::MAX),
                        turn_meta_row(row, &catalog)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            rows.reverse();
            let turn_ids = rows.iter().map(|(_, turn)| &turn.id).collect::<Vec<_>>();
            let mut items: HashMap<String, Vec<ThreadHistoryItemDto>> = HashMap::new();
            let mut stmt = conn.prepare(
                "SELECT turn_id, item_json FROM thread_history_items
                 WHERE thread_id=?1 AND turn_id IN (SELECT value FROM json_each(?2))
                 ORDER BY turn_id, created_at ASC, rowid ASC",
            )?;
            let found = stmt.query_map(
                params![thread_id, serde_json::to_string(&turn_ids)?],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            for (turn_id, raw) in found.filter_map(|row| row.ok()) {
                let Ok(mut item) = serde_json::from_str::<ThreadHistoryItemDto>(&raw) else {
                    continue;
                };
                crate::history::normalize_legacy_file_change(&mut item);
                items.entry(turn_id).or_default().push(item);
            }
            drop(stmt);
            let mut numbers = Vec::with_capacity(rows.len());
            let mut turns = Vec::with_capacity(rows.len());
            for (number, mut turn) in rows {
                turn.items = items.remove(&turn.id).unwrap_or_default();
                peer_work::annotate(conn, thread_id, &turn.id, &mut turn.items, true)?;
                numbers.push(number);
                turns.push(turn);
            }
            Ok((numbers, turns))
        })
    }

    /// The export turn picker reads turn metadata and user messages only; it
    /// never decodes replies or tool output.
    pub fn export_turn_options(&self, thread_id: &str) -> Result<Vec<ExportTurnOption>> {
        let imported = self.get_thread(thread_id)?.source == "local_codex_import";
        self.db.with(|conn| {
            // The kind list lets SQLite use thread_history_conversation_idx.
            let mut stmt = conn.prepare(
                "SELECT t.id, t.started_at, t.status, json_extract(i.item_json, '$.text')
                 FROM thread_turns t LEFT JOIN thread_history_items i
                   ON i.thread_id=t.thread_id AND i.turn_id=t.id
                  AND json_extract(i.item_json, '$.kind') IN ('userMessage', 'agentMessage')
                  AND json_extract(i.item_json, '$.kind')='userMessage'
                 WHERE t.thread_id=?1
                 ORDER BY t.ordinal ASC, t.rowid ASC, i.created_at ASC, i.rowid ASC",
            )?;
            let mut rows = stmt.query(params![thread_id])?;
            let mut options: Vec<ExportTurnOption> = Vec::new();
            let mut previewed = false;
            while let Some(row) = rows.next()? {
                let id: String = row.get(0)?;
                if options.last().is_none_or(|last| last.id != id) {
                    options.push(ExportTurnOption {
                        number: u32::try_from(options.len() + 1).unwrap_or(u32::MAX),
                        id,
                        started_at: row.get(1)?,
                        status: row.get(2)?,
                        user_prompt_preview: String::new(),
                    });
                    previewed = false;
                }
                let Some(text) = row.get::<_, Option<String>>(3)? else {
                    continue;
                };
                if previewed {
                    continue;
                }
                // Imported Codex turns hide injected context, like the transcript.
                let text = if imported {
                    let Some(text) = crate::local_sessions::sanitize_codex_user_text(&text) else {
                        continue;
                    };
                    text
                } else {
                    text
                };
                options.last_mut().unwrap().user_prompt_preview = text;
                previewed = true;
            }
            Ok(options)
        })
    }
}
