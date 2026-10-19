//! Claude ACP sends context occupancy while streaming, but billable usage often
//! arrives only at prompt completion. Read completed native assistant messages
//! during the turn, including the response which produced the first tool call.
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::usage::Tokens;

/// Read-only compatibility for turns saved before responsePhase existed. Replay
/// each original turn's native window, including launch receipts: end_turn alone
/// is insufficient when Claude is still waiting for a background command.
pub(crate) fn legacy_background_reply_phases(
    path: &std::path::Path,
    session: &str,
    turns: &mut [pockymoe_protocol::ThreadTurnDto],
) -> std::io::Result<()> {
    let mut readers: Vec<_> = turns
        .iter()
        .map(|turn| {
            let start = turn
                .started_at
                .as_deref()
                .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())?
                .with_timezone(&chrono::Utc);
            let end = turn
                .completed_at
                .as_deref()
                .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())?
                .with_timezone(&chrono::Utc);
            let mut reader = ClaudeUsageReader::unopened(PathBuf::new(), session);
            reader.started = start;
            reader.complete_tail = true;
            Some((reader, end))
        })
        .collect();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(at) = entry["timestamp"]
            .as_str()
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        else {
            continue;
        };
        for (reader, end) in readers.iter_mut().flatten() {
            if at <= *end {
                reader.record(&entry);
                // Historical inspection never delivers notices or usage.
                reader.tasks.take_notices();
            }
        }
    }
    for (turn, reader) in turns.iter_mut().zip(readers) {
        if let Some((reader, _)) = reader {
            reader.annotate_background_reply_phases(&mut turn.items, turn.status == "completed");
        }
    }
    Ok(())
}

pub(super) struct ClaudeUsageReader {
    home: PathBuf,
    session: String,
    path: Option<PathBuf>,
    offset: u64,
    started: chrono::DateTime<chrono::Utc>,
    next_lookup: Instant,
    messages: HashMap<String, Value>,
    child_offsets: HashMap<PathBuf, u64>,
    last_parent_usage: Option<Tokens>,
    parent_model: Option<String>,
    completion: super::claude_completion::ClaudeCompletion,
    background: super::claude_background::ClaudeBackgroundAgents,
    complete_tail: bool,
    tasks: super::claude_tasks::ClaudeNativeTasks,
}

impl ClaudeUsageReader {
    pub fn new(session: &str) -> Self {
        Self::with_home(
            crate::local_sessions::LocalSessionHomes::from_env().claude_home,
            session,
        )
    }

    fn with_home(home: PathBuf, session: &str) -> Self {
        let mut reader = Self::unopened(home, session);
        reader.find_path();
        // Exclude old turns without reading their potentially huge tool output.
        if let Some(file) = reader.path.as_ref().and_then(|p| File::open(p).ok()) {
            reader.offset = file.metadata().map(|m| m.len()).unwrap_or(0);
        }
        for path in reader.child_paths() {
            if let Ok(meta) = path.metadata() {
                reader.child_offsets.insert(path, meta.len());
            }
        }
        reader
    }

    fn unopened(home: PathBuf, session: &str) -> Self {
        Self {
            home,
            session: crate::import_id::parse_session_ref(session).raw_id,
            path: None,
            offset: 0,
            started: chrono::Utc::now(),
            next_lookup: Instant::now(),
            messages: HashMap::new(),
            child_offsets: HashMap::new(),
            last_parent_usage: None,
            parent_model: None,
            completion: Default::default(),
            background: Default::default(),
            complete_tail: false,
            tasks: Default::default(),
        }
    }

    fn find_path(&mut self) {
        if self.path.is_some() || Instant::now() < self.next_lookup {
            return;
        }
        self.next_lookup = Instant::now() + Duration::from_secs(1);
        if uuid::Uuid::parse_str(&self.session).is_err() {
            return;
        }
        let name = format!("{}.jsonl", self.session);
        self.path = walkdir::WalkDir::new(self.home.join("projects"))
            .max_depth(2)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .find(|entry| entry.file_type().is_file() && entry.file_name() == name.as_str())
            .map(|entry| entry.into_path());
    }

