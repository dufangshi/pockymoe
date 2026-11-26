//! When a native Claude turn is finished, from the CLI's own bookends.
//!
//! The runtime launches Claude Code with `CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS`,
//! as claude-agent-acp does, so `session_state_changed` reports when the whole
//! session is idle. A turn completes when (1) Claude reported the result of one
//! of the prompts this turn sent, (2) no SDK task (background Bash, Monitor,
//! async Agent) is still running, and (3) the session is idle. Background
//! work therefore holds the turn, like the ACP path's background hold, and its
//! task notification plus Claude's follow-up answer stay in the same turn.
//!
//! The state is per session and survives turns: a task left running after a
//! turn (an interrupted turn's background shell, a dev server) still blocks
//! release, and a scheduled wake running between turns is visible.
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct Completion {
    /// Running SDK tasks: id → backgrounded.
    tasks: HashMap<String, bool>,
    /// Last `session_state_changed` value, once Claude reports one.
    state: Option<String>,
    /// Prompt UUIDs this turn sent (its prompt and steering messages).
    prompts: HashSet<String>,
    /// A result frame answered one of `prompts`.
    result: bool,
    /// That result reported an error (API error, refusal, max turns, abort).
    error: Option<Value>,
}

impl Completion {
    pub fn begin_turn(&mut self, prompt: &str) {
        self.prompts.clear();
        self.prompts.insert(prompt.into());
        self.result = false;
        self.error = None;
    }

    pub fn add_prompt(&mut self, prompt: &str) {
        self.prompts.insert(prompt.into());
    }

    pub fn record(&mut self, message: &Value) {
        match message["type"].as_str() {
            Some("result") => {
                let ours = match message["user_message_uuids"].as_array() {
                    Some(ids) => ids
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|id| self.prompts.contains(id)),
                    // CLIs that do not name the prompt: the first result counts.
                    None => message["user_message_uuid"]
                        .as_str()
                        .map_or(message["origin"].is_null(), |id| self.prompts.contains(id)),
                };
                if ours && !self.result {
                    self.result = true;
                    if message["is_error"] == true {
                        self.error = Some(message.clone());
                    }
                }
            }
            Some("system") => match message["subtype"].as_str() {
                Some("session_state_changed") => {
                    self.state = message["state"].as_str().map(str::to_owned)
                }
                Some("task_started") => {
                    if let Some(id) = message["task_id"].as_str() {
                        self.tasks.insert(
                            id.into(),
                            message["is_backgrounded"].as_bool().unwrap_or(true),
                        );
                    }
                }
                Some("task_updated" | "task_notification") => {
                    if let Some(id) = message["task_id"].as_str() {
                        if terminal(message) {
                            // A task settling while Claude is idle wakes it with
                            // the notification; its follow-up answer belongs to
                            // this turn, so the turn waits for the next idle.
                            if self.tasks.remove(id).is_some()
                                && self.state.as_deref() == Some("idle")
                            {
                                self.state = Some("running".into());
                            }
                        } else if let Some(background) =
                            message["patch"]["is_backgrounded"].as_bool()
                        {
                            if let Some(task) = self.tasks.get_mut(id) {
                                *task = background;
                            }
                        }
                    }
                }
                Some("background_tasks_changed") => {
                    if let Some(tasks) = message["tasks"].as_array() {
                        // A level snapshot: the CLI's own list of what still runs.
                        if tasks.iter().all(|task| task["task_id"].is_string()) {
                            let previous = std::mem::take(&mut self.tasks);
                            if self.state.as_deref() == Some("idle")
                                && previous.keys().any(|id| {
                                    !tasks.iter().any(|t| t["task_id"].as_str() == Some(id))
                                })
                            {
                                self.state = Some("running".into());
                            }
                            self.tasks = tasks
                                .iter()
                                .filter_map(|t| t["task_id"].as_str())
                                .map(|id| {
                                    (id.to_owned(), previous.get(id).copied().unwrap_or(true))
                                })
                                .collect();
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn idle(&self) -> bool {
        self.state.as_deref().is_none_or(|state| state == "idle")
    }

    /// Without session states (an older CLI), only backgrounded tasks hold the
    /// turn: a foreground task settles with its tool result.
    fn holding(&self) -> bool {
        if self.state.is_some() {
            !self.tasks.is_empty()
        } else {
            self.tasks.values().any(|background| *background)
        }
    }

    pub fn complete(&self) -> bool {
        self.result && self.idle() && !self.holding()
    }

    /// The prompt is answered and Claude is idle, but tasks still run: the
    /// turn shows one waiting row until their notification wakes Claude.
    pub fn waiting(&self) -> Option<usize> {
        (self.result && self.idle() && self.holding()).then_some(self.tasks.len())
    }

    pub fn error(&self) -> Option<&Value> {
        self.error.as_ref()
    }

    /// Work that must finish (or be stopped) before the process is released.
    pub fn holds_background_work(&self) -> bool {
        !self.tasks.is_empty()
    }

    /// Claude is working without a Pockymoe turn (a scheduled wake).
    pub fn autonomous_running(&self) -> bool {
        self.state.as_deref().is_some_and(|state| state != "idle")
    }
}

fn terminal(message: &Value) -> bool {
    matches!(
        message["status"]
            .as_str()
            .or_else(|| message["patch"]["status"].as_str()),
        Some("completed" | "failed" | "stopped" | "killed" | "cancelled")
    )
}
