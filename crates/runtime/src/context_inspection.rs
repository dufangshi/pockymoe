//! Opt-in context diagnostics. Raw prompts stay local; only counts leave the collector.
use crate::{actor::EventBus, db::Database};
use anyhow::Result;
use pockymoe_protocol::{now_rfc3339, ThreadEventEnvelope, ThreadEventType};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
};

const SETTINGS_KEY: &str = "context-inspection-settings";
pub struct InspectionState {
    enabled: AtomicBool,
    pub root: PathBuf,
    snapshots: Mutex<HashMap<String, Value>>,
}
impl InspectionState {
    pub fn new(db: &Database, database: &Path) -> Self {
        let enabled = db
            .with(|c| {
                Ok(
                    c.query_row("SELECT value FROM kv WHERE key=?1", [SETTINGS_KEY], |r| {
                        r.get::<_, String>(0)
                    })
                    .ok()
                    .and_then(|v| serde_json::from_str::<Value>(&v).ok())
                    .is_some_and(|v| v["enabled"] == true),
                )
            })
            .unwrap_or(false);
        Self {
            enabled: AtomicBool::new(enabled),
            root: database.with_extension("context-inspection"),
            snapshots: Default::default(),
        }
    }
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }
    pub fn settings(&self) -> Value {
        json!({"enabled":self.enabled()})
    }
    pub fn cached(&self, session: &str) -> Option<Value> {
        self.snapshots
            .lock()
            .unwrap()
            .get(session)
            .filter(|v| usable_snapshot(v))
            .cloned()
    }
    pub fn store(&self, session: &str, snapshot: Value) {
        self.snapshots
            .lock()
            .unwrap()
            .insert(session.into(), snapshot);
    }
}
/// Old collectors could persist raw media token estimates above native usage.
/// A separate composition is valid only when explicitly labeled as such.
pub fn usable_snapshot(snapshot: &Value) -> bool {
    if snapshot["source"] != "codexTrace" {
        return true;
    }
    let Some(used) = snapshot["usedTokens"].as_u64() else {
        return false;
    };
    let Some(categories) = snapshot["categories"].as_array() else {
        return false;
    };
    let sum = categories
        .iter()
        .filter(|v| v["kind"] == "used")
        .try_fold(0u64, |sum, v| sum.checked_add(v["tokens"].as_u64()?));
    sum.is_some_and(|sum| sum <= used || snapshot["compositionTokens"].as_u64() == Some(sum))
}
pub fn save_settings(db: &Database, enabled: bool) -> Result<()> {
    db.with(|c| {
        c.execute(
            "INSERT OR REPLACE INTO kv(key,value) VALUES(?1,?2)",
            rusqlite::params![SETTINGS_KEY, json!({"enabled":enabled}).to_string()],
        )?;
        Ok(())
    })
}
pub fn emit(state: &InspectionState, session: &str, thread: &str, bus: &EventBus, snapshot: Value) {
    if !state.enabled() {
        return;
    }
    state.store(session, snapshot.clone());
    let used = snapshot["usedTokens"].as_u64().unwrap_or(0);
    let size = snapshot["windowTokens"].as_u64().unwrap_or(0);
    if size == 0 {
        return;
    }
    bus.emit(ThreadEventEnvelope {event_type:ThreadEventType::ThreadContextUpdated,thread_id:thread.into(),timestamp:now_rfc3339(),
        payload:json!({"contextUsage":{"availability":"available","tokensInContextWindow":used,"modelContextWindow":size,
            "remainingPercent":(100.0*size.saturating_sub(used) as f64/size as f64).round(),"updatedAt":snapshot["updatedAt"],"details":snapshot}})});
}

