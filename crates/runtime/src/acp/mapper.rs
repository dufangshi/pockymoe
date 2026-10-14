use std::collections::{HashMap, HashSet};

use pockymoe_protocol::{now_rfc3339, ThreadHistoryItemDto, ThreadItemKind, ThreadSubagentDto};
use serde_json::Value;

use crate::actor::GoalState;

#[derive(Debug, Default)]
pub struct MappedUpdate {
    pub deltas: Vec<(String, String, i64)>,
    pub items: Vec<ThreadHistoryItemDto>,
    pub title: Option<String>,
    pub usage: Option<Value>,
    pub goal: Option<Option<GoalState>>,
    pub plan: Option<Vec<(String, String)>>,
    pub active_subagents: Option<Vec<ThreadSubagentDto>>,
}

pub struct TurnMapper {
    turn_id: String,
    agent_segments: Vec<ThreadHistoryItemDto>,
    thought_segments: Vec<ThreadHistoryItemDto>,
    active_agent_segment: Option<usize>,
    active_thought_segment: Option<usize>,
    agent_message_id: Option<String>,
    agent_message_segments: HashMap<String, usize>,
    thought_message_id: Option<String>,
    tools: Vec<ThreadHistoryItemDto>,
    tool_payloads: HashMap<String, Value>,
    plans: Vec<ThreadHistoryItemDto>,
    compactions: Vec<ThreadHistoryItemDto>,
    native_notices: Vec<ThreadHistoryItemDto>,
    active_wait: Option<usize>,
    seen_notices: HashSet<String>,
    subagents: HashMap<String, ThreadSubagentDto>,
    stream: Option<LiveStream>,
    agent_reconcile: Option<Reconcile>,
    thought_reconcile: Option<Reconcile>,
    seq: i64,
}

/// Display-only text of one live model attempt (DSH streams it beside ACP,
/// which sends only committed messages). Committed chunks confirm or replace it.
struct LiveStream {
    attempt_id: String,
    agent: Option<usize>,
    thought: Option<usize>,
    reconciled: bool,
}

struct Reconcile {
    index: usize,
    /// Provisional text not yet matched by committed chunks.
    remaining: String,
    started: bool,
}

impl TurnMapper {
    pub fn new(turn_id: impl Into<String>) -> Self {
        Self {
            turn_id: turn_id.into(),
            agent_segments: Vec::new(),
            thought_segments: Vec::new(),
            active_agent_segment: None,
            active_thought_segment: None,
            agent_message_id: None,
            agent_message_segments: HashMap::new(),
            thought_message_id: None,
            tools: Vec::new(),
            tool_payloads: HashMap::new(),
            plans: Vec::new(),
            compactions: Vec::new(),
            native_notices: Vec::new(),
            active_wait: None,
            seen_notices: HashSet::new(),
            subagents: HashMap::new(),
            stream: None,
            agent_reconcile: None,
            thought_reconcile: None,
            seq: 0,
        }
    }

    pub fn stream_start(&mut self, attempt_id: &str) {
        self.stream = Some(LiveStream {
            attempt_id: attempt_id.into(),
            agent: None,
            thought: None,
            reconciled: false,
        });
    }

    pub fn stream_text(&mut self, attempt_id: &str, kind: &str, text: &str) -> MappedUpdate {
        let mut mapped = MappedUpdate::default();
        let Some(stream) = self
            .stream
            .as_ref()
            .filter(|stream| stream.attempt_id == attempt_id && !stream.reconciled)
        else {
            return mapped;
        };
        let (agent, thought) = (stream.agent, stream.thought);
        if kind == "text" {
            let index = match agent {
                Some(index) => index,
                None => {
                    let index = self.push_segment(ThreadItemKind::AgentMessage);
                    self.active_thought_segment = None;
                    self.stream.as_mut().unwrap().agent = Some(index);
                    index
                }
            };
            let segment = &mut self.agent_segments[index];
            segment.text.push_str(text);
            mapped.deltas.push((
                segment.id.clone(),
                text.to_string(),
                segment.sequence.unwrap_or_default(),
            ));
        } else if kind == "reasoning" {
            let index = match thought {
                Some(index) => index,
                None => {
                    let index = self.push_segment(ThreadItemKind::Reasoning);
                    self.stream.as_mut().unwrap().thought = Some(index);
                    index
                }
            };
            let segment = &mut self.thought_segments[index];
            segment.text.push_str(text);
            mapped.items.push(segment.clone());
        }
        mapped
    }

    /// A committed attempt waits for its ACP chunks; a discarded one keeps its
    /// partial text as failed output, and a retry streams into new segments.
    pub fn stream_end(&mut self, attempt_id: &str, committed: bool) -> MappedUpdate {
        let mut mapped = MappedUpdate::default();
        if self
            .stream
            .as_ref()
            .map(|stream| stream.attempt_id.as_str())
            != Some(attempt_id)
        {
            return mapped;
        }
        let stream = self.stream.take().unwrap();
        if stream.reconciled {
            return mapped;
        }
        if committed {
            self.agent_reconcile = stream.agent.map(|index| Reconcile {
                index,
                remaining: self.agent_segments[index].text.clone(),
                started: false,
            });
            self.thought_reconcile = stream.thought.map(|index| Reconcile {
                index,
                remaining: self.thought_segments[index].text.clone(),
                started: false,
            });
        } else {
            if let Some(index) = stream.agent {
                let segment = &mut self.agent_segments[index];
                segment.status = Some("failed".into());
                mapped.items.push(segment.clone());
            }
            if let Some(index) = stream.thought {
                let segment = &mut self.thought_segments[index];
                segment.status = Some("failed".into());
                mapped.items.push(segment.clone());
            }
            self.close_text_segments();
        }
        mapped
    }

    fn push_segment(&mut self, kind: ThreadItemKind) -> usize {
        let sequence = self.next_sequence();
        let (segments, name) = if kind == "reasoning" {
            (&mut self.thought_segments, "thought")
        } else {
            (&mut self.agent_segments, "assistant")
        };
        let index = segments.len();
        let mut segment = item(
            format!("{}:{name}:{}", self.turn_id, index + 1),
            kind,
            String::new(),
            "running",
            &self.turn_id,
        );
        segment.sequence = Some(sequence);
        segments.push(segment);
        index
    }

