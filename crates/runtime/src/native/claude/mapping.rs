//! Claude Code SDK frames → the shared ACP-shaped updates `TurnMapper` reads.
//!
//! Every frame type the installed CLI sends is listed in `docs/native-harnesses.md`
//! (coverage matrix). Tool calls render as claude-agent-acp renders them for
//! Pockymoe (title, kind, content, locations), and the native protocol adds what
//! ACP drops: exit codes, exact whole-file diffs from Claude's own structured
//! results, API retry and model fallback notices, permission denials with their
//! reason, background subagents from SDK task frames, and per-request usage from
//! the final `message_delta` counts.
use crate::acp::mapper::{MappedUpdate, TurnMapper};
use pockymoe_protocol::ThreadSubagentDto;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Tools that update the plan instead of rendering a tool row (as on ACP).
const PLAN_TOOLS: [&str; 5] = [
    "TodoWrite",
    "TaskCreate",
    "TaskUpdate",
    "TaskList",
    "TaskGet",
];

struct Request {
    model: Value,
    usage: Value,
    parent: bool,
    observed_at: String,
}

pub(super) struct Mapper {
    pub inner: TurnMapper,
    /// The API message streaming now, per parent tool (`""` = main agent).
    streaming: HashMap<String, String>,
    uuids: HashMap<String, String>,
    streamed: HashSet<(String, String)>,
    tools: HashMap<String, Value>,
    requests: BTreeMap<String, Request>,
    last_parent: Option<String>,
    parent_model: Option<String>,
    window: Option<u64>,
    /// Stream usage is the billing record: no JSONL reader (environments).
    bills: bool,
    tasks: BTreeMap<String, Value>,
    background: BTreeMap<String, ThreadSubagentDto>,
    /// SDK task id → the Agent tool use that launched it.
    background_tasks: HashMap<String, String>,
    compaction: Option<String>,
    compacted: bool,
    /// A stop is in progress: the refused tool results are interruptions.
    stopping: bool,
}

impl Mapper {
    #[cfg(test)]
    pub fn new(turn: &str) -> Self {
        Self::with(turn, true, None)
    }

    pub fn with(turn: &str, bills: bool, window: Option<u64>) -> Self {
        Self {
            inner: TurnMapper::new(turn),
            streaming: HashMap::new(),
            uuids: HashMap::new(),
            streamed: HashSet::new(),
            tools: HashMap::new(),
            requests: BTreeMap::new(),
            last_parent: None,
            parent_model: None,
            window,
            bills,
            tasks: Default::default(),
            background: Default::default(),
            background_tasks: HashMap::new(),
            compaction: None,
            compacted: false,
            stopping: false,
        }
    }

    pub fn set_stopping(&mut self) {
        self.stopping = true;
    }

    /// Whether stream usage is billed (no JSONL reader found the session).
    pub fn set_bills(&mut self, bills: bool) {
        self.bills = bills;
    }

    pub fn window(&self) -> Option<u64> {
        self.window
    }

    pub fn finish(self, interrupted: bool) -> Vec<pockymoe_protocol::ThreadHistoryItemDto> {
        let mut items = self.inner.finish(interrupted);
        for item in &mut items {
            if let Some(uuid) = item
                .extra
                .get("nativeMessageId")
                .and_then(Value::as_str)
                .and_then(|id| self.uuids.get(id))
            {
                item.extra.insert("nativeMessageId".into(), json!(uuid));
            }
        }
        items
    }

    fn record_usage(&mut self, id: &str, model: &Value, usage: &Value, parent: bool, merge: bool) {
        if id.is_empty() || !usage.is_object() {
            return;
        }
        let now = pockymoe_protocol::now_rfc3339();
        let entry = self.requests.entry(id.into()).or_insert_with(|| Request {
            model: model.clone(),
            usage: json!({}),
            parent,
            observed_at: now,
        });
        if model.as_str().is_some_and(|m| m != "<synthetic>") {
            entry.model = model.clone();
        }
        for (key, value) in usage.as_object().into_iter().flatten() {
            if !value.is_number() && !value.is_object() {
                continue;
            }
            // A consolidated snapshot can predate the final count: keep the larger.
            let keep = merge
                || entry.usage[key].as_u64().unwrap_or(0) <= value.as_u64().unwrap_or(u64::MAX);
            if keep {
                entry.usage[key] = value.clone();
            }
        }
        if parent {
            self.last_parent = Some(id.into());
            if let Some(model) = model.as_str().filter(|m| *m != "<synthetic>") {
                self.parent_model = Some(model.into());
            }
        }
    }