    pub fn poll(&mut self) -> Vec<Value> {
        self.complete_tail = false;
        self.find_path();
        let Some(mut file) = self.path.as_ref().and_then(|p| File::open(p).ok()) else {
            return Vec::new();
        };
        if file.metadata().is_ok_and(|m| m.len() < self.offset) {
            self.completion.invalidate();
            self.background = Default::default();
            self.offset = 0;
        }
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut reader = BufReader::new(file);
        let mut reports = Vec::new();
        loop {
            let mut line = String::new();
            let Ok(count) = reader.read_line(&mut line) else {
                break;
            };
            // Do not consume a partially written JSONL record.
            if count == 0 {
                self.complete_tail = true;
                break;
            }
            if !line.ends_with('\n') {
                break;
            }
            self.offset += count as u64;
            match serde_json::from_str(&line) {
                Ok(entry) => {
                    if let Some(report) = self.record(&entry) {
                        reports.push(report);
                    }
                }
                Err(_) => self.completion.invalidate(),
            }
        }
        self.poll_children(&mut reports);
        reports
    }

    pub fn poll_final(&mut self) -> Vec<Value> {
        self.next_lookup = Instant::now();
        self.poll()
    }

    pub fn abandoned_tools(
        &self,
        error: &str,
        delivered_text: Option<&str>,
    ) -> Option<Vec<String>> {
        if !self.complete_tail {
            return None;
        }
        self.completion.abandoned_tools(error, delivered_text)
    }

    pub fn background_subagents(&self) -> Vec<pockymoe_protocol::ThreadSubagentDto> {
        self.background.active()
    }

    pub fn background_pending(&self) -> bool {
        self.tasks.pending()
    }

    pub fn waiting_count(&self) -> usize {
        self.tasks.waiting_count()
    }

    pub fn take_task_notices(&mut self) -> Vec<Value> {
        self.tasks.take_notices()
    }

    pub fn annotate_background_reply_phases(
        &self,
        items: &mut [pockymoe_protocol::ThreadHistoryItemDto],
        completed: bool,
    ) {
        if !items.iter().any(|item| {
            matches!(
                item.extra.get("origin").and_then(Value::as_str),
                Some("nativeBackgroundWait" | "nativeTaskNotification")
            )
        }) {
            return;
        }
        let final_index = items.iter().rposition(|item| item.kind == "agentMessage");
        for (index, item) in items
            .iter_mut()
            .enumerate()
            .filter(|(_, item)| item.kind == "agentMessage")
        {
            let confirmed = completed
                && Some(index) == final_index
                && self.complete_tail
                && !self.background_pending()
                && self.background_subagents().is_empty()
                && self.completion.confirms_final_reply(&item.text);
            item.extra.insert(
                "responsePhase".into(),
                json!(if confirmed { "final" } else { "commentary" }),
            );
        }
    }

    pub fn record_sdk(&mut self, message: &Value) {
        self.tasks.record_sdk(message);
    }

    pub fn expect_prompt(&mut self, prompt: &str) {
        self.completion.expect_prompt(prompt);
    }

    pub fn completed_queued_command(&self, delivered_text: Option<&str>) -> Option<&str> {
        if !self.complete_tail || !self.background.active().is_empty() {
            return None;
        }
        self.completion.completed_queued_command(delivered_text)
    }

    fn record(&mut self, entry: &Value) -> Option<Value> {
        let at = entry["timestamp"].as_str()?;
        if entry["sessionId"] != self.session
            || entry["isSidechain"] == true
            || chrono::DateTime::parse_from_rfc3339(at).ok()? < self.started
        {
            return None;
        }
        self.completion.record(entry);
        self.background.record(entry);
        self.tasks.record(entry);
        self.record_usage(entry, true)
    }