    /// The live segment a committed chunk must confirm, if one was streamed.
    fn take_reconcile(&mut self, thought: bool) -> Option<Reconcile> {
        let pending = if thought {
            self.thought_reconcile.take()
        } else {
            self.agent_reconcile.take()
        };
        if pending.is_some() {
            return pending;
        }
        // ACP outran the bridge: freeze the live attempt even if this kind has
        // no segment yet, so late frames cannot add a second copy.
        let stream = self.stream.as_mut()?;
        stream.reconciled = true;
        let index = if thought {
            stream.thought.take()
        } else {
            stream.agent.take()
        }?;
        let segments = if thought {
            &self.thought_segments
        } else {
            &self.agent_segments
        };
        Some(Reconcile {
            index,
            remaining: segments[index].text.clone(),
            started: false,
        })
    }

    /// Apply a committed chunk to its streamed segment. Matching text is
    /// already displayed; divergent text replaces the provisional stream.
    fn reconcile_chunk(
        &mut self,
        thought: bool,
        mut pending: Reconcile,
        text: &str,
    ) -> MappedUpdate {
        let mut mapped = MappedUpdate::default();
        let segment = if thought {
            &mut self.thought_segments[pending.index]
        } else {
            &mut self.agent_segments[pending.index]
        };
        if !pending.started {
            segment.text.clear();
            pending.started = true;
        }
        segment.text.push_str(text);
        if pending.remaining.starts_with(text) {
            pending.remaining.drain(..text.len());
        } else {
            pending.remaining.clear();
            mapped.items.push(segment.clone());
        }
        if !pending.remaining.is_empty() {
            if thought {
                self.thought_reconcile = Some(pending);
            } else {
                self.agent_reconcile = Some(pending);
            }
        }
        mapped
    }

    /// One durable timeline anchor per waiting episode. A wake updates this
    /// same row, then its streamed reply starts the next segment in this turn.
    pub fn native_background_wait(&mut self, count: usize) -> MappedUpdate {
        if let Some(index) = self.active_wait {
            let notice = &mut self.native_notices[index];
            if notice.status.as_deref() == Some("waiting") {
                return MappedUpdate::default();
            }
            notice.status = Some("waiting".into());
            return MappedUpdate {
                items: vec![notice.clone()],
                ..Default::default()
            };
        }
        self.close_text_segments();
        let mut notice = item(
            format!("{}:native-wait:{}", self.turn_id, self.native_notices.len()),
            ThreadItemKind::Generic,
            String::new(),
            "waiting",
            &self.turn_id,
        );
        notice.sequence = Some(self.next_sequence());
        notice
            .extra
            .insert("origin".into(), serde_json::json!("nativeBackgroundWait"));
        notice
            .extra
            .insert("backgroundTaskCount".into(), serde_json::json!(count));
        notice.extra.insert(
            "waitingStartedAt".into(),
            serde_json::json!(notice.created_at),
        );
        self.active_wait = Some(self.native_notices.len());
        self.native_notices.push(notice.clone());
        MappedUpdate {
            items: vec![notice],
            ..Default::default()
        }
    }

    pub fn native_task_notification(&mut self, message: &Value) -> MappedUpdate {
        let normalized;
        let message = if message["type"] == "user" {
            let Some(value) = super::claude_tasks::normalize_notification(message) else {
                return MappedUpdate::default();
            };
            normalized = value;
            &normalized
        } else {
            message
        };
        if message["type"] != "system" || message["subtype"] != "task_notification" {
            return MappedUpdate::default();
        }
        let Some(task) = message["task_id"].as_str().filter(|id| !id.is_empty()) else {
            return MappedUpdate::default();
        };
        let terminal = matches!(
            message["status"].as_str(),
            Some("completed" | "failed" | "stopped" | "killed" | "cancelled")
        );
        let key = if terminal {
            format!("task:{task}")
        } else {
            message["notificationKey"].as_str().unwrap_or(task).into()
        };
        if !self.seen_notices.insert(key) {
            return MappedUpdate::default();
        }
        self.close_text_segments();
        let index = self.active_wait.take();
        let mut notice = index
            .map(|i| self.native_notices[i].clone())
            .unwrap_or_else(|| {
                let mut notice = item(
                    format!("{}:native-task:{}", self.turn_id, self.native_notices.len()),
                    ThreadItemKind::Generic,
                    String::new(),
                    "completed",
                    &self.turn_id,
                );
                notice.sequence = Some(self.next_sequence());
                notice
            });
        notice.text = message["summary"]
            .as_str()
            .unwrap_or("Background task notification received")
            .into();
        notice.status = Some("completed".into());
        notice
            .extra
            .insert("origin".into(), serde_json::json!("nativeTaskNotification"));
        notice
            .extra
            .insert("taskId".into(), serde_json::json!(task));
        notice
            .extra
            .insert("taskStatus".into(), message["status"].clone());
        notice.extra.insert(
            "awakenedAt".into(),
            message["timestamp"]
                .as_str()
                .map(str::to_owned)
                .map_or_else(now_rfc3339, |at| at)
                .into(),
        );
        if let Some(event) = message["event"].as_str() {
            notice.detail_text = Some(event.into());
        }
        if let Some(i) = index {
            self.native_notices[i] = notice.clone();
        } else {
            self.native_notices.push(notice.clone());
        }
        MappedUpdate {
            items: vec![notice],
            ..Default::default()
        }
    }