    fn usage_update(&self) -> MappedUpdate {
        let tokens = |r: &Request| crate::usage::Tokens::parse(&r.usage);
        let last = self
            .last_parent
            .as_ref()
            .and_then(|id| self.requests.get(id))
            .and_then(tokens)
            .unwrap_or_default();
        let usage = if self.bills {
            let total = self
                .requests
                .values()
                .filter_map(tokens)
                .fold(crate::usage::Tokens::default(), |sum, t| sum.add(&t));
            json!({
                "source":"claude-native","model":self.parent_model,"total":total,"last":last,
                "cumulative":false,"modelContextWindow":self.window,
                "observedAt":pockymoe_protocol::now_rfc3339(),
                "requests":self.requests.iter().filter_map(|(id,r)| Some(json!({
                    "messageId":id,"model":r.model,"total":tokens(r)?,"last":tokens(r)?,
                    "parent":r.parent,"observedAt":r.observed_at,
                }))).collect::<Vec<_>>(),
            })
        } else {
            // The JSONL reader bills (exactly as on ACP); the stream gives the
            // live context occupancy only.
            match self.window {
                Some(size) => json!({"used":last.total_tokens,"size":size}),
                None => return MappedUpdate::default(),
            }
        };
        MappedUpdate {
            usage: Some(usage),
            ..Default::default()
        }
    }

    fn subagents_update(&self, mapped: &mut MappedUpdate) {
        let mut agents = self.inner.active_subagents();
        for agent in self.background.values() {
            if let Some(found) = agents.iter_mut().find(|a| a.id == agent.id) {
                *found = agent.clone();
            } else {
                agents.push(agent.clone());
            }
        }
        mapped.active_subagents = Some(agents);
    }

    fn notice(&mut self, severity: &str, title: &str, description: &str) -> MappedUpdate {
        self.inner.apply(&json!({"sessionUpdate":"notice","severity":severity,"title":title,"description":description}))
    }

    pub fn apply(&mut self, message: &Value) -> Vec<MappedUpdate> {
        let mut updates = Vec::new();
        let parent = message["parent_tool_use_id"].as_str();
        match message["type"].as_str() {
            Some("stream_event") => self.stream(message, parent, &mut updates),
            Some("assistant" | "user") => self.message(message, parent, &mut updates),
            Some("result") => {
                let models = message["modelUsage"].as_object();
                let model = self.parent_model.clone().unwrap_or_default();
                let window = models.and_then(|models| {
                    models
                        .iter()
                        .filter(|(name, _)| {
                            !model.is_empty()
                                && (model.starts_with(name.as_str()) || name.starts_with(&model))
                        })
                        .chain(models.iter())
                        .find_map(|(_, m)| m["contextWindow"].as_u64().filter(|w| *w > 0))
                });
                if window.is_some() {
                    self.window = window;
                }
                if !self.requests.is_empty() {
                    updates.push(self.usage_update());
                }
                if message["stop_reason"] == "refusal" || message["terminal_reason"] == "refusal" {
                    let explanation = message["result"]
                        .as_str()
                        .unwrap_or("Claude declined this request.");
                    updates.push(self.notice(
                        "warning",
                        "Claude declined this request",
                        explanation,
                    ));
                }
            }
            Some("tool_progress") => {
                if let Some(id) = message["tool_use_id"]
                    .as_str()
                    .filter(|id| self.tools.contains_key(*id))
                {
                    let mut update = self.tools[id].clone();
                    if update["status"] != "completed" && update["status"] != "failed" {
                        update["sessionUpdate"] = json!("tool_call_update");
                        update["status"] = json!("in_progress");
                        updates.push(self.inner.apply(&update));
                    }
                }
            }
            Some("system") => self.system(message, &mut updates),
            _ => {}
        }
        updates
    }

    fn stream(&mut self, message: &Value, parent: Option<&str>, updates: &mut Vec<MappedUpdate>) {
        let event = &message["event"];
        let key = parent.unwrap_or("").to_owned();
        match event["type"].as_str() {
            Some("message_start") => {
                let id = event["message"]["id"].as_str().unwrap_or("").to_owned();
                self.record_usage(
                    &id,
                    &event["message"]["model"],
                    &event["message"]["usage"],
                    parent.is_none(),
                    true,
                );
                self.streaming.insert(key, id);
                updates.push(self.usage_update());
            }
            Some("message_delta") => {
                if let Some(id) = self.streaming.get(&key).cloned() {
                    // Final counts: null fields keep the message_start values.
                    self.record_usage(&id, &Value::Null, &event["usage"], parent.is_none(), true);
                    updates.push(self.usage_update());
                }
            }
            Some("content_block_start") if parent.is_none() => {
                let block = &event["content_block"];
                let name = block["name"].as_str().unwrap_or("");
                if block["type"] == "tool_use" && !PLAN_TOOLS.contains(&name) {
                    if let Some(id) = block["id"]
                        .as_str()
                        .filter(|id| !self.tools.contains_key(*id))
                    {
                        // The call is visible as soon as Claude starts writing it.
                        let update = tool_call(name, id, &json!({}));
                        self.tools.insert(id.into(), update.clone());
                        updates.push(self.inner.apply(&update));
                    }
                }
            }
            Some("content_block_delta") if parent.is_none() => {
                let current = self.streaming.get(&key).cloned().unwrap_or_default();
                let delta = &event["delta"];
                let (kind, text) = match delta["type"].as_str() {
                    Some("text_delta") => ("agent_message_chunk", delta["text"].as_str()),
                    Some("thinking_delta") => ("agent_thought_chunk", delta["thinking"].as_str()),
                    _ => ("", None),
                };
                if let Some(text) = text.filter(|t| !t.is_empty()) {
                    self.streamed.insert((current.clone(), kind.into()));
                    updates.push(self.inner.apply(&json!({"sessionUpdate":kind,"messageId":current,"content":{"type":"text","text":text}})));
                }
            }
            _ => {}
        }
    }

