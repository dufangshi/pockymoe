//! Output throughput from actual usage counters, not text length. Tool and user
//! waits are excluded. Live speed uses the latest confirmed response interval;
//! the legacy rolling minute is retained. Old turns without timing stay unavailable.
//!
//! Each model request is also split the way benchmarks do: the time to its first
//! output (queueing, prefill and any thinking that does not stream), the uncached
//! input it had to prefill in that time, and the output it streamed after that.
//! A request is sent when the turn starts and whenever the last running tool
//! returns; its first output is its first streamed text, thought or tool call.
use super::*;
use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct GenerationCache(std::sync::Mutex<HashMap<(String, String), Tracker>>);

#[derive(Clone)]
struct Span {
    start: i64,
    end: i64,
    tokens: f64,
}

struct Tracker {
    at: i64,
    llm_ms: i64,
    output: u64,
    tools: HashSet<String>,
    pending: Vec<Span>,
    samples: VecDeque<Span>,
    output_ms: i64,
    latest_rate: Option<f64>,
    latest_ms: i64,
    latest_at: Option<i64>,
    request: Request,
    first_token_ms: i64,
    first_token_requests: u32,
    prefill_tokens: u64,
    prefill_cached_tokens: u64,
    decode_tokens: u64,
    decode_ms: i64,
}

/// The model request in flight.
#[derive(Default)]
struct Request {
    sent: Option<i64>,
    first: Option<i64>,
    last: Option<i64>,
    output: u64,
    counted: bool,
}

impl Request {
    /// Output streamed after the first token, and over how long.
    fn decode(&self) -> (u64, i64) {
        match (self.first, self.last) {
            (Some(first), Some(last)) if self.output > 0 && last > first => {
                (self.output, last - first)
            }
            _ => (0, 0),
        }
    }
}

impl Tracker {
    fn new(at: i64) -> Self {
        Self {
            at,
            llm_ms: 0,
            output: 0,
            tools: HashSet::new(),
            pending: vec![],
            samples: VecDeque::new(),
            output_ms: 0,
            latest_rate: None,
            latest_ms: 0,
            latest_at: None,
            request: Request {
                sent: Some(at),
                ..Request::default()
            },
            first_token_ms: 0,
            first_token_requests: 0,
            prefill_tokens: 0,
            prefill_cached_tokens: 0,
            decode_tokens: 0,
            decode_ms: 0,
        }
    }

    /// The model streamed something: text, a thought or a tool call.
    fn output(&mut self, at: i64) {
        self.advance(at);
        if !self.tools.is_empty() || self.request.sent.is_none() {
            return;
        }
        self.request.first.get_or_insert(at);
        self.request.last = Some(at);
    }

    fn begin_request(&mut self, at: i64) {
        let (tokens, ms) = self.request.decode();
        self.decode_tokens += tokens;
        self.decode_ms += ms;
        self.request = Request {
            sent: Some(at),
            ..Request::default()
        };
    }

    fn advance(&mut self, at: i64) {
        let at = at.max(self.at);
        if self.tools.is_empty() && at > self.at {
            self.llm_ms += at - self.at;
            if let Some(last) = self.pending.last_mut().filter(|s| s.end == self.at) {
                last.end = at;
            } else {
                self.pending.push(Span {
                    start: self.at,
                    end: at,
                    tokens: 0.0,
                });
            }
        }
        self.at = at;
        while self.samples.front().is_some_and(|s| s.end <= at - 60000) {
            self.samples.pop_front();
        }
    }

    fn tool(&mut self, at: i64, id: &str, running: bool) {
        self.advance(at);
        let busy = !self.tools.is_empty();
        if running {
            self.tools.insert(id.into());
        } else {
            self.tools.remove(id);
        }
        // Once every tool has returned, the model is asked again.
        if busy && self.tools.is_empty() {
            self.begin_request(at);
        }
    }

    /// `input` is the request's input and cached input tokens, when reported.
    fn usage(&mut self, at: i64, output: u64, input: Option<(u64, u64)>) {
        self.advance(at);
        if output <= self.output {
            return;
        }
        self.request.output += output - self.output;
        if let (Some(sent), Some(first), false, Some((input, cached))) = (
            self.request.sent,
            self.request.first,
            self.request.counted,
            input,
        ) {
            self.request.counted = true;
            self.first_token_ms += first - sent;
            self.first_token_requests += 1;
            self.prefill_tokens += input.saturating_sub(cached);
            self.prefill_cached_tokens += cached.min(input);
        }
        let elapsed: i64 = self.pending.iter().map(|s| s.end - s.start).sum();
        let delta = output - self.output;
        self.output = output;
        if elapsed > 0 {
            // Measure the latest confirmed response instead of dividing old
            // tokens by a new, not-yet-reported request's growing duration.
            // Include latency/reasoning: SDKs may batch streamed chunks and
            // token totals cover hidden output too. Visible chunk timestamps
            // alone cannot establish decoder throughput for those tokens.
            let measured_ms = elapsed;
            self.output_ms += measured_ms;
            self.latest_ms = measured_ms;
            self.latest_rate = Some(delta as f64 * 1000.0 / measured_ms as f64);
            self.latest_at = Some(self.at);
            for mut span in self.pending.drain(..) {
                span.tokens = delta as f64 * (span.end - span.start) as f64 / elapsed as f64;
                if span.end > self.at - 60000 {
                    self.samples.push_back(span);
                }
            }
        }
    }

