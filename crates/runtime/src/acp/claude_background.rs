//! Launching an async Agent completes the tool call, not the background work.
//! Track the native launch receipt until its SDK-origin task notification.
use std::collections::HashMap;

use pockymoe_protocol::ThreadSubagentDto;
use serde_json::Value;

#[derive(Default)]
pub(super) struct ClaudeBackgroundAgents {
    tools: HashMap<String, ThreadSubagentDto>,
    known: HashMap<String, ThreadSubagentDto>,
    running: HashMap<String, ThreadSubagentDto>,
}

impl ClaudeBackgroundAgents {
    pub fn record(&mut self, entry: &Value) {
        let content = &entry["message"]["content"];
        if entry["type"] == "assistant" {
            for block in content.as_array().into_iter().flatten() {
                if block["type"] != "tool_use"
                    || !matches!(block["name"].as_str(), Some("Agent" | "Task"))
                {
                    continue;
                }
                let Some(id) = block["id"].as_str() else {
                    continue;
                };
                self.tools.insert(
                    id.into(),
                    ThreadSubagentDto {
                        id: id.into(),
                        name: block["input"]["description"].as_str().map(str::to_owned),
                        status: "running".into(),
                        started_at: entry["timestamp"].as_str().map(str::to_owned),
                        completed_at: None,
                        parent_tool_call_id: Some(id.into()),
                        is_background: Some(true),
                    },
                );
            }
            return;
        }
        if entry["type"] != "user" {
            return;
        }
        let result = &entry["toolUseResult"];
        if result["isAsync"] == true && result["status"] == "async_launched" {
            if let Some(agent_id) = result["agentId"].as_str() {
                for block in content.as_array().into_iter().flatten() {
                    let Some(tool_id) = block["tool_use_id"].as_str() else {
                        continue;
                    };
                    let Some(agent) = self.tools.get(tool_id) else {
                        continue;
                    };
                    self.known.insert(agent_id.into(), agent.clone());
                    self.running.insert(agent_id.into(), agent.clone());
                }
            }
        }
        if let Some(agent_id) = result["resumedAgentId"].as_str() {
            if let Some(agent) = self.known.get(agent_id) {
                self.running.insert(agent_id.into(), agent.clone());
            }
        }
        // A human may quote the same XML. Only SDK-origin notifications settle
        // a launch; sidechains, other sessions and old turns are filtered upstream.
        if entry.pointer("/origin/kind").and_then(Value::as_str) != Some("task-notification")
            && entry["turnOrigin"] != "task_notification"
        {
            return;
        }
        let Some(text) = content.as_str() else { return };
        let Some(header) = text.strip_prefix("<task-notification>") else {
            return;
        };
        let header = header.split("<result>").next().unwrap_or(header);
        if !matches!(
            field(header, "status"),
            Some("completed" | "failed" | "stopped" | "killed" | "cancelled")
        ) {
            return;
        }
        if let Some(id) = field(header, "task-id") {
            if self.running.get(id).is_some_and(|agent| {
                field(header, "tool-use-id").is_none_or(|tool| tool == agent.id)
            }) {
                self.running.remove(id);
            }
        }
    }

    pub fn active(&self) -> Vec<ThreadSubagentDto> {
        self.running.values().cloned().collect()
    }
}

fn field<'a>(header: &'a str, tag: &str) -> Option<&'a str> {
    header
        .split_once(&format!("<{tag}>"))?
        .1
        .split_once(&format!("</{tag}>"))
        .map(|(value, _)| value.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn async_receipt_survives_final_text_until_its_native_notification() {
        let mut agents = ClaudeBackgroundAgents::default();
        agents.record(
            &json!({"type":"assistant","timestamp":"2099-01-01T00:00:00Z",
            "message":{"content":[{"type":"tool_use","id":"tool-1","name":"Agent",
                "input":{"description":"Independent review","run_in_background":true}}]}}),
        );
        assert!(agents.active().is_empty()); // A tool invocation alone proves no launch.
        agents.record(&json!({"type":"user","toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"agent-1"},
            "message":{"content":[{"type":"tool_result","tool_use_id":"tool-1"}]}}));
        agents.record(&json!({"type":"assistant","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Main reply done"}]}}));
        let active = agents.active();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name.as_deref(), Some("Independent review"));
        assert_eq!(active[0].is_background, Some(true));
        let mut notification = json!({"type":"user","message":{"content":
            "<task-notification><task-id>agent-1</task-id><tool-use-id>tool-1</tool-use-id><status>completed</status></task-notification>"}});
        agents.record(&notification); // Human quoting the XML cannot settle it.
        assert_eq!(agents.active().len(), 1);
        notification["origin"] = json!({"kind":"task-notification","producer":"session-task"});
        agents.record(&notification);
        assert!(agents.active().is_empty());
        agents.record(&json!({"type":"user","toolUseResult":{"resumedAgentId":"agent-1"}}));
        assert_eq!(agents.active().len(), 1);
        agents.record(&notification);
        assert!(agents.active().is_empty());
    }

    #[test]
    fn unrelated_shell_tasks_and_notifications_do_not_change_agents() {
        let mut agents = ClaudeBackgroundAgents::default();
        agents.record(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"shell","name":"Bash"}]}}));
        agents.record(&json!({"type":"user","toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"shell-task"},
            "message":{"content":[{"type":"tool_result","tool_use_id":"shell"}]}}));
        assert!(agents.active().is_empty());
    }

    #[test]
    fn stopped_task_id_only_notification_clears_a_background_agent() {
        let mut agents = ClaudeBackgroundAgents::default();
        agents.record(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-1","name":"Agent","input":{"description":"Implement theme packs"}}]}}));
        agents.record(&json!({"type":"user","toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"agent-1"},"message":{"content":[{"type":"tool_result","tool_use_id":"tool-1"}]}}));
        let mut notice = json!({"type":"user","message":{"content":"<task-notification><task-id>agent-1</task-id><status>stopped</status><summary>Background agent didn't finish before the previous session ended</summary></task-notification>"}});
        agents.record(&notice);
        assert_eq!(agents.active().len(), 1);
        notice["origin"] = json!({"kind":"task-notification"});
        agents.record(&notice);
        assert!(agents.active().is_empty());
        agents.record(&notice);
        assert!(agents.active().is_empty());
    }
}
