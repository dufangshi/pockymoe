//! SDK bookends refine ACP's unresolved prompt state. An idle frame alone has
//! no turn identity and must never complete a prompt.
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct ClaudeLifecycle {
    sequence: u64,
    initialized: bool,
    commands: HashMap<String, (u64, bool)>,
    tasks: HashSet<String>,
    settled_tasks: HashSet<String>,
    tasks_known: bool,
    background_seen: bool,
    successful_autonomous_result: Option<u64>,
    idle: bool,
    draining: bool,
}

impl ClaudeLifecycle {
    pub fn begin_turn(&mut self) -> u64 {
        self.draining = false;
        self.background_seen = !self.tasks.is_empty();
        self.sequence + 1
    }

    // ACP may settle the foreground reply while SDK tasks still own follow-up
    // work. Keep the same output subscription until their actual idle bookend;
    // removing the last task alone precedes the autonomous model execution.
    pub fn background_pending(&self) -> bool {
        self.background_seen && (!self.tasks_known || !self.tasks.is_empty() || !self.idle)
    }

    /// SDK tasks (background Bash, Monitor, async Agent) still running in this
    /// process, including after an interrupted turn. Unknown task state counts.
    pub fn holds_background_work(&self) -> bool {
        !self.tasks.is_empty() || (self.background_seen && !self.tasks_known)
    }

    pub fn waiting_count(&self) -> usize {
        if self.idle {
            self.tasks.len()
        } else {
            0
        }
    }

    pub fn steering_started(&mut self) -> bool {
        if self.draining {
            return false;
        }
        self.commands.clear();
        self.idle = false;
        true
    }

    pub fn begin_drain(&mut self, id: &str, since: u64) -> bool {
        if self.draining || !self.completed_coalesced_command(id, since) {
            return false;
        }
        self.draining = true;
        true
    }

