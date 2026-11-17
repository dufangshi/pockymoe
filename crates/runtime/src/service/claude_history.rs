//! Read-only recovery for scheduled Claude turns that never reach the idle ACP stream.
//! Do not submit prompts, pump the SDK scheduler, or manufacture completion while it runs.
use super::*;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::time::{Duration, Instant, SystemTime};

#[derive(Default)]
pub(super) struct HistoryCache(std::sync::Mutex<HashMap<String, CacheEntry>>);
impl HistoryCache {
    pub(super) fn forget(&self, id: &str) {
        self.0.lock().unwrap().remove(id);
    }
}
struct CacheEntry {
    checked: Instant,
    reader: Arc<Mutex<ScheduledReader>>,
}

struct ScheduledTurn {
    id: String,
    prompt: String,
    started: String,
    completed: String,
    items: Vec<ThreadHistoryItemDto>,
    model: Option<String>,
    usage: Option<Value>,
}

struct PendingScheduledTurn {
    id: String,
    prompt: String,
    started: String,
    mapper: crate::acp::TurnMapper,
    model: Option<String>,
    // Claude repeats a message's usage on its thinking/text/tool blocks.
    // Keep the latest snapshot per request, then sum distinct requests once.
    usage: HashMap<String, Value>,
    last_usage: Option<crate::usage::Tokens>,
}

// Only explicit scheduled origins qualify. Keep attribution across polls, including
// tool results and terminal thinking records that precede the final text.
#[derive(Default)]
struct ScheduledReader {
    path: PathBuf,
    session: String,
    since: String,
    stamp: Option<(u64, SystemTime)>,
    identity: Option<FileIdentity>,
    offset: u64,
    partial: Vec<u8>,
    current: Option<PendingScheduledTurn>,
    // Clear only after the import transaction commits (or has no work).
    turns: Vec<ScheduledTurn>,
    #[cfg(test)]
    bytes_read: u64,
}

#[derive(PartialEq, Eq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    created: Option<SystemTime>,
}
impl FileIdentity {
    fn new(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                device: meta.dev(),
                inode: meta.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                created: meta.created().ok(),
            }
        }
    }
}

impl ScheduledReader {
    fn poll(
        &mut self,
        path: PathBuf,
        thread: &str,
        session: &str,
        since: &str,
    ) -> std::io::Result<()> {
        let mut file = std::fs::File::open(&path)?;
        let meta = file.metadata()?;
        let stamp = (meta.len(), meta.modified()?);
        let identity = FileIdentity::new(&meta);
        if self.path != path
            || self.session != session
            || self.since != since
            || self.identity.as_ref() != Some(&identity)
            || meta.len() < self.offset
            || (self
                .stamp
                .is_some_and(|old| old.0 == stamp.0 && old != stamp))
        {
            *self = Self {
                path,
                session: session.into(),
                since: since.into(),
                identity: Some(identity),
                ..Self::default()
            };
        }
        if self.stamp == Some(stamp) {
            return Ok(());
        }
        file.seek(SeekFrom::Start(self.offset))?;
        // Bound this read to the observed length: a busy writer cannot keep the
        // import worker chasing an ever-growing tail.
        let mut reader = BufReader::new(file.take(meta.len() - self.offset));
        loop {
            let count = reader.read_until(b'\n', &mut self.partial)?;
            self.offset += count as u64;
            #[cfg(test)]
            {
                self.bytes_read += count as u64;
            }
            if count == 0 {
                break;
            }
            let complete = self.partial.ends_with(b"\n");
            if let Ok(value) = serde_json::from_slice::<Value>(&self.partial) {
                self.record(value, thread, session, since);
                self.partial.clear();
            } else if complete {
                // Match the old reader's treatment of malformed complete lines.
                self.partial.clear();
            }
        }
        self.stamp = Some(stamp);
        Ok(())
    }

