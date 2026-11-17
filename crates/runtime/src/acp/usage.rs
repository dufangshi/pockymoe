//! Codex ACP currently forwards context occupancy live and only the final API
//! request's tokens at completion. Tail its local rollout for complete turn usage,
//! and the rollouts of the sub-agents the turn starts: their requests are billed
//! like the parent's and appear in no report of the parent session.
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::local_sessions::LocalSessionHomes;
use crate::usage::Tokens;

/// Adapter deltas and final snapshots describe the same prompt. Reconcile them
/// before persistence so retries and final reports never bill an API call twice.
#[derive(Default)]
pub(super) struct AdapterUsageAccumulator {
    total: Tokens,
    last: Option<Tokens>,
    final_received: bool,
    reports: std::collections::HashSet<String>,
}

impl AdapterUsageAccumulator {
    pub fn apply(&mut self, mut report: Value) -> Option<Value> {
        let tokens = report.get("total").and_then(Tokens::parse)?;
        if report["reportKind"] == "delta" {
            if self.final_received {
                return None;
            }
            if let Some(id) = report["reportId"].as_str() {
                if !self.reports.insert(id.to_string()) {
                    return None;
                }
            }
            self.total = self.total.add(&tokens);
            self.last = Some(tokens);
        } else {
            self.final_received = true;
            self.total = tokens;
        }
        report["total"] = json!(self.total);
        report["last"] = json!(self.last.as_ref().unwrap_or(&self.total));
        report["cumulative"] = json!(false);
        if let Some(object) = report.as_object_mut() {
            object.remove("reportId");
            object.remove("reportKind");
        }
        Some(report)
    }
}

#[derive(Default)]
struct RolloutBaseline {
    offset: u64,
    tokens: Option<Tokens>,
    context: Option<Value>,
}

fn usage_record(line: &str) -> Option<Value> {
    if !line.contains("\"token_count\"")
        && !line.contains("\"turn_context\"")
        && !line.contains("\"SubAgentActivity\"")
    {
        return None;
    }
    serde_json::from_str(line).ok()
}

// Read backwards only as far as the latest usage and model context. Long-lived
// sessions can contain hundreds of megabytes of older tool output.
fn read_baseline_tail(reader: &mut (impl Read + Seek)) -> std::io::Result<RolloutBaseline> {
    let mut result = RolloutBaseline::default();
    let mut position = reader.seek(SeekFrom::End(0))?;
    let mut block = [0; 64 * 1024];
    let mut reversed_line = Vec::new();
    let mut found_complete_end = false;
    let inspect = |line: &mut Vec<u8>, result: &mut RolloutBaseline| {
        line.reverse();
        if let Some(entry) = std::str::from_utf8(line).ok().and_then(usage_record) {
            if result.context.is_none() && entry["type"] == "turn_context" {
                result.context = Some(entry);
            } else if result.tokens.is_none()
                && entry["type"] == "event_msg"
                && entry["payload"]["type"] == "token_count"
            {
                result.tokens = entry["payload"]["info"]["total_token_usage"]
                    .as_object()
                    .and_then(|_| Tokens::parse(&entry["payload"]["info"]["total_token_usage"]));
            }
        }
        line.clear();
    };
    while position > 0 {
        let count = usize::try_from(position.min(block.len() as u64)).unwrap();
        position -= count as u64;
        reader.seek(SeekFrom::Start(position))?;
        reader.read_exact(&mut block[..count])?;
        for index in (0..count).rev() {
            if block[index] != b'\n' {
                reversed_line.push(block[index]);
                continue;
            }
            if !found_complete_end {
                // The bytes after the last newline may still be in flight. Keep
                // the offset before them so the next live poll reads them whole.
                result.offset = position + index as u64 + 1;
                reversed_line.clear();
                found_complete_end = true;
            } else {
                inspect(&mut reversed_line, &mut result);
                if result.tokens.is_some() && result.context.is_some() {
                    return Ok(result);
                }
            }
        }
        // Model context from the previous turn is optional; the next turn emits
        // its own context. Do not scan a huge previous turn just to recover it.
        if result.tokens.is_some() {
            return Ok(result);
        }
    }
    if found_complete_end && !reversed_line.is_empty() {
        inspect(&mut reversed_line, &mut result);
    }
    Ok(result)
}

/// One Codex session's rollout, read forward from where the turn found it.
struct SessionTail {
    id: String,
    /// The turn's own session; the others are sub-agents it started.
    parent: bool,
    path: Option<PathBuf>,
    offset: u64,
    /// The session's cumulative counter after the last request seen. `None`
    /// for a sub-agent before its first request: a fork can start with the
    /// parent's counter, so that first request is taken from its own figures.
    previous: Option<Tokens>,
    next_lookup: std::time::Instant,
    model: Option<String>,
    effort: Option<String>,
    tier: Option<String>,
    requests: u32,
}