    pub fn apply(&mut self, update: &Value) -> MappedUpdate {
        let body = update.get("update").unwrap_or(update);
        let kind = body
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or("");
        let mut mapped = MappedUpdate::default();
        if matches!(
            kind,
            "agent_message_chunk" | "agent_thought_chunk" | "tool_call"
        ) {
            if let Some(index) = self.active_wait {
                let notice = &mut self.native_notices[index];
                if notice.status.as_deref() == Some("waiting") {
                    // Human steering can resume work before a task notification.
                    // Retain the anchor so that its eventual wake reuses it.
                    notice.status = Some("running".into());
                    mapped.items.push(notice.clone());
                }
            }
        }
        let created_at = acp_update_created_at(update);
        match kind {
            "agent_message_chunk" => {
                if let Some(text) = content_text(body) {
                    let message_id = body
                        .get("messageId")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    if let Some(pending) = self.take_reconcile(false) {
                        self.active_agent_segment = Some(pending.index);
                        self.active_thought_segment = None;
                        if let Some(id) = &message_id {
                            self.agent_message_segments
                                .insert(id.clone(), pending.index);
                        }
                        self.agent_message_id = message_id;
                        return self.reconcile_chunk(false, pending, &text);
                    }
                    if message_id.is_some() && message_id != self.agent_message_id {
                        self.active_agent_segment = None;
                    }
                    // Background tools and wait notices can arrive while a single
                    // foreground answer is streaming. The adapter's native message
                    // identity, rather than these interleaved events, owns its text.
                    if self.active_agent_segment.is_none() {
                        self.active_agent_segment = message_id
                            .as_ref()
                            .and_then(|id| self.agent_message_segments.get(id).copied());
                    }
                    self.agent_message_id = message_id.clone();
                    self.active_thought_segment = None;
                    let segment_index = if let Some(index) = self.active_agent_segment {
                        index
                    } else {
                        let index = self.agent_segments.len();
                        let sequence = self.next_sequence();
                        let mut segment = item(
                            format!("{}:assistant:{}", self.turn_id, index + 1),
                            ThreadItemKind::AgentMessage,
                            String::new(),
                            "running",
                            &self.turn_id,
                        );
                        if let Some(created_at) = created_at {
                            segment.created_at = Some(created_at);
                        }
                        segment.sequence = Some(sequence);
                        self.agent_segments.push(segment);
                        self.active_agent_segment = Some(index);
                        index
                    };
                    if let Some(id) = message_id {
                        self.agent_message_segments.insert(id, segment_index);
                    }
                    if let Some(segment) = self.agent_segments.get_mut(segment_index) {
                        segment.text.push_str(&text);
                        mapped.deltas.push((
                            segment.id.clone(),
                            text,
                            segment.sequence.unwrap_or_default(),
                        ));
                    }
                }
            }
            "agent_thought_chunk" => {
                if let Some(text) = content_text(body) {
                    let message_id = body
                        .get("messageId")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    if let Some(pending) = self.take_reconcile(true) {
                        self.active_thought_segment = Some(pending.index);
                        self.active_agent_segment = None;
                        self.thought_message_id = message_id;
                        return self.reconcile_chunk(true, pending, &text);
                    }
                    if message_id.is_some()
                        && (message_id != self.thought_message_id || text == "\n\n")
                    {
                        self.active_thought_segment = None;
                    }
                    self.thought_message_id = message_id;
                    if text.trim().is_empty() && self.active_thought_segment.is_none() {
                        return mapped;
                    }
                    self.active_agent_segment = None;
                    let segment_index = if let Some(index) = self.active_thought_segment {
                        index
                    } else {
                        let index = self.thought_segments.len();
                        let sequence = self.next_sequence();
                        let mut segment = item(
                            format!("{}:thought:{}", self.turn_id, index + 1),
                            ThreadItemKind::Reasoning,
                            String::new(),
                            "running",
                            &self.turn_id,
                        );
                        if let Some(created_at) = created_at {
                            segment.created_at = Some(created_at);
                        }
                        segment.sequence = Some(sequence);
                        self.thought_segments.push(segment);
                        self.active_thought_segment = Some(index);
                        index
                    };
                    if let Some(segment) = self.thought_segments.get_mut(segment_index) {
                        segment.text.push_str(&text);
                        mapped.items.push(segment.clone());
                    }
                }
            }
            "tool_call" | "tool_call_update" => {
                if let Some(tool_id) = body.get("toolCallId").and_then(Value::as_str) {
                    let existing_metadata = self
                        .tools
                        .iter()
                        .find(|candidate| candidate.id == tool_id)
                        .map(|candidate| (candidate.created_at.clone(), candidate.sequence));
                    if existing_metadata.is_none() {
                        self.close_text_segments();
                        // A step's committed text precedes its tool calls.
                        self.agent_reconcile = None;
                        self.thought_reconcile = None;
                    }
                    let payload = merge_tool_payload(self.tool_payloads.get(tool_id), body);
                    self.tool_payloads
                        .insert(tool_id.to_string(), payload.clone());
                    let mut item = tool_item(&self.turn_id, &payload);
                    let (existing_created_at, existing_sequence) =
                        existing_metadata.unwrap_or((None, None));
                    if let Some(created_at) = existing_created_at.clone().or(created_at.clone()) {
                        item.created_at = Some(created_at);
                    }
                    item.sequence = existing_sequence.or_else(|| Some(self.next_sequence()));
                    if let Some(existing) = self
                        .tools
                        .iter_mut()
                        .find(|candidate| candidate.id == item.id)
                    {
                        *existing = item.clone();
                    } else {
                        self.tools.push(item.clone());
                    }
                    if item.kind == "agentToolCall" {
                        let status = item.status.clone().unwrap_or_else(|| "running".into());
                        if matches!(status.as_str(), "completed" | "failed" | "interrupted") {
                            self.subagents.remove(tool_id);
                        } else {
                            let name = payload
                                .get("name")
                                .and_then(Value::as_str)
                                .or_else(|| payload.get("title").and_then(Value::as_str))
                                .map(str::to_string);
                            let started_at = existing_created_at
                                .clone()
                                .or_else(|| item.created_at.clone())
                                .or_else(|| created_at.clone());
                            let parent_tool_call_id = payload
                                .get("parentToolCallId")
                                .or_else(|| payload.get("parent_tool_call_id"))
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            self.subagents.insert(
                                tool_id.to_string(),
                                ThreadSubagentDto {
                                    id: tool_id.to_string(),
                                    name,
                                    status,
                                    started_at,
                                    completed_at: None,
                                    parent_tool_call_id,
                                    is_background: None,
                                },
                            );
                        }
                        mapped.active_subagents = Some(self.subagents.values().cloned().collect());
                    }
                    mapped.items.push(item);
                }
            }
            "plan" => {
                if let Some(mut item) = plan_item(&self.turn_id, body) {
                    let existing_sequence = self.plans.first().and_then(|current| current.sequence);
                    if existing_sequence.is_none() {
                        self.close_text_segments();
                    }
                    if let Some(created_at) = self
                        .plans
                        .first()
                        .and_then(|current| current.created_at.clone())
                        .or(created_at)
                    {
                        item.created_at = Some(created_at);
                    }
                    item.sequence = existing_sequence.or_else(|| Some(self.next_sequence()));
                    self.plans = vec![item.clone()];
                    mapped.items.push(item);
                    mapped.plan = plan_steps(body);
                }
            }
            "compaction_update" | "compaction_summary_chunk" => {
                if let Some(mut item) = compaction_item(&self.turn_id, body) {
                    let existing_sequence = self
                        .compactions
                        .first()
                        .and_then(|current| current.sequence);
                    if existing_sequence.is_none() {
                        self.close_text_segments();
                    }
                    if let Some(created_at) = self
                        .compactions
                        .first()
                        .and_then(|current| current.created_at.clone())
                        .or(created_at)
                    {
                        item.created_at = Some(created_at);
                    }
                    item.sequence = existing_sequence.or_else(|| Some(self.next_sequence()));
                    self.compactions = vec![item.clone()];
                    mapped.items.push(item);
                }
            }
            "session_info_update" => {
                mapped.title = body
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                mapped.goal = goal_from_update(update).or_else(|| goal_from_update(body));
            }
            "usage_update" => {
                mapped.usage = Some(body.clone());
            }
            _ => {}
        }
        mapped
    }