    fn record(&mut self, value: Value, thread: &str, session: &str, since: &str) {
        let current = &mut self.current;
        let turns = &mut self.turns;
        for value in std::iter::once(value) {
            if value["sessionId"] != session || value["isSidechain"] == true {
                continue;
            }
            let at = value["timestamp"].as_str().unwrap_or("");
            let kind = value["type"].as_str().unwrap_or("");
            let content = &value["message"]["content"];
            if kind == "user" && value["turnOrigin"] == "scheduled" && at >= since {
                let Some(uuid) = value["uuid"].as_str().filter(|v| !v.is_empty()) else {
                    continue;
                };
                let prompt = text_content(content);
                let id = format!("{thread}:scheduled:{uuid}");
                *current = Some(PendingScheduledTurn {
                    mapper: crate::acp::TurnMapper::new(id.clone()),
                    id,
                    prompt,
                    started: at.into(),
                    model: None,
                    usage: HashMap::new(),
                    last_usage: None,
                });
                continue;
            }
            let Some(pending) = current.as_mut() else {
                continue;
            };
            if kind == "assistant" {
                if let Some(model) = value["message"]["model"].as_str().filter(|v| !v.is_empty()) {
                    pending.model = Some(model.into());
                }
                if let (Some(message), Some(usage)) = (
                    value["message"]["id"].as_str().filter(|v| !v.is_empty()),
                    crate::usage::Tokens::parse(&value["message"]["usage"]),
                ) {
                    pending.usage.insert(message.into(), json!({"messageId":message,"model":value["message"]["model"],"parent":true,"observedAt":at,"total":usage,"last":usage}));
                    pending.last_usage = Some(usage);
                }
            }
            let blocks = content
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![json!({"type":"text","text":content})]);
            let mut final_text = false;
            for block in blocks {
                let update = match (kind, block["type"].as_str().unwrap_or("")) {
                    ("assistant", "text") => {
                        final_text |= block["text"]
                            .as_str()
                            .is_some_and(|text| !text.trim().is_empty());
                        json!({"sessionUpdate":"agent_message_chunk","messageId":value["message"]["id"],"content":block})
                    }
                    ("assistant", "thinking") => {
                        json!({"sessionUpdate":"agent_thought_chunk","messageId":value["message"]["id"],"content":{"type":"text","text":block["thinking"]}})
                    }
                    ("assistant", "tool_use") => {
                        json!({"sessionUpdate":"tool_call","toolCallId":block["id"],"title":block["name"],"kind":if block["name"]=="Bash" {"execute"} else {"other"},"status":"pending","rawInput":block["input"]})
                    }
                    ("user", "tool_result") => {
                        json!({"sessionUpdate":"tool_call_update","toolCallId":block["tool_use_id"],"status":if block["is_error"]==true {"failed"} else {"completed"},"rawOutput":block["content"]})
                    }
                    // A new ordinary prompt ends attribution even without a prior terminal.
                    ("user", "text") => {
                        *current = None;
                        break;
                    }
                    _ => continue,
                };
                pending
                    .mapper
                    .apply(&json!({"update":update,"createdAt":at}));
            }
            if final_text
                && matches!(
                    value["message"]["stop_reason"].as_str(),
                    Some("end_turn" | "stop_sequence" | "max_tokens")
                )
            {
                if let Some(pending) = current.take() {
                    let usage = pending.last_usage.map(|last| {
                    let total = pending
                        .usage
                        .values().filter_map(|r|crate::usage::Tokens::parse(&r["total"]))
                        .fold(crate::usage::Tokens::default(), |sum, usage| sum.add(&usage));
                    json!({"total":total,"last":last,"cumulative":false,"requests":pending.usage.into_values().collect::<Vec<_>>(),"source":"claudeScheduledImport"})
                });
                    turns.push(ScheduledTurn {
                        id: pending.id,
                        prompt: pending.prompt,
                        started: pending.started,
                        completed: at.into(),
                        items: pending.mapper.finish(false),
                        model: pending.model,
                        usage,
                    });
                }
            }
        }
    }
}

