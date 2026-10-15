use pockymoe_protocol::{ThreadHistoryItemDto, ThreadTurnDto};

/// Older ACP records retained before/after text but no file metadata. Derive the
/// presentation on read, without rewriting the user's saved transcript.
pub fn normalize_legacy_file_change(item: &mut ThreadHistoryItemDto) {
    if item.kind != "fileChange" {
        return;
    }
    let Some(detail) = item.detail_text.as_deref() else {
        return;
    };
    let Some((_, files)) = detail.split_once("Result:\nFile: ") else {
        return;
    };
    let mut paths = Vec::new();
    let mut patches = Vec::new();
    let mut added = 0usize;
    let mut removed = 0usize;
    for file in files.split("\n\nFile: ") {
        let Some((path, texts)) = file.split_once("\n\nBefore:\n") else {
            return;
        };
        let Some((old, new)) = texts.split_once("\n\nAfter:\n") else {
            return;
        };
        let old = if old == "(new file)" { "" } else { old };
        let diff = similar::TextDiff::from_lines(old, new);
        for change in diff.iter_all_changes() {
            match change.tag() {
                similar::ChangeTag::Insert => added += 1,
                similar::ChangeTag::Delete => removed += 1,
                _ => {}
            }
        }
        patches.push(
            diff.unified_diff()
                .context_radius(3)
                .header(&format!("a/{path}"), &format!("b/{path}"))
                .to_string(),
        );
        paths.push(path.to_string());
    }
    item.text = paths.join(", ");
    item.preview_text = Some(item.text.clone());
    item.detail_text = Some(patches.join("\n"));
    item.extra.insert("changedFiles".into(), paths.len().into());
    item.extra.insert("addedLines".into(), added.into());
    item.extra.insert("removedLines".into(), removed.into());
}

/// Tool kinds whose `detailText` (output, file contents, results) is served
/// through `/items/{id}/detail` in compact history responses.
pub const TOOL_ITEM_KINDS: &[&str] = &[
    "commandExecution",
    "fileChange",
    "fileRead",
    "webSearch",
    "toolCall",
    "agentToolCall",
    "skillToolCall",
];

/// Kinds whose readers all treat a missing `previewText` as `text`. fileChange
/// derives its summary badges from `previewText`, so it keeps the field.
const PREVIEW_FALLBACK_KINDS: &[&str] = &[
    "commandExecution",
    "fileRead",
    "webSearch",
    "toolCall",
    "agentToolCall",
    "skillToolCall",
];

/// Compact item encoding, opted into by clients that send `compact=1`:
/// every tool kind defers `detailText`, `previewText` equal to `text` and
/// `sourceTurnId` equal to the parent turn are omitted. Older clients and
/// the WebSocket stream keep the legacy encoding of `defer_tool_details`.
pub fn compact_turn(turn: &mut ThreadTurnDto) {
    for item in &mut turn.items {
        if TOOL_ITEM_KINDS.contains(&item.kind.as_str())
            && item.detail_text.take().is_some_and(|text| !text.is_empty())
        {
            item.extra.insert("hasDeferredDetail".into(), true.into());
        }
        if PREVIEW_FALLBACK_KINDS.contains(&item.kind.as_str())
            && item.preview_text.as_deref() == Some(item.text.as_str())
        {
            item.preview_text = None;
        }
        if item.source_turn_id.as_deref() == Some(turn.id.as_str()) {
            item.source_turn_id = None;
        }
    }
}

/// Apply only at the transport boundary, after the complete item is durable.
pub fn defer_tool_details(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            if matches!(
                fields.get("kind").and_then(serde_json::Value::as_str),
                Some("commandExecution" | "fileChange")
            ) {
                if fields
                    .remove("detailText")
                    .is_some_and(|text| !text.is_null())
                {
                    fields.insert("hasDeferredDetail".into(), true.into());
                }
            }
            for value in fields.values_mut() {
                defer_tool_details(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                defer_tool_details(value);
            }
        }
        _ => {}
    }
}

/// One stored item, as much of it as a folded turn's activity needs.
pub struct ActivityRow {
    pub item_id: String,
    pub kind: Option<String>,
    pub status: String,
    /// A conversation item the transcript keeps in a summary: a prompt, a reply
    /// with text, or a background wait or wake.
    pub anchor: bool,
    /// The `text` of a file change: its paths, comma separated.
    pub paths: Option<String>,
    pub changed_files: i64,
    pub added: i64,
    pub removed: i64,
    /// Stored RFC 3339 times; a stretch reports its first start and last update.
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

const ACTIVITY_FILES: usize = 40;

fn activity_key(kind: Option<&str>) -> Option<&'static str> {
    Some(match kind? {
        "commandExecution" => "commands",
        "fileChange" => "fileChanges",
        "fileRead" => "reads",
        "webSearch" => "searches",
        "toolCall" | "skillToolCall" => "tools",
        "agentToolCall" => "agents",
        "reasoning" => "reasoning",
        _ => return None,
    })
}