    pub fn record(&mut self, message: &Value) {
        self.sequence += 1;
        match message["type"].as_str() {
            Some("command_lifecycle") => {
                let Some(id) = message["command_uuid"].as_str() else {
                    return;
                };
                match message["state"].as_str() {
                    Some("started") => {
                        self.idle = false;
                        // Only the current/recent commands are needed for joins.
                        self.commands
                            .retain(|_, (seq, _)| self.sequence - *seq < 128);
                        self.commands.insert(id.into(), (self.sequence, false));
                    }
                    Some("completed") => {
                        if let Some(command) = self.commands.get_mut(id) {
                            command.1 = true;
                        }
                    }
                    Some("cancelled" | "refused" | "discarded") => {
                        self.commands.remove(id);
                    }
                    _ => {}
                }
            }
            Some("result") => {
                self.idle = false;
                self.successful_autonomous_result = (message["subtype"] == "success"
                    && message["is_error"] == false
                    && message["num_turns"].as_u64().is_some_and(|count| count > 0)
                    && message.pointer("/origin/kind").and_then(Value::as_str)
                        == Some("task-notification"))
                .then_some(self.sequence);
            }
            Some("system") => match message["subtype"].as_str() {
                Some("init") => {
                    if !self.initialized {
                        self.tasks_known = true;
                        self.initialized = true;
                    }
                    // An autonomous wake emits init again before its first tool.
                    // Preserve task ownership and duplicate protection across it.
                    self.idle = false;
                    self.successful_autonomous_result = None;
                }
                Some("session_state_changed") => {
                    self.idle = message["state"] == "idle";
                }
                Some("background_tasks_changed") => {
                    if let Some(tasks) = message["tasks"].as_array() {
                        // Reject malformed level snapshots rather than assuming no work.
                        if tasks.iter().all(|task| task["task_id"].is_string()) {
                            self.tasks = tasks
                                .iter()
                                .filter_map(|task| task["task_id"].as_str().map(str::to_owned))
                                .collect();
                            self.background_seen |= !self.tasks.is_empty();
                            self.tasks_known = true;
                        } else {
                            self.tasks_known = false;
                        }
                    }
                }
                Some("task_started") => {
                    if let Some(id) = message["task_id"].as_str() {
                        self.tasks.insert(id.into());
                        self.settled_tasks.remove(id);
                        self.background_seen = true;
                    }
                }
                Some("task_notification" | "task_updated") => {
                    let status = message["status"]
                        .as_str()
                        .or_else(|| message["patch"]["status"].as_str());
                    if matches!(
                        status,
                        Some("completed" | "failed" | "stopped" | "killed" | "cancelled")
                    ) {
                        if let Some(id) = message["task_id"].as_str() {
                            self.tasks.remove(id);
                            if self.settled_tasks.insert(id.into()) {
                                self.background_seen = true;
                                self.idle = false;
                            }
                        }
                    }
                }
                Some("worker_shutdown") => {
                    self.tasks_known = false;
                    self.idle = false;
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub fn completed_coalesced_command(&self, id: &str, since: u64) -> bool {
        self.tasks_known
            && self.tasks.is_empty()
            && self.idle
            && self.commands.get(id).is_some_and(|(started, completed)| {
                *started >= since
                    && *completed
                    && self
                        .successful_autonomous_result
                        .is_some_and(|result| result > *started)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn foreground_idle_does_not_end_background_work_or_its_autonomous_followup() {
        let mut state = ClaudeLifecycle::default();
        state.begin_turn();
        state.record(&json!({"type":"system","subtype":"init"}));
        state.record(&json!({"type":"system","subtype":"task_started","task_id":"bash"}));
        state.record(&json!({"type":"system","subtype":"session_state_changed","state":"idle"}));
        assert!(state.background_pending());
        state.record(&json!({"type":"system","subtype":"task_notification","task_id":"bash","status":"completed"}));
        state.record(&json!({"type":"system","subtype":"init"}));
        assert!(
            state.background_pending(),
            "a repeated autonomous init cannot end follow-up work"
        );
        state.record(&json!({"type":"result","subtype":"success","is_error":false,"num_turns":1,"origin":{"kind":"task-notification"}}));
        assert!(
            state.background_pending(),
            "result alone lacks its idle bookend"
        );
        state.record(&json!({"type":"system","subtype":"session_state_changed","state":"idle"}));
        assert!(!state.background_pending());
        state.record(&json!({"type":"system","subtype":"task_notification","task_id":"bash","status":"completed"}));
        assert!(
            !state.background_pending(),
            "a repeated notification cannot reopen idle work"
        );
        state.begin_turn();
        assert!(
            !state.background_pending(),
            "ordinary turns do not inherit a stale hold"
        );
    }

    #[test]
    fn release_waits_for_tasks_left_running_after_the_turn() {
        let mut state = ClaudeLifecycle::default();
        state.begin_turn();
        state.record(&json!({"type":"system","subtype":"init"}));
        assert!(!state.holds_background_work());
        state.record(&json!({"type":"system","subtype":"task_started","task_id":"dev-server"}));
        // An interrupted turn ends while the background shell keeps running.
        state.begin_turn();
        assert!(state.holds_background_work());
        state.record(&json!({"type":"system","subtype":"task_notification","task_id":"dev-server","status":"killed"}));
        assert!(!state.holds_background_work());
        state.record(&json!({"type":"system","subtype":"task_started","task_id":"build"}));
        state.record(&json!({"type":"system","subtype":"worker_shutdown"}));
        state.record(&json!({"type":"system","subtype":"background_tasks_changed","tasks":[]}));
        assert!(!state.holds_background_work());
    }

    #[test]
    fn background_snapshot_and_terminal_patch_keep_the_followup_open() {
        let mut state = ClaudeLifecycle::default();
        state.record(&json!({"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"older-task"}]}));
        state.begin_turn();
        assert!(state.background_pending());
        state.record(&json!({"type":"system","subtype":"task_updated","task_id":"older-task","patch":{"status":"completed"}}));
        assert!(state.background_pending());
        state.record(&json!({"type":"system","subtype":"session_state_changed","state":"idle"}));
        assert!(!state.background_pending());
    }
    fn ready() -> ClaudeLifecycle {
        let mut state = ClaudeLifecycle::default();
        for message in [
            json!({"type":"system","subtype":"init"}),
            json!({"type":"command_lifecycle","command_uuid":"owned","state":"started"}),
            json!({"type":"command_lifecycle","command_uuid":"owned","state":"completed"}),
            json!({"type":"result","subtype":"success","is_error":false,"num_turns":1,"origin":{"kind":"task-notification"}}),
            json!({"type":"system","subtype":"session_state_changed","state":"idle"}),
        ] {
            state.record(&message);
        }
        state
    }
    #[test]
    fn requires_owned_completed_command_success_idle_and_no_background_work() {
        let mut state = ready();
        assert!(state.completed_coalesced_command("owned", 1));
        assert!(!state.completed_coalesced_command("other", 1));
        let next = state.begin_turn();
        assert!(!state.completed_coalesced_command("owned", next));
        state.record(
            &json!({"type":"system","subtype":"task_started","task_id":"previous-turn-agent"}),
        );
        assert!(!state.completed_coalesced_command("owned", 1));
        state.record(&json!({"type":"system","subtype":"background_tasks_changed","tasks":[]}));
        assert!(state.completed_coalesced_command("owned", 1));
        state.record(&json!({"type":"system","subtype":"session_state_changed","state":"running"}));
        assert!(!state.completed_coalesced_command("owned", 1));
    }
    #[test]
    fn missing_or_failed_bookends_never_prove_completion() {
        let mut state = ready();
        state.tasks_known = false;
        assert!(!state.completed_coalesced_command("owned", 1));
        state.tasks_known = true;
        state.record(&json!({"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":1,"origin":{"kind":"task-notification"}}));
        state.record(&json!({"type":"system","subtype":"session_state_changed","state":"idle"}));
        assert!(!state.completed_coalesced_command("owned", 1));
        state = ready();
        state.record(
            &json!({"type":"command_lifecycle","command_uuid":"owned","state":"discarded"}),
        );
        assert!(!state.completed_coalesced_command("owned", 1));
    }

    #[test]
    fn steering_and_housekeeping_cannot_claim_the_same_input() {
        let mut state = ready();
        assert!(state.steering_started());
        assert!(!state.begin_drain("owned", 1));
        state = ready();
        assert!(state.begin_drain("owned", 1));
        assert!(!state.steering_started());
        assert!(!state.begin_drain("owned", 1));
        state.begin_turn();
        assert!(state.steering_started());
    }
}