    fn message(&mut self, message: &Value, parent: Option<&str>, updates: &mut Vec<MappedUpdate>) {
        let assistant = message["type"] == "assistant";
        let id = message["message"]["id"].as_str().unwrap_or("").to_owned();
        if assistant && parent.is_none() {
            if let Some(uuid) = message["uuid"].as_str() {
                self.uuids.insert(id.clone(), uuid.into());
            }
        }
        // The compaction summary arrives as a synthetic user message.
        if !assistant && self.compaction.is_some() && message["isSynthetic"] == true {
            if let Some(text) = message["message"]["content"].as_str() {
                let summary = text
                    .split_once("Summary:")
                    .map(|(_, s)| s)
                    .unwrap_or(text)
                    .trim();
                let compaction = self.compaction.take().unwrap_or_default();
                updates.push(self.inner.apply(&json!({"sessionUpdate":"compaction_update","compactionId":compaction,"status":"completed","summary":summary})));
            }
        }
        if !assistant {
            if let Some(text) = message["message"]["content"].as_str() {
                // `/cost`, `/context` and other local commands answer as stdout.
                if let Some(output) = text
                    .strip_prefix("<local-command-stdout>")
                    .and_then(|t| t.strip_suffix("</local-command-stdout>"))
                    .map(str::trim)
                    .filter(|t| !t.is_empty() && (!self.compacted || !t.starts_with("Compacted")))
                {
                    updates.push(self.inner.apply(&json!({"sessionUpdate":"agent_message_chunk","messageId":message["uuid"],"content":{"type":"text","text":output}})));
                }
            }
        }
        let results = message["message"]["content"]
            .as_array()
            .map(|blocks| blocks.iter().filter(|b| b["type"] == "tool_result").count())
            .unwrap_or(0);
        for block in message["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let update = match block["type"].as_str() {
                // A subagent's own words stay inside its Agent result, as on ACP.
                Some("text" | "thinking") if assistant && parent.is_none() => {
                    let (kind, key) = if block["type"] == "text" {
                        ("agent_message_chunk", "text")
                    } else {
                        ("agent_thought_chunk", "thinking")
                    };
                    if self.streamed.contains(&(id.clone(), kind.into()))
                        || block[key].as_str().is_none_or(str::is_empty)
                    {
                        continue;
                    }
                    json!({"sessionUpdate":kind,"messageId":id,"content":{"type":"text","text":block[key]}})
                }
                Some("tool_use") if assistant => {
                    let name = block["name"].as_str().unwrap_or("Tool");
                    let input = &block["input"];
                    let tool_id = block["id"].as_str().unwrap_or("");
                    if name == "TodoWrite" {
                        json!({"sessionUpdate":"plan", "entries":input["todos"].as_array().into_iter().flatten().map(|t|json!({
                            "content": if t["status"]=="in_progress" { t.get("activeForm").filter(|a| a.is_string()).unwrap_or(&t["content"]) } else { &t["content"] },
                            "status":t["status"],"priority":"medium"})).collect::<Vec<_>>()})
                    } else if PLAN_TOOLS.contains(&name) {
                        self.tools.insert(
                            tool_id.into(),
                            json!({"name":name,"rawInput":input,"plan":true}),
                        );
                        continue;
                    } else {
                        let update = tool_call(name, tool_id, input);
                        self.tools.insert(tool_id.into(), update.clone());
                        update
                    }
                }
                Some("tool_result") => {
                    let tool_id = block["tool_use_id"].as_str().unwrap_or("");
                    let structured = (results == 1).then_some(&message["tool_use_result"]);
                    let call = self.tools.get(tool_id).cloned();
                    if call.as_ref().is_some_and(|c| c["plan"] == true) {
                        if let Some(plan) =
                            self.task_plan(call.as_ref().unwrap(), block, structured)
                        {
                            updates.push(plan);
                        }
                        continue;
                    }
                    let mut update = tool_result(call, tool_id, block, structured);
                    if self.stopping && update["status"] == "failed" {
                        update["status"] = json!("interrupted");
                    }
                    self.tools.insert(tool_id.into(), update.clone());
                    update
                }
                _ => continue,
            };
            updates.push(self.inner.apply(&stamp(update, parent)));
        }
        if assistant && message["message"]["usage"].is_object() {
            let usage = message["message"]["usage"].clone();
            self.record_usage(
                &id,
                &message["message"]["model"],
                &usage,
                parent.is_none(),
                false,
            );
            updates.push(self.usage_update());
        }
        // TurnMapper drops an Agent at its launch result; a backgrounded one
        // still runs, so every subagent list carries the background ones too.
        for update in updates.iter_mut().filter(|u| u.active_subagents.is_some()) {
            self.subagents_update(update);
        }
    }

    fn task_plan(
        &mut self,
        call: &Value,
        block: &Value,
        structured: Option<&Value>,
    ) -> Option<MappedUpdate> {
        if block["is_error"] == true {
            return None;
        }
        let tool = call["name"].as_str().unwrap_or("");
        let input = &call["rawInput"];
        let result = structured.unwrap_or(&Value::Null);
        match tool {
            "TaskList" => {
                if let Some(tasks) = result["tasks"].as_array() {
                    self.tasks.clear();
                    for task in tasks {
                        if let Some(id) = task["id"].as_str() {
                            self.tasks.insert(id.into(), task.clone());
                        }
                    }
                }
            }
            "TaskCreate" | "TaskUpdate" => {
                let id = input["taskId"]
                    .as_str()
                    .or_else(|| result["task"]["id"].as_str())
                    .or_else(|| result["taskId"].as_str())?;
                if input["status"] == "deleted" {
                    self.tasks.remove(id);
                } else {
                    let task = self
                        .tasks
                        .entry(id.into())
                        .or_insert_with(|| json!({"subject":"Task","status":"pending"}));
                    for key in ["subject", "status", "activeForm"] {
                        if let Some(value) = input.get(key).or_else(|| result["task"].get(key)) {
                            task[key] = value.clone();
                        }
                    }
                }
            }
            _ => return None,
        }
        Some(self.inner.apply(&json!({"sessionUpdate":"plan","entries":self.tasks.values().map(|t|json!({
            "content": if t["status"]=="in_progress" { t.get("activeForm").filter(|a| a.is_string()).unwrap_or(&t["subject"]) } else { &t["subject"] },
            "status":t["status"],"priority":"medium"})).collect::<Vec<_>>() })))
    }

    fn system(&mut self, message: &Value, updates: &mut Vec<MappedUpdate>) {
        match message["subtype"].as_str() {
            Some("init") => {
                for server in message["mcp_servers"].as_array().into_iter().flatten() {
                    let name = server["name"].as_str().unwrap_or("MCP");
                    match server["status"].as_str() {
                        Some("failed") => updates.push(self.notice(
                            "warning",
                            &format!("MCP server \"{name}\" failed to start"),
                            "Its tools are unavailable in this session.",
                        )),
                        Some("needs-auth") => updates.push(self.notice(
                            "info",
                            &format!("MCP server \"{name}\" needs authentication"),
                            "Sign in to it from Claude Code to use its tools.",
                        )),
                        _ => {}
                    }
                }
            }
            Some("status") => {
                if message["status"] == "compacting" {
                    let id = message["uuid"].as_str().unwrap_or("compaction").to_owned();
                    updates.push(self.inner.apply(&json!({"sessionUpdate":"compaction_update","compactionId":id,"status":"in_progress","summary":"Compacting context…"})));
                    self.compaction = Some(id);
                } else if message["compact_result"] == "failed" {
                    let error = message["compact_error"]
                        .as_str()
                        .unwrap_or("Compaction failed");
                    self.compaction = None;
                    updates.push(self.notice("warning", "Context compaction failed", error));
                }
            }
            Some("compact_boundary") => {
                let meta = &message["compact_metadata"];
                self.compacted = true;
                let summary = format!(
                    "Context compacted{}: {} → {} tokens",
                    if meta["trigger"] == "auto" {
                        " automatically"
                    } else {
                        ""
                    },
                    meta["pre_tokens"].as_u64().unwrap_or(0),
                    meta["post_tokens"].as_u64().unwrap_or(0)
                );
                let id = self
                    .compaction
                    .clone()
                    .unwrap_or_else(|| message["uuid"].as_str().unwrap_or("compaction").into());
                // The summary text follows as a synthetic user message.
                self.compaction = Some(id.clone());
                updates.push(self.inner.apply(&json!({"sessionUpdate":"compaction_update","compactionId":id,"status":"completed","summary":summary})));
                if let (Some(size), Some(used)) = (self.window, meta["post_tokens"].as_u64()) {
                    updates.push(MappedUpdate {
                        usage: Some(json!({"used":used,"size":size})),
                        ..Default::default()
                    });
                }
            }
            Some("api_retry") => {
                let attempt = message["attempt"].as_u64().unwrap_or(0);
                let max = message["max_retries"].as_u64().unwrap_or(0);
                let status = message["error_status"].as_u64();
                let title = if status.is_none() {
                    format!("Reconnecting to Claude, attempt {attempt} of {max}")
                } else {
                    format!("Retrying Claude, attempt {attempt} of {max}")
                };
                let mut detail = match (status, message["error"].as_str()) {
                    (Some(code), Some(error)) => format!("{error} (HTTP {code})"),
                    (Some(code), None) => format!("HTTP {code}"),
                    (None, Some(error)) => error.to_owned(),
                    (None, None) => String::new(),
                };
                if let Some(delay) = message["retry_delay_ms"].as_u64() {
                    detail.push_str(&format!(
                        "{}next try in {:.0}s",
                        if detail.is_empty() { "" } else { "; " },
                        delay as f64 / 1000.0
                    ));
                }
                updates.push(self.notice("warning", &title, &detail));
            }
            Some("model_refusal_fallback") => {
                let original = message["original_model"].as_str().unwrap_or("The model");
                let fallback = message["fallback_model"]
                    .as_str()
                    .unwrap_or("another model");
                let outcome = if message["direction"] == "revert" || message["scope"] == "local" {
                    format!("The session stays on {original}.")
                } else {
                    format!("The session continues on {fallback}.")
                };
                let title =
                    format!("{original} declined this request; retried with {fallback}. {outcome}");
                let detail = message["api_refusal_explanation"].as_str().unwrap_or("");
                updates.push(self.notice("warning", &title, detail));
            }
            Some("informational")
                if message["level"] == "warning" || message["level"] == "notice" =>
            {
                let content = message["content"].as_str().unwrap_or("").trim();
                let (title, detail) = content.split_once('\n').unwrap_or((content, ""));
                updates.push(self.notice(
                    if message["level"] == "warning" {
                        "warning"
                    } else {
                        "info"
                    },
                    title,
                    detail.trim(),
                ));
            }
            Some("local_command_output") => {
                if let Some(text) = message["content"].as_str().filter(|t| !t.trim().is_empty()) {
                    updates.push(self.inner.apply(&json!({"sessionUpdate":"agent_message_chunk","messageId":message["uuid"],"content":{"type":"text","text":text}})));
                }
            }
            Some("permission_denied") => {
                if let Some(id) = message["tool_use_id"].as_str() {
                    if let Some(mut update) = self.tools.get(id).cloned() {
                        let reason = message["decision_reason"]
                            .as_str()
                            .or(message["message"].as_str())
                            .unwrap_or("denied");
                        update["sessionUpdate"] = json!("tool_call_update");
                        update["status"] = json!("failed");
                        update["content"] = json!([{"type":"content","content":{"type":"text","text":format!("Permission denied: {reason}")}}]);
                        self.tools.insert(id.into(), update.clone());
                        updates.push(self.inner.apply(&update));
                    }
                }
            }
            Some("memory_recall") => {
                let memories = message["memories"].as_array().cloned().unwrap_or_default();
                let title = if message["mode"] == "synthesize" {
                    "Recalled synthesized memory".to_owned()
                } else {
                    format!(
                        "Recalled {} {}",
                        memories.len(),
                        if memories.len() == 1 {
                            "memory"
                        } else {
                            "memories"
                        }
                    )
                };
                let id = message["uuid"].as_str().unwrap_or("memory");
                updates.push(self.inner.apply(&json!({"sessionUpdate":"tool_call","toolCallId":id,"name":"memory_recall","title":title,"kind":"read","status":"completed",
                    "locations":memories.iter().filter_map(|m| Some(json!({"path":m["path"].as_str()?}))).collect::<Vec<_>>()})));
            }
            Some("task_started") => {
                // A backgrounded Agent outlives its launch result: list it as a
                // running subagent until its task ends.
                if message["task_type"] == "local_agent" && message["is_backgrounded"] == true {
                    if let Some(tool) = message["tool_use_id"].as_str() {
                        if let Some(task) = message["task_id"].as_str() {
                            self.background_tasks.insert(task.into(), tool.into());
                        }
                        self.background.insert(
                            tool.into(),
                            ThreadSubagentDto {
                                id: tool.into(),
                                name: message["description"].as_str().map(str::to_owned),
                                status: "running".into(),
                                started_at: Some(pockymoe_protocol::now_rfc3339()),
                                completed_at: None,
                                parent_tool_call_id: Some(tool.into()),
                                is_background: Some(true),
                            },
                        );
                        let mut mapped = MappedUpdate::default();
                        self.subagents_update(&mut mapped);
                        updates.push(mapped);
                    }
                }
            }
            Some("task_notification" | "task_updated") => {
                let terminal = matches!(
                    message["status"]
                        .as_str()
                        .or(message["patch"]["status"].as_str()),
                    Some("completed" | "failed" | "stopped" | "killed" | "cancelled")
                );
                if terminal {
                    let tool = message["tool_use_id"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| {
                            self.background_tasks
                                .get(message["task_id"].as_str()?)
                                .cloned()
                        });
                    if let Some(tool) = tool {
                        if self.background.remove(&tool).is_some() {
                            let mut mapped = MappedUpdate::default();
                            self.subagents_update(&mut mapped);
                            updates.push(mapped);
                        }
                    }
                }
                if message["subtype"] == "task_notification" {
                    updates.push(self.inner.native_task_notification(message));
                }
            }
            _ => {}
        }
    }
}