// Frozen whole-file implementation: differential oracle for the incremental reader.
#[cfg(test)]
// Only explicit scheduled origins qualify. Ordinary user/steer/tool-result
// echoes, subagent logs and replayed history must not become duplicate turns.
fn scheduled_turns(raw: &str, thread: &str, session: &str, since: &str) -> Vec<ScheduledTurn> {
    let mut current: Option<PendingScheduledTurn> = None;
    let mut turns = Vec::new();
    for line in raw.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value["sessionId"] != session || value["isSidechain"] == true {
            continue;
        }
        let at = value["timestamp"].as_str().unwrap_or("");
        let kind = value["type"].as_str().unwrap_or("");
        let content = &value["message"]["content"];
        if kind == "user" && value["turnOrigin"] == "scheduled" && at >= since {
            let Some(uuid) = value["uuid"].as_str().filter(|v| !v.is_empty()) else {
                continue;
            };
            let prompt = text_content(content);
            let id = format!("{thread}:scheduled:{uuid}");
            current = Some(PendingScheduledTurn {
                mapper: crate::acp::TurnMapper::new(id.clone()),
                id,
                prompt,
                started: at.into(),
                model: None,
                usage: HashMap::new(),
                last_usage: None,
            });
            continue;
        }
        let Some(pending) = current.as_mut() else {
            continue;
        };
        if kind == "assistant" {
            if let Some(model) = value["message"]["model"].as_str().filter(|v| !v.is_empty()) {
                pending.model = Some(model.into());
            }
            if let (Some(message), Some(usage)) = (
                value["message"]["id"].as_str().filter(|v| !v.is_empty()),
                crate::usage::Tokens::parse(&value["message"]["usage"]),
            ) {
                pending.usage.insert(message.into(), json!({"messageId":message,"model":value["message"]["model"],"parent":true,"observedAt":at,"total":usage,"last":usage}));
                pending.last_usage = Some(usage);
            }
        }
        let blocks = content
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![json!({"type":"text","text":content})]);
        let mut final_text = false;
        for block in blocks {
            let update = match (kind, block["type"].as_str().unwrap_or("")) {
                ("assistant", "text") => {
                    final_text |= block["text"]
                        .as_str()
                        .is_some_and(|text| !text.trim().is_empty());
                    json!({"sessionUpdate":"agent_message_chunk","messageId":value["message"]["id"],"content":block})
                }
                ("assistant", "thinking") => {
                    json!({"sessionUpdate":"agent_thought_chunk","messageId":value["message"]["id"],"content":{"type":"text","text":block["thinking"]}})
                }
                ("assistant", "tool_use") => {
                    json!({"sessionUpdate":"tool_call","toolCallId":block["id"],"title":block["name"],"kind":if block["name"]=="Bash" {"execute"} else {"other"},"status":"pending","rawInput":block["input"]})
                }
                ("user", "tool_result") => {
                    json!({"sessionUpdate":"tool_call_update","toolCallId":block["tool_use_id"],"status":if block["is_error"]==true {"failed"} else {"completed"},"rawOutput":block["content"]})
                }
                // A new ordinary prompt ends attribution even without a prior terminal.
                ("user", "text") => {
                    current = None;
                    break;
                }
                _ => continue,
            };
            pending
                .mapper
                .apply(&json!({"update":update,"createdAt":at}));
        }
        if final_text
            && matches!(
                value["message"]["stop_reason"].as_str(),
                Some("end_turn" | "stop_sequence" | "max_tokens")
            )
        {
            if let Some(pending) = current.take() {
                let usage = pending.last_usage.map(|last| {
                    let total = pending
                        .usage
                        .values().filter_map(|r|crate::usage::Tokens::parse(&r["total"]))
                        .fold(crate::usage::Tokens::default(), |sum, usage| sum.add(&usage));
                    json!({"total":total,"last":last,"cumulative":false,"requests":pending.usage.into_values().collect::<Vec<_>>(),"source":"claudeScheduledImport"})
                });
                turns.push(ScheduledTurn {
                    id: pending.id,
                    prompt: pending.prompt,
                    started: pending.started,
                    completed: at.into(),
                    items: pending.mapper.finish(false),
                    model: pending.model,
                    usage,
                });
            }
        }
    }
    turns
}