fn tally(counts: &mut serde_json::Map<String, serde_json::Value>, key: &str, value: i64) {
    if value > 0 {
        let current = counts
            .get(key)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        counts.insert(key.into(), (current + value).into());
    }
}

fn tally_row(
    counts: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    row: &ActivityRow,
) {
    tally(counts, key, 1);
    tally(
        counts,
        "failed",
        i64::from(matches!(
            row.status.as_str(),
            "failed" | "error" | "declined"
        )),
    );
    if key == "fileChanges" {
        tally(counts, "changedFiles", row.changed_files);
        tally(counts, "addedLines", row.added);
        tally(counts, "removedLines", row.removed);
    }
}

/// What a folded turn did, from its stored items in order: counts by kind, the
/// number of distinct changed files and up to forty of them with the latest edit
/// to open, and `segments`, the same
/// counts for each stretch of operations, keyed by the conversation item that
/// follows it (`before`, null at the end), so narration can sit between them.
pub fn turn_activity(rows: &[ActivityRow]) -> serde_json::Map<String, serde_json::Value> {
    let mut activity = serde_json::Map::new();
    let mut files: Vec<(String, String, i64, i64)> = Vec::new();
    let mut segments = Vec::new();
    let mut segment = serde_json::Map::new();
    for row in rows {
        if row.anchor || matches!(row.kind.as_deref(), Some("userMessage" | "agentMessage")) {
            if row.anchor && !segment.is_empty() {
                segment.insert("before".into(), row.item_id.clone().into());
                segments.push(serde_json::Value::Object(std::mem::take(&mut segment)));
            }
            continue;
        }
        let Some(key) = activity_key(row.kind.as_deref()) else {
            continue;
        };
        tally_row(&mut activity, key, row);
        tally_row(&mut segment, key, row);
        if let Some(started) = &row.started_at {
            if !segment.contains_key("startedAt") {
                segment.insert("startedAt".into(), started.clone().into());
            }
        }
        if let Some(ended) = row.ended_at.as_ref().or(row.started_at.as_ref()) {
            let later = segment
                .get("endedAt")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|current| ended.as_str() > current);
            if later {
                segment.insert("endedAt".into(), ended.clone().into());
            }
        }
        if key != "fileChanges" || matches!(row.status.as_str(), "failed" | "error" | "declined") {
            continue;
        }
        // One patch can list several files; its line counts stay with the first.
        let mut counts = Some((row.added, row.removed));
        for path in row
            .paths
            .as_deref()
            .unwrap_or_default()
            .split(", ")
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let (added, removed) = counts.take().unwrap_or((0, 0));
            match files.iter_mut().find(|file| file.0 == path) {
                Some(file) => {
                    file.1 = row.item_id.clone();
                    file.2 += added;
                    file.3 += removed;
                }
                None => files.push((path.to_string(), row.item_id.clone(), added, removed)),
            }
        }
    }
    if !segment.is_empty() {
        segment.insert("before".into(), serde_json::Value::Null);
        segments.push(serde_json::Value::Object(segment));
    }
    if !files.is_empty() {
        activity.insert("fileCount".into(), files.len().into());
        activity.insert(
            "files".into(),
            files
                .into_iter()
                .take(ACTIVITY_FILES)
                .map(|(path, item_id, added, removed)| {
                    serde_json::json!({ "path": path, "itemId": item_id, "addedLines": added, "removedLines": removed })
                })
                .collect(),
        );
    }
    if !segments.is_empty() {
        activity.insert("segments".into(), segments.into());
    }
    activity
}

/// Keep aligned with thread-ui's answerSections.ts. Length is in Unicode code
/// points, so Chinese and emoji do not accidentally use UTF-8 byte thresholds.
fn is_substantive_answer(item: &ThreadHistoryItemDto) -> bool {
    if item.kind != "agentMessage" {
        return false;
    }
    if item
        .extra
        .get("responsePhase")
        .and_then(serde_json::Value::as_str)
        == Some("final")
    {
        return true;
    }
    let text = item.text.trim();
    let length = text.chars().count();
    if length >= 600 {
        return true;
    }
    if length < 300 {
        return false;
    }
    let heading = text.lines().any(|line| {
        let hashes = line.chars().take_while(|c| *c == '#').count();
        (1..=6).contains(&hashes) && line[hashes..].starts_with([' ', '\t'])
    });
    let paragraphs = text
        .lines()
        .fold((0, false), |(count, in_paragraph), line| {
            if line.trim().is_empty() {
                (count, false)
            } else {
                (count + usize::from(!in_paragraph), true)
            }
        })
        .0;
    let table_lines = text
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.starts_with('|') && line.ends_with('|') && line.len() > 1
        })
        .count();
    (heading && paragraphs >= 3) || table_lines >= 3
}