/// A JSONL reader report as a mapped usage update.
pub(super) fn usage(usage: Value) -> MappedUpdate {
    MappedUpdate {
        usage: Some(usage),
        ..Default::default()
    }
}

fn stamp(mut value: Value, parent: Option<&str>) -> Value {
    if let Some(parent) = parent {
        value["_meta"] = json!({"claudeCode":{"parentToolUseId":parent}});
    }
    value
}

fn text_content(text: impl Into<String>) -> Value {
    json!({"type":"content","content":{"type":"text","text":text.into()}})
}

/// The tool row Claude's `tool_use` opens, as claude-agent-acp titles it.
fn tool_call(name: &str, id: &str, input: &Value) -> Value {
    let path = input["file_path"]
        .as_str()
        .or(input["notebook_path"].as_str())
        .or(input["path"].as_str());
    let mut content = Vec::new();
    let (kind, title) = match name {
        "Bash" | "PowerShell" => {
            if let Some(d) = input["description"].as_str() {
                content.push(text_content(d));
            }
            (
                "execute",
                input["command"].as_str().unwrap_or("Terminal").to_owned(),
            )
        }
        "Read" => {
            let range = match (input["offset"].as_u64(), input["limit"].as_u64()) {
                (offset, Some(limit)) if limit > 0 => {
                    let start = offset.unwrap_or(1).max(1);
                    format!(" ({start} - {})", start + limit - 1)
                }
                (Some(offset), _) => format!(" (from line {offset})"),
                _ => String::new(),
            };
            (
                "read",
                path.map_or("Read File".into(), |p| format!("Read {p}{range}")),
            )
        }
        "Write" => {
            if let (Some(p), Some(text)) = (path, input["content"].as_str()) {
                content.push(json!({"type":"diff","path":p,"oldText":null,"newText":text}));
            }
            (
                "edit",
                path.map_or("Preparing file…".into(), |p| format!("Write {p}")),
            )
        }
        "Edit" => {
            if let Some(p) = path {
                if input["old_string"].is_string() || input["new_string"].is_string() {
                    content.push(json!({"type":"diff","path":p,"oldText":input["old_string"],"newText":input["new_string"].as_str().unwrap_or("")}));
                }
            }
            ("edit", path.map_or("Edit".into(), |p| format!("Edit {p}")))
        }
        "MultiEdit" => {
            for edit in input["edits"].as_array().into_iter().flatten() {
                if let Some(p) = path {
                    content.push(json!({"type":"diff","path":p,"oldText":edit["old_string"],"newText":edit["new_string"].as_str().unwrap_or("")}));
                }
            }
            (
                "edit",
                path.map_or("MultiEdit".into(), |p| format!("Edit {p}")),
            )
        }
        "NotebookEdit" => (
            "edit",
            path.map_or("NotebookEdit".into(), |p| format!("Edit notebook {p}")),
        ),
        "Glob" => (
            "search",
            format!(
                "Find {}{}",
                input["path"]
                    .as_str()
                    .map(|p| format!("`{p}` "))
                    .unwrap_or_default(),
                input["pattern"]
                    .as_str()
                    .map(|p| format!("`{p}`"))
                    .unwrap_or_default()
            )
            .trim()
            .to_owned(),
        ),
        "Grep" => {
            let mut title = "grep".to_owned();
            if input["-i"] == true {
                title.push_str(" -i");
            }
            if input["output_mode"] == "files_with_matches" {
                title.push_str(" -l");
            } else if input["output_mode"] == "count" {
                title.push_str(" -c");
            }
            if let Some(p) = input["pattern"].as_str() {
                title.push_str(&format!(" \"{p}\""));
            }
            if let Some(p) = input["path"].as_str() {
                title.push_str(&format!(" {p}"));
            }
            ("search", title)
        }
        "WebFetch" => {
            if let Some(p) = input["prompt"].as_str() {
                content.push(text_content(p));
            }
            (
                "fetch",
                input["url"]
                    .as_str()
                    .map_or("Fetch".into(), |u| format!("Fetch {u}")),
            )
        }
        "WebSearch" => (
            "fetch",
            input["query"]
                .as_str()
                .map_or("Web search".into(), |q| format!("Search \"{q}\"")),
        ),
        "Agent" | "Task" => {
            if let Some(p) = input["prompt"].as_str() {
                content.push(text_content(p));
            }
            (
                "agent",
                input["description"].as_str().unwrap_or("Task").to_owned(),
            )
        }
        "ExitPlanMode" => {
            if let Some(p) = input["plan"].as_str() {
                content.push(text_content(p));
            }
            ("switch_mode", "Approve Plan".to_owned())
        }
        "EnterPlanMode" => ("switch_mode", "Enter plan mode".to_owned()),
        "AskUserQuestion" => {
            let questions: Vec<_> = input["questions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|q| q["question"].as_str())
                .collect();
            for q in &questions {
                content.push(text_content(*q));
            }
            let title = if questions.len() == 1 {
                questions[0].to_owned()
            } else {
                "Asking for your input".to_owned()
            };
            ("other", title)
        }
        "Skill" => (
            "other",
            input["skill"]
                .as_str()
                .map_or("Load skill".into(), |s| format!("Load skill: {s}")),
        ),
        _ => ("other", name.to_owned()),
    };
    let locations = match (name, path) {
        ("Read", Some(p)) => {
            vec![json!({"path":p,"line":input["offset"].as_u64().unwrap_or(1).max(1)})]
        }
        ("Write" | "Edit" | "MultiEdit" | "NotebookEdit", Some(p)) => vec![json!({"path":p})],
        ("Glob", Some(p)) => vec![json!({"path":p})],
        _ => Vec::new(),
    };
    json!({"sessionUpdate":"tool_call","toolCallId":id,"name":name,"title":title,"kind":kind,
        "status":"in_progress","rawInput":input,"content":content,"locations":locations,
        "_meta":{"claudeCode":{"toolName":name}}})
}

/// Plain text of a tool result's content (a string or content blocks).
fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| match b["type"].as_str() {
                Some("text") => b["text"].as_str().map(str::to_owned),
                Some("tool_reference") => b["tool_name"].as_str().map(|t| format!("Tool: {t}")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Result content blocks: text as text, images as images.
fn result_blocks(content: &Value, error: bool) -> Vec<Value> {
    let fence = |t: &str| {
        if error {
            format!("```\n{}\n```", t.trim_end())
        } else {
            t.to_owned()
        }
    };
    match content {
        Value::String(text) if !text.is_empty() => vec![text_content(fence(text))],
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| match b["type"].as_str() {
                Some("image") if b["source"]["type"] == "base64" => Some(json!({"type":"content","content":{
                    "type":"image","data":b["source"]["data"],"mimeType":b["source"]["media_type"]}})),
                _ => {
                    let text = result_text(&Value::Array(vec![b.clone()]));
                    (!text.is_empty()).then(|| text_content(fence(&text)))
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Apply Claude's structured patch hunks to the original file.
fn patched(original: &str, hunks: &Value) -> Option<String> {
    let lines: Vec<&str> = original.split_inclusive('\n').collect();
    let mut out = String::new();
    let mut at = 0usize;
    for hunk in hunks.as_array()? {
        let start = (hunk["oldStart"].as_u64()? as usize).saturating_sub(1);
        if start < at || start > lines.len() {
            return None;
        }
        out.extend(lines[at..start].iter().copied());
        at = start;
        for line in hunk["lines"].as_array()? {
            let line = line.as_str()?;
            let (tag, body) = line.split_at(line.len().min(1));
            match tag {
                " " => {
                    out.push_str(lines.get(at)?);
                    at += 1;
                }
                "-" => {
                    if lines.get(at)?.trim_end_matches(['\n', '\r']) != body {
                        return None;
                    }
                    at += 1;
                }
                "+" => {
                    out.push_str(body);
                    out.push('\n');
                }
                "\\" => {
                    // "\ No newline at end of file": the previous line had none.
                    if out.ends_with('\n') {
                        out.pop();
                    }
                }
                _ => return None,
            }
        }
    }
    out.extend(lines[at.min(lines.len())..].iter().copied());
    Some(out)
}

/// A finished tool row from Claude's `tool_result` and its structured result.
fn tool_result(call: Option<Value>, id: &str, block: &Value, structured: Option<&Value>) -> Value {
    let mut update = call.unwrap_or_else(|| json!({"toolCallId":id,"kind":"other"}));
    let name = update["name"].as_str().unwrap_or("").to_owned();
    let error = block["is_error"] == true;
    let content = &block["content"];
    let structured = structured.filter(|s| s.is_object());
    update["sessionUpdate"] = json!("tool_call_update");
    update["status"] = json!(if error { "failed" } else { "completed" });
    update["rawOutput"] = content.clone();
    update["content"] = json!(result_blocks(content, error));
    match name.as_str() {
        "Bash" | "PowerShell" if !error => {
            let output = structured
                .filter(|s| s["isImage"] != true && s["backgroundTaskId"].is_null())
                .map(|s| {
                    [s["stdout"].as_str(), s["stderr"].as_str()]
                        .into_iter()
                        .flatten()
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_else(|| result_text(content));
            let interrupted = structured.is_some_and(|s| s["interrupted"] == true);
            let mut text = output.trim_end().to_owned();
            if interrupted {
                text.push_str("\n[Command was aborted before completion]");
            }
            update["content"] = if text.trim().is_empty() {
                json!([])
            } else {
                json!([text_content(format!(
                    "```console\n{}\n```",
                    text.trim_start_matches('\n')
                ))])
            };
            // Native has the exit status ACP never forwards.
            update["rawOutput"] = json!({"formatted_output":text,"exit_code":if interrupted || structured.is_some_and(|s| s["backgroundTaskId"].is_string()) { Value::Null } else { json!(0) },
                "interrupted":interrupted,"backgroundTaskId":structured.map(|s| s["backgroundTaskId"].clone()).unwrap_or(Value::Null)});
        }
        "Bash" | "PowerShell" => {
            let text = result_text(content);
            let code = text
                .strip_prefix("Exit code ")
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|code| code.parse::<i64>().ok());
            update["rawOutput"] = json!({"formatted_output":text,"exit_code":code});
        }
        "Write" | "Edit" | "MultiEdit" if !error => {
            if let Some(s) = structured {
                let path = s["filePath"]
                    .as_str()
                    .or(update["rawInput"]["file_path"].as_str())
                    .unwrap_or("");
                let original = s["originalFile"].as_str();
                let new = if name == "Write" {
                    s["content"].as_str().map(str::to_owned)
                } else {
                    original.and_then(|o| patched(o, &s["structuredPatch"]))
                };
                // The whole-file before and after, exactly as Claude wrote it.
                match (original, new) {
                    (Some(old), Some(new)) => {
                        update["content"] =
                            json!([{"type":"diff","path":path,"oldText":old,"newText":new}])
                    }
                    (None, Some(new)) if s["type"] == "create" => {
                        update["content"] =
                            json!([{"type":"diff","path":path,"oldText":null,"newText":new}])
                    }
                    (None, _) if s["type"] == "update" => {
                        update["content"] = json!([text_content(format!(
                            "Updated `{path}` (previous content too large to diff)"
                        ))])
                    }
                    // Keep the input's own replacement snippet.
                    _ => {
                        update["content"] =
                            tool_call(&name, id, &update["rawInput"])["content"].clone()
                    }
                }
            } else {
                update["content"] = tool_call(&name, id, &update["rawInput"])["content"].clone();
            }
        }
        "Agent" | "Task" if !error => {
            let blocks = structured
                .filter(|s| s["status"] == "completed")
                .and_then(|s| s["content"].as_array().cloned())
                .map(Value::Array)
                .unwrap_or_else(|| content.clone());
            let text = result_text(&blocks)
                .lines()
                .filter(|l| !l.starts_with("agentId:"))
                .collect::<Vec<_>>()
                .join("\n");
            let text = match (text.find("<usage>"), text.find("</usage>")) {
                (Some(a), Some(b)) if b > a => format!("{}{}", &text[..a], &text[b + 8..]),
                _ => text,
            };
            update["content"] = json!([text_content(text.trim())]);
        }
        "ExitPlanMode" if !error => {
            update["title"] = json!("Exited Plan Mode");
            update["content"] = update["rawInput"]["plan"]
                .as_str()
                .map(|p| json!([text_content(p)]))
                .unwrap_or(json!([]));
        }
        _ => {}
    }
    update
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_apply_to_the_original_file() {
        let original = "alpha\nbeta\ngamma\n";
        let hunks = json!([{"oldStart":1,"oldLines":2,"newStart":1,"newLines":2,"lines":[" alpha","-beta","+BETA"]}]);
        assert_eq!(patched(original, &hunks).unwrap(), "alpha\nBETA\ngamma\n");
        let tail = json!([{"oldStart":3,"lines":[" gamma","+delta"]}]);
        assert_eq!(
            patched(original, &tail).unwrap(),
            "alpha\nbeta\ngamma\ndelta\n"
        );
        // A hunk that does not match the original is never guessed at.
        let wrong = json!([{"oldStart":1,"lines":["-nope"]}]);
        assert!(patched(original, &wrong).is_none());
        let no_newline = json!([{"oldStart":1,"lines":["-x","\\ No newline at end of file","+y","\\ No newline at end of file"]}]);
        assert_eq!(patched("x", &no_newline).unwrap(), "y");
    }

    #[test]
    fn bash_output_is_text_with_its_exit_status() {
        let call = tool_call("Bash", "b", &json!({"command":"ls","description":"List"}));
        assert_eq!(call["title"], "ls");
        let done = tool_result(
            Some(call.clone()),
            "b",
            &json!({"content":"a\nb"}),
            Some(&json!({"stdout":"a\nb","stderr":"","interrupted":false})),
        );
        assert_eq!(done["rawOutput"]["formatted_output"], "a\nb");
        assert_eq!(done["rawOutput"]["exit_code"], 0);
        assert_eq!(
            done["content"][0]["content"]["text"],
            "```console\na\nb\n```"
        );
        let failed = tool_result(
            Some(call),
            "b",
            &json!({"content":"Exit code 2\nboom","is_error":true}),
            None,
        );
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["rawOutput"]["exit_code"], 2);
    }
}
