use crate::acp::mapper::{MappedUpdate, TurnMapper};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub(super) struct Mapper {
    pub inner: TurnMapper,
    messages: HashMap<String, String>,
    uuids: HashMap<String, String>,
    streamed: HashSet<(String, String)>,
    tools: HashMap<String, Value>,
    usage: HashMap<String, Value>,
    last_parent: Value,
    tasks: std::collections::BTreeMap<String, Value>,
    /// Write tool uses' previous file contents, from the PreToolUse hook.
    write_previews: HashMap<String, (String, String)>,
}
impl Mapper {
    pub fn new(turn: &str) -> Self {
        Self {
            inner: TurnMapper::new(turn),
            messages: HashMap::new(),
            uuids: HashMap::new(),
            streamed: HashSet::new(),
            tools: HashMap::new(),
            usage: HashMap::new(),
            last_parent: Value::Null,
            tasks: Default::default(),
            write_previews: HashMap::new(),
        }
    }
    pub fn write_preview(&mut self, tool_use: &str, path: &str, previous: String) {
        self.write_previews
            .insert(tool_use.into(), (path.into(), previous));
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
    fn usage_update(&self, window: Option<u64>) -> MappedUpdate {
        let mut total = crate::usage::Tokens::default();
        for message in self.usage.values() {
            if let Some(tokens) = crate::usage::Tokens::parse(&message["usage"]) {
                total = total.add(&tokens);
            }
        }
        let last = crate::usage::Tokens::parse(&self.last_parent["usage"]).unwrap_or_default();
        MappedUpdate {
            usage: Some(
                json!({"source":"claude-native","model":self.last_parent["model"],"total":total,"last":last,"cumulative":false,"modelContextWindow":window,"requests":self.usage.iter().map(|(id,m)|json!({"id":id,"model":m["model"],"total":crate::usage::Tokens::parse(&m["usage"])})).collect::<Vec<_>>() }),
            ),
            ..Default::default()
        }
    }
    pub fn apply(&mut self, message: &Value) -> Vec<MappedUpdate> {
        let mut updates = Vec::new();
        let parent = message["parent_tool_use_id"].as_str();
        if message["type"] == "stream_event" {
            let event = &message["event"];
            if event["type"] == "message_start" {
                self.messages.insert(
                    parent.unwrap_or("").into(),
                    event["message"]["id"].as_str().unwrap_or("").into(),
                );
            }
            if event["type"] == "content_block_delta" {
                let current = self
                    .messages
                    .get(parent.unwrap_or(""))
                    .cloned()
                    .unwrap_or_default();
                let delta = &event["delta"];
                let (kind, text) = match delta["type"].as_str() {
                    Some("text_delta") => ("agent_message_chunk", delta["text"].as_str()),
                    Some("thinking_delta") => ("agent_thought_chunk", delta["thinking"].as_str()),
                    _ => ("", None),
                };
                if let Some(text) = text {
                    self.streamed.insert((current.clone(), kind.into()));
                    updates.push(self.inner.apply(&stamp(json!({"sessionUpdate":kind,"messageId":current,"content":{"type":"text","text":text}}), parent)));
                }
            }
        }
        if matches!(message["type"].as_str(), Some("assistant" | "user")) {
            let id = message["message"]["id"].as_str().unwrap_or("");
            if let Some(uuid) = message["uuid"].as_str() {
                self.uuids.insert(id.into(), uuid.into());
            }
            for block in message["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let update = match block["type"].as_str() {
                    Some("text" | "thinking") if message["type"] == "assistant" => {
                        let (kind, key) = if block["type"] == "text" {
                            ("agent_message_chunk", "text")
                        } else {
                            ("agent_thought_chunk", "thinking")
                        };
                        if self.streamed.contains(&(id.into(), kind.into())) {
                            continue;
                        }
                        json!({"sessionUpdate":kind,"messageId":id,"content":{"type":"text","text":block[key]}})
                    }
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or("Tool");
                        let input = &block["input"];
                        if name == "TodoWrite" {
                            json!({"sessionUpdate":"plan", "entries":input["todos"].as_array().into_iter().flatten().map(|t|json!({"content":t["content"],"status":t["status"],"priority":"medium"})).collect::<Vec<_>>()})
                        } else {
                            let kind = match name {
                                "Bash" => "execute",
                                "Read" | "Glob" | "Grep" => "read",
                                "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => "edit",
                                "WebFetch" | "WebSearch" => "fetch",
                                _ => "other",
                            };
                            let update = json!({"sessionUpdate":"tool_call","toolCallId":block["id"],"name":name,"title":name,"kind":kind,"status":"in_progress","rawInput":input,"locations":input["file_path"].as_str().map(|p|vec![json!({"path":p})]).unwrap_or_default()});
                            self.tools
                                .insert(block["id"].as_str().unwrap_or("").into(), update.clone());
                            update
                        }
                    }
                    Some("tool_result") => {
                        let id = block["tool_use_id"].as_str().unwrap_or("");
                        let mut update = self
                            .tools
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| json!({"toolCallId":id}));
                        update["sessionUpdate"] = json!("tool_call_update");
                        update["status"] = json!(if block["is_error"] == true {
                            "failed"
                        } else {
                            "completed"
                        });
                        let tool = update["name"].as_str().unwrap_or("").to_owned();
                        if matches!(tool.as_str(), "TaskCreate" | "TaskUpdate" | "TaskList")
                            && block["is_error"] != true
                        {
                            let result = &message["tool_use_result"];
                            if tool == "TaskList" {
                                if let Some(tasks) = result["tasks"].as_array() {
                                    for task in tasks {
                                        if let Some(id) = task["id"].as_str() {
                                            self.tasks.insert(id.into(), task.clone());
                                        }
                                    }
                                }
                            } else {
                                let input = &update["rawInput"];
                                let id = input["taskId"]
                                    .as_str()
                                    .or_else(|| result["task"]["id"].as_str())
                                    .or_else(|| result["taskId"].as_str());
                                if let Some(id) = id {
                                    if input["status"] == "deleted" {
                                        self.tasks.remove(id);
                                    } else {
                                        let task = self.tasks.entry(id.into()).or_insert_with(
                                            || json!({"subject":"Task","status":"pending"}),
                                        );
                                        for key in ["subject", "status"] {
                                            if let Some(value) =
                                                input.get(key).or_else(|| result["task"].get(key))
                                            {
                                                task[key] = value.clone();
                                            }
                                        }
                                    }
                                }
                            }
                            updates.push(self.inner.apply(&json!({"sessionUpdate":"plan","entries":self.tasks.values().map(|t|json!({"content":t["subject"],"status":t["status"],"priority":"medium"})).collect::<Vec<_>>() })));
                        }
                        update["rawOutput"] = block["content"].clone();
                        update["content"] = if let Some(text) = block["content"].as_str() {
                            json!([{"type":"content","content":{"type":"text","text":text}}])
                        } else {
                            json!([{"type":"content","content":{"type":"text","text":block["content"].to_string()}}])
                        };
                        // Edit's exact replacement snippet is a truthful local
                        // diff. A Write is diffed against the contents the
                        // PreToolUse hook read just before it ran (empty for a
                        // new file); without that reading no diff is invented.
                        if tool == "Write" && block["is_error"] != true {
                            let id = block["tool_use_id"].as_str().unwrap_or("");
                            if let (Some((path, old)), Some(new)) = (
                                self.write_previews.remove(id),
                                update["rawInput"]["content"].as_str(),
                            ) {
                                update["content"] = json!([{"type":"diff","path":path,"oldText":old,"newText":new}]);
                            }
                        }
                        if tool == "Edit" && block["is_error"] != true {
                            let input = &update["rawInput"];
                            if let (Some(path), Some(old), Some(new)) = (
                                input["file_path"].as_str(),
                                input["old_string"].as_str(),
                                input["new_string"].as_str(),
                            ) {
                                update["content"] = json!([{"type":"diff","path":path,"oldText":old,"newText":new}]);
                            }
                        }
                        update
                    }
                    _ => continue,
                };
                updates.push(self.inner.apply(&stamp(update, parent)));
            }
            if message["type"] == "assistant" && message["message"]["usage"].is_object() {
                self.usage.insert(id.into(), message["message"].clone());
                if parent.is_none() {
                    self.last_parent = message["message"].clone();
                }
                updates.push(self.usage_update(None));
            }
        }
        if message["type"] == "result" && !self.usage.is_empty() {
            let window = message["modelUsage"]
                .as_object()
                .and_then(|models| models.values().find_map(|m| m["contextWindow"].as_u64()));
            updates.push(self.usage_update(window));
        }
        if message["type"] == "system" {
            match message["subtype"].as_str() {
                Some("compact_boundary") => updates.push(self.inner.apply(&json!({"sessionUpdate":"notice","title":"Context compacted","description":"Claude compacted the session context."}))),
                Some("task_notification") => updates.push(self.inner.native_task_notification(message)),
                _ => {}
            }
        }
        updates
    }
}
fn stamp(mut value: Value, parent: Option<&str>) -> Value {
    if let Some(parent) = parent {
        value["_meta"] = json!({"claudeCode":{"parentToolUseId":parent}});
    }
    value
}
