//! Native results own completion. Foreground Agent tasks are not background
//! waits: unlike ACP, their tool result and the final result already settle them.
use serde_json::Value;
use std::collections::HashMap;
#[derive(Default)]
pub(super) struct Completion {
    tasks: HashMap<String, bool>,
    state: Option<String>,
    result: bool,
}
impl Completion {
    pub fn record(&mut self, message: &Value) {
        match message["type"].as_str() {
            Some("result") => self.result = true,
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
                Some("task_updated") => {
                    if let Some(id) = message["task_id"].as_str() {
                        if terminal(message) {
                            self.tasks.remove(id);
                        } else if let Some(background) =
                            message["patch"]["is_backgrounded"].as_bool()
                        {
                            self.tasks.insert(id.into(), background);
                        }
                    }
                }
                Some("task_notification") => {
                    if let Some(id) = message["task_id"].as_str() {
                        if terminal(message) {
                            self.tasks.remove(id);
                        }
                    }
                }
                Some("background_tasks_changed") => {
                    if let Some(tasks) = message["tasks"].as_array() {
                        self.tasks = tasks
                            .iter()
                            .filter_map(|t| {
                                Some((
                                    t["task_id"].as_str()?.into(),
                                    t["is_backgrounded"].as_bool().unwrap_or(true),
                                ))
                            })
                            .collect();
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    pub fn pending(&self) -> bool {
        self.tasks.values().any(|v| *v)
    }
    pub fn complete(&self) -> bool {
        self.result && !self.pending() && self.state.as_deref().is_none_or(|s| s == "idle")
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