impl SessionTail {
    fn new(id: String, parent: bool, model: Option<String>) -> Self {
        Self {
            id,
            parent,
            path: None,
            offset: 0,
            previous: None,
            next_lookup: std::time::Instant::now(),
            model,
            effort: None,
            tier: None,
            requests: 0,
        }
    }

    fn update_context(&mut self, entry: &Value) {
        if let Some(model) = entry["payload"]["model"].as_str() {
            self.model = Some(model.to_string());
        }
        self.effort = entry["payload"]
            .get("effort")
            .or_else(|| entry["payload"].get("reasoning_effort"))
            .and_then(Value::as_str)
            .map(str::to_string);
        self.tier = entry["payload"]["service_tier"].as_str().map(|tier| {
            if matches!(tier, "fast" | "priority") {
                "fast"
            } else {
                "standard"
            }
            .into()
        });
    }
}

/// The turn's token usage, read from Codex's own records: one entry per model
/// request of the session and of every sub-agent started during the turn, at
/// any depth. A sub-agent's requests never reach the parent session's counter,
/// so a turn that delegates (the `ultra` effort does by itself) was reported
/// with the parent's share only.
pub(super) struct CodexUsageReader {
    home: PathBuf,
    start_time: String,
    /// The parent first, then sub-agents in the order they were started.
    sessions: Vec<SessionTail>,
    requests: Vec<Value>,
    /// The parent's latest request: what its context holds now.
    last: Option<Tokens>,
    context_window: Option<Value>,
    latest: Option<Value>,
}

impl CodexUsageReader {
    pub(super) fn new(session_id: &str) -> Self {
        Self::at(
            LocalSessionHomes::from_env().codex_home,
            session_id,
            pockymoe_protocol::now_rfc3339(),
        )
    }

    fn at(home: PathBuf, session_id: &str, start_time: String) -> Self {
        let mut parent = SessionTail::new(
            crate::import_id::parse_session_ref(session_id).raw_id,
            true,
            None,
        );
        parent.previous = Some(Tokens::default());
        let mut reader = Self {
            home,
            start_time,
            sessions: vec![parent],
            requests: Vec::new(),
            last: None,
            context_window: None,
            latest: None,
        };
        // Snapshot the existing cumulative total before session/prompt starts.
        reader.locate(0);
        let parent = &mut reader.sessions[0];
        if let Some(mut file) = parent.path.as_ref().and_then(|path| File::open(path).ok()) {
            if let Ok(baseline) = read_baseline_tail(&mut file) {
                parent.offset = baseline.offset;
                parent.previous = Some(baseline.tokens.unwrap_or_default());
                if let Some(context) = baseline.context {
                    parent.update_context(&context);
                }
            }
        }
        reader
    }

    pub(super) fn poll(&mut self) -> Vec<Value> {
        self.read()
    }

    pub(super) fn poll_final(&mut self) -> Vec<Value> {
        // A short first turn, or a sub-agent that just started, can finish
        // before the periodic missing-file retry.
        for session in &mut self.sessions {
            session.next_lookup = std::time::Instant::now();
        }
        self.read()
    }

    fn locate(&mut self, index: usize) {
        let session = &mut self.sessions[index];
        if session.path.is_none() && session.next_lookup <= std::time::Instant::now() {
            // A sub-agent's file appears a moment after the parent records its start.
            let retry = if session.parent { 5 } else { 1 };
            session.next_lookup = std::time::Instant::now() + std::time::Duration::from_secs(retry);
            session.path = crate::local_sessions::find_codex_rollout(&self.home, &session.id);
        }
    }

    fn read(&mut self) -> Vec<Value> {
        let before = self.requests.len();
        // Sub-agents found while reading join the end and are read in this pass.
        let mut index = 0;
        while index < self.sessions.len() {
            self.read_session(index);
            index += 1;
        }
        if self.requests.len() == before {
            return Vec::new();
        }
        let total = self
            .requests
            .iter()
            .filter_map(|request| Tokens::parse(&request["total"]))
            .fold(Tokens::default(), |sum, tokens| sum.add(&tokens));
        let parent = &self.sessions[0];
        let update = json!({"total":total,"last":self.last.clone().unwrap_or_default(),"modelContextWindow":self.context_window,"cumulative":false,"requests":self.requests,"source":"codexRollout","model":parent.model,"reasoningEffort":parent.effort,"pricingTierKey":parent.tier});
        if self.latest.as_ref() == Some(&update) {
            return Vec::new();
        }
        self.latest = Some(update.clone());
        vec![update]
    }