pub fn summarize_completed_turn(mut turn: ThreadTurnDto, keep_narration: bool) -> ThreadTurnDto {
    if turn.status == "inProgress" {
        return turn;
    }

    let mut final_agent_index = None;
    for (index, item) in turn.items.iter().enumerate().rev() {
        if item.kind == "agentMessage" && !item.text.trim().is_empty() {
            final_agent_index = Some(index);
            break;
        }
    }

    let background_anchor = |item: &ThreadHistoryItemDto| {
        matches!(
            item.extra.get("origin").and_then(serde_json::Value::as_str),
            Some("nativeBackgroundWait" | "nativeTaskNotification")
        )
    };
    let has_background = turn.items.iter().any(background_anchor);
    let items: Vec<ThreadHistoryItemDto> = turn
        .items
        .iter()
        .enumerate()
        .filter(|(index, item)| {
            item.kind == "userMessage"
                || Some(*index) == final_agent_index
                || background_anchor(item)
                || ((has_background || keep_narration)
                    && item.kind == "agentMessage"
                    && !item.text.trim().is_empty())
                || is_substantive_answer(item)
        })
        .map(|(_, item)| item.clone())
        .collect();
    let deferred = turn.items.len().saturating_sub(items.len());
    if deferred == 0 {
        return turn;
    }

    turn.items = items;
    turn.has_deferred_items = Some(true);
    turn.deferred_item_count = Some(deferred as u32);
    turn
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn turn(items: serde_json::Value) -> ThreadTurnDto {
        serde_json::from_value(json!({
            "id": "turn-1",
            "startedAt": null,
            "status": "completed",
            "error": null,
            "items": items,
        }))
        .unwrap()
    }

    #[test]
    fn summary_preserves_substantive_answers_and_defers_operations_without_background_events() {
        let long = "中文".repeat(300);
        let structured = format!(
            "## Explanation\n\n{}\n\n{}",
            "a".repeat(150),
            "b".repeat(150)
        );
        let summary = summarize_completed_turn(
            turn(json!([
                {"id":"prompt","kind":"userMessage","text":"Explain and implement"},
                {"id":"before","kind":"commandExecution","text":"before"},
                {"id":"answer","kind":"agentMessage","text":long},
                {"id":"structured","kind":"agentMessage","text":structured},
                {"id":"short","kind":"agentMessage","text":"中".repeat(299)},
                {"id":"after","kind":"commandExecution","text":"after"},
                {"id":"final","kind":"agentMessage","text":"Done","responsePhase":"final"},
                {"id":"checkpoint","kind":"agentMessage","text":"Checking the next result"}
            ])),
            false,
        );
        assert_eq!(
            summary
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["prompt", "answer", "structured", "final", "checkpoint"]
        );
        assert_eq!(summary.deferred_item_count, Some(3));
        assert_eq!(summary.has_deferred_items, Some(true));
        let emoji: ThreadHistoryItemDto = serde_json::from_value(
            json!({"id":"emoji","kind":"agentMessage","text":"😀".repeat(300)}),
        )
        .unwrap();
        assert!(!is_substantive_answer(&emoji));
    }

    #[test]
    fn turn_activity_counts_stretches_between_conversation_items() {
        let row = |id: &str, kind: &str, status: &str, anchor: bool| ActivityRow {
            item_id: id.into(),
            kind: Some(kind.into()),
            status: status.into(),
            anchor,
            paths: None,
            changed_files: 0,
            added: 0,
            removed: 0,
            started_at: None,
            ended_at: None,
        };
        let edit = ActivityRow {
            paths: Some("src/a.rs, src/b.rs".into()),
            changed_files: 2,
            added: 5,
            removed: 1,
            ..row("edit", "fileChange", "completed", false)
        };
        let rows = vec![
            row("prompt", "userMessage", "", true),
            ActivityRow {
                started_at: Some("2026-10-10T10:00:00Z".into()),
                ended_at: Some("2026-10-10T10:00:09Z".into()),
                ..row("c1", "commandExecution", "completed", false)
            },
            row("say", "agentMessage", "completed", true),
            edit,
            row("c2", "commandExecution", "failed", false),
            // A reply without text is not shown, so it does not split a stretch.
            row("empty", "agentMessage", "completed", false),
            row("final", "agentMessage", "completed", true),
            row("c3", "commandExecution", "completed", false),
        ];
        assert_eq!(
            serde_json::Value::Object(turn_activity(&rows)),
            json!({
                "commands": 3, "failed": 1, "fileChanges": 1, "changedFiles": 2, "addedLines": 5, "removedLines": 1,
                "fileCount": 2,
                "files": [
                    {"path": "src/a.rs", "itemId": "edit", "addedLines": 5, "removedLines": 1},
                    {"path": "src/b.rs", "itemId": "edit", "addedLines": 0, "removedLines": 0}
                ],
                "segments": [
                    {"before": "say", "commands": 1, "startedAt": "2026-10-10T10:00:00Z", "endedAt": "2026-10-10T10:00:09Z"},
                    {"before": "final", "commands": 1, "failed": 1, "fileChanges": 1, "changedFiles": 2, "addedLines": 5, "removedLines": 1},
                    {"before": null, "commands": 1}
                ]
            })
        );
        assert!(turn_activity(&[row("prompt", "userMessage", "", true)]).is_empty());
    }

    #[test]
    fn narration_keeps_every_reply_with_text_in_a_summary() {
        let items = json!([
            {"id":"prompt","kind":"userMessage","text":"Go"},
            {"id":"say","kind":"agentMessage","text":"Reading the queue"},
            {"id":"empty","kind":"agentMessage","text":" "},
            {"id":"cmd","kind":"commandExecution","text":"ls"},
            {"id":"final","kind":"agentMessage","text":"Done"}
        ]);
        let ids = |turn: ThreadTurnDto| {
            turn.items
                .into_iter()
                .map(|item| item.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(summarize_completed_turn(turn(items.clone()), false)),
            ["prompt", "final"]
        );
        assert_eq!(
            ids(summarize_completed_turn(turn(items), true)),
            ["prompt", "say", "final"]
        );
    }

    #[test]
    fn compact_turn_defers_detail_for_every_tool_kind() {
        let mut items = TOOL_ITEM_KINDS
            .iter()
            .map(|kind| json!({"id": kind, "kind": kind, "text": "t", "detailText": "full output"}))
            .collect::<Vec<_>>();
        items.push(json!({"id":"empty","kind":"fileRead","text":"t","detailText":""}));
        for kind in [
            "reasoning",
            "image",
            "contextCompaction",
            "agentMessage",
            "hook",
        ] {
            items.push(json!({"id": kind, "kind": kind, "text": "t", "detailText": "kept"}));
        }
        let mut turn = turn(json!(items));
        compact_turn(&mut turn);
        let value = serde_json::to_value(&turn).unwrap();
        for item in value["items"].as_array().unwrap() {
            let kind = item["kind"].as_str().unwrap();
            if item["id"] == "empty" {
                assert!(
                    item.get("detailText").is_none() && item.get("hasDeferredDetail").is_none()
                );
            } else if TOOL_ITEM_KINDS.contains(&kind) {
                assert!(item.get("detailText").is_none(), "{item}");
                assert_eq!(item["hasDeferredDetail"], true, "{item}");
            } else {
                assert_eq!(item["detailText"], "kept", "{item}");
                assert!(item.get("hasDeferredDetail").is_none(), "{item}");
            }
        }
    }

    #[test]
    fn compact_turn_drops_redundant_preview_and_source_turn() {
        let mut turn = turn(json!([
            {"id":"a","kind":"commandExecution","text":"ls","previewText":"ls","sourceTurnId":"turn-1"},
            {"id":"b","kind":"commandExecution","text":"ls -la","previewText":"ls","sourceTurnId":"turn-0"},
            {"id":"c","kind":"fileChange","text":"a.rs","previewText":"a.rs","sourceTurnId":"turn-1"},
            {"id":"d","kind":"artifact","text":"x","previewText":"x"},
        ]));
        compact_turn(&mut turn);
        let value = serde_json::to_value(&turn).unwrap();
        let items = value["items"].as_array().unwrap();
        assert!(items[0].get("previewText").is_none());
        assert!(items[0].get("sourceTurnId").is_none());
        assert_eq!(items[1]["previewText"], "ls");
        assert_eq!(items[1]["sourceTurnId"], "turn-0");
        // fileChange badges and artifact titles read previewText directly.
        assert_eq!(items[2]["previewText"], "a.rs");
        assert!(items[2].get("sourceTurnId").is_none());
        assert_eq!(items[3]["previewText"], "x");
    }

    #[test]
    fn legacy_deferral_keeps_other_tool_details() {
        let mut value = json!({"turns":[{"items":[
            {"id":"a","kind":"commandExecution","text":"ls","detailText":"out"},
            {"id":"b","kind":"fileRead","text":"a.rs","detailText":"contents"},
        ]}]});
        defer_tool_details(&mut value);
        assert!(value["turns"][0]["items"][0].get("detailText").is_none());
        assert_eq!(value["turns"][0]["items"][0]["hasDeferredDetail"], true);
        assert_eq!(value["turns"][0]["items"][1]["detailText"], "contents");
    }
}