pub fn claude_snapshot(raw: &Value, detail: &str) -> Option<Value> {
    let size = raw["rawMaxTokens"].as_u64().filter(|s| *s > 0)?;
    let used = raw["totalTokens"].as_u64()?;
    let categories:Vec<_>=raw["categories"].as_array()?.iter().filter_map(|v| {
        let kind=v["kind"].as_str().unwrap_or(if v["isDeferred"]==true {"deferred"} else {"used"});
        if !matches!(kind,"used"|"buffer"|"free"|"deferred") {return None;}
        let label=v["name"].as_str().unwrap_or("Other");
        let id=v["id"].as_str().map(str::to_owned).unwrap_or_else(|| label.to_lowercase().replace([' ','(',')'],"_"));
        Some(json!({"id":id,"label":label,"kind":kind,"tokens":v["tokens"].as_u64()?,"precision":if detail=="full" {"reported"}else{"estimated"}}))
    }).collect();
    let mut groups = Vec::new();
    for (key, id, label) in [
        ("mcpTools", "mcp_tools", "MCP tools"),
        ("memoryFiles", "memory_files", "Memory files"),
        ("agents", "custom_agents", "Agents"),
        ("mcpInstructions", "mcp_instructions", "MCP instructions"),
    ] {
        let items:Vec<_>=raw[key].as_array().into_iter().flatten().filter_map(|v| {
            Some(json!({"name":v["name"].as_str().or(v["path"].as_str()).or(v["agentType"].as_str()).or(v["serverName"].as_str())?,
                "tokens":v["tokens"].as_u64()?,"path":v["path"],"source":v["serverName"].as_str().or(v["source"].as_str()),"loaded":v["isLoaded"]}))
        }).collect();
        if !items.is_empty() {
            groups.push(json!({"id":id,"label":label,"items":items}));
        }
    }
    if let Some(skills) = raw
        .pointer("/skills/skillFrontmatter")
        .and_then(Value::as_array)
    {
        let items:Vec<_>=skills.iter().filter_map(|v|Some(json!({"name":v["name"].as_str()?,"tokens":v["tokens"].as_u64()?,"source":v["source"]}))).collect();
        if !items.is_empty() {
            groups.push(json!({"id":"skills","label":"Skills","items":items}));
        }
    }
    let mut snapshot = json!({"provider":"claude","model":raw["model"],"source":if detail=="full"{"claudeFull"}else{"claudeSummary"},
        "precision":if detail=="full"{"reported"}else{"estimated"},"usedTokens":used,"windowTokens":size,"updatedAt":now_rfc3339(),
        "autoCompactThreshold":raw["autoCompactThreshold"],"autoCompactEnabled":raw["isAutoCompactEnabled"],"categories":categories,"groups":groups});
    fn remove_nulls(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.retain(|_, v| !v.is_null());
                for v in m.values_mut() {
                    remove_nulls(v);
                }
            }
            Value::Array(a) => {
                for v in a {
                    remove_nulls(v);
                }
            }
            _ => {}
        }
    }
    remove_nulls(&mut snapshot);
    Some(snapshot)
}