    fn snapshot(&mut self, at: i64, active: bool) -> Value {
        self.advance(at);
        let mut recent_ms = 0;
        let mut recent_tokens = 0.0;
        // Unreported pending spans have no matching token count yet. Treating
        // them as zero-token samples made the displayed speed collapse mid-reply.
        for span in &self.samples {
            let ms = (span.end - span.start.max(self.at - 60000)).max(0);
            recent_ms += ms;
            recent_tokens += span.tokens * ms as f64 / (span.end - span.start) as f64;
        }
        let (request_tokens, request_ms) = self.request.decode();
        let (decode_tokens, decode_ms) = (
            self.decode_tokens + request_tokens,
            self.decode_ms + request_ms,
        );
        json!({"outputTokens":self.output,"llmTimeMs":self.llm_ms,
            "averageTokensPerSecond":(self.llm_ms > 0 && self.output > 0).then(||self.output as f64 * 1000.0 / self.llm_ms as f64),
            "recentTokensPerSecond":(recent_ms > 0 && recent_tokens > 0.0).then(||recent_tokens * 1000.0 / recent_ms as f64),
            "windowSeconds":60,"active":active,"state":if self.tools.is_empty() {"llm"} else {"tool"},
            "measurement":"usageIntervals","updatedAt":chrono::DateTime::from_timestamp_millis(self.at).map(|at|at.to_rfc3339()),
            "outputTimeMs":self.output_ms,
            "averageOutputTokensPerSecond":(self.output_ms > 0 && self.output > 0).then(||self.output as f64 * 1000.0 / self.output_ms as f64),
            "latestOutputTokensPerSecond":self.latest_rate,
            "latestOutputTimeMs":self.latest_ms,
            "latestOutputMeasuredAt":self.latest_at.and_then(chrono::DateTime::from_timestamp_millis).map(|at|at.to_rfc3339()),
            "firstTokenMs":(self.first_token_requests > 0).then(||self.first_token_ms as f64 / self.first_token_requests as f64),
            "firstTokenRequests":self.first_token_requests,
            "prefillTokens":self.prefill_tokens,
            "prefillCachedTokens":self.prefill_cached_tokens,
            "prefillTokensPerSecond":(self.first_token_ms > 0 && self.prefill_tokens > 0).then(||self.prefill_tokens as f64 * 1000.0 / self.first_token_ms as f64),
            "decodeTokens":decode_tokens,
            "decodeMs":decode_ms,
            "decodeTokensPerSecond":(decode_ms > 0 && decode_tokens > 0).then(||decode_tokens as f64 * 1000.0 / decode_ms as f64)})
    }
}

fn millis(at: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| at.timestamp_millis())
}

