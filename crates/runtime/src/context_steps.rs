//! One record per model request of a thread: what the request read and wrote,
//! how long the model and the tools before it took, and, when context
//! inspection supplies one, what the context was made of
//! (docs/context-inspection.md, "Steps"). Diagnostic data: it lives beside the
//! database, one append-only file per thread, and never holds prompt text.
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Steps kept per thread; the oldest go when a file is rewritten.
const MAX_STEPS: usize = 1500;
/// A file is rewritten to `MAX_STEPS` once it holds this many lines.
const COMPACT_LINES: usize = 4000;

pub struct StepUsage {
    pub input: u64,
    pub cached: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: u64,
}

#[derive(Default)]
struct ThreadState {
    next_seq: u64,
    lines: usize,
    /// The newest step: when it was recorded, its request totals and the
    /// occupancy of the composition attached to it.
    last: Option<LastStep>,
    /// A composition that described no recorded step yet, with its arrival time.
    pending: Option<(Value, i64)>,
}
struct LastStep {
    seq: u64,
    at_ms: i64,
    input: u64,
    output: u64,
    mix_used: Option<u64>,
}

pub struct ContextSteps {
    root: PathBuf,
    threads: Mutex<HashMap<String, ThreadState>>,
}

fn valid_thread(thread: &str) -> bool {
    !thread.is_empty()
        && thread.len() <= 128
        && thread
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

impl ContextSteps {
    pub fn new(database: &Path) -> Self {
        Self {
            root: database.with_extension("context-steps"),
            threads: Default::default(),
        }
    }

    fn path(&self, thread: &str) -> PathBuf {
        self.root.join(format!("{thread}.jsonl"))
    }

    fn rows(&self, thread: &str) -> Vec<Value> {
        std::fs::read_to_string(self.path(thread))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// The thread's state, rebuilt from its file after a restart.
    fn state<'a>(
        &self,
        threads: &'a mut HashMap<String, ThreadState>,
        thread: &str,
    ) -> &'a mut ThreadState {
        threads.entry(thread.to_owned()).or_insert_with(|| {
            let rows = self.rows(thread);
            let steps = fold(&rows);
            let last = steps.last().map(|step| LastStep {
                seq: step["seq"].as_u64().unwrap_or(0),
                at_ms: step["at"].as_str().and_then(millis).unwrap_or(0),
                input: step["inputTokens"].as_u64().unwrap_or(0),
                output: step["outputTokens"].as_u64().unwrap_or(0),
                mix_used: step["usedTokens"].as_u64(),
            });
            ThreadState {
                next_seq: last.as_ref().map_or(1, |last| last.seq + 1),
                lines: rows.len(),
                last,
                pending: None,
            }
        })
    }

    fn append(&self, thread: &str, state: &mut ThreadState, row: &Value) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.path(thread);
        if state.lines >= COMPACT_LINES {
            let steps = fold(&self.rows(thread));
            let kept = &steps[steps.len().saturating_sub(MAX_STEPS)..];
            let mut text = String::new();
            for step in kept {
                text.push_str(&stored(step).to_string());
                text.push('\n');
            }
            let temporary = path.with_extension("jsonl.tmp");
            std::fs::write(&temporary, text)?;
            std::fs::rename(&temporary, &path)?;
            state.lines = kept.len();
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.write_all(format!("{row}\n").as_bytes())?;
        state.lines += 1;
        Ok(())
    }

    /// A model request reported its usage. `model_ms` and `tool_ms` cover the
    /// time since the step before it (or the turn's start).
    #[allow(clippy::too_many_arguments)]
    pub fn record_step(
        &self,
        thread: &str,
        turn: &str,
        at: &str,
        model: Option<&str>,
        usage: StepUsage,
        model_ms: i64,
        tool_ms: i64,
    ) -> Result<()> {
        if !valid_thread(thread) {
            return Ok(());
        }
        let at_ms = millis(at).unwrap_or(0);
        let mut threads = self.threads.lock().unwrap();
        let state = self.state(&mut threads, thread);
        let seq = state.next_seq;
        state.next_seq += 1;
        let previous_at = state.last.as_ref().map_or(i64::MIN, |last| last.at_ms);
        let row = json!({"t":"step","seq":seq,"turnId":turn,"at":at,"model":model,
            "inputTokens":usage.input,"cachedInputTokens":usage.cached,"cacheWriteInputTokens":usage.cache_write,
            "outputTokens":usage.output,"reasoningOutputTokens":usage.reasoning,
            "modelMs":model_ms.max(0),"toolMs":tool_ms.max(0)});
        self.append(thread, state, &row)?;
        state.last = Some(LastStep {
            seq,
            at_ms,
            input: usage.input,
            output: usage.output,
            mix_used: None,
        });
        // A composition that arrived after the step before this one describes
        // this request: the collector saw the request before its usage report.
        if let Some((mix, arrived)) = state.pending.take() {
            if arrived >= previous_at {
                self.attach(thread, state, mix)?;
            }
        }
        Ok(())
    }

    fn attach(&self, thread: &str, state: &mut ThreadState, mut mix: Value) -> Result<()> {
        let Some(last) = state.last.as_mut() else {
            return Ok(());
        };
        mix["t"] = json!("mix");
        mix["seq"] = json!(last.seq);
        last.mix_used = mix["usedTokens"].as_u64();
        self.append(thread, state, &mix)
    }

    /// Context inspection produced a snapshot. It is attached to the request it
    /// describes: the newest step when their totals agree or it has none yet,
    /// otherwise the next step to be recorded.
    pub fn observe_snapshot(&self, thread: &str, snapshot: &Value, at: &str) -> Result<()> {
        if !valid_thread(thread) {
            return Ok(());
        }
        let Some(mix) = composition(snapshot) else {
            return Ok(());
        };
        let used = mix["usedTokens"].as_u64();
        let mut threads = self.threads.lock().unwrap();
        let state = self.state(&mut threads, thread);
        let fits = state.last.as_ref().is_some_and(|last| {
            last.mix_used.is_none()
                || last.mix_used == used
                || used == Some(last.input + last.output)
                || used == Some(last.input)
        });
        if fits {
            self.attach(thread, state, mix)
        } else {
            state.pending = Some((mix, millis(at).unwrap_or(0)));
            Ok(())
        }
    }

    /// The thread's steps, oldest first, each with the composition attached to it.
    pub fn list(&self, thread: &str) -> Vec<Value> {
        if !valid_thread(thread) {
            return Vec::new();
        }
        let steps = fold(&self.rows(thread));
        steps[steps.len().saturating_sub(MAX_STEPS)..].to_vec()
    }

    pub fn remove(&self, thread: &str) {
        if !valid_thread(thread) {
            return;
        }
        self.threads.lock().unwrap().remove(thread);
        let _ = std::fs::remove_file(self.path(thread));
    }
}

