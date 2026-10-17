//! Output throughput from actual usage counters, not text length. Tool and user
//! waits are excluded. Live speed uses the latest confirmed response interval;
//! the legacy rolling minute is retained. Old turns without timing stay unavailable.
//!
//! Each model request is also split the way benchmarks do: the time to its first
//! output (queueing, prefill and any thinking that does not stream), the uncached
//! input it had to prefill in that time, and how fast its answer streamed after
//! that. A request is sent when the turn starts and whenever the last running tool
//! returns; its first output is its first streamed text, thought or tool call.
//!
//! Only requests that visibly streamed are split. Many replies arrive in one burst
//! (thinking that is not shown, then the tool call): their wait is the whole
//! reply, not a first token, and their output has no stream to time. The decode
//! counts the request's own answer tokens (its usage report, reasoning left out)
//! over the time its answer streamed, never the turn's total, which may carry
//! tokens of earlier requests reported late.
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
    /// The request before, until this one has output: a report arriving in
    /// between is still its response's.
    previous: Option<Request>,
    /// Earlier requests' share of the split.
    split: Split,
    /// The latest per-request usage report, to tell a new one from a repeat.
    report: Option<Report>,
}

/// A request's output must stream at least this long to be timed: chunks of a
/// reply delivered at once land tens of milliseconds apart, while a fast local
/// model's short answer still streams for a few hundred.
const MIN_STREAM_MS: i64 = 300;

/// One response's own usage: what the model read and wrote for this request.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Report {
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub reasoning: u64,
}

#[derive(Default, Clone, Copy)]
struct Split {
    /// Model requests that produced output, streamed or not.
    requests: u32,
    first_token_ms: i64,
    first_token_requests: u32,
    prefill_ms: i64,
    prefill_tokens: u64,
    prefill_cached_tokens: u64,
    decode_tokens: u64,
    decode_ms: i64,
    /// Requests whose reply arrived in one burst, left out of the split.
    burst_requests: u32,
}

impl Split {
    fn add(mut self, other: Split) -> Split {
        self.requests += other.requests;
        self.first_token_ms += other.first_token_ms;
        self.first_token_requests += other.first_token_requests;
        self.prefill_ms += other.prefill_ms;
        self.prefill_tokens += other.prefill_tokens;
        self.prefill_cached_tokens += other.prefill_cached_tokens;
        self.decode_tokens += other.decode_tokens;
        self.decode_ms += other.decode_ms;
        self.burst_requests += other.burst_requests;
        self
    }
}

/// The model request in flight.
#[derive(Default)]
struct Request {
    sent: Option<i64>,
    /// Its first and latest output of any kind, thoughts included.
    first: Option<i64>,
    last: Option<i64>,
    /// When its answer (text or a tool call) started, and its latest chunk.
    answer: Option<(i64, i64)>,
    /// When its streamed thinking started, and its latest chunk.
    thought: Option<(i64, i64)>,
    /// Its own usage report, when one arrived while it ran.
    report: Option<Report>,
}