    fn read_session(&mut self, index: usize) {
        self.locate(index);
        let Some(path) = self.sessions[index].path.clone() else {
            return;
        };
        let Ok(mut file) = File::open(path) else {
            return;
        };
        if file
            .seek(SeekFrom::Start(self.sessions[index].offset))
            .is_err()
        {
            return;
        }
        let mut reader = BufReader::new(file);
        loop {
            let mut line = String::new();
            let Ok(count) = reader.read_line(&mut line) else {
                break;
            };
            if count == 0 || !line.ends_with('\n') {
                break;
            }
            self.sessions[index].offset += count as u64;
            let Some(entry) = usage_record(&line) else {
                continue;
            };
            if entry["type"] == "turn_context" {
                self.sessions[index].update_context(&entry);
                continue;
            }
            if entry["type"] != "event_msg" {
                continue;
            }
            let before_turn = entry
                .get("timestamp")
                .and_then(Value::as_str)
                .is_some_and(|t| t < self.start_time.as_str());
            let payload = &entry["payload"];
            if payload["type"] == "item_completed" {
                // A sub-agent started in this turn: its requests are this turn's.
                // One started in an earlier turn was accounted for (or not) there.
                let item = &payload["item"];
                if item["type"] == "SubAgentActivity" && item["kind"] == "started" && !before_turn {
                    if let Some(id) = item["agent_thread_id"].as_str() {
                        if !self.sessions.iter().any(|session| session.id == id) {
                            self.sessions.push(SessionTail::new(
                                id.to_string(),
                                false,
                                item["model"].as_str().map(str::to_string),
                            ));
                        }
                    }
                }
                continue;
            }
            if payload["type"] != "token_count" {
                continue;
            }
            let info = &payload["info"];
            let Some(total) = info.get("total_token_usage").and_then(Tokens::parse) else {
                continue;
            };
            let session = &mut self.sessions[index];
            if before_turn {
                session.previous = Some(total);
                continue;
            }
            let last = info.get("last_token_usage").and_then(Tokens::parse);
            let used = match &session.previous {
                Some(previous) => total.cumulative_delta(previous, last.as_ref()),
                None => last.clone().unwrap_or_else(|| total.clone()),
            };
            session.previous = Some(total);
            if session.parent {
                self.last = Some(last.unwrap_or_else(|| used.clone()));
                if let Some(window) = info.get("model_context_window").filter(|v| !v.is_null()) {
                    self.context_window = Some(window.clone());
                }
            }
            // Codex repeats a count when only its rate limits changed.
            if used.total_tokens == 0 {
                continue;
            }
            session.requests += 1;
            let mut request = json!({"requestId":format!("{}:{}",session.id,session.requests),"model":session.model,"total":used,"last":used,"observedAt":entry.get("timestamp")});
            if session.parent {
                request["parent"] = json!(true);
            } else {
                request["subagentId"] = json!(session.id);
            }
            self.requests.push(request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARENT: &str = "01a1e7ef-aa01-7492-bbc3-686c15438155";
    const CHILD: &str = "01a1e7ef-bd2f-7cb1-ba50-7e7eda0463d1";
    const GRANDCHILD: &str = "01a1e7ef-cd5b-7c72-9bdc-fe263a960c02";
    const EARLIER: &str = "01a1e7ef-0000-7000-8000-000000000001";

    fn count(at: &str, total: (u64, u64), last: (u64, u64)) -> String {
        let usage = |(input, output): (u64, u64)| json!({"input_tokens":input,"cached_input_tokens":0,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":input+output});
        json!({"timestamp":at,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":usage(total),"last_token_usage":usage(last),"model_context_window":258400}}}).to_string()
    }
    fn context(model: &str) -> String {
        json!({"type":"turn_context","payload":{"model":model,"effort":"ultra"}}).to_string()
    }
    fn started(at: &str, child: &str) -> String {
        json!({"timestamp":at,"type":"event_msg","payload":{"type":"item_completed","item":{"type":"SubAgentActivity","kind":"started","model":"gpt-6.1-sol","agent_thread_id":child}}}).to_string()
    }
    struct Home(tempfile::TempDir);
    impl Home {
        fn append(&self, session: &str, lines: &[String]) {
            use std::io::Write;
            let directory = self.0.path().join("sessions/2026/11/17");
            std::fs::create_dir_all(&directory).unwrap();
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(directory.join(format!("rollout-2026-11-17T00-00-00-{session}.jsonl")))
                .unwrap();
            for line in lines {
                writeln!(file, "{line}").unwrap();
            }
        }
    }
    fn tokens(value: &Value) -> (u64, u64) {
        (
            value["inputTokens"].as_u64().unwrap(),
            value["outputTokens"].as_u64().unwrap(),
        )
    }

    #[test]
    fn a_turn_counts_its_sub_agents_at_every_depth_and_nothing_from_before_it() {
        let home = Home(tempfile::tempdir().unwrap());
        // Before the turn: the session's earlier work, and a sub-agent of an earlier turn.
        home.append(
            PARENT,
            &[
                context("gpt-6.1-sol"),
                started("2026-11-17T04:00:00.000Z", EARLIER),
                count("2026-11-17T04:00:01.000Z", (1000, 100), (1000, 100)),
            ],
        );
        home.append(
            EARLIER,
            &[count("2026-11-17T04:00:02.000Z", (500, 50), (500, 50))],
        );
        let mut reader = CodexUsageReader::at(
            home.0.path().to_path_buf(),
            &format!("codex::{PARENT}"),
            "2026-11-17T05:00:00.000Z".into(),
        );
        assert!(reader.poll().is_empty());

        // The turn: two requests of its own, one repeated count, and a sub-agent.
        home.append(
            PARENT,
            &[
                // Every turn states its model again.
                context("gpt-6.1-sol"),
                started("2026-11-17T05:00:01.000Z", CHILD),
                count("2026-11-17T05:00:02.000Z", (3000, 150), (2000, 50)),
                count("2026-11-17T05:00:03.000Z", (3000, 150), (2000, 50)),
            ],
        );
        // A fork starts with the parent's counter: its first request is its own figures.
        home.append(
            CHILD,
            &[
                context("gpt-6-luna"),
                count("2026-11-17T05:00:04.000Z", (9000, 400), (700, 30)),
                started("2026-11-17T05:00:05.000Z", GRANDCHILD),
                count("2026-11-17T05:00:06.000Z", (9900, 440), (900, 40)),
            ],
        );
        home.append(
            GRANDCHILD,
            &[count("2026-11-17T05:00:07.000Z", (300, 20), (300, 20))],
        );
        let update = reader.poll().pop().expect("usage of the turn");
        assert_eq!(update["source"], "codexRollout");
        assert_eq!(update["cumulative"], false);
        // Parent 2000+50, child 700+30 and 900+40, grandchild 300+20.
        assert_eq!(tokens(&update["total"]), (3900, 140));
        // The context meter follows the parent's own latest request.
        assert_eq!(tokens(&update["last"]), (2000, 50));
        let requests = update["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0]["parent"], true);
        assert_eq!(requests[0]["model"], "gpt-6.1-sol");
        assert_eq!(requests[0]["requestId"], format!("{PARENT}:1"));
        assert_eq!(requests[1]["model"], "gpt-6-luna");
        assert_eq!(requests[1]["subagentId"], CHILD);
        assert!(requests[1].get("parent").is_none());
        assert_eq!(requests[3]["subagentId"], GRANDCHILD);
        assert!(requests
            .iter()
            .all(|request| request["subagentId"] != EARLIER));

        // Nothing new: no update. More work in the sub-agent after that: counted.
        assert!(reader.poll().is_empty());
        home.append(
            CHILD,
            &[count("2026-11-17T05:00:08.000Z", (11000, 500), (1100, 60))],
        );
        let update = reader.poll_final().pop().unwrap();
        assert_eq!(tokens(&update["total"]), (5000, 200));
        assert_eq!(update["requests"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn a_turn_without_sub_agents_reports_the_same_total_as_its_session_counter() {
        let home = Home(tempfile::tempdir().unwrap());
        home.append(
            PARENT,
            &[
                context("gpt-6.1-sol"),
                count("2026-11-17T04:00:01.000Z", (1000, 100), (1000, 100)),
            ],
        );
        let mut reader = CodexUsageReader::at(
            home.0.path().to_path_buf(),
            PARENT,
            "2026-11-17T05:00:00.000Z".into(),
        );
        home.append(
            PARENT,
            &[
                context("gpt-6.1-sol"),
                count("2026-11-17T05:00:01.000Z", (2200, 130), (1200, 30)),
                count("2026-11-17T05:00:02.000Z", (3500, 190), (1300, 60)),
            ],
        );
        let update = reader.poll().pop().unwrap();
        assert_eq!(tokens(&update["total"]), (2500, 90));
        assert_eq!(update["modelContextWindow"], 258400);
        assert_eq!(update["model"], "gpt-6.1-sol");
        assert_eq!(update["reasoningEffort"], "ultra");
    }
}