    fn next_sequence(&mut self) -> i64 {
        self.seq += 1;
        self.seq
    }

    pub fn active_subagents(&self) -> Vec<ThreadSubagentDto> {
        self.subagents.values().cloned().collect()
    }

    fn close_text_segments(&mut self) {
        self.active_agent_segment = None;
        self.active_thought_segment = None;
    }

    pub fn finish(mut self, interrupted: bool) -> Vec<ThreadHistoryItemDto> {
        let status = if interrupted {
            "interrupted"
        } else {
            "completed"
        };
        // A discarded live attempt stays failed beside its retry.
        for segment in self
            .agent_segments
            .iter_mut()
            .chain(self.thought_segments.iter_mut())
            .filter(|segment| segment.status.as_deref() != Some("failed"))
        {
            segment.status = Some(status.into());
        }
        if self.agent_segments.is_empty()
            && self.thought_segments.is_empty()
            && self.tools.is_empty()
            && self.plans.is_empty()
            && self.compactions.is_empty()
            && self.native_notices.is_empty()
            && !interrupted
        {
            let mut agent = item(
                format!("{}:assistant:1", self.turn_id),
                ThreadItemKind::AgentMessage,
                "(no output)".into(),
                status,
                &self.turn_id,
            );
            agent.sequence = Some(self.next_sequence());
            self.agent_segments.push(agent);
        }
        for notice in &mut self.native_notices {
            if matches!(notice.status.as_deref(), Some("waiting" | "running")) {
                notice.status = Some(status.into());
            }
        }
        let mut items = Vec::new();
        items.extend(self.thought_segments);
        items.extend(self.tools);
        items.extend(self.plans);
        items.extend(self.compactions);
        items.extend(self.native_notices);
        items.extend(self.agent_segments);
        items.sort_by_key(|item| item.sequence.unwrap_or(i64::MAX));
        items
    }

    pub(super) fn completed_tool_ids(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter(|tool| tool.status.as_deref() == Some("completed"))
            .map(|tool| tool.id.clone())
            .collect()
    }

    pub(super) fn final_agent_text(&self) -> Option<&str> {
        self.agent_segments
            .last()
            .map(|segment| segment.text.as_str())
    }

    pub(super) fn reconcile_abandoned_tools(&mut self, ids: &[String]) {
        for id in ids {
            if let Some(payload) = self.tool_payloads.get_mut(id) {
                payload["status"] = Value::String("interrupted".into());
                payload["content"] = serde_json::json!([{"type":"content","content":{
                    "type":"text","text":"Streaming tool call discarded after steering; no execution recorded."
                }}]);
                if let Some(obj) = payload.as_object_mut() {
                    obj.remove("rawOutput");
                }
                if let Some(item) = self.tools.iter_mut().find(|tool| tool.id == *id) {
                    let mut reconciled = tool_item(&self.turn_id, payload);
                    reconciled.created_at = item.created_at.clone();
                    reconciled.sequence = item.sequence;
                    *item = reconciled;
                }
            }
        }
    }
}