/// DSH's occupancy is a provider-anchored projection; composition is a separate
/// native heuristic. Preserve their signed difference instead of rescaling it.
pub fn dsh_snapshot(raw: &Value) -> Option<Value> {
    let size = raw
        .pointer("/pressure/contextWindow")?
        .as_u64()
        .filter(|v| *v > 0)?;
    let used = raw.pointer("/pressure/projectedTokens")?.as_u64()?;
    let breakdown = &raw["breakdown"];
    let system = breakdown["systemTokens"].as_u64()?;
    let tools = breakdown["toolsTokens"].as_u64()?;
    let messages = breakdown["messageTokens"].as_u64()?;
    let composition = system.checked_add(tools)?.checked_add(messages)?;
    let mut totals = BTreeMap::from([
        ("system_prompt", system),
        ("tools", tools),
        ("messages", messages),
    ]);
    let mut groups: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    let nodes: Vec<_> = raw["nodes"].as_array().into_iter().flatten().collect();
    let current_system = nodes
        .iter()
        .rev()
        .find(|n| n["system"] == true && n["tokens"].as_u64().unwrap_or(0) > 0);
    let nodes_total = nodes
        .iter()
        .try_fold(0u64, |sum, n| sum.checked_add(n["tokens"].as_u64()?));
    // Refine only when these node prices match the native composition fold.
    // A future DSH change otherwise still exposes its authoritative 3 buckets.
    if nodes_total == system.checked_add(messages)
        && current_system.map_or(0, |n| n["tokens"].as_u64().unwrap_or(0)) == system
    {
        for node in &nodes {
            if current_system.is_some_and(|n| std::ptr::eq(*node, *n)) {
                continue;
            }
            let id = match node["kind"].as_str() {
                Some("agent-instructions") => "memory_files",
                Some("skill-catalog" | "skill-invocation") => "skills",
                Some("plugin")
                    if node["plugin"] == "@deepseek-ai/dsh-system-prompt"
                        && node["form"] == "snapshot" =>
                {
                    "runtime_context"
                }
                _ => continue,
            };
            let tokens = node["tokens"].as_u64().unwrap_or(0);
            if tokens == 0 {
                continue;
            }
            *totals.entry(id).or_default() += tokens;
            *totals.get_mut("messages").unwrap() -= tokens;
            let mut item = json!({"name":node["name"].as_str().unwrap_or(id),"tokens":tokens});
            if let Some(path) = node["path"].as_str() {
                item["path"] = json!(path);
            }
            groups.entry(id).or_default().push(item);
        }
    }
    for tool in raw["tools"].as_array().into_iter().flatten() {
        if let (Some(name), Some(tokens)) = (tool["name"].as_str(), tool["tokens"].as_u64()) {
            let mut item = json!({"name":name,"tokens":tokens});
            if let Some(server) = name
                .strip_prefix("mcp__")
                .and_then(|s| s.split_once("__").map(|p| p.0))
            {
                item["source"] = json!(format!("MCP · {server}"));
            }
            groups.entry("tools").or_default().push(item);
        }
    }
    let mut categories: Vec<_> = totals
        .into_iter()
        .map(|(id, tokens)| json!({"id":id,"kind":"used","tokens":tokens,"precision":"estimated"}))
        .collect();
    categories.push(json!({"id":"free_space","kind":"free","tokens":size.saturating_sub(used)}));
    let mut snapshot = json!({"provider":"deepseek","model":raw["model"],"source":"dshProjection","precision":"estimated",
        "usedTokens":used,"windowTokens":size,"updatedAt":now_rfc3339(),"stale":raw["stale"] == true,
        "compositionTokens":composition,"attributionDeltaTokens":i128::from(used)-i128::from(composition),
        "asOfSeq":raw["asOfSeq"],"route":raw["route"],"categories":categories,
        "groups":groups.into_iter().map(|(id,items)|json!({"id":id,"items":items})).collect::<Vec<_>>()});
    if let Some(enabled) = raw["autoCompactEnabled"].as_bool() {
        snapshot["autoCompactEnabled"] = json!(enabled);
    }
    if let Some(ratio) = raw["autoCompactRatio"]
        .as_f64()
        .filter(|v| *v > 0.0 && *v <= 1.0)
    {
        snapshot["autoCompactThreshold"] = json!((size as f64 * ratio).floor() as u64);
    }
    Some(snapshot)
}

fn tokenizer() -> &'static tiktoken_rs::CoreBPE {
    static BPE: OnceLock<tiktoken_rs::CoreBPE> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::o200k_base().expect("bundled tokenizer is valid"))
}
#[derive(Default)]
pub(crate) struct CodexAnalyzer {
    cache: HashMap<[u8; 32], u64>,
}
impl CodexAnalyzer {
    /// Count only readable content. Media and opaque provider state belong in
    /// the correlated native residual, never in a tokenizer's base64 input.
    fn content_tokens(&mut self, value: &Value) -> u64 {
        match value {
            Value::String(text) => self.count(text),
            Value::Array(values) => values.iter().map(|value| self.content_tokens(value)).sum(),
            Value::Object(object) => {
                match object.get("type").and_then(Value::as_str) {
                    Some(
                        "input_text" | "output_text" | "text" | "summary_text" | "reasoning_text",
                    )
                    | None => object
                        .get("text")
                        .and_then(Value::as_str)
                        .map(|text| self.count(text))
                        .unwrap_or(0),
                    // Unknown block types are deliberately not stringified.
                    _ => 0,
                }
            }
            _ => 0,
        }
    }

