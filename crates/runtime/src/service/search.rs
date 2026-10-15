use anyhow::{ensure, Result};
use rusqlite::params;
use serde_json::{json, Value};

use super::Supervisor;

/// Return only conversational excerpts. Searching must not hydrate tool output,
/// pricing or harness sessions, or transfer entire history to the browser.
impl Supervisor {
    /// Searches only this device's durable journal. Workspace/thread scope is
    /// part of the SQL predicate, never a browser-side result filter.
    pub fn search_conversations(
        &self,
        query: &str,
        workspace_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Value> {
        let query = query.trim();
        ensure!(
            !query.is_empty() && query.chars().count() <= 200,
            "bad_request: query must contain 1 to 200 characters"
        );
        ensure!(
            (1..=100).contains(&limit) && offset <= 10_000,
            "bad_request: limit must be 1 to 100 and offset at most 10000"
        );
        if let Some(id) = workspace_id {
            self.get_workspace(id)?;
        }
        let needle = query.to_lowercase();
        // FTS5 trigram has no tokens below three Unicode characters. Such
        // queries intentionally scan the compact conversational projection.
        // Always verify literal containment: punctuation is data, not syntax.
        let use_fts = needle.chars().count() >= 3;
        let fts_query = format!("\"{}\"", needle.replace('"', "\"\""));
        let visible = if self.thread_host.get().is_some() {
            Some(serde_json::to_string(
                &self
                    .list_threads(None, true)?
                    .iter()
                    .map(|t| &t.id)
                    .collect::<Vec<_>>(),
            )?)
        } else {
            None
        };
        self.db.with(|conn| {
            let candidates = if use_fts {
                "JOIN search_documents_fts f ON f.rowid=d.id"
            } else { "" };
            let predicate = if use_fts { "AND search_documents_fts MATCH ?5" } else { "AND ?5 IS NOT NULL" };
            let sql = format!(
                "SELECT d.thread_id, t.title, t.workspace_id, w.label, w.abs_path,
                        d.turn_id, d.item_id, d.kind, d.text, d.created_at
                 FROM search_documents d {candidates}
                 JOIN threads t ON t.id=d.thread_id
                 JOIN workspaces w ON w.id=t.workspace_id
                 WHERE (?1 IS NULL OR t.workspace_id=?1)
                   AND (?6 IS NULL OR t.id IN (SELECT value FROM json_each(?6)))
                   AND instr(d.folded, ?2)>0 {predicate}
                   AND (d.kind='title' OR EXISTS (
                     SELECT 1 FROM thread_turns r WHERE r.id=d.turn_id AND r.thread_id=d.thread_id))
                   AND (t.source!='local_codex_import' OR d.kind='title' OR NOT EXISTS (
                     SELECT 1 FROM search_documents earlier
                     WHERE earlier.thread_id=d.thread_id AND earlier.turn_id=d.turn_id
                       AND earlier.kind=d.kind AND earlier.text=d.text
                       AND (earlier.phase='commentary')=(d.phase='commentary')
                       AND (earlier.created_at<d.created_at OR
                         (earlier.created_at=d.created_at AND earlier.id<d.id))))
                 ORDER BY d.created_at DESC, d.id DESC LIMIT ?3 OFFSET ?4"
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(params![workspace_id, needle, limit + 1, offset, fts_query, visible])?;
            let mut matches = Vec::new();
            while let Some(row) = rows.next()? {
                let kind: String = row.get(7)?;
                let text: String = row.get(8)?;
                let Some(excerpt) = search_excerpt(&text, &needle) else { continue };
                matches.push(json!({
                    "threadId": row.get::<_, String>(0)?,
                    "threadTitle": row.get::<_, String>(1)?,
                    "workspaceId": row.get::<_, String>(2)?,
                    "workspaceLabel": row.get::<_, String>(3)?,
                    "workspacePath": row.get::<_, String>(4)?,
                    "turnId": row.get::<_, Option<String>>(5)?,
                    "itemId": row.get::<_, Option<String>>(6)?,
                    "kind": if kind == "title" { "title" } else { "message" },
                    "role": match kind.as_str() { "userMessage" => "You", "agentMessage" => "Assistant", _ => "Title" },
                    "text": excerpt,
                    "createdAt": row.get::<_, String>(9)?,
                }));
            }
            let has_more = matches.len() > limit;
            matches.truncate(limit);
            Ok(json!({"matches": matches, "hasMore": has_more,
                "nextOffset": if has_more { Some(offset + limit) } else { None },
                "scope": "device"}))
        })
    }

    pub fn search_thread_messages(&self, id: &str, query: &str, limit: usize) -> Result<Value> {
        let imported = self.get_thread(id)?.source == "local_codex_import";
        let query = query.trim();
        ensure!(
            !query.is_empty() && query.chars().count() <= 200,
            "bad_request: query must contain 1 to 200 characters"
        );
        ensure!(
            (1..=100).contains(&limit),
            "bad_request: limit must be between 1 and 100"
        );
        let needle = query.to_lowercase();
        self.db.with(|conn| {
            let mut statement = conn.prepare(
                "SELECT i.turn_id, i.item_id, json_extract(i.item_json, '$.kind'),
                        json_extract(i.item_json, '$.text'), i.created_at,
                        COALESCE(json_extract(i.item_json, '$.phase'), json_extract(i.item_json, '$.status'))
                 FROM thread_turns t JOIN thread_history_items i
                   ON i.thread_id=t.thread_id AND i.turn_id=t.id
                 WHERE t.thread_id=?1
                   AND json_extract(i.item_json, '$.kind') IN ('userMessage', 'agentMessage')
                 ORDER BY t.ordinal DESC, i.created_at ASC, i.rowid ASC",
            )?;
            let mut rows = statement.query(params![id])?;
            let mut matches = Vec::new();
            let mut has_more = false;
            let mut current_turn = String::new();
            let mut seen = std::collections::HashSet::new();
            while let Some(row) = rows.next()? {
                let turn_id: String = row.get(0)?;
                let kind: String = row.get(2)?;
                let mut text: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();
                if imported {
                    if current_turn != turn_id { current_turn.clone_from(&turn_id); seen.clear(); }
                    if kind == "userMessage" {
                        let Some(cleaned) = crate::local_sessions::sanitize_codex_user_text(&text) else { continue };
                        text = cleaned;
                    }
                    let commentary = kind == "agentMessage" && row.get::<_, Option<String>>(5)?.as_deref() == Some("commentary");
                    if !seen.insert((kind.clone(), text.trim().to_string(), commentary)) { continue; }
                }
                let Some(excerpt) = search_excerpt(&text, &needle) else { continue };
                if matches.len() == limit {
                    has_more = true;
                    break;
                }
                matches.push(json!({
                    "turnId": turn_id,
                    "itemId": row.get::<_, String>(1)?,
                    "role": if kind == "userMessage" { "You" } else { "Assistant" },
                    "text": excerpt,
                    "createdAt": row.get::<_, String>(4)?,
                }));
            }
            Ok(json!({ "matches": matches, "hasMore": has_more }))
        })
    }
}

fn search_excerpt(text: &str, needle: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let start = lower.find(needle)?;
    let end = start + needle.len();
    // Lowercasing can expand a Unicode character. Map the folded match back to
    // original characters before slicing, retaining emoji and non-ASCII case.
    let chars: Vec<char> = text.chars().collect();
    let mut folded = 0;
    let mut first = None;
    let mut last = 0;
    for (index, ch) in chars.iter().enumerate() {
        let next = folded + ch.to_lowercase().map(char::len_utf8).sum::<usize>();
        if next > start && first.is_none() {
            first = Some(index);
        }
        if folded < end {
            last = index + 1;
        }
        folded = next;
        if folded >= end {
            break;
        }
    }
    let begin = first.unwrap_or(0).saturating_sub(100);
    let finish = (last + 240).min(chars.len());
    Some(format!(
        "{}{}{}",
        if begin > 0 { "…" } else { "" },
        chars[begin..finish].iter().collect::<String>(),
        if finish < chars.len() { "…" } else { "" }
    ))
}

#[cfg(test)]
mod tests {
    use super::search_excerpt;

    #[test]
    fn excerpts_keep_literal_unicode_matches_bounded() {
        let text = format!(
            "{}🙂 İSTANBUL 中文 100% _ {}",
            "before ".repeat(300),
            "after ".repeat(300)
        );
        let excerpt = search_excerpt(&text, "i\u{307}stanbul 中文 100% _").unwrap();
        assert!(excerpt.contains("🙂 İSTANBUL 中文 100% _"));
        assert!(excerpt.starts_with('…') && excerpt.ends_with('…'));
        assert!(excerpt.chars().count() < 400);
        assert_eq!(search_excerpt("ÄÖ 中文", "äö 中文"), Some("ÄÖ 中文".into()));
        assert!(search_excerpt("different", "missing").is_none());
    }
}