fn acp_update_created_at(update: &Value) -> Option<String> {
    let raw = update
        .pointer("/_meta/agentTimestampMs")
        .or_else(|| update.pointer("/update/_meta/agentTimestampMs"))?
        .as_f64()?;
    if !raw.is_finite() || raw <= 0.0 || raw > i64::MAX as f64 {
        return None;
    }
    chrono::DateTime::from_timestamp_millis(raw.trunc() as i64)
        .map(|timestamp| timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn content_text(body: &Value) -> Option<String> {
    body.pointer("/content/text")
        .and_then(Value::as_str)
        .or_else(|| body.pointer("/update/content/text").and_then(Value::as_str))
        .map(str::to_string)
}

fn merge_tool_payload(previous: Option<&Value>, patch: &Value) -> Value {
    let mut merged = previous
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(fields) = patch.as_object() {
        for (key, value) in fields {
            merged.insert(key.clone(), value.clone());
        }
    }
    // Codex ACP's terminal references are accompanied by output in _meta.
    // Accumulate deltas separately; a final rawOutput remains authoritative.
    let previous_output = previous
        .and_then(|value| value.get("_remoteTerminalOutput"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if let Some(delta) = patch
        .pointer("/_meta/terminal_output_delta/data")
        .and_then(Value::as_str)
        .or_else(|| {
            patch
                .pointer("/_meta/terminal_output/data")
                .and_then(Value::as_str)
        })
    {
        merged.insert(
            "_remoteTerminalOutput".into(),
            Value::String(format!("{previous_output}{delta}")),
        );
    }
    Value::Object(merged)
}

fn tool_item(turn_id: &str, body: &Value) -> ThreadHistoryItemDto {
    let id = body
        .get("toolCallId")
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_string();
    let raw_kind = body.get("kind").and_then(Value::as_str).unwrap_or("");
    let tool_name = body.get("name").and_then(Value::as_str).or_else(|| {
        body.pointer("/_meta/x.ai~1tool/name")
            .and_then(Value::as_str)
    });
    let normalized_name = format!(
        "{} {}",
        tool_name.unwrap_or(""),
        body.get("title").and_then(Value::as_str).unwrap_or("")
    )
    .to_ascii_lowercase();
    let command = record_value(body.get("rawInput"), &["command", "cmd", "argv"]);
    let kind = if is_subagent_tool(raw_kind, &normalized_name) {
        ThreadItemKind::AgentToolCall
    } else {
        match raw_kind {
            "edit" | "delete" | "move" => ThreadItemKind::FileChange,
            "read" => ThreadItemKind::FileRead,
            "fetch" => ThreadItemKind::WebSearch,
            "think" => ThreadItemKind::Reasoning,
            "execute" => ThreadItemKind::CommandExecution,
            // Harnesses that report every tool as "other" (DSH): a command
            // input is a shell run, and a todo list is not a file.
            _ if command.is_some() => ThreadItemKind::CommandExecution,
            _ if normalized_name.contains("todo") => ThreadItemKind::ToolCall,
            _ if normalized_name.contains("web") || normalized_name.contains("http") => {
                ThreadItemKind::WebSearch
            }
            _ if normalized_name.contains("read") => ThreadItemKind::FileRead,
            _ if normalized_name.contains("edit")
                || normalized_name.contains("write")
                || normalized_name.contains("patch") =>
            {
                ThreadItemKind::FileChange
            }
            _ => ThreadItemKind::ToolCall,
        }
    };
    let location_text = tool_locations(body)
        .into_iter()
        .map(|(path, line)| match line {
            Some(line) => format!("{path}:{line}"),
            None => path,
        })
        .collect::<Vec<_>>()
        .join(", ");
    let input_path = record_value(
        body.get("rawInput"),
        &["path", "file_path", "target_file", "file", "uri"],
    );
    let title = command
        .clone()
        .or_else(|| (!location_text.is_empty()).then_some(location_text.clone()))
        .or(input_path)
        .or_else(|| nonempty_string(body.get("title")))
        .or_else(|| tool_name.map(str::to_string))
        .unwrap_or_else(|| "Tool call".into());
    let status = match body.get("status").and_then(Value::as_str).unwrap_or("") {
        "completed" | "failed" | "interrupted" => body.get("status").and_then(Value::as_str),
        "in_progress" => Some("running"),
        _ => Some("running"),
    };
    let detail = tool_detail(body, tool_name, raw_kind, &location_text);
    let mut mapped = item(
        id,
        kind,
        title.clone(),
        status.unwrap_or("running"),
        turn_id,
    );
    mapped.preview_text = Some(title);
    mapped.detail_text = (!detail.is_empty()).then_some(detail);
    if mapped.kind == ThreadItemKind::FileChange {
        apply_file_changes(&mut mapped, body);
    }
    mapped
}

fn is_subagent_tool(raw_kind: &str, normalized_name: &str) -> bool {
    let primary_name = normalized_name.split_whitespace().next().unwrap_or("");
    matches!(
        raw_kind,
        "agent"
            | "agent_tool_call"
            | "agentToolCall"
            | "subagent"
            | "subagent_tool_call"
            | "collabAgentToolCall"
            | "collab_agent_tool_call"
    ) || matches!(primary_name, "task" | "agent" | "subagent")
        || normalized_name.contains("subagent")
        || normalized_name.contains("spawn_agent")
}

fn apply_file_changes(mapped: &mut ThreadHistoryItemDto, body: &Value) {
    let mut paths = Vec::new();
    let mut patches = Vec::new();
    let mut added = 0usize;
    let mut removed = 0usize;
    for entry in body
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if entry["type"] != "diff" {
            continue;
        }
        let path = entry["path"].as_str().unwrap_or("unknown");
        let old = entry["oldText"].as_str().unwrap_or("");
        let new = entry["newText"].as_str().unwrap_or("");
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
        paths.push(path);
    }
    if !paths.is_empty() {
        mapped.text = paths.join(", ");
        mapped.preview_text = Some(mapped.text.clone());
        mapped.detail_text = Some(patches.join("\n"));
        mapped
            .extra
            .insert("changedFiles".into(), serde_json::json!(paths.len()));
        mapped
            .extra
            .insert("addedLines".into(), serde_json::json!(added));
        mapped
            .extra
            .insert("removedLines".into(), serde_json::json!(removed));
    }
}

fn nonempty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn record_value(value: Option<&Value>, keys: &[&str]) -> Option<String> {
    let record = value?.as_object()?;
    keys.iter().find_map(|key| match record.get(*key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_string()),
        Some(Value::Array(values)) if values.iter().all(Value::is_string) => Some(
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    })
}

fn tool_locations(body: &Value) -> Vec<(String, Option<u64>)> {
    body.get("locations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|location| {
            Some((
                location.get("path")?.as_str()?.to_string(),
                location.get("line").and_then(Value::as_u64),
            ))
        })
        .collect()
}

fn tool_detail(body: &Value, tool_name: Option<&str>, raw_kind: &str, locations: &str) -> String {
    let mut parts = Vec::new();
    if let Some(name) = tool_name.or_else(|| body.get("title").and_then(Value::as_str)) {
        parts.push(format!("Tool: {name}"));
    }
    if !raw_kind.is_empty() {
        parts.push(format!("Kind: {raw_kind}"));
    }
    if let Some(status) = body.get("status").and_then(Value::as_str) {
        parts.push(format!("Status: {status}"));
    }
    if !locations.is_empty() {
        parts.push(format!("Locations:\n{locations}"));
    }
    if let Some(input) = body.get("rawInput") {
        parts.push(format!("Input:\n{}", pretty_json(input)));
    }
    let content = tool_content_text(body);
    let terminal_output = body
        .get("rawOutput")
        .filter(|value| !value.is_null())
        .map(readable_output)
        .or_else(|| {
            body.get("_remoteTerminalOutput")
                .and_then(Value::as_str)
                .filter(|output| !output.is_empty())
                .map(str::to_owned)
        });
    if raw_kind == "execute" && terminal_output.is_some() {
        parts.push(format!("Output:\n{}", terminal_output.unwrap()));
        return parts.join("\n\n");
    }
    if !content.is_empty() {
        parts.push(format!("Result:\n{content}"));
    } else if let Some(output) = body.get("rawOutput") {
        parts.push(format!("Output:\n{}", readable_output(output)));
    }
    parts.join("\n\n")
}

fn tool_content_text(body: &Value) -> String {
    body.get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| match entry.get("type").and_then(Value::as_str) {
            Some("content") => content_block_text(entry.get("content")?),
            Some("diff") => Some(format!(
                "File: {}\n\nBefore:\n{}\n\nAfter:\n{}",
                entry
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                entry
                    .get("oldText")
                    .and_then(Value::as_str)
                    .unwrap_or("(new file)"),
                entry.get("newText").and_then(Value::as_str).unwrap_or("")
            )),
            Some("terminal") => entry
                .get("terminalId")
                .and_then(Value::as_str)
                .map(|id| format!("Terminal: {id}")),
            _ => content_block_text(entry),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn content_block_text(block: &Value) -> Option<String> {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => nonempty_string(block.get("text")),
        Some("resource_link") => {
            let label = block
                .get("title")
                .or_else(|| block.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("Resource");
            let uri = block.get("uri").and_then(Value::as_str).unwrap_or("");
            Some(format!("[{label}]({uri})"))
        }
        Some("resource") => block
            .pointer("/resource/text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                block
                    .pointer("/resource/uri")
                    .and_then(Value::as_str)
                    .map(|uri| format!("Resource: {uri}"))
            }),
        Some("image") => Some(format!(
            "Image: {}",
            block
                .get("uri")
                .or_else(|| block.get("mimeType"))
                .and_then(Value::as_str)
                .unwrap_or("image")
        )),
        Some("audio") => Some(format!(
            "Audio: {}",
            block
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("audio")
        )),
        _ => None,
    }
}

fn readable_output(output: &Value) -> String {
    for pointer in [
        "/formatted_output",
        "/aggregatedOutput",
        "/output_for_prompt",
        "/raw_output",
        "/output",
        "/content",
        "/FileContent/content",
    ] {
        if let Some(text) = output.pointer(pointer).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    pretty_json(output)
}

fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn plan_steps(body: &Value) -> Option<Vec<(String, String)>> {
    let entries = body.get("entries").and_then(Value::as_array)?;
    let steps = entries
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("content").and_then(Value::as_str)?.to_string(),
                entry
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("pending")
                    .to_string(),
            ))
        })
        .collect::<Vec<_>>();
    (!steps.is_empty()).then_some(steps)
}