    fn tool(
        &mut self,
        tool: &Value,
        mcp: bool,
        totals: &mut BTreeMap<String, u64>,
        groups: &mut BTreeMap<String, Vec<Value>>,
    ) {
        let name = tool["name"]
            .as_str()
            .or(tool["type"].as_str())
            .unwrap_or("Tool");
        let mcp = mcp || name.starts_with("mcp__");
        if let Some(children) = tool["tools"].as_array() {
            let mut header = tool.clone();
            header.as_object_mut().unwrap().remove("tools");
            let id = if mcp { "mcp_tools" } else { "system_tools" };
            *totals.entry(id.into()).or_default() += self.count(&header.to_string());
            for child in children {
                self.tool(child, mcp, totals, groups);
            }
            return;
        }
        let deferred = tool["defer_loading"] == true;
        let id = match (mcp, deferred) {
            (true, true) => "mcp_tools_deferred",
            (true, false) => "mcp_tools",
            (false, true) => "system_tools_deferred",
            (false, false) => "system_tools",
        };
        let tokens = self.count(&tool.to_string());
        *totals.entry(id.into()).or_default() += tokens;
        groups
            .entry(id.into())
            .or_default()
            .push(json!({"name":name,"tokens":tokens,"loaded":!deferred}));
    }
    fn count(&mut self, text: &str) -> u64 {
        let key: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        if let Some(v) = self.cache.get(&key) {
            return *v;
        }
        let tokens = tokenizer().encode_ordinary(text).len() as u64;
        // Bound memory on long-lived sessions; cache entries contain hashes/counts only.
        if self.cache.len() >= 8192 {
            self.cache.clear();
        }
        self.cache.insert(key, tokens);
        tokens
    }
    pub(crate) fn analyze(&mut self, request: &Value, window: u64, model: &str) -> Option<Value> {
        // Incremental transport requires a logical-window reconstruction. Never display a delta as the full window.
        if request["previous_response_id"].is_string() {
            return None;
        }
        let mut totals: BTreeMap<String, u64> = BTreeMap::new();
        let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for tool in request["tools"].as_array().into_iter().flatten() {
            self.tool(tool, false, &mut totals, &mut groups);
        }
        for item in request["input"].as_array()? {
            match item["type"].as_str().unwrap_or("") {
                "additional_tools" => {
                    for tool in item["tools"].as_array().into_iter().flatten() {
                        self.tool(tool, false, &mut totals, &mut groups);
                    }
                }
                "message" => {
                    let parts = item["content"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                    for part in parts {
                        let cat = match part["text"].as_str() {
                            Some(t) if t.starts_with("<skills_instructions>") => "skills",
                            Some(t) if t.contains("<INSTRUCTIONS>") && t.len() < 64000 => {
                                "memory_files"
                            }
                            _ if matches!(item["role"].as_str(), Some("developer" | "system")) => {
                                "system_prompt"
                            }
                            _ => "messages",
                        };
                        let tokens = self.content_tokens(part);
                        if tokens > 0 {
                            *totals.entry(cat.into()).or_default() += tokens;
                        }
                    }
                }
                "reasoning" | "compaction" | "context_compaction" => {
                    // Only visible summaries/content count; content_tokens skips
                    // encrypted_content and raw media recursively.
                    let tokens = item
                        .get("summary")
                        .map(|value| self.content_tokens(value))
                        .unwrap_or_default()
                        .saturating_add(
                            item.get("content")
                                .map(|value| self.content_tokens(value))
                                .unwrap_or_default(),
                        );
                    if tokens > 0 {
                        *totals.entry("messages".into()).or_default() += tokens;
                    }
                }
                "custom_tool_call" | "function_call" => {
                    let tokens = item
                        .get("input")
                        .or_else(|| item.get("arguments"))
                        .map(|value| self.content_tokens(value))
                        .unwrap_or_default();
                    if tokens > 0 {
                        *totals.entry("messages".into()).or_default() += tokens;
                    }
                }
                "custom_tool_call_output" | "function_call_output" => {
                    let tokens = item
                        .get("output")
                        .map(|value| self.content_tokens(value))
                        .unwrap_or_default();
                    if tokens > 0 {
                        *totals.entry("messages".into()).or_default() += tokens;
                    }
                }
                "agent_message" => {
                    let tokens = self.content_tokens(&item["content"]);
                    if tokens > 0 {
                        *totals.entry("messages".into()).or_default() += tokens;
                    }
                }
                _ => {}
            }
        }
        let estimated: u64 = totals
            .iter()
            .filter(|(id, _)| !id.ends_with("_deferred"))
            .map(|(_, tokens)| *tokens)
            .sum();
        let used = estimated;
        let mut categories:Vec<_>=totals.into_iter().map(|(id,tokens)|json!({"kind":if id.ends_with("_deferred"){"deferred"}else{"used"},"id":id,"tokens":tokens,"precision":"estimated"})).collect();
        categories
            .push(json!({"id":"free_space","kind":"free","tokens":window.saturating_sub(used)}));
        Some(
            json!({"provider":"codex","model":model,"source":"codexTrace","precision":"estimated","usedTokens":used,
            "windowTokens":window,"updatedAt":now_rfc3339(),"categories":categories,"groups":groups.into_iter().map(|(id,items)|json!({"id":id,"items":items})).collect::<Vec<_>>(),
            "estimatedInputTokens":estimated}),
        )
    }
}

pub(crate) struct CodexTraceReader {
    root: PathBuf,
    session: String,
    offsets: HashMap<PathBuf, u64>,
    analyzer: CodexAnalyzer,
    latest: Option<Value>,
    context: Option<Value>,
    reported: Option<(String, u64, u64)>,
}
impl CodexTraceReader {
    pub fn new(root: PathBuf, session: String) -> Self {
        Self {
            root,
            session,
            offsets: Default::default(),
            analyzer: Default::default(),
            latest: None,
            context: None,
            reported: None,
        }
    }
    pub fn native_usage(&mut self, usage: &Value) {
        let window = usage["modelContextWindow"]
            .as_u64()
            .or_else(|| usage["size"].as_u64());
        if let Some(window) = window.filter(|v| *v > 0) {
            tracing::debug!(session=%self.session, window, "Context window observed");
            self.context = Some(json!({"modelContextWindow":window}));
        }
    }
    pub fn poll(&mut self) -> Vec<Value> {
        use std::io::{BufRead, BufReader, Seek, SeekFrom};
        let mut out = Vec::new();
        for entry in walkdir::WalkDir::new(&self.root)
            .max_depth(3)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_name() == "trace.jsonl")
        {
            let path = entry.path().to_path_buf();
            let offset = *self.offsets.get(&path).unwrap_or(&0);
            let Ok(mut file) = std::fs::File::open(&path) else {
                continue;
            };
            if file.seek(SeekFrom::Start(offset)).is_err() {
                continue;
            }
            let mut reader = BufReader::new(file);
            let mut position = offset;
            loop {
                let mut line = String::new();
                let Ok(n) = reader.read_line(&mut line) else {
                    break;
                };
                if n == 0 || !line.ends_with('\n') {
                    break;
                }
                position += n as u64;
                let Ok(event) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let payload = &event["payload"];
                if payload["type"] == "inference_started" {
                    tracing::debug!(session=%self.session,native=?payload["thread_id"],"Context request observed");
                }
                if payload["type"] == "compaction_installed"
                    && event["thread_id"].as_str() == Some(self.session.as_str())
                {
                    self.latest = None;
                    self.reported = None;
                }
                if payload["type"] == "inference_completed"
                    && event["thread_id"].as_str() == Some(self.session.as_str())
                {
                    if let (Some(call), Some(relative)) = (
                        payload["inference_call_id"].as_str(),
                        payload
                            .pointer("/response_payload/path")
                            .and_then(Value::as_str),
                    ) {
                        let relative = Path::new(relative);
                        if relative
                            .components()
                            .all(|c| matches!(c, std::path::Component::Normal(_)))
                        {
                            let response_path = path.parent().unwrap().join(relative);
                            if let Some(response) = std::fs::metadata(&response_path)
                                .ok()
                                .filter(|m| m.len() <= 16 * 1024 * 1024)
                                .and_then(|_| std::fs::read(&response_path).ok())
                                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                            {
                                if self.latest.as_ref().is_some_and(|s| s["requestId"] == call) {
                                    if let (Some(input), Some(total)) = (
                                        response
                                            .pointer("/token_usage/input_tokens")
                                            .and_then(Value::as_u64),
                                        response
                                            .pointer("/token_usage/total_tokens")
                                            .and_then(Value::as_u64),
                                    ) {
                                        self.reported = Some((call.into(), input, total));
                                        if let Some(snapshot) = self.latest.as_mut() {
                                            snapshot["updatedAt"] = json!(now_rfc3339());
                                        }
                                        if let Some(snapshot) = self.calibrated_snapshot() {
                                            out.push(snapshot);
                                        }
                                    }
                                }
                            }
                            let _ = std::fs::remove_file(response_path);
                        }
                    }
                }
                if payload["type"] != "inference_started"
                    || payload["thread_id"].as_str() != Some(self.session.as_str())
                {
                    // Discard diagnostic tool/output payloads after their event becomes durable.
                    for value in payload.as_object().into_iter().flat_map(|m| m.values()) {
                        if value["raw_payload_id"].is_string() {
                            if let Some(relative) =
                                value["path"].as_str().map(Path::new).filter(|p| {
                                    p.starts_with("payloads")
                                        && p.components()
                                            .all(|c| matches!(c, std::path::Component::Normal(_)))
                                })
                            {
                                let _ = std::fs::remove_file(path.parent().unwrap().join(relative));
                            }
                        }
                    }
                    continue;
                }
                // Every request invalidates correlation with the preceding
                // response, including deltas, unreadable/oversized payloads
                // and failed calls. None may revive an old calibrated reading.
                self.latest = None;
                self.reported = None;
                let Some(relative) = payload
                    .pointer("/request_payload/path")
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let relative = Path::new(relative);
                if relative
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
                {
                    continue;
                }
                let request_path = path.parent().unwrap().join(relative);
                let request = std::fs::metadata(&request_path)
                    .ok()
                    .filter(|m| m.len() <= 16 * 1024 * 1024)
                    .and_then(|_| std::fs::read(&request_path).ok())
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
                if let Some(request) = request {
                    let window = self
                        .context
                        .as_ref()
                        .and_then(|v| v["modelContextWindow"].as_u64())
                        .unwrap_or(0);
                    if let Some(mut snapshot) = self.analyzer.analyze(
                        &request,
                        window,
                        payload["model"].as_str().unwrap_or(""),
                    ) {
                        snapshot["turnId"] = payload["codex_turn_id"].clone();
                        snapshot["requestId"] = payload["inference_call_id"].clone();
                        self.latest = Some(snapshot);
                        // Do not publish this pre-response estimate. Codex can
                        // report a very different native input total after the
                        // request completes; publishing both values makes the
                        // UI oscillate between two context readings. The
                        // calibrated snapshot below is the first visible one.
                    }
                }
                // Requests contain private context and are unnecessary after aggregation.
                let _ = std::fs::remove_file(request_path);
            }
            self.offsets.insert(path, position);
        }
        out
    }
    pub fn calibrated_snapshot(&mut self) -> Option<Value> {
        let mut snapshot = self.latest.clone()?;
        let context = self.context.as_ref()?;
        let (call, _input, total) = self.reported.as_ref()?;
        if snapshot["requestId"] != call.as_str() {
            return None;
        }
        let total = *total;
        let window = context["modelContextWindow"].as_u64()?;
        let estimated = snapshot["estimatedInputTokens"].as_u64()?;
        let mut categories = snapshot["categories"]
            .as_array()?
            .iter()
            .filter(|v| v["id"] != "free_space" && v["id"] != "unattributed")
            .cloned()
            .collect::<Vec<_>>();
        if total >= estimated {
            categories.push(json!({"id":"unattributed","kind":"used","tokens":total-estimated,"precision":"estimated"}));
        } else {
            // Keep the signed discrepancy explicit, as with DSH. Do not invent
            // negative categories or spread a tokenizer mismatch over messages.
            snapshot["compositionTokens"] = json!(estimated);
            snapshot["attributionDeltaTokens"] = json!(i128::from(total) - i128::from(estimated));
        }
        categories
            .push(json!({"id":"free_space","kind":"free","tokens":window.saturating_sub(total)}));
        snapshot["categories"] = json!(categories);
        snapshot["usedTokens"] = json!(total);
        snapshot["windowTokens"] = json!(window);
        self.latest = Some(snapshot.clone());
        Some(snapshot)
    }
    pub fn matching_native_snapshot(&mut self, usage: &Value) -> Option<Value> {
        let native = crate::usage::context_usage(usage, "")?;
        let (_, _, total) = self.reported.as_ref()?;
        if native["tokensInContextWindow"].as_u64()? != *total {
            return None;
        }
        self.calibrated_snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dsh_preserves_native_occupancy_and_signed_composition_difference() {
        let mut raw = json!({"pressure":{"projectedTokens":200,"contextWindow":1000},
            "breakdown":{"systemTokens":10,"toolsTokens":40,"messageTokens":50},
            "asOfSeq":8,"model":"m","stale":false,"autoCompactEnabled":true,"autoCompactRatio":0.75,
            "nodes":[{"seq":1,"system":true,"tokens":10},{"seq":2,"kind":"agent-instructions","tokens":20,"name":"AGENTS.md","path":"/repo/AGENTS.md"},{"seq":3,"kind":"skill-catalog","tokens":5,"name":"skill-catalog"},{"seq":4,"kind":"plugin","plugin":"@deepseek-ai/dsh-system-prompt","form":"snapshot","tokens":5,"name":"sandbox:policy"},{"seq":7,"tokens":20}],
            "tools":[{"name":"mcp__docs__read","tokens":41}]});
        let s = dsh_snapshot(&raw).unwrap();
        assert_eq!(s["usedTokens"], 200);
        assert_eq!(s["compositionTokens"], 100);
        assert_eq!(s["attributionDeltaTokens"], 100);
        assert_eq!(s["autoCompactThreshold"], 750);
        let categories = s["categories"].as_array().unwrap();
        assert_eq!(
            categories
                .iter()
                .filter(|v| v["kind"] == "used")
                .map(|v| v["tokens"].as_u64().unwrap())
                .sum::<u64>(),
            100
        );
        assert!(categories
            .iter()
            .any(|v| v["id"] == "messages" && v["tokens"] == 20));
        assert!(categories
            .iter()
            .any(|v| v["id"] == "memory_files" && v["tokens"] == 20));
        raw["pressure"]["projectedTokens"] = json!(80);
        raw["stale"] = json!(true);
        let s = dsh_snapshot(&raw).unwrap();
        assert_eq!(s["usedTokens"], 80);
        assert_eq!(s["attributionDeltaTokens"], -20);
        assert_eq!(s["stale"], true);
        assert!(!s["categories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == "unattributed"));
        raw["nodes"] = json!([]);
        let s = dsh_snapshot(&raw).unwrap();
        assert_eq!(s["categories"].as_array().unwrap().len(), 4);
        raw["pressure"] = json!({"pressureTokens":200,"contextWindow":1000});
        assert!(dsh_snapshot(&raw).is_none());
    }
    #[test]
    fn claude_deferred_is_not_occupancy_and_absent_diagnostics_stay_absent() {
        let s=claude_snapshot(&json!({"rawMaxTokens":1000,"totalTokens":100,"categories":[{"id":"messages","kind":"used","tokens":100},{"id":"system_tools_deferred","kind":"deferred","tokens":300}],"systemTools":null}),"full").unwrap();
        assert_eq!(s["usedTokens"], 100);
        assert_eq!(s["categories"][1]["kind"], "deferred");
        assert_eq!(s["groups"], json!([]));
    }
    #[test]
    fn codex_reuses_content_but_never_treats_transport_delta_as_full_context() {
        let mut a = CodexAnalyzer::default();
        let r =
            json!({"input":[{"type":"message","role":"user","content":[{"text":"hello world"}]}]});
        let first = a.analyze(&r, 1000, "m").unwrap();
        let second = a.analyze(&r, 1000, "m").unwrap();
        assert_eq!(second["usedTokens"], first["usedTokens"]);
        assert_eq!(a.cache.len(), 1);
        assert!(a
            .analyze(&json!({"previous_response_id":"r","input":[]}), 1000, "m")
            .is_none());
    }
    #[test]
    fn inspection_is_opt_in_and_setting_survives_database_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        {
            let db = Database::open(&path).unwrap();
            let state = InspectionState::new(&db, &path);
            assert!(!state.enabled());
            save_settings(&db, true).unwrap();
        }
        let db = Database::open(&path).unwrap();
        assert!(InspectionState::new(&db, &path).enabled());
    }
    #[test]
    fn codex_deferred_schemas_and_opaque_reasoning_do_not_inflate_occupancy() {
        let mut analyzer = CodexAnalyzer::default();
        let snapshot=analyzer.analyze(&json!({"tools":[{"type":"function","name":"eager"},{"type":"function","name":"later","defer_loading":true}],"input":[{"type":"reasoning","encrypted_content":"private-ciphertext"}]}),1000,"m").unwrap();
        let categories = snapshot["categories"].as_array().unwrap();
        assert_eq!(
            categories
                .iter()
                .filter(|v| v["kind"] == "used")
                .map(|v| v["tokens"].as_u64().unwrap())
                .sum::<u64>(),
            snapshot["usedTokens"].as_u64().unwrap()
        );
        assert!(categories
            .iter()
            .any(|v| v["id"] == "system_tools_deferred" && v["kind"] == "deferred"));
        assert!(!snapshot.to_string().contains("private-ciphertext"));
    }
    #[test]
    fn codex_media_bytes_and_encrypted_reasoning_are_not_counted_as_text() {
        let mut analyzer = CodexAnalyzer::default();
        let huge = format!("data:image/png;base64,{}", "A".repeat(2_000_000));
        let snapshot = analyzer
            .analyze(
                &json!({
                    "input":[
                        {"type":"message","role":"user","content":[{"type":"input_image","image_url":huge}]},
                        {"type":"reasoning","encrypted_content":"opaque"},
                        {"type":"custom_tool_call_output","output":[{"type":"input_image","image_url":huge}]}
                    ]
                }),
                828_400,
                "m",
            )
            .unwrap();
        assert_eq!(snapshot["estimatedInputTokens"], 0);
        assert!(!snapshot.to_string().contains("data:image"));
        assert!(!snapshot.to_string().contains("opaque"));
    }
    #[test]
    fn codex_keeps_tokenizer_discrepancy_separate_from_native_occupancy() {
        let dir = tempfile::tempdir().unwrap();
        let mut reader = CodexTraceReader::new(dir.path().into(), "session".into());
        reader.context = Some(json!({"modelContextWindow":1000}));
        reader.latest = Some(
            json!({"provider":"codex","model":"m","source":"codexTrace","precision":"estimated",
            "estimatedInputTokens":900,"requestId":"call","categories":[
                {"id":"system_prompt","kind":"used","tokens":100,"precision":"estimated"},
                {"id":"messages","kind":"used","tokens":800,"precision":"estimated"}],"groups":[]}),
        );
        reader.reported = Some(("call".into(), 100, 100));
        let snapshot = reader.calibrated_snapshot().unwrap();
        assert_eq!(snapshot["usedTokens"], 100);
        assert_eq!(snapshot["compositionTokens"], 900);
        assert_eq!(snapshot["attributionDeltaTokens"], -800);
        assert_eq!(
            snapshot["categories"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|v| v["kind"] == "used")
                .map(|v| v["tokens"].as_u64().unwrap())
                .sum::<u64>(),
            900
        );
    }
    #[test]
    fn trace_correlates_response_and_discards_private_payloads() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("bundle");
        std::fs::create_dir_all(bundle.join("payloads")).unwrap();
        let request = json!({"input":[{"type":"message","role":"user","content":[{"text":"PRIVATE PROMPT CONTENT"}]}]});
        std::fs::write(bundle.join("payloads/1.json"), request.to_string()).unwrap();
        std::fs::write(
            bundle.join("payloads/2.json"),
            json!({"token_usage":{"input_tokens":100,"total_tokens":110},"output_items":[]})
                .to_string(),
        )
        .unwrap();
        let events = [
            json!({"thread_id":"session","payload":{"type":"inference_started","thread_id":"session","inference_call_id":"call","codex_turn_id":"turn","model":"m","request_payload":{"path":"payloads/1.json"}}}),
            json!({"thread_id":"session","payload":{"type":"inference_completed","inference_call_id":"call","response_payload":{"path":"payloads/2.json"}}}),
        ];
        let trace = bundle.join("trace.jsonl");
        std::fs::write(&trace, format!("{}\n", events[0])).unwrap();
        let mut reader = CodexTraceReader::new(dir.path().into(), "session".into());
        reader.native_usage(&json!({"modelContextWindow":1000}));
        assert!(
            reader.poll().is_empty(),
            "request-start estimates must never update occupancy"
        );
        assert!(reader.calibrated_snapshot().is_none());
        std::fs::OpenOptions::new()
            .append(true)
            .open(&trace)
            .unwrap()
            .write_all(format!("{}\n", events[1]).as_bytes())
            .unwrap();
        let snapshots = reader.poll();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0]["usedTokens"], 110);
        assert!(!snapshots[0].to_string().contains("PRIVATE"));
        assert!(!bundle.join("payloads/1.json").exists());
        assert!(!bundle.join("payloads/2.json").exists());
        assert!(reader.poll().is_empty());
        let timestamp = snapshots[0]["updatedAt"].clone();
        assert_eq!(
            reader
                .matching_native_snapshot(&json!({"size":1000,"used":110}))
                .unwrap()["updatedAt"],
            timestamp
        );
        assert!(
            reader
                .matching_native_snapshot(&json!({"size":1000,"used":60}))
                .is_none(),
            "a new native total must not revive an old trace count"
        );
        // A delta does not describe a full context and must invalidate the
        // preceding request/response association instead of re-emitting it.
        std::fs::write(
            bundle.join("payloads/delta.json"),
            json!({"previous_response_id":"response","input":[]}).to_string(),
        )
        .unwrap();
        std::fs::OpenOptions::new().append(true).open(&trace).unwrap()
            .write_all(format!("{}\n",json!({"payload":{"type":"inference_started","thread_id":"session","inference_call_id":"delta","request_payload":{"path":"payloads/delta.json"}}})).as_bytes()).unwrap();
        assert!(reader.poll().is_empty());
        assert!(reader.calibrated_snapshot().is_none());
        assert!(reader
            .matching_native_snapshot(&json!({"size":1000,"used":110}))
            .is_none());
    }
}