fn millis(at: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|time| time.timestamp_millis())
}

/// What a step keeps of a snapshot: occupancy and category sizes, no item lists.
fn composition(snapshot: &Value) -> Option<Value> {
    let used = snapshot["usedTokens"].as_u64()?;
    let mut categories = Map::new();
    let mut counts = Map::new();
    for category in snapshot["categories"].as_array()? {
        if category["kind"] != "used" {
            continue;
        }
        let (Some(id), Some(tokens)) = (category["id"].as_str(), category["tokens"].as_u64())
        else {
            continue;
        };
        categories.insert(id.to_owned(), json!(tokens));
        if let Some(count) = category["count"].as_u64() {
            counts.insert(id.to_owned(), json!(count));
        }
    }
    for group in snapshot["groups"].as_array().into_iter().flatten() {
        if let (Some(id), Some(items)) = (group["id"].as_str(), group["items"].as_array()) {
            if categories.contains_key(id) && !counts.contains_key(id) {
                counts.insert(id.to_owned(), json!(items.len()));
            }
        }
    }
    if categories.is_empty() {
        return None;
    }
    Some(
        json!({"usedTokens":used,"windowTokens":snapshot["windowTokens"],"source":snapshot["source"],
        "categories":categories,"counts":counts}),
    )
}

/// Steps with their latest composition, in recording order.
fn fold(rows: &[Value]) -> Vec<Value> {
    let mut steps: Vec<Value> = Vec::new();
    let mut index: HashMap<u64, usize> = HashMap::new();
    for row in rows {
        let Some(seq) = row["seq"].as_u64() else {
            continue;
        };
        match row["t"].as_str() {
            Some("step") => {
                let mut step = row.clone();
                step.as_object_mut().unwrap().remove("t");
                index.insert(seq, steps.len());
                steps.push(step);
            }
            Some("mix") => {
                if let Some(step) = index.get(&seq).and_then(|at| steps.get_mut(*at)) {
                    for key in [
                        "usedTokens",
                        "windowTokens",
                        "source",
                        "categories",
                        "counts",
                    ] {
                        step[key] = row[key].clone();
                    }
                }
            }
            _ => {}
        }
    }
    steps
}