fn plan_item(turn_id: &str, body: &Value) -> Option<ThreadHistoryItemDto> {
    let entries = body.get("entries").and_then(Value::as_array)?;
    let text = entries
        .iter()
        .filter_map(|entry| {
            let step = entry.get("content").and_then(Value::as_str)?;
            let done = entry.get("status").and_then(Value::as_str) == Some("completed");
            Some(format!("- [{}] {step}", if done { "x" } else { " " }))
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(item(
        format!("{turn_id}:plan"),
        ThreadItemKind::Plan,
        text,
        "inProgress",
        turn_id,
    ))
}

fn compaction_item(turn_id: &str, body: &Value) -> Option<ThreadHistoryItemDto> {
    let from_content = content_text(body);
    let text = body
        .get("summary")
        .and_then(Value::as_str)
        .or(from_content.as_deref())
        .unwrap_or("Context compaction")
        .to_string();
    Some(item(
        format!("{turn_id}:compaction"),
        ThreadItemKind::ContextCompaction,
        text,
        "completed",
        turn_id,
    ))
}

fn item(
    id: String,
    kind: ThreadItemKind,
    text: String,
    status: &str,
    turn_id: &str,
) -> ThreadHistoryItemDto {
    ThreadHistoryItemDto {
        id,
        created_at: Some(now_rfc3339()),
        kind,
        text,
        preview_text: None,
        detail_text: None,
        status: Some(status.into()),
        sequence: None,
        source_turn_id: Some(turn_id.into()),
        artifact: None,
        extra: Default::default(),
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use serde_json::json;

    fn chunk(kind: &str, text: &str) -> Value {
        json!({"sessionUpdate":kind,"messageId":"m1","content":{"type":"text","text":text}})
    }

    #[test]
    fn committed_chunks_confirm_streamed_text_without_duplicates() {
        let mut mapper = TurnMapper::new("turn");
        mapper.stream_start("a1");
        assert!(mapper.stream_text("a1", "reasoning", "think").items[0].text == "think");
        let first = mapper.stream_text("a1", "text", "Hel");
        let second = mapper.stream_text("a1", "text", "lo");
        assert_eq!(first.deltas[0].1, "Hel");
        assert_eq!(second.deltas[0].0, first.deltas[0].0);
        assert!(mapper.stream_end("a1", true).items.is_empty());
        let thought = mapper.apply(&chunk("agent_thought_chunk", "think"));
        let message = mapper.apply(&chunk("agent_message_chunk", "Hello"));
        assert!(thought.items.is_empty() && thought.deltas.is_empty());
        assert!(message.items.is_empty() && message.deltas.is_empty());
        let items = mapper.finish(false);
        let texts: Vec<_> = items
            .iter()
            .map(|item| (item.kind.as_str(), item.text.as_str()))
            .collect();
        assert_eq!(texts, [("reasoning", "think"), ("agentMessage", "Hello")]);
    }

    #[test]
    fn native_message_identity_keeps_text_whole_across_background_events() {
        let mut mapper = TurnMapper::new("turn");
        let text = |id: &str, text: &str| {
            serde_json::json!({
                "sessionUpdate": "agent_message_chunk", "messageId": id,
                "content": { "type": "text", "text": text }
            })
        };
        let first = mapper.apply(&text("answer", "## New passkey "));
        mapper.native_background_wait(2);
        mapper.apply(&serde_json::json!({
            "sessionUpdate":"tool_call", "toolCallId":"child-read",
            "title":"Read file", "kind":"read", "status":"in_progress"
        }));
        let second = mapper.apply(&text("answer", "on all devices\n\nCa"));
        mapper.apply(&serde_json::json!({
            "sessionUpdate":"tool_call_update", "toolCallId":"child-read", "status":"completed"
        }));
        let third = mapper.apply(&text("answer", "n approve once."));
        assert_eq!(first.deltas[0].0, second.deltas[0].0);
        assert_eq!(first.deltas[0].0, third.deltas[0].0);
        let next = mapper.apply(&text("next-response", "Next update"));
        assert_ne!(first.deltas[0].0, next.deltas[0].0);
        let messages: Vec<_> = mapper
            .finish(false)
            .into_iter()
            .filter(|item| item.kind == "agentMessage")
            .map(|item| item.text)
            .collect();
        assert_eq!(
            messages,
            [
                "## New passkey on all devices\n\nCan approve once.",
                "Next update"
            ]
        );
    }

    #[test]
    fn divergent_committed_text_replaces_the_stream_and_later_blocks_append() {
        let mut mapper = TurnMapper::new("turn");
        mapper.stream_start("a1");
        mapper.stream_text("a1", "text", "draft");
        mapper.stream_end("a1", true);
        let replaced = mapper.apply(&chunk("agent_message_chunk", "final"));
        assert_eq!(replaced.items[0].text, "final");
        let appended = mapper.apply(&chunk("agent_message_chunk", " block"));
        assert_eq!(appended.deltas[0].1, " block");
        assert_eq!(mapper.finish(false)[0].text, "final block");
    }

    #[test]
    fn discarded_attempts_fail_and_retries_stream_into_new_segments() {
        let mut mapper = TurnMapper::new("turn");
        mapper.stream_start("a1");
        let partial = mapper.stream_text("a1", "text", "par");
        let failed = mapper.stream_end("a1", false);
        assert_eq!(failed.items[0].status.as_deref(), Some("failed"));
        mapper.stream_start("a2");
        let retry = mapper.stream_text("a2", "text", "ok");
        assert_ne!(retry.deltas[0].0, partial.deltas[0].0);
        mapper.stream_end("a2", true);
        assert!(mapper
            .apply(&chunk("agent_message_chunk", "ok"))
            .deltas
            .is_empty());
        let items = mapper.finish(false);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].status.as_deref(), Some("failed"));
        assert_eq!(items[1].text, "ok");
        assert_eq!(items[1].status.as_deref(), Some("completed"));
    }

    #[test]
    fn committed_text_before_its_first_live_frame_is_not_duplicated() {
        let mut mapper = TurnMapper::new("turn");
        mapper.stream_start("a1");
        // The whole short reply arrives in the final flush, after ACP's chunk.
        assert_eq!(
            mapper.apply(&chunk("agent_message_chunk", "Hi")).deltas[0].1,
            "Hi"
        );
        assert!(mapper.stream_text("a1", "text", "Hi").deltas.is_empty());
        assert!(mapper.stream_end("a1", true).items.is_empty());
        let items = mapper.finish(false);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "Hi");
    }

    #[test]
    fn committed_chunk_before_end_frame_freezes_the_live_attempt() {
        let mut mapper = TurnMapper::new("turn");
        mapper.stream_start("a1");
        mapper.stream_text("a1", "text", "same");
        assert!(mapper
            .apply(&chunk("agent_message_chunk", "same"))
            .items
            .is_empty());
        assert!(mapper.stream_text("a1", "text", " late").deltas.is_empty());
        assert!(mapper.stream_end("a1", true).items.is_empty());
        assert_eq!(mapper.finish(false)[0].text, "same");
    }
}

