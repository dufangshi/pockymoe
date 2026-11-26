//! Keep native session defaults aligned with ACP permissions, including goal continuations.
use super::modes::ProductSessionPolicy;
use serde_json::{json, Value};
use std::collections::HashMap;

pub(crate) fn native_policy(policy: &ProductSessionPolicy) -> Value {
    let sandbox = policy
        .sandbox_mode
        .as_deref()
        .unwrap_or(if policy.auto_approve() {
            "danger-full-access"
        } else {
            "workspace-write"
        });
    let sandbox_policy = match sandbox {
        "danger-full-access" => json!({"type":"dangerFullAccess"}),
        "read-only" => json!({"type":"readOnly"}),
        _ => json!({"type":"workspaceWrite", "networkAccess":false}),
    };
    json!({"sandbox":sandbox,"sandboxPolicy":sandbox_policy,
        "approvalPolicy":if policy.auto_approve() {"never"} else {"on-request"},
        "approvalsReviewer":"user"})
}

pub(crate) struct PermissionBridge {
    initial: Value,
    threads: HashMap<String, Value>,
    pending: HashMap<String, (Option<String>, Value)>,
}
impl PermissionBridge {
    pub fn new(initial: Value) -> Self {
        Self {
            initial,
            threads: HashMap::new(),
            pending: HashMap::new(),
        }
    }
    pub fn request(&mut self, message: &mut Value) {
        let method = message["method"].as_str().unwrap_or("").to_string();
        if !matches!(
            method.as_str(),
            "thread/start" | "thread/resume" | "thread/fork" | "thread/settings/update"
        ) {
            return;
        }
        let source = message["params"]["threadId"].as_str().map(str::to_owned);
        let mut policy = source
            .as_ref()
            .and_then(|id| self.threads.get(id))
            .unwrap_or(&self.initial)
            .clone();
        let params = &mut message["params"];
        if method == "thread/settings/update" {
            // Only track explicit permission updates; never reinterpret collaboration settings.
            if params["sandboxPolicy"].is_null() {
                return;
            }
            policy["sandboxPolicy"] = params["sandboxPolicy"].clone();
            policy["sandbox"] = json!(match params["sandboxPolicy"]["type"].as_str() {
                Some("dangerFullAccess") => "danger-full-access",
                Some("readOnly") => "read-only",
                _ => "workspace-write",
            });
            for key in ["approvalPolicy", "approvalsReviewer"] {
                if !params[key].is_null() {
                    policy[key] = params[key].clone();
                }
            }
        } else {
            // Audit forks can explicitly request read-only. Preserve all explicit overrides.
            if !params["permissions"].is_null() || !params["sandbox"].is_null() {
                return;
            }
            for key in ["sandbox", "approvalPolicy", "approvalsReviewer"] {
                if params[key].is_null() {
                    params[key] = policy[key].clone();
                }
            }
        }
        self.pending.insert(
            message["id"].to_string(),
            (
                if method == "thread/fork" {
                    None
                } else {
                    source
                },
                policy,
            ),
        );
    }
    pub fn response(&mut self, message: &Value) {
        let Some((source, policy)) = self.pending.remove(&message["id"].to_string()) else {
            return;
        };
        if message.get("error").is_some() {
            return;
        }
        let id = message
            .pointer("/result/thread/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or(source);
        if let Some(id) = id {
            self.threads.insert(id, policy);
        }
    }
}