fn text_content(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

impl Supervisor {
    // History readers register sessions here. Checking their native files also
    // catches completions while an idle Web page has stopped history polling.
    pub(super) async fn observe_claude_scheduled_history(&self) {
        let ids: Vec<_> = self
            .claude_history
            .0
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for id in ids {
            if let Err(error) = self.sync_claude_scheduled_history(&id).await {
                if error.to_string().contains("thread not found") {
                    self.claude_history.forget(&id);
                } else {
                    tracing::warn!(%id, %error, "Claude scheduled history recovery failed");
                }
            }
        }
    }

    /// Recover finished native scheduler turns before UI/CLI history reads.
    pub async fn sync_claude_scheduled_history(&self, id: &str) -> Result<()> {
        // The observer calls this every two seconds for every thread ever
        // opened. Decide from memory whether it is this thread's turn before
        // reading anything: the checks below read the database.
        if self
            .claude_history
            .0
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|entry| entry.checked.elapsed() < Duration::from_secs(5))
        {
            return Ok(());
        }
        if self.environments.for_thread(id)?.is_some() {
            return Ok(());
        }
        // Never import an in-flight user turn or race its admission/settlement.
        let live = self.live.lock().await;
        if live.contains_key(id) {
            return Ok(());
        }
        drop(live);
        let thread = self.get_thread(id)?;
        if !(thread.provider == Provider::Claude
            || (thread.provider == Provider::Acp && thread.agent_id.as_deref() == Some("claude")))
        {
            return Ok(());
        }
        let Some(session) = thread.provider_session_id.as_deref() else {
            return Ok(());
        };
        let session = parse_session_ref(session).raw_id;
        if Uuid::parse_str(&session).is_err() {
            return Ok(());
        }
        {
            let mut cache = self.claude_history.0.lock().unwrap();
            if cache
                .get(id)
                .is_some_and(|entry| entry.checked.elapsed() < Duration::from_secs(5))
            {
                return Ok(());
            }
            cache
                .entry(id.into())
                .and_modify(|entry| entry.checked = Instant::now())
                .or_insert(CacheEntry {
                    checked: Instant::now(),
                    reader: Arc::new(Mutex::new(ScheduledReader::default())),
                });
        }
        let home = self.local_session_homes.claude_home.join("projects");
        let name = format!("{session}.jsonl");
        let reader = self
            .claude_history
            .0
            .lock()
            .unwrap()
            .get(id)
            .map(|entry| entry.reader.clone());
        let Some(reader) = reader else { return Ok(()) };
        let thread_id = id.to_owned();
        let since = thread.created_at.clone();
        let mut reader = reader.lock_owned().await;
        reader = tokio::task::spawn_blocking(move || {
            let path = Some(reader.path.clone())
                .filter(|path| {
                    path.is_file() && path.file_name().is_some_and(|file| file == name.as_str())
                })
                .or_else(|| {
                    walkdir::WalkDir::new(home)
                        .max_depth(2)
                        .into_iter()
                        .filter_map(|e| e.ok())
                        .find(|e| e.file_type().is_file() && e.file_name() == name.as_str())
                        .map(|e| e.into_path())
                });
            if let Some(path) = path {
                if let Err(error) = reader.poll(path, &thread_id, &session, &since) {
                    // Retry from scratch after I/O failure; never checkpoint a
                    // partially consumed line or lose a completed import.
                    *reader = ScheduledReader::default();
                    tracing::debug!(%error, "Claude scheduled history file read deferred");
                }
            }
            reader
        })
        .await?;
        let turns = &reader.turns;
        if turns.is_empty() {
            return Ok(());
        }
        // File I/O must not block admission of unrelated turns. Recheck ownership
        // and session identity under the admission lock before committing.
        let live = self.live.lock().await;
        if live.contains_key(id)
            || self.get_thread(id)?.provider_session_id != thread.provider_session_id
        {
            return Ok(());
        }
        let mut ran = 0;
        let imported = self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            let mut imported = 0;
            for turn in turns {
                // A timer missed by an older runtime may precede ordinary turns
                // that were already saved. Insert at its native chronological
                // position rather than displaying it as the newest user turn.
                let ordinal: i64 = tx.query_row("SELECT COALESCE(MIN(CASE WHEN started_at>?2 THEN ordinal END),COALESCE(MAX(ordinal),0)+1) FROM thread_turns WHERE thread_id=?1", params![id,turn.started], |r| r.get(0))?;
                let model = turn.model.as_deref().or(thread.model.as_deref());
                let usage = turn.usage.as_ref().map(serde_json::to_string).transpose()?;
                let inserted = tx.execute("INSERT OR IGNORE INTO thread_turns(id,thread_id,status,model,reasoning_effort,display_prompt,started_at,completed_at,ordinal,token_usage_json,origin) VALUES(?1,?2,'completed',?3,?4,?5,?6,?7,?8,?9,'automation')", params![turn.id,id,model,thread.reasoning_effort,turn.prompt,turn.started,turn.completed,ordinal,usage])?;
                if inserted == 0 {
                    // Upgrade already-recovered timers without duplicating their
                    // history or touching active execution/ordinary user turns.
                    if usage.is_some() {
                        imported += tx.execute("UPDATE thread_turns SET token_usage_json=?1,model=?2 WHERE id=?3 AND thread_id=?4 AND token_usage_json IS NULL", params![usage,model,turn.id,id])?;
                    }
                    continue;
                }
                tx.execute("UPDATE thread_turns SET ordinal=ordinal+1 WHERE thread_id=?1 AND id<>?2 AND ordinal>=?3", params![id,turn.id,ordinal])?;
                imported += 1;
                ran += 1;
                let user: ThreadHistoryItemDto = serde_json::from_value(json!({"id":format!("{}:user",turn.id),"kind":"userMessage","text":turn.prompt,"status":"completed","createdAt":turn.started,"sourceTurnId":turn.id,"sequence":0}))?;
                for item in std::iter::once(&user).chain(turn.items.iter()) {
                    tx.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![Uuid::new_v4().to_string(),id,turn.id,item.id,serde_json::to_string(item)?,item.created_at.as_deref().unwrap_or(&turn.started),turn.completed])?;
                }
                tx.execute("UPDATE threads SET updated_at=MAX(updated_at,?1) WHERE id=?2", params![turn.completed,id])?;
            }
            tx.commit()?;
            Ok(imported)
        })?;
        reader.turns.clear();
        drop(reader);
        drop(live);
        if ran > 0 {
            // A scheduled turn is activity, though no Pockymoe prompt started it.
            if let Some(session) = thread.provider_session_id.as_deref() {
                self.runtime(thread.provider)?.touch_session(session).await;
            }
        }
        if imported > 0 {
            self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
                thread_id: id.into(),
                timestamp: now_rfc3339(),
                payload: json!({"reason":"scheduled_history_recovered"}),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(final_reply: bool) -> String {
        let mut rows = vec![
            json!({"type":"user","uuid":"timer-1","turnOrigin":"scheduled","message":{"content":"Timer check"}}),
            json!({"type":"assistant","message":{"id":"progress","content":[{"type":"text","text":"Checking agents."}],"stop_reason":"tool_use"}}),
            json!({"type":"assistant","message":{"id":"tool-request","content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"echo checked"}}],"stop_reason":"tool_use"}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"checked","is_error":false}]}}),
            // Claude writes terminal thinking before its final text. Thinking alone
            // must not prematurely complete the turn and drop the later reply.
            json!({"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Summarize."}],"stop_reason":"end_turn"}}),
        ];
        if final_reply {
            rows.push(json!({"type":"assistant","message":{"id":"final","content":[{"type":"text","text":"All ten agents checked."}],"stop_reason":"end_turn"}}));
        }
        for (n, row) in rows.iter_mut().enumerate() {
            row["sessionId"] = json!("session");
            row["timestamp"] = json!(format!("2030-01-01T00:00:{n:02}Z"));
            if row["type"] == "assistant" {
                row["message"]["model"] = json!("claude-opus-4-6");
                row["message"]["usage"] = json!({"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":20,"cache_creation_input_tokens":30});
            }
        }
        rows.iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn snapshots(turns: &[ScheduledTurn]) -> Value {
        Value::Array(
            turns
                .iter()
                .map(|turn| {
                    let mut usage = turn.usage.clone();
                    if let Some(requests) =
                        usage.as_mut().and_then(|u| u["requests"].as_array_mut())
                    {
                        requests.sort_by_key(|r| r["messageId"].as_str().unwrap().to_owned());
                    }
                    let mut items = serde_json::to_value(&turn.items).unwrap();
                    for item in items.as_array_mut().unwrap() {
                        // TurnMapper supplies wall-clock item creation timestamps on
                        // each replay; compare all stable fields and native turn times.
                        item.as_object_mut().unwrap().remove("createdAt");
                    }
                    json!({"id":turn.id,"prompt":turn.prompt,"started":turn.started,
                "completed":turn.completed,"items":items,"model":turn.model,"usage":usage})
                })
                .collect(),
        )
    }

    #[test]
    fn incremental_reader_reads_only_the_append_after_a_large_prefix() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut file = std::fs::File::create(&path).unwrap();
        let row =
            json!({"sessionId":"session","type":"user","message":{"content":"x".repeat(1024)}})
                .to_string()
                + "\n";
        for _ in 0..8192 {
            file.write_all(row.as_bytes()).unwrap();
        }
        file.flush().unwrap();
        let mut reader = ScheduledReader::default();
        reader
            .poll(path.clone(), "thread", "session", "2026")
            .unwrap();
        let initial = reader.bytes_read;
        let appended = records(true);
        file.write_all(appended.as_bytes()).unwrap();
        file.flush().unwrap();
        reader
            .poll(path.clone(), "thread", "session", "2026")
            .unwrap();
        assert_eq!(reader.bytes_read - initial, appended.len() as u64);
        assert_eq!(reader.turns.len(), 1);
        reader.turns.clear();
        reader.poll(path, "thread", "session", "2026").unwrap();
        assert!(reader.turns.is_empty());
        assert_eq!(reader.bytes_read, initial + appended.len() as u64);
        eprintln!(
            "JSONL read bytes: initial={initial}, append={}, unchanged=0",
            appended.len()
        );
    }

    #[test]
    fn incremental_reader_matches_whole_file_across_turns_partial_lines_and_rotation() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut reader = ScheduledReader::default();
        let first = records(true);
        let second = records(true).replace("timer-1", "timer-2");
        let third = records(true).replace("timer-1", "timer-3");
        let raw = format!("{first}\n{second}\n{third}\n{{\"truncated\":");
        // Split inside a turn, JSON record, and UTF-8 codepoint as well as at EOF.
        let raw = raw.replace("Timer check", "Timer 检查");
        let unicode = raw.find('检').unwrap() + 1;
        let cuts = [
            unicode,
            first.len() / 2,
            first.len() + 8,
            raw.len() - 2,
            raw.len(),
        ];
        let mut cuts = cuts.to_vec();
        cuts.sort_unstable();
        let mut offset = 0;
        let mut file = std::fs::File::create(&path).unwrap();
        for end in cuts {
            file.write_all(&raw.as_bytes()[offset..end]).unwrap();
            file.flush().unwrap();
            reader
                .poll(path.clone(), "thread", "session", "2026")
                .unwrap();
            // The old read_to_string cannot decode a split UTF-8 character;
            // its next successful read must still produce identical results.
            if let Ok(prefix) = std::str::from_utf8(&raw.as_bytes()[..end]) {
                assert_eq!(
                    snapshots(&reader.turns),
                    snapshots(&scheduled_turns(prefix, "thread", "session", "2026"))
                );
            }
            assert_eq!(reader.bytes_read, end as u64, "never reread the prefix");
            offset = end;
        }
        assert_eq!(reader.turns.len(), 3);
        assert_eq!(reader.partial, b"{\"truncated\":");
        // Complete the invalid EOF line, then append a fourth turn.
        let tail = format!("true}}\n{}", records(true).replace("timer-1", "timer-4"));
        file.write_all(tail.as_bytes()).unwrap();
        file.flush().unwrap();
        reader
            .poll(path.clone(), "thread", "session", "2026")
            .unwrap();
        assert_eq!(
            snapshots(&reader.turns),
            snapshots(&scheduled_turns(
                &(raw.clone() + &tail),
                "thread",
                "session",
                "2026"
            ))
        );
        let bytes = reader.bytes_read;
        reader
            .poll(path.clone(), "thread", "session", "2026")
            .unwrap();
        assert_eq!(reader.bytes_read, bytes, "unchanged file reads no bytes");
        drop(file);

        // Shrink on the same inode, then replace with a larger file: length
        // alone is insufficient to recognize the latter.
        for (replace, content) in [
            (false, first.clone()),
            (true, format!("{second}\n{third}\n")),
        ] {
            if replace {
                let replacement = dir.path().join("replacement");
                std::fs::write(&replacement, &content).unwrap();
                std::fs::remove_file(&path).unwrap();
                std::fs::rename(replacement, &path).unwrap();
            } else {
                std::fs::write(&path, &content).unwrap();
            }
            reader
                .poll(path.clone(), "thread", "session", "2026")
                .unwrap();
            assert_eq!(
                snapshots(&reader.turns),
                snapshots(&scheduled_turns(&content, "thread", "session", "2026"))
            );
            assert_eq!(reader.bytes_read, content.len() as u64);
        }
    }

    #[test]
    fn incremental_reader_keeps_pending_attribution_after_completed_turns_are_acknowledged() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let first = records(true);
        let pending = records(false).replace("timer-1", "timer-2");
        std::fs::write(&path, format!("{first}\n{pending}\n")).unwrap();
        let mut reader = ScheduledReader::default();
        reader
            .poll(path.clone(), "thread", "session", "2026")
            .unwrap();
        assert_eq!(reader.turns.len(), 1);
        reader.turns.clear(); // successful transaction
        let full = records(true).replace("timer-1", "timer-2");
        let tail = full
            .strip_prefix(&pending)
            .unwrap()
            .trim_start_matches('\n');
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(tail.as_bytes())
            .unwrap();
        let before = reader.bytes_read;
        reader.poll(path, "thread", "session", "2026").unwrap();
        assert_eq!(reader.bytes_read - before, tail.len() as u64);
        assert_eq!(
            snapshots(&reader.turns),
            snapshots(&scheduled_turns(&full, "thread", "session", "2026"))
        );
    }

    #[test]
    fn scheduled_usage_counts_requests_once_and_keeps_the_latest_snapshot() {
        let raw = records(true);
        let mut rows: Vec<Value> = raw
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let mut duplicate = rows[2].clone();
        duplicate["message"]["content"] =
            json!([{"type":"thinking","thinking":"Tool preparation"}]);
        duplicate["message"]["usage"]["output_tokens"] = json!(1);
        rows.insert(2, duplicate);
        let raw = rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let turns = scheduled_turns(&raw, "thread", "session", "2026");
        assert_eq!(turns[0].model.as_deref(), Some("claude-opus-4-6"));
        let usage = turns[0].usage.as_ref().unwrap();
        assert_eq!(usage["total"]["inputTokens"], 180);
        assert_eq!(usage["total"]["cachedInputTokens"], 60);
        assert_eq!(usage["total"]["cacheWriteInputTokens"], 90);
        assert_eq!(usage["total"]["outputTokens"], 15);
        assert_eq!(usage["total"]["totalTokens"], 195);
        assert_eq!(usage["last"]["totalTokens"], 65);
        assert_eq!(usage["requests"].as_array().unwrap().len(), 3);
        assert_eq!(usage["source"], "claudeScheduledImport");
        // Missing request IDs cannot be charged once per content block.
        let anonymous = raw.replace("\"id\":\"progress\",", "");
        assert_eq!(
            scheduled_turns(&anonymous, "thread", "session", "2026")[0]
                .usage
                .as_ref()
                .unwrap()["total"]["totalTokens"],
            130
        );
    }

    #[test]
    fn scheduled_replies_progress_and_tools_are_recovered_with_stable_ids() {
        let turns = scheduled_turns(&records(true), "thread", "session", "2026");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].id, "thread:scheduled:timer-1");
        assert_eq!(turns[0].prompt, "Timer check");
        assert_eq!(turns[0].completed, "2030-01-01T00:00:05Z");
        assert!(turns[0]
            .items
            .iter()
            .any(|i| i.kind == "agentMessage" && i.text == "Checking agents."));
        assert!(turns[0]
            .items
            .iter()
            .any(|i| i.kind == "agentMessage" && i.text == "All ten agents checked."));
        let tool = turns[0]
            .items
            .iter()
            .find(|i| i.kind == "commandExecution")
            .unwrap();
        assert!(tool.detail_text.as_deref().unwrap().contains("checked"));
        assert_eq!(tool.status.as_deref(), Some("completed"));
        let again = scheduled_turns(&records(true), "thread", "session", "2026");
        assert_eq!(
            turns[0].items.iter().map(|i| &i.id).collect::<Vec<_>>(),
            again[0].items.iter().map(|i| &i.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn unfinished_foreign_sidechain_old_and_ordinary_messages_are_not_imported() {
        assert!(scheduled_turns(&records(false), "t", "session", "2026").is_empty());
        assert!(scheduled_turns(&records(true), "t", "different", "2026").is_empty());
        assert!(scheduled_turns(&records(true), "t", "session", "2031").is_empty());
        assert!(scheduled_turns(
            &records(true).replace("\"scheduled\"", "\"user\""),
            "t",
            "session",
            "2026"
        )
        .is_empty());
        let sidechain = records(true)
            .lines()
            .map(|line| {
                let mut row: Value = serde_json::from_str(line).unwrap();
                row["isSidechain"] = json!(true);
                row.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(scheduled_turns(&sidechain, "t", "session", "2026").is_empty());
    }

    #[test]
    fn ordinary_prompt_cannot_inherit_an_unfinished_timer_and_partial_json_is_safe() {
        let ordinary = json!({"sessionId":"session","timestamp":"2030-01-01T00:01:00Z","type":"user","message":{"content":"New user request"}});
        let final_reply = json!({"sessionId":"session","timestamp":"2030-01-01T00:01:01Z","type":"assistant","message":{"content":[{"type":"text","text":"User reply"}],"stop_reason":"end_turn"}});
        let raw = format!(
            "{}\n{ordinary}\n{final_reply}\n{{\"partial\":",
            records(false)
        );
        assert!(scheduled_turns(&raw, "t", "session", "2026").is_empty());
        assert_eq!(
            scheduled_turns(&(records(true) + "\n{\"partial\":"), "t", "session", "2026").len(),
            1
        );
    }

    #[tokio::test]
    async fn scheduled_history_is_durable_idempotent_and_does_not_change_execution_state() {
        use crate::fake::FakeRuntime;
        let dir = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig::from_env().unwrap();
        config.database_url = dir.path().join("isolated.sqlite");
        config.workspace_root = dir.path().join("workspaces");
        config.fake_runtime = true;
        config.relay_server_url = None;
        config.relay_agent_token = None;
        let homes = LocalSessionHomes {
            claude_home: dir.path().join("claude"),
            codex_home: dir.path().join("codex"),
            grok_home: dir.path().join("grok"),
        };
        let supervisor = Supervisor::new(
            config.clone(),
            Database::open(&dir.path().join("isolated.sqlite")).unwrap(),
            vec![Arc::new(FakeRuntime::new(Provider::Claude))],
        )
        .with_local_session_homes(homes.clone());
        let workspace = supervisor
            .create_workspace(CreateWorkspaceInput {
                abs_path: Some(dir.path().to_string_lossy().into_owned()),
                git_url: None,
                label: Some("test".into()),
            })
            .unwrap();
        let thread = supervisor
            .create_thread(CreateThreadInput {
                workspace_id: workspace.id,
                title: Some("timer".into()),
                provider: Some(Provider::Claude),
                agent_id: None,
                model: "default".into(),
                reasoning_effort: None,
                approval_mode: "yolo".into(),
                parent_thread_id: None,
            })
            .await
            .unwrap();
        let session = "4457ff5e-84aa-4ecd-b698-c0d3098aac34";
        supervisor
            .db
            .with(|conn| {
                conn.execute(
                    "UPDATE threads SET provider_session_id=?1 WHERE id=?2",
                    params![format!("claude::{session}"), thread.id],
                )?;
                Ok(())
            })
            .unwrap();
        let project = homes.claude_home.join("projects/test");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join(format!("{session}.jsonl"));
        std::fs::write(
            &path,
            records(false).replace("\"session\"", &format!("\"{session}\"")),
        )
        .unwrap();
        let first = supervisor
            .get_thread_detail(&thread.id, None)
            .await
            .unwrap();
        assert!(first.turns.is_empty());
        let raw = records(true).replace("\"session\"", &format!("\"{session}\""));
        std::fs::write(&path, &raw).unwrap();
        // The idle page need not issue another read: the observer must persist
        // the completed timer and notify it through the existing Web event.
        let mut events = supervisor.bus.subscribe();
        supervisor
            .claude_history
            .0
            .lock()
            .unwrap()
            .get_mut(&thread.id)
            .unwrap()
            .checked = Instant::now() - Duration::from_secs(6);
        // A failed transaction must retain the completed result even though
        // the file offset has reached EOF. Retrying an unchanged file imports it.
        supervisor.db.with(|conn| {
            conn.execute_batch("CREATE TRIGGER reject_scheduled BEFORE INSERT ON thread_turns BEGIN SELECT RAISE(ABORT, 'injected import failure'); END;")?;
            Ok(())
        }).unwrap();
        assert!(supervisor
            .sync_claude_scheduled_history(&thread.id)
            .await
            .is_err());
        supervisor
            .db
            .with(|conn| {
                conn.execute_batch("DROP TRIGGER reject_scheduled")?;
                Ok(())
            })
            .unwrap();
        supervisor
            .claude_history
            .0
            .lock()
            .unwrap()
            .get_mut(&thread.id)
            .unwrap()
            .checked = Instant::now() - Duration::from_secs(6);
        supervisor.observe_claude_scheduled_history().await;
        let event = events.try_recv().unwrap();
        assert_eq!(event.event_type, "thread.updated");
        assert_eq!(event.payload["reason"], "scheduled_history_recovered");
        for _ in 0..2 {
            supervisor.claude_history.0.lock().unwrap().clear();
            let detail = supervisor
                .get_thread_detail_view(&thread.id, Some(3), true)
                .await
                .unwrap();
            assert_eq!(detail.turns.len(), 1);
            assert_eq!(detail.thread.status, "idle");
            assert_eq!(detail.thread.active_turn_id, None);
            assert_eq!(detail.turns[0].status, "completed");
        }
        assert!(
            events.try_recv().is_err(),
            "duplicates must not notify again"
        );
        let transcript = supervisor
            .transcript(&thread.id, &crate::interaction::TranscriptQuery::default())
            .unwrap();
        assert_eq!(transcript["turns"].as_array().unwrap().len(), 1);
        let detail = supervisor
            .get_thread_turn_detail(&thread.id, &format!("{}:scheduled:timer-1", thread.id))
            .await
            .unwrap();
        assert!(detail
            .items
            .iter()
            .any(|i| i.text == "All ten agents checked."));
        assert_eq!(
            detail.token_usage.as_ref().unwrap()["total"]["totalTokens"],
            195
        );
        assert!(
            detail.price_estimate.as_ref().unwrap()["totalUsd"]
                .as_f64()
                .unwrap()
                > 0.0
        );
        // A runtime upgrade must backfill timers already imported by the old
        // reader, even when the native file has not changed.
        supervisor.db.with(|conn| {
            conn.execute("UPDATE thread_turns SET token_usage_json=NULL,model='default' WHERE thread_id=?1", [&thread.id])?;
            Ok(())
        }).unwrap();
        supervisor.claude_history.0.lock().unwrap().clear();
        let backfilled = supervisor
            .get_thread_detail(&thread.id, None)
            .await
            .unwrap();
        assert_eq!(backfilled.turns.len(), 1);
        assert_eq!(
            backfilled.turns[0].model.as_deref(),
            Some("claude-opus-4-6")
        );
        assert_eq!(backfilled.turns[0].items.len(), detail.items.len());
        assert_eq!(backfilled.turns[0].token_usage, detail.token_usage);
        assert_eq!(backfilled.turns[0].price_estimate, detail.price_estimate);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            raw,
            "native history must remain read-only"
        );
        drop(supervisor);
        let reopened = Supervisor::new(
            config,
            Database::open(&dir.path().join("isolated.sqlite")).unwrap(),
            vec![Arc::new(FakeRuntime::new(Provider::Claude))],
        )
        .with_local_session_homes(homes);
        let detail = reopened.get_thread_detail(&thread.id, None).await.unwrap();
        assert_eq!(
            detail.turns.len(),
            1,
            "restart must preserve history without duplicating it"
        );
        reopened.db.with(|conn| {
            conn.execute("INSERT INTO thread_turns(id,thread_id,status,started_at,ordinal) VALUES('later-user',?1,'completed','2031-01-01T00:00:00Z',2)", [&thread.id])?;
            Ok(())
        }).unwrap();
        let delayed = format!("{raw}\n{}", raw.replace("timer-1", "timer-2"));
        std::fs::write(&path, delayed).unwrap();
        reopened.claude_history.0.lock().unwrap().clear();
        let detail = reopened.get_thread_detail(&thread.id, None).await.unwrap();
        assert_eq!(
            detail
                .turns
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                format!("{}:scheduled:timer-1", thread.id).as_str(),
                format!("{}:scheduled:timer-2", thread.id).as_str(),
                "later-user",
            ],
            "late recovery must not move an older timer after later ordinary turns"
        );
    }
}