#[cfg(test)]
mod presentation_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn other_kind_tools_are_classified_by_their_input() {
        // DSH reports every tool as "other" with its name as the title.
        let item = |name: &str, input: Value| {
            tool_item(
                "turn",
                &json!({"toolCallId":name,"kind":"other","title":name,"status":"completed","rawInput":input}),
            )
        };
        let todo = item(
            "todo_write",
            json!({"todos":[{"content":"x","status":"pending"}]}),
        );
        assert_eq!(todo.kind, "toolCall");
        let write = item(
            "write",
            json!({"file_path":"inventory/core.py","content":"x"}),
        );
        assert_eq!(write.kind, "fileChange");
        assert_eq!(write.text, "inventory/core.py");
        let bash = item("bash", json!({"command":"python3 -m unittest"}));
        assert_eq!(bash.kind, "commandExecution");
        assert_eq!(bash.text, "python3 -m unittest");
    }

    #[test]
    fn terminal_output_survives_references_deltas_and_final_snapshot() {
        let mut mapper = TurnMapper::new("turn");
        mapper.apply(&json!({"sessionUpdate":"tool_call","toolCallId":"cmd","kind":"execute","title":"Run tests","content":[{"type":"terminal","terminalId":"cmd"}]}));
        mapper.apply(&json!({"sessionUpdate":"tool_call_update","toolCallId":"cmd","_meta":{"terminal_output_delta":{"data":"first\n","terminal_id":"cmd"}}}));
        let update = mapper.apply(&json!({"sessionUpdate":"tool_call_update","toolCallId":"cmd","_meta":{"terminal_output_delta":{"data":"second\n","terminal_id":"cmd"}}}));
        assert!(update.items[0]
            .detail_text
            .as_ref()
            .unwrap()
            .contains("first\nsecond\n"));
        let done = mapper.apply(&json!({"sessionUpdate":"tool_call_update","toolCallId":"cmd","status":"completed","rawOutput":{"formatted_output":"first\nsecond\n","exit_code":0}}));
        let detail = done.items[0].detail_text.as_ref().unwrap();
        assert!(detail.contains("Output:\nfirst\nsecond\n"));
        assert!(!detail.contains("Terminal: cmd"));
        assert_eq!(detail.matches("first").count(), 1);
    }

    #[test]
    fn reasoning_respects_message_and_summary_boundaries() {
        let mut mapper = TurnMapper::new("turn");
        for (id, text, at) in [
            ("a", "**First", "2026-09-20T02:00:00Z"),
            ("a", " step**", "2026-09-20T02:00:01Z"),
            ("a", "\n\n", "2026-09-20T02:00:02Z"),
            ("a", "**Second**", "2026-09-20T02:00:03Z"),
            ("b", "**Third**", "2026-09-20T02:00:04Z"),
        ] {
            mapper.apply(&json!({"sessionUpdate":"agent_thought_chunk","messageId":id,"content":{"type":"text","text":text},"createdAt":at}));
        }
        let items = mapper.finish(false);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].text, "**First step**");
        assert_eq!(items[1].text, "**Second**");
        assert_eq!(items[2].text, "**Third**");
        assert_ne!(items[0].id, items[1].id);
    }

    #[test]
    fn file_changes_include_paths_counts_and_unified_diff() {
        let mapped = tool_item(
            "turn",
            &json!({"toolCallId":"edit","kind":"edit","title":"Editing files","status":"completed","content":[{"type":"diff","path":"src/main.rs","oldText":"fn main() {\n    old();\n}\n","newText":"fn main() {\n    new();\n    done();\n}\n"}]}),
        );
        assert_eq!(mapped.text, "src/main.rs");
        assert_eq!(mapped.extra["addedLines"], 2);
        assert_eq!(mapped.extra["removedLines"], 1);
        assert_eq!(mapped.extra["changedFiles"], 1);
        let detail = mapped.detail_text.unwrap();
        assert!(detail.contains("-    old();"));
        assert!(detail.contains("+    new();"));
        assert!(detail.contains("@@"));
    }

    #[test]
    fn native_agent_tool_calls_are_reported_as_active_subagents() {
        let mut mapper = TurnMapper::new("turn");
        let started = mapper.apply(&json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "agent-1",
            "kind": "agent",
            "name": "Task",
            "title": "Investigate the failing test",
            "status": "in_progress"
        }));
        assert_eq!(started.items[0].kind, "agentToolCall");
        assert_eq!(started.active_subagents.as_ref().unwrap().len(), 1);
        assert_eq!(
            started.active_subagents.as_ref().unwrap()[0].status,
            "running"
        );

        let finished = mapper.apply(&json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "agent-1",
            "status": "completed"
        }));
        assert!(finished.active_subagents.as_ref().unwrap().is_empty());
    }
}