    fn record_usage(&mut self, entry: &Value, parent: bool) -> Option<Value> {
        let at = entry["timestamp"].as_str()?;
        if entry["sessionId"] != self.session
            || chrono::DateTime::parse_from_rfc3339(at).ok()? < self.started
        {
            return None;
        }
        let message = &entry["message"];
        if entry["type"] != "assistant" {
            return None;
        }
        let id = message["id"].as_str()?.to_owned();
        let tokens = Tokens::parse(&message["usage"])?;
        // One API response may produce multiple JSONL content records with the
        // same message ID. They are snapshots, never additive usage deltas.
        if self
            .messages
            .get(&id)
            .and_then(|v| Tokens::parse(&v["total"]))
            .is_some_and(|previous| {
                previous == tokens || previous.output_tokens > tokens.output_tokens
            })
        {
            return None;
        }
        if parent {
            self.last_parent_usage = Some(tokens.clone());
            // Claude writes its API errors as replies from a "<synthetic>" model;
            // the turn keeps the model that actually ran.
            self.parent_model = message["model"]
                .as_str()
                .filter(|model| *model != "<synthetic>")
                .map(str::to_owned)
                .or(self.parent_model.take());
        }
        self.messages.insert(
            id.clone(),
            json!({"messageId":id,"total":tokens,"last":tokens,
            "model":message["model"],"parent":parent,"observedAt":at}),
        );
        let total = self
            .messages
            .values()
            .filter_map(|v| Tokens::parse(&v["total"]))
            .fold(Tokens::default(), |sum, tokens| sum.add(&tokens));
        let mut requests: Vec<_> = self.messages.values().cloned().collect();
        requests.sort_by(|a, b| a["messageId"].as_str().cmp(&b["messageId"].as_str()));
        Some(
            json!({"total":total,"last":self.last_parent_usage.clone().unwrap_or_default(),
            "requests":requests,"cumulative":false,"source":"claudeRollout",
            "model":self.parent_model,"observedAt":at}),
        )
    }