impl Supervisor {
    pub(super) fn observe_generation_event(
        &self,
        event: &pockymoe_protocol::ThreadEventEnvelope,
    ) -> Result<()> {
        let Some(at) = millis(&event.timestamp) else {
            return Ok(());
        };
        let turn = event.payload["turnId"]
            .as_str()
            .or_else(|| {
                event
                    .payload
                    .pointer("/request/turnId")
                    .and_then(Value::as_str)
            })
            .map(str::to_owned)
            .or_else(|| {
                if event.event_type != "thread.request.resolved" {
                    return None;
                }
                let request = format!("request:{}", event.payload["requestId"].as_str()?);
                self.generation
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(key, tracker)| {
                        key.0 == event.thread_id && tracker.tools.contains(&request)
                    })
                    .map(|(key, _)| key.1.clone())
            });
        let Some(turn) = turn else {
            return Ok(());
        };
        let key = (event.thread_id.clone(), turn.clone());
        if event.event_type == "thread.turn.started" {
            self.generation
                .0
                .lock()
                .unwrap()
                .entry(key)
                .or_insert_with(|| Tracker::new(at));
            return Ok(());
        }
        let (output, input) = if event.event_type == "runtime.usage.updated" {
            let row = self.db.with(|conn| Ok(conn.query_row("SELECT json_extract(token_usage_json,'$.total.outputTokens'),json_extract(token_usage_json,'$.last.inputTokens'),json_extract(token_usage_json,'$.last.cachedInputTokens') FROM thread_turns WHERE thread_id=?1 AND id=?2", params![event.thread_id,turn], |row| Ok((row.get::<_,Option<u64>>(0)?,row.get::<_,Option<u64>>(1)?,row.get::<_,Option<u64>>(2)?))).optional()?))?;
            let (output, input, cached) = row.unwrap_or((None, None, None));
            (output, input.map(|input| (input, cached.unwrap_or(0))))
        } else {
            (None, None)
        };
        let speed = {
            let mut cache = self.generation.0.lock().unwrap();
            let Some(tracker) = cache.get_mut(&key) else {
                return Ok(());
            };
            match event.event_type.as_str() {
                "thread.request.created" => {
                    let Some(id) = event.payload.pointer("/request/id").and_then(Value::as_str)
                    else {
                        return Ok(());
                    };
                    tracker.tool(at, &format!("request:{id}"), true);
                }
                "thread.request.resolved" => {
                    let Some(id) = event.payload["requestId"].as_str() else {
                        return Ok(());
                    };
                    tracker.tool(at, &format!("request:{id}"), false);
                }
                // Streamed text and thoughts only mark the request's output; they
                // are too frequent to store a snapshot each.
                "thread.output.delta" => {
                    tracker.output(at);
                    return Ok(());
                }
                "thread.item.started"
                    if matches!(
                        event.payload["item"]["kind"].as_str(),
                        Some("agentMessage" | "reasoning")
                    ) =>
                {
                    tracker.output(at);
                    return Ok(());
                }
                "thread.item.started" | "thread.item.completed" => {
                    let item = &event.payload["item"];
                    if !matches!(
                        item["kind"].as_str(),
                        Some(
                            "commandExecution"
                                | "fileChange"
                                | "fileRead"
                                | "webSearch"
                                | "toolCall"
                                | "agentToolCall"
                                | "skillToolCall"
                        )
                    ) {
                        return Ok(());
                    }
                    let Some(id) = item["id"].as_str() else {
                        return Ok(());
                    };
                    let running = !matches!(
                        item["status"].as_str(),
                        Some("completed" | "failed" | "interrupted")
                    );
                    // The model writing a tool call is output of its request.
                    if running && !tracker.tools.contains(id) {
                        tracker.output(at);
                    }
                    tracker.tool(at, id, running);
                }
                "runtime.usage.updated" => {
                    if let Some(output) = output {
                        tracker.usage(at, output, input);
                    }
                }
                "thread.turn.completed" => {
                    let mut tracker = cache.remove(&key).unwrap();
                    let speed = tracker.snapshot(at, false);
                    drop(cache);
                    return self.save_generation_speed(&key, speed);
                }
                _ => return Ok(()),
            }
            tracker.snapshot(at, true)
        };
        self.save_generation_speed(&key, speed)
    }

    pub(super) fn refresh_generation_speeds(&self) -> Result<()> {
        let now = millis(&now_rfc3339()).unwrap();
        let snapshots: Vec<_> = self
            .generation
            .0
            .lock()
            .unwrap()
            .iter_mut()
            .map(|(key, tracker)| (key.clone(), tracker.snapshot(now, true)))
            .collect();
        for (key, speed) in snapshots {
            self.save_generation_speed(&key, speed)?;
        }
        Ok(())
    }

    fn save_generation_speed(&self, key: &(String, String), speed: Value) -> Result<()> {
        let payload = self.db.with(|conn| {
            let row = conn.query_row("SELECT token_usage_json,model,reasoning_effort FROM thread_turns WHERE thread_id=?1 AND id=?2", params![key.0,key.1], |r| Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?))).optional()?;
            let Some((Some(raw),model,effort)) = row else { return Ok(None); };
            let mut usage: Value = serde_json::from_str(&raw)?;
            // An observer snapshot taken before completion must not overwrite the
            // final timing after another callback has removed the tracker.
            if usage["generationSpeed"]["active"] == false && speed["active"] == true { return Ok(None); }
            if usage["generationSpeed"]["updatedAt"].as_str().and_then(millis) > speed["updatedAt"].as_str().and_then(millis)
                || usage["generationSpeed"]["outputTokens"].as_u64() > speed["outputTokens"].as_u64() { return Ok(None); }
            usage["generationSpeed"] = speed;
            conn.execute("UPDATE thread_turns SET token_usage_json=?1 WHERE thread_id=?2 AND id=?3", params![usage.to_string(),key.0,key.1])?;
            Ok(Some(json!({"turnId":key.1,"model":model,"reasoningEffort":effort,"tokenUsage":crate::usage::public_usage(&usage),"priceEstimate":usage["priceEstimate"]})))
        })?;
        if let Some(payload) = payload {
            self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadTurnTokenUpdated,
                thread_id: key.0.clone(),
                timestamp: now_rfc3339(),
                payload,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unreported_reply_and_idle_tail_do_not_dilute_confirmed_output_speed() {
        let mut t = Tracker::new(0);
        t.usage(10000, 1000, None);
        let reported = t.snapshot(10000, true);
        let pending = t.snapshot(50000, true);
        assert_eq!(
            pending["recentTokensPerSecond"],
            reported["recentTokensPerSecond"]
        );
        assert_eq!(pending["latestOutputTokensPerSecond"], 100.0);
        let done = t.snapshot(80000, false);
        assert_eq!(done["averageOutputTokensPerSecond"], 100.0);
        assert_eq!(done["outputTimeMs"], 10000);
        assert_eq!(done["recentTokensPerSecond"], Value::Null);
    }

    #[test]
    fn latest_response_is_not_the_turn_average_or_diluted_by_the_previous_response() {
        let mut t = Tracker::new(0);
        t.usage(10000, 1000, None);
        t.tool(10000, "tool", true);
        t.tool(110000, "tool", false);
        t.usage(112000, 2000, None);
        let s = t.snapshot(112000, false);
        assert_eq!(s["latestOutputTokensPerSecond"], 500.0);
        assert_eq!(s["latestOutputTimeMs"], 2000);
        assert_eq!(s["llmTimeMs"], 12000);
        assert!((s["averageOutputTokensPerSecond"].as_f64().unwrap() - 2000.0 / 12.0).abs() < 1e-8);
    }

    #[test]
    fn tool_minute_is_not_llm_time_and_parallel_tools_pause_only_once() {
        let mut t = Tracker::new(0);
        t.usage(10000, 1000, None);
        t.tool(10000, "a", true);
        t.tool(20000, "b", true);
        t.tool(40000, "a", false);
        t.tool(70000, "b", false);
        t.usage(80000, 2000, None);
        let s = t.snapshot(80000, false);
        assert_eq!(s["llmTimeMs"], 20000);
        assert_eq!(s["averageTokensPerSecond"], 100.0);
        assert_eq!(s["recentTokensPerSecond"], 100.0);
        t.usage(80000, 2000, None);
        assert_eq!(
            t.snapshot(80000, false),
            s,
            "duplicate usage must not count twice"
        );
    }
    #[test]
    fn requests_split_into_first_token_prefill_and_decode() {
        // Sent at 0, first thought at 2s, tool call at 6s with 400 output tokens
        // from 10k input (8k cached); the tool runs 10s, then the second request
        // streams from 17s to 21s for 200 more tokens on 3k uncached input.
        let mut t = Tracker::new(0);
        t.output(2000);
        t.output(4000);
        t.output(6000);
        t.tool(6000, "edit", true);
        t.usage(6500, 400, Some((10000, 8000)));
        t.tool(16000, "edit", false);
        t.output(17000);
        t.output(21000);
        t.usage(21500, 600, Some((13000, 10000)));
        let s = t.snapshot(21500, false);
        assert_eq!(s["firstTokenRequests"], 2);
        assert_eq!(s["firstTokenMs"], 1500.0);
        assert_eq!(s["prefillTokens"], 5000);
        assert_eq!(s["prefillCachedTokens"], 18000);
        assert!((s["prefillTokensPerSecond"].as_f64().unwrap() - 5000.0 / 3.0).abs() < 1e-8);
        assert_eq!(s["decodeTokens"], 600);
        assert_eq!(s["decodeMs"], 8000);
        assert_eq!(s["decodeTokensPerSecond"], 75.0);
        // A usage report repeated for the same request is not a new first token.
        t.usage(21600, 600, Some((13000, 10000)));
        assert_eq!(t.snapshot(21600, false)["firstTokenRequests"], 2);
    }

    #[test]
    fn rolling_window_clips_old_requests_and_never_invents_tokens() {
        let mut t = Tracker::new(0);
        assert_eq!(t.snapshot(5000, true)["recentTokensPerSecond"], Value::Null);
        t.usage(10000, 1000, None);
        t.usage(70000, 4000, None);
        assert_eq!(t.snapshot(70000, true)["recentTokensPerSecond"], 50.0);
        t.tool(70000, "wait", true);
        assert_eq!(
            t.snapshot(140000, true)["recentTokensPerSecond"],
            Value::Null
        );
        assert!(
            (t.snapshot(140000, true)["averageTokensPerSecond"]
                .as_f64()
                .unwrap()
                - 4000.0 / 70.0)
                .abs()
                < 1e-8
        );
    }
}