fn goal_from_update(update: &Value) -> Option<Option<GoalState>> {
    let meta = update.get("_meta")?.as_object()?;
    if !meta.contains_key("goal") {
        return None;
    }
    let goal = meta.get("goal")?;
    if goal.is_null() {
        return Some(None);
    }
    Some(Some(GoalState {
        objective: goal
            .get("objective")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        status: goal
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("active")
            .into(),
        tokens_used: goal.get("tokensUsed").and_then(Value::as_u64).unwrap_or(0) as u32,
        time_used_seconds: goal
            .get("timeUsedSeconds")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
    }))
}

#[cfg(test)]
mod background_wait_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn wake_reuses_wait_anchor_and_followup_segments_stay_in_order() {
        let mut mapper = TurnMapper::new("turn");
        mapper.apply(&json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Waiting."}}));
        let wait = mapper.native_background_wait(1).items.remove(0);
        assert!(mapper.native_background_wait(1).items.is_empty());
        let message = json!({"type":"user","origin":{"kind":"task-notification"},"content":[{"type":"text","text":"<task-notification><task-id>watch</task-id><summary>Build progress</summary><event>Linux finished</event></task-notification>"}]});
        let wake = mapper.native_task_notification(&message).items.remove(0);
        assert_eq!(wait.id, wake.id);
        assert_eq!(wait.sequence, wake.sequence);
        assert!(mapper.native_task_notification(&message).items.is_empty());
        mapper.apply(&json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Checking."}}));
        let items = mapper.finish(false);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].text, "Waiting.");
        assert_eq!(items[1].text, "Build progress");
        assert_eq!(items[2].text, "Checking.");
        assert!(items
            .iter()
            .all(|i| i.source_turn_id.as_deref() == Some("turn")));
    }
    #[test]
    fn steering_keeps_the_anchor_for_its_eventual_native_wake() {
        let mut mapper = TurnMapper::new("turn");
        let wait = mapper.native_background_wait(1).items.remove(0);
        let resumed = mapper.apply(&json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Handling your correction."}}));
        assert!(resumed
            .items
            .iter()
            .any(|i| i.id == wait.id && i.status.as_deref() == Some("running")));
        let waiting_again = mapper.native_background_wait(1).items.remove(0);
        assert_eq!(wait.id, waiting_again.id);
        let wake = mapper.native_task_notification(&json!({"type":"system","subtype":"task_notification","task_id":"build","status":"completed","summary":"Build done"})).items.remove(0);
        assert_eq!(wake.id, wait.id);
        assert_eq!(
            mapper
                .finish(false)
                .iter()
                .filter(|i| i.extra.contains_key("origin"))
                .count(),
            1
        );
    }

    #[test]
    fn interrupted_wait_does_not_remain_waiting_after_reload() {
        let mut mapper = TurnMapper::new("turn");
        mapper.native_background_wait(1);
        assert_eq!(
            mapper.finish(true)[0].status.as_deref(),
            Some("interrupted")
        );
    }
}