/// A folded step as one stored line.
fn stored(step: &Value) -> Value {
    let mut row = step.clone();
    row["t"] = json!("step");
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, cached: u64, output: u64) -> StepUsage {
        StepUsage {
            input,
            cached,
            cache_write: 0,
            output,
            reasoning: 0,
        }
    }
    fn snapshot(used: u64, messages: u64) -> Value {
        json!({"usedTokens":used,"windowTokens":200000,"source":"codexTrace","categories":[
            {"id":"system_prompt","kind":"used","tokens":100},
            {"id":"user_messages","kind":"used","tokens":messages,"count":2},
            {"id":"free_space","kind":"free","tokens":5}],
            "groups":[{"id":"system_prompt","items":[{"name":"a","tokens":1}]},{"id":"skills","items":[]}]})
    }
    const T: &str = "2026-11-10T10:00:0";

    #[test]
    fn a_composition_joins_the_request_it_describes_in_either_order() {
        let dir = tempfile::tempdir().unwrap();
        let steps = ContextSteps::new(&dir.path().join("db.sqlite"));
        // Usage first, then its composition.
        steps
            .record_step(
                "thread-1",
                "turn-1",
                &format!("{T}1Z"),
                Some("m"),
                usage(1000, 0, 50),
                900,
                0,
            )
            .unwrap();
        steps
            .observe_snapshot("thread-1", &snapshot(1050, 400), &format!("{T}1Z"))
            .unwrap();
        // The next request's composition arrives before its usage report.
        steps
            .observe_snapshot("thread-1", &snapshot(2100, 900), &format!("{T}3Z"))
            .unwrap();
        assert_eq!(steps.list("thread-1").len(), 1);
        steps
            .record_step(
                "thread-1",
                "turn-1",
                &format!("{T}4Z"),
                Some("m"),
                usage(2000, 1000, 100),
                700,
                1300,
            )
            .unwrap();
        // A refreshed reading of the same context replaces the one attached.
        steps
            .observe_snapshot("thread-1", &snapshot(2100, 950), &format!("{T}5Z"))
            .unwrap();
        let listed = steps.list("thread-1");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["categories"]["user_messages"], 400);
        assert_eq!(
            listed[0]["counts"],
            json!({"user_messages":2,"system_prompt":1})
        );
        assert_eq!(listed[0]["usedTokens"], 1050);
        assert_eq!(listed[1]["seq"], 2);
        assert_eq!(listed[1]["cachedInputTokens"], 1000);
        assert_eq!(listed[1]["toolMs"], 1300);
        assert_eq!(listed[1]["categories"]["user_messages"], 950);
        assert!(listed[1]["categories"].get("free_space").is_none());

        // A new process continues the numbering and still pairs by totals.
        let again = ContextSteps::new(&dir.path().join("db.sqlite"));
        again
            .record_step(
                "thread-1",
                "turn-2",
                &format!("{T}7Z"),
                None,
                usage(3000, 2000, 10),
                5,
                5,
            )
            .unwrap();
        assert_eq!(again.list("thread-1").last().unwrap()["seq"], 3);
        assert!(again
            .list("thread-1")
            .last()
            .unwrap()
            .get("categories")
            .is_none());

        again.remove("thread-1");
        assert!(again.list("thread-1").is_empty());
        // Names that could leave the directory are ignored.
        again
            .record_step("../x", "t", &format!("{T}1Z"), None, usage(1, 0, 1), 0, 0)
            .unwrap();
        assert!(!dir.path().join("x.jsonl").exists());
    }

    #[test]
    fn a_stale_composition_is_not_given_to_a_later_request() {
        let dir = tempfile::tempdir().unwrap();
        let steps = ContextSteps::new(&dir.path().join("db.sqlite"));
        steps
            .record_step(
                "t",
                "turn",
                &format!("{T}1Z"),
                None,
                usage(100, 0, 10),
                0,
                0,
            )
            .unwrap();
        steps
            .observe_snapshot("t", &snapshot(110, 5), &format!("{T}1Z"))
            .unwrap();
        // Arrives, is held, but two steps pass before anything claims it.
        steps
            .observe_snapshot("t", &snapshot(999, 7), &format!("{T}2Z"))
            .unwrap();
        steps
            .record_step(
                "t",
                "turn",
                &format!("{T}3Z"),
                None,
                usage(200, 0, 10),
                0,
                0,
            )
            .unwrap();
        steps
            .record_step(
                "t",
                "turn",
                &format!("{T}4Z"),
                None,
                usage(300, 0, 10),
                0,
                0,
            )
            .unwrap();
        let listed = steps.list("t");
        assert_eq!(listed[1]["categories"]["user_messages"], 7);
        assert!(listed[2].get("categories").is_none());
    }

    #[test]
    fn a_long_thread_keeps_its_newest_steps() {
        let dir = tempfile::tempdir().unwrap();
        let steps = ContextSteps::new(&dir.path().join("db.sqlite"));
        for index in 0..(COMPACT_LINES as u64 + 10) {
            steps
                .record_step(
                    "t",
                    "turn",
                    &format!("{T}1Z"),
                    None,
                    usage(index, 0, 1),
                    0,
                    0,
                )
                .unwrap();
        }
        let listed = steps.list("t");
        assert_eq!(listed.len(), MAX_STEPS);
        assert_eq!(listed.last().unwrap()["seq"], COMPACT_LINES as u64 + 10);
        let lines = std::fs::read_to_string(dir.path().join("db.context-steps/t.jsonl"))
            .unwrap()
            .lines()
            .count();
        assert!(lines <= MAX_STEPS + 11, "{lines}");
    }
}