    fn child_paths(&self) -> Vec<PathBuf> {
        let Some(parent) = &self.path else {
            return Vec::new();
        };
        std::fs::read_dir(parent.with_extension("").join("subagents"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| {
                        name.strip_prefix("agent-")
                            .and_then(|name| name.strip_suffix(".jsonl"))
                            .is_some_and(|id| {
                                !id.is_empty()
                                    && id.bytes().all(|b| {
                                        b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
                                    })
                            })
                    })
            })
            .collect()
    }

    fn poll_children(&mut self, reports: &mut Vec<Value>) {
        for path in self.child_paths() {
            let mut offset = self.child_offsets.get(&path).copied().unwrap_or(0);
            let Ok(mut file) = File::open(&path) else {
                continue;
            };
            let Ok(meta) = file.metadata() else {
                continue;
            };
            if meta.len() < offset {
                offset = 0;
            }
            if meta.len() == offset || file.seek(SeekFrom::Start(offset)).is_err() {
                continue;
            }
            let agent = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .strip_prefix("agent-")
                .unwrap_or("");
            let mut reader = BufReader::new(file);
            loop {
                let mut line = String::new();
                let Ok(count) = reader.read_line(&mut line) else {
                    break;
                };
                if count == 0 || !line.ends_with('\n') {
                    break;
                }
                offset += count as u64;
                let Ok(entry) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if entry["agentId"].as_str().is_some_and(|id| id != agent) {
                    continue;
                }
                // Child messages contribute billing only. Their end_turn and
                // tools must never finish or wake the foreground turn.
                if let Some(report) = self.record_usage(&entry, false) {
                    reports.push(report);
                }
            }
            self.child_offsets.insert(path, offset);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn native_children_bill_the_parent_once_without_rebilling_history_or_changing_its_lifecycle() {
        let home = tempfile::tempdir().unwrap();
        let session = uuid::Uuid::new_v4().to_string();
        let project = home.path().join("projects/project");
        let children = project.join(&session).join("subagents");
        std::fs::create_dir_all(&children).unwrap();
        let file = project.join(format!("{session}.jsonl"));
        std::fs::write(&file, "").unwrap();
        let record = |id: &str, model: &str, output: u64| {
            json!({"type":"assistant",
            "timestamp":"2099-01-01T00:00:00Z","sessionId":session,"isSidechain":true,"agentId":"child",
            "message":{"id":id,"model":model,"stop_reason":"end_turn","content":[{"type":"text","text":"child done"}],
                "usage":{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":20,
                    "cache_creation":{"ephemeral_1h_input_tokens":20},"output_tokens":output}}})
        };
        let child = children.join("agent-child.jsonl");
        std::fs::write(
            &child,
            format!("{}\n", record("old", "claude-sonnet-4-5", 1000)),
        )
        .unwrap();
        let mut reader = ClaudeUsageReader::with_home(home.path().into(), &session);
        assert!(reader.poll().is_empty());
        let mut parent = record("parent", "claude-opus-5-5", 30);
        parent["isSidechain"] = json!(false);
        parent["message"]["stop_reason"] = json!("tool_use");
        let mut parent_file = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap();
        writeln!(parent_file, "{parent}").unwrap();
        assert_eq!(reader.poll()[0]["total"]["outputTokens"], 30);
        let mut out = std::fs::OpenOptions::new()
            .append(true)
            .open(&child)
            .unwrap();
        writeln!(out, "{}", record("child-answer", "claude-sonnet-4-5", 40)).unwrap();
        let report = reader.poll().pop().unwrap();
        assert_eq!(report["total"]["outputTokens"], 70);
        assert_eq!(report["total"]["inputTokens"], 260);
        assert_eq!(report["total"]["cacheWriteOneHourInputTokens"], 40);
        assert_eq!(report["last"]["outputTokens"], 30);
        assert_eq!(report["model"], "claude-opus-5-5");
        assert_eq!(report["requests"].as_array().unwrap().len(), 2);
        assert!(!reader.completion.confirms_final_reply("child done"));
        writeln!(out, "{}", record("child-answer", "claude-sonnet-4-5", 40)).unwrap();
        assert!(reader.poll().is_empty());
        writeln!(out, "{}", record("child-answer", "claude-sonnet-4-5", 60)).unwrap();
        assert_eq!(reader.poll()[0]["total"]["outputTokens"], 90);
        let mut foreign = record("foreign", "claude-sonnet-4-5", 999);
        foreign["sessionId"] = json!("other");
        writeln!(out, "{foreign}").unwrap();
        foreign["sessionId"] = json!(session);
        foreign["agentId"] = json!("other");
        writeln!(out, "{foreign}").unwrap();
        foreign["agentId"] = json!("child");
        foreign["timestamp"] = json!("2000-01-01T00:00:00Z");
        writeln!(out, "{foreign}").unwrap();
        assert!(reader.poll().is_empty());
        // A newly created child file can contain replayed old records and a
        // partially written current request. Neither is consumed prematurely.
        let new_child = children.join("agent-new.jsonl");
        let mut fresh = record("new-answer", "claude-haiku-4-5", 50);
        fresh["agentId"] = json!("new");
        foreign["agentId"] = json!("new");
        std::fs::write(&new_child, format!("{foreign}\n{fresh}")).unwrap();
        assert!(reader.poll().is_empty());
        let mut out = std::fs::OpenOptions::new()
            .append(true)
            .open(&new_child)
            .unwrap();
        writeln!(out).unwrap();
        assert_eq!(reader.poll_final()[0]["total"]["outputTokens"], 140);
        assert!(reader.poll_final().is_empty());
    }

    #[test]
    fn synthetic_error_reply_keeps_the_model_that_ran() {
        let home = tempfile::tempdir().unwrap();
        let session = uuid::Uuid::new_v4().to_string();
        let project = home.path().join("projects/project");
        std::fs::create_dir_all(&project).unwrap();
        let file = project.join(format!("{session}.jsonl"));
        std::fs::write(&file, "").unwrap();
        let mut reader = ClaudeUsageReader::with_home(home.path().into(), &session);
        assert!(reader.poll().is_empty());
        let record = |id: &str, model: &str, output: u64| {
            json!({"type":"assistant","timestamp":"2099-01-01T00:00:00Z","sessionId":session,
            "message":{"id":id,"model":model,"content":[{"type":"text","text":"reply"}],
                "usage":{"input_tokens":0,"output_tokens":output}}})
        };
        let mut out = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap();
        writeln!(out, "{}", record("ran", "claude-opus-5-5", 30)).unwrap();
        writeln!(out, "{}", record("error", "<synthetic>", 0)).unwrap();
        let report = reader.poll().pop().unwrap();
        assert_eq!(report["model"], "claude-opus-5-5");
    }

    #[test]
    fn background_reply_phase_requires_native_end_turn_and_no_unfinished_work() {
        use pockymoe_protocol::ThreadHistoryItemDto;
        let home = tempfile::tempdir().unwrap();
        let session = uuid::Uuid::new_v4().to_string();
        let mut reader = ClaudeUsageReader::with_home(home.path().into(), &session);
        let entry = |value: Value| {
            let mut value = value;
            value["sessionId"] = json!(session);
            value["timestamp"] = json!("2099-01-01T00:00:00Z");
            value
        };
        let item = |id: &str, kind: &str, text: &str| -> ThreadHistoryItemDto {
            serde_json::from_value(json!({"id":id,"kind":kind,"text":text})).unwrap()
        };
        let mut wake = item("wake", "generic", "Build finished");
        wake.extra
            .insert("origin".into(), json!("nativeTaskNotification"));
        let mut items = vec![
            wake,
            item("progress", "agentMessage", "Waiting on other platforms"),
            item("reply", "agentMessage", "Done"),
        ];
        let text = |text: &str, stop: &str| {
            entry(
                json!({"type":"assistant","message":{"stop_reason":stop,"content":[{"type":"text","text":text}]}}),
            )
        };
        reader.complete_tail = true;
        reader.record(&text("Done", "tool_use"));
        reader.annotate_background_reply_phases(&mut items, true);
        assert_eq!(items[2].extra["responsePhase"], "commentary");
        reader.record(&text("Done", "end_turn"));
        reader.annotate_background_reply_phases(&mut items, true);
        assert_eq!(items[1].extra["responsePhase"], "commentary");
        assert_eq!(items[2].extra["responsePhase"], "final");
        reader.annotate_background_reply_phases(&mut items, false);
        assert_eq!(items[2].extra["responsePhase"], "commentary");
        reader.complete_tail = false;
        reader.annotate_background_reply_phases(&mut items, true);
        assert_eq!(items[2].extra["responsePhase"], "commentary");
        reader.complete_tail = true;
        reader.record(&entry(json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"launch","name":"Bash"}]}})));
        reader.record(&entry(json!({"type":"user","toolUseResult":{"backgroundTaskId":"build"},"message":{"content":[{"type":"tool_result","tool_use_id":"launch"}]}})));
        reader.record(&text("Done", "end_turn"));
        reader.annotate_background_reply_phases(&mut items, true);
        assert_eq!(
            items[2].extra["responsePhase"], "commentary",
            "foreground end_turn while waiting is not the final report"
        );
        reader.record(&entry(json!({"type":"user","origin":{"kind":"task-notification"},"message":{"content":"<task-notification><task-id>build</task-id><status>completed</status></task-notification>"}})));
        reader.record(&text("Done", "end_turn"));
        reader.annotate_background_reply_phases(&mut items, true);
        assert_eq!(items[2].extra["responsePhase"], "final");
        items[2].text = "Unconfirmed text".into();
        reader.annotate_background_reply_phases(&mut items, true);
        assert_eq!(items[2].extra["responsePhase"], "commentary");
    }

    #[test]
    fn background_agents_exclude_history_foreign_sessions_and_sidechains() {
        let home = tempfile::tempdir().unwrap();
        let session = uuid::Uuid::new_v4().to_string();
        let project = home.path().join("projects/project");
        std::fs::create_dir_all(&project).unwrap();
        let file = project.join(format!("{session}.jsonl"));
        let launch = json!({"type":"assistant","timestamp":"2099-01-01T00:00:00Z","sessionId":session,
            "message":{"content":[{"type":"tool_use","id":"tool","name":"Agent","input":{"description":"Review"}}]}});
        let receipt = json!({"type":"user","timestamp":"2099-01-01T00:00:01Z","sessionId":session,
            "toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"agent"},
            "message":{"content":[{"type":"tool_result","tool_use_id":"tool"}]}});
        std::fs::write(&file, format!("{launch}\n{receipt}\n")).unwrap();
        let mut reader = ClaudeUsageReader::with_home(home.path().into(), &session);
        reader.poll();
        assert!(reader.background_subagents().is_empty());
        let mut out = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap();
        for mut entry in [launch.clone(), receipt.clone()] {
            entry["sessionId"] = json!("foreign");
            writeln!(out, "{entry}").unwrap();
            entry["sessionId"] = json!(session);
            entry["isSidechain"] = json!(true);
            writeln!(out, "{entry}").unwrap();
        }
        reader.poll();
        assert!(reader.background_subagents().is_empty());
        writeln!(out, "{launch}\n{receipt}").unwrap();
        reader.poll();
        assert_eq!(reader.background_subagents().len(), 1);
        let mut finished = json!({"type":"user","timestamp":"2099-01-01T00:00:02Z","sessionId":session,
            "origin":{"kind":"task-notification"},"isSidechain":true,
            "message":{"content":"<task-notification><task-id>agent</task-id><status>completed</status></task-notification>"}});
        writeln!(out, "{finished}").unwrap();
        reader.poll();
        assert_eq!(reader.background_subagents().len(), 1);
        finished["isSidechain"] = json!(false);
        writeln!(out, "{finished}").unwrap();
        reader.poll();
        assert!(reader.background_subagents().is_empty());
    }

    #[test]
    fn first_tool_response_is_visible_before_turn_completion_without_rebilling_history_or_blocks() {
        let home = tempfile::tempdir().unwrap();
        let session = uuid::Uuid::new_v4().to_string();
        let project = home.path().join("projects/project");
        std::fs::create_dir_all(&project).unwrap();
        let file = project.join(format!("{session}.jsonl"));
        let record = |id: &str, output: u64| {
            json!({"type":"assistant","sessionId":session,"timestamp":"2099-01-01T00:00:00Z",
            "message":{"id":id,"model":"claude-sonnet-4-5","content":[{"type":"tool_use","name":"Bash"}],
                "usage":{"input_tokens":100,"output_tokens":output,"cache_read_input_tokens":200,"cache_creation_input_tokens":50,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":50}}}})
        };
        std::fs::write(&file, format!("{}\n", record("old", 900))).unwrap();
        let mut reader = ClaudeUsageReader::with_home(home.path().into(), &session);
        assert!(reader.poll().is_empty());
        let mut out = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap();
        writeln!(out, "{}", record("first", 40)).unwrap();
        let reports = reader.poll();
        assert_eq!(reports[0]["total"]["outputTokens"], 40);
        assert_eq!(reports[0]["total"]["inputTokens"], 350);
        assert_eq!(reports[0]["total"]["cacheWriteInputTokens"], 50);
        assert_eq!(reports[0]["total"]["cacheWriteOneHourInputTokens"], 50);
        writeln!(out, "{}", record("first", 40)).unwrap();
        assert!(reader.poll().is_empty());
        writeln!(out, "{}", record("first", 60)).unwrap();
        assert_eq!(reader.poll()[0]["total"]["outputTokens"], 60);
        let mut unrelated = record("nested", 2000);
        unrelated["isSidechain"] = json!(true);
        writeln!(out, "{unrelated}").unwrap();
        unrelated["isSidechain"] = json!(false);
        unrelated["sessionId"] = json!("other");
        writeln!(out, "{unrelated}").unwrap();
        assert!(reader.poll().is_empty());
        write!(out, "{}", record("second", 140)).unwrap();
        assert!(reader.poll().is_empty());
        writeln!(out).unwrap();
        assert_eq!(reader.poll_final()[0]["total"]["outputTokens"], 200);
    }

    #[test]
    fn short_first_turn_discovers_its_new_file_on_final_poll() {
        let home = tempfile::tempdir().unwrap();
        let session = uuid::Uuid::new_v4().to_string();
        let mut reader = ClaudeUsageReader::with_home(home.path().into(), &session);
        let project = home.path().join("projects/project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join(format!("{session}.jsonl")),
            format!(
                "{}\n",
                json!({"type":"assistant","sessionId":session,"timestamp":"2099-01-01T00:00:00Z",
            "message":{"id":"m","usage":{"input_tokens":1,"output_tokens":2}}})
            ),
        )
        .unwrap();
        assert_eq!(reader.poll_final()[0]["total"]["outputTokens"], 2);
    }

    #[test]
    fn steered_streamed_bash_is_interrupted_without_failing_the_completed_reply() {
        use super::super::mapper::TurnMapper;
        let home = tempfile::tempdir().unwrap();
        let session = uuid::Uuid::new_v4().to_string();
        let project = home.path().join("projects/project");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join(format!("{session}.jsonl"));
        // Old history must not supply steering or completion evidence.
        let entry = |value: Value| {
            let mut value = value;
            value["sessionId"] = json!(session);
            value["timestamp"] = json!("2099-01-01T00:00:00Z");
            value
        };
        let initial = entry(json!({"type":"user","uuid":"initial","message":{"content":"start"}}));
        let steer =
            entry(json!({"type":"user","uuid":"steer","message":{"content":"change direction"}}));
        let final_reply = entry(
            json!({"type":"assistant","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"done"}]}}),
        );
        std::fs::write(&path, format!("{initial}\n{steer}\n{final_reply}\n")).unwrap();
        let mut reader = ClaudeUsageReader::with_home(home.path().into(), &session);
        let error = json!({"code":-32603,"data":{"errorKind":"incomplete_tool_call"},
            "message":"Internal error: Claude ended the turn without returning results for tool calls: toolu_orphan"}).to_string();
        reader.poll_final();
        assert!(reader.abandoned_tools(&error, Some("done")).is_none());

        let mut out = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(out, "{initial}").unwrap();
        // Matches the incident: a complete-looking streamed input was never
        // committed as a native tool_use before the next user prompt.
        let mut mapper = TurnMapper::new("turn");
        mapper.apply(
            &json!({"sessionUpdate":"tool_call","toolCallId":"toolu_orphan",
            "kind":"execute","rawInput":{"command":"do-something-with-side-effects"}}),
        );
        mapper.apply(
            &json!({"sessionUpdate":"tool_call","toolCallId":"toolu_done",
            "kind":"execute","rawInput":{"command":"read-results"}}),
        );
        mapper.apply(&json!({"sessionUpdate":"tool_call_update","toolCallId":"toolu_done","status":"completed"}));
        writeln!(out, "{steer}").unwrap();
        reader.poll();
        assert!(reader.abandoned_tools(&error, Some("done")).is_none());
        writeln!(out, "{final_reply}").unwrap();
        reader.poll_final();
        // The late adapter failure is drained before reconciliation.
        mapper.apply(
            &json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}),
        );
        mapper.apply(&json!({"sessionUpdate":"tool_call_update","toolCallId":"toolu_orphan","status":"failed",
            "content":[{"type":"content","content":{"type":"text","text":"Claude ended without a result"}}]}));
        let ids = reader
            .abandoned_tools(&error, mapper.final_agent_text())
            .unwrap();
        mapper.reconcile_abandoned_tools(&ids);
        let items = mapper.finish(false);
        let orphan = items.iter().find(|item| item.id == "toolu_orphan").unwrap();
        assert_eq!(orphan.status.as_deref(), Some("interrupted"));
        assert!(orphan
            .detail_text
            .as_deref()
            .unwrap()
            .contains("no execution recorded"));
        assert!(!orphan
            .detail_text
            .as_deref()
            .unwrap()
            .contains("ended without a result"));
        assert!(items.iter().any(|item| item.kind == "agentMessage"
            && item.text == "done"
            && item.status.as_deref() == Some("completed")));
        assert_eq!(
            items
                .iter()
                .find(|item| item.id == "toolu_done")
                .unwrap()
                .status
                .as_deref(),
            Some("completed")
        );
        // A partial/corrupt tail must not make absence of a committed ID proof.
        write!(out, "{{").unwrap();
        reader.poll_final();
        assert!(reader.abandoned_tools(&error, Some("done")).is_none());
        writeln!(out).unwrap();
        reader.poll_final();
        assert!(reader.abandoned_tools(&error, Some("done")).is_none());
    }
}