impl Request {
    /// The request's share of the split; nothing until it has output. A burst is
    /// only known once the request is over.
    fn split(&self, done: bool) -> Split {
        let (Some(sent), Some(first), Some(last)) = (self.sent, self.first, self.last) else {
            return Split::default();
        };
        if last - first < MIN_STREAM_MS {
            return Split {
                requests: 1,
                burst_requests: done.into(),
                ..Split::default()
            };
        }
        let mut split = Split {
            requests: 1,
            first_token_ms: first - sent,
            first_token_requests: 1,
            ..Split::default()
        };
        if let Some(report) = self.report {
            split.prefill_ms = first - sent;
            split.prefill_tokens = report.input.saturating_sub(report.cached);
            split.prefill_cached_tokens = report.cached.min(report.input);
            // Thinking that streams (as local models' does) is decoded in view:
            // it is timed and counted with the answer. Hidden thinking is written
            // before the first output, so only the answer is.
            let thought = self
                .thought
                .is_some_and(|(start, end)| end - start >= MIN_STREAM_MS);
            let window = if thought {
                Some((first, last))
            } else {
                self.answer
            };
            if let Some((start, end)) = window.filter(|(start, end)| end - start >= MIN_STREAM_MS) {
                split.decode_tokens = if thought {
                    report.output
                } else {
                    report.output.saturating_sub(report.reasoning)
                };
                split.decode_ms = end - start;
            }
        }
        split
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
            previous: None,
            split: Split::default(),
            report: None,
        }
    }

    /// The model streamed something: answer text or a tool call, or a thought.
    fn output(&mut self, at: i64, answer: bool) {
        self.advance(at);
        if !self.tools.is_empty() || self.request.sent.is_none() {
            return;
        }
        if let Some(previous) = self.previous.take() {
            self.split = self.split.add(previous.split(true));
        }
        self.request.first.get_or_insert(at);
        self.request.last = Some(at);
        let window = if answer {
            &mut self.request.answer
        } else {
            &mut self.request.thought
        };
        *window = Some((window.map_or(at, |(start, _)| start), at));
    }

    fn begin_request(&mut self, at: i64) {
        let request = std::mem::replace(
            &mut self.request,
            Request {
                sent: Some(at),
                ..Request::default()
            },
        );
        if let Some(previous) = self.previous.replace(request) {
            self.split = self.split.add(previous.split(true));
        }
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

    /// `output` is the turn's output so far; `report` the latest response's own
    /// usage, when the harness reports one.
    fn usage(&mut self, at: i64, output: u64, report: Option<Report>) {
        self.advance(at);
        // A new report belongs to the request with output: this one, or the one
        // before while this one has none yet.
        if report.is_some() && report != self.report {
            self.report = report;
            if self.request.first.is_some() {
                self.request.report = report;
            } else if let Some(previous) = self.previous.as_mut() {
                previous.report = report;
            }
        }
        if output <= self.output {
            return;
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
        let split = self
            .split
            .add(
                self.previous
                    .as_ref()
                    .map(|previous| previous.split(true))
                    .unwrap_or_default(),
            )
            .add(self.request.split(!active));
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
            "splitMethod":"streamedRequests",
            "llmRequests":split.requests,
            "firstTokenMs":(split.first_token_requests > 0).then(||split.first_token_ms as f64 / split.first_token_requests as f64),
            "firstTokenRequests":split.first_token_requests,
            "burstRequests":split.burst_requests,
            "prefillTokens":split.prefill_tokens,
            "prefillCachedTokens":split.prefill_cached_tokens,
            "prefillTokensPerSecond":(split.prefill_ms > 0 && split.prefill_tokens > 0).then(||split.prefill_tokens as f64 * 1000.0 / split.prefill_ms as f64),
            "decodeTokens":split.decode_tokens,
            "decodeMs":split.decode_ms,
            "decodeTokensPerSecond":(split.decode_ms > 0 && split.decode_tokens > 0).then(||split.decode_tokens as f64 * 1000.0 / split.decode_ms as f64)})
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
        let (output, report) = if event.event_type == "runtime.usage.updated" {
            let row = self.db.with(|conn| Ok(conn.query_row("SELECT json_extract(token_usage_json,'$.total.outputTokens'),json_extract(token_usage_json,'$.last.inputTokens'),json_extract(token_usage_json,'$.last.cachedInputTokens'),json_extract(token_usage_json,'$.last.outputTokens'),json_extract(token_usage_json,'$.last.reasoningOutputTokens') FROM thread_turns WHERE thread_id=?1 AND id=?2", params![event.thread_id,turn], |row| Ok((row.get::<_,Option<u64>>(0)?,row.get::<_,Option<u64>>(1)?,row.get::<_,Option<u64>>(2)?,row.get::<_,Option<u64>>(3)?,row.get::<_,Option<u64>>(4)?))).optional()?))?;
            let (output, input, cached, last_output, reasoning) = row.unwrap_or_default();
            let report = input.zip(last_output).map(|(input, output)| Report {
                input,
                cached: cached.unwrap_or(0),
                output,
                reasoning: reasoning.unwrap_or(0),
            });
            (output, report)
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
                    tracker.output(at, true);
                    return Ok(());
                }
                "thread.item.started"
                    if matches!(
                        event.payload["item"]["kind"].as_str(),
                        Some("agentMessage" | "reasoning")
                    ) =>
                {
                    // A message or thought opened before its first token carries no
                    // output yet; counting it put the first token at the turn start.
                    let item = &event.payload["item"];
                    if item["text"]
                        .as_str()
                        .is_some_and(|text| !text.trim().is_empty())
                    {
                        tracker.output(at, item["kind"] == "agentMessage");
                    }
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
                        tracker.output(at, true);
                    }
                    tracker.tool(at, id, running);
                }
                "runtime.usage.updated" => {
                    if let Some(output) = output {
                        tracker.usage(at, output, report);
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
    fn report(input: u64, cached: u64, output: u64, reasoning: u64) -> Option<Report> {
        Some(Report {
            input,
            cached,
            output,
            reasoning,
        })
    }

    #[test]
    fn requests_split_into_first_token_prefill_and_decode() {
        // Sent at 0, answer streams from 2s, tool call at 6s with 400 output tokens
        // from 10k input (8k cached); the tool runs 10s, then the second request
        // streams from 17s to 21s for 200 more tokens on 3k uncached input.
        let mut t = Tracker::new(0);
        t.output(2000, true);
        t.output(4000, true);
        t.output(6000, true);
        t.tool(6000, "edit", true);
        t.usage(6500, 400, report(10000, 8000, 400, 0));
        t.tool(16000, "edit", false);
        t.output(17000, true);
        t.output(21000, true);
        t.usage(21500, 600, report(13000, 10000, 200, 0));
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
        t.usage(21600, 600, report(13000, 10000, 200, 0));
        assert_eq!(t.snapshot(21600, false)["firstTokenRequests"], 2);
    }

    #[test]
    fn streamed_thinking_is_timed_with_the_answer_as_a_local_model_reports_it() {
        // A local model: sent at 0, first thought token at 0.5 s (prefill of 2000
        // uncached tokens), thinking streams to 3.5 s, the answer to 5.6 s, then a
        // tool call. All 600 output tokens (300 of them reasoning) streamed in view.
        let mut t = Tracker::new(0);
        for at in (500..=3500).step_by(100) {
            t.output(at, false);
        }
        for at in (3600..=5600).step_by(100) {
            t.output(at, true);
        }
        t.tool(5600, "edit", true);
        t.usage(5650, 600, report(2000, 0, 600, 300));
        t.tool(6000, "edit", false);
        let s = t.snapshot(6000, true);
        assert_eq!(s["llmRequests"], 1);
        assert_eq!(s["firstTokenMs"], 500.0);
        assert_eq!(s["prefillTokensPerSecond"], 4000.0);
        assert_eq!(s["decodeTokens"], 600);
        assert_eq!(s["decodeMs"], 5100);
        assert!((s["decodeTokensPerSecond"].as_f64().unwrap() - 600.0 / 5.1).abs() < 1e-9);
    }

    #[test]
    fn a_reply_that_arrives_in_one_burst_is_left_out_of_the_split() {
        // Hidden thinking, then the thought summary and the tool call land 40 ms
        // apart: the 5 s wait is the whole reply, and nothing streamed to time.
        let mut t = Tracker::new(0);
        t.output(5000, false);
        t.output(5040, true);
        t.tool(5040, "edit", true);
        t.usage(5100, 800, report(20000, 15000, 800, 300));
        t.tool(9000, "edit", false);
        let s = t.snapshot(9000, true);
        assert_eq!(s["llmRequests"], 1);
        assert_eq!(s["burstRequests"], 1);
        assert_eq!(s["firstTokenRequests"], 0);
        assert_eq!(s["firstTokenMs"], Value::Null);
        assert_eq!(s["prefillTokensPerSecond"], Value::Null);
        assert_eq!(s["decodeTokensPerSecond"], Value::Null);
    }

    #[test]
    fn decode_counts_the_requests_own_answer_not_the_turn_total() {
        // The first request streams but is never reported on its own; the second
        // one's report (450 out, 150 of them reasoning) comes with the turn total
        // jumping by 1300. Its answer streamed from 10 s to 13 s.
        let mut t = Tracker::new(0);
        t.output(1000, true);
        t.output(4000, true);
        t.tool(4000, "read", true);
        t.tool(8000, "read", false);
        t.output(9000, false);
        t.output(10000, true);
        t.output(13000, true);
        t.usage(13100, 1300, report(30000, 28000, 450, 150));
        let s = t.snapshot(13100, false);
        assert_eq!(s["firstTokenRequests"], 2);
        assert_eq!(s["prefillTokens"], 2000);
        assert_eq!(s["decodeTokens"], 300);
        assert_eq!(s["decodeMs"], 3000);
        assert_eq!(s["decodeTokensPerSecond"], 100.0);
    }

    #[test]
    fn a_report_after_a_quick_tool_still_belongs_to_the_request_that_wrote_it() {
        let mut t = Tracker::new(0);
        t.output(1000, true);
        t.output(3000, true);
        t.tool(3000, "read", true);
        t.tool(3100, "read", false);
        // The reply's usage is read after the next request was sent.
        t.usage(3200, 500, report(9000, 8000, 500, 0));
        t.output(6000, true);
        let s = t.snapshot(6000, true);
        assert_eq!(s["decodeTokens"], 500);
        assert_eq!(s["decodeMs"], 2000);
        assert_eq!(s["prefillTokens"], 1000);
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
