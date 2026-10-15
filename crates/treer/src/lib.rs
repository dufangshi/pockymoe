//! Treer AIS v1 wire contract and pure forwarding/transcript decisions.
pub mod messages;
use pockymoe_protocol::{ThreadItemKind, ThreadTurnDto};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const CAPABILITIES: &[&str] = &[
    "prompt.submit",
    "prompt.queue",
    "prompt.notify",
    "prompt.queue.read",
    "transcript.read",
    "state.observe",
    "abort",
];
pub const IDENTITY_KEYS: &[&str] = &[
    "TREER_AGENT_ID",
    "TREER_WORKLOAD_CREDENTIAL",
    "TREER_AGENT_SERVER_URL",
    "TREER_WORKSPACE_ID",
    "TREER_SERVER_ID",
];

pub fn identity_env(
    agent: &str,
    identity: &pockymoe_protocol::treer::AttachTreerIdentity,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("TREER_AGENT_ID".into(), agent.into()),
        ("POCKYMOE_HOST_GUIDE".into(), "1".into()),
    ];
    for (key, value) in [
        ("TREER_WORKLOAD_CREDENTIAL", &identity.workload_credential),
        ("TREER_AGENT_SERVER_URL", &identity.agent_server_url),
        ("TREER_WORKSPACE_ID", &identity.workspace_id),
        ("TREER_SERVER_ID", &identity.server_id),
    ] {
        if let Some(value) = value {
            env.push((key.into(), value.clone()));
        }
    }
    env
}

pub fn validate_hello(hello: &pockymoe_protocol::treer::AttachHello) -> anyhow::Result<()> {
    use pockymoe_protocol::treer::*;
    anyhow::ensure!(
        (TREER_ATTACH_MIN_PROTOCOL..=TREER_ATTACH_PROTOCOL).contains(&hello.protocol_version),
        "unsupported_protocol"
    );
    anyhow::ensure!(
        valid_id(&hello.agent_id) && valid_id(&hello.instance_id),
        "invalid_identity"
    );
    anyhow::ensure!(
        hello.thread_id.is_some() != hello.create.is_some(),
        "invalid_target"
    );
    if let Some(create) = &hello.create {
        anyhow::ensure!(
            std::path::Path::new(&create.cwd).is_absolute(),
            "invalid_cwd"
        );
    }
    Ok(())
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Controller,
    ProxyUi,
}
/// Headers are public provenance markers, not a defense against same-user local processes.
pub fn source(source: &str, host: &str, port: u16, origin: bool, fetch: bool) -> Option<Source> {
    match source {
        "controller-api" if host.eq_ignore_ascii_case("agent-interface") && !origin && !fetch => {
            Some(Source::Controller)
        }
        "proxy-ui"
            if [
                format!("127.0.0.1:{port}"),
                format!("localhost:{port}"),
                format!("[::1]:{port}"),
            ]
            .contains(&host.to_ascii_lowercase()) =>
        {
            Some(Source::ProxyUi)
        }
        _ => None,
    }
}

/// The only thread visibility decision for Treer entry points.
/// A Treer machine belongs to one workspace; browser humans currently have
/// workspace-wide Agent UI access. Unbound hub threads never join that scope.
pub fn thread_in_scope(
    source: Source,
    bound_thread: &str,
    thread: &str,
    root: Option<&str>,
    root_has_binding: bool,
    root_is_creating: bool,
) -> bool {
    let root = root.unwrap_or(thread);
    match source {
        Source::Controller => root == bound_thread,
        // FUTURE TREER POLICY HOOK: intersect this result with policy/check and
        // per-node permission bits when Treer supplies a verified browser human.
        // Ask Treer at request time; do not add a second ACL or cache policy here.
        Source::ProxyUi => root_has_binding || root_is_creating,
    }
}

/// Keep labels valid path components; the thread ID distinguishes siblings.
pub fn agent_label(title: &str, thread: &str) -> String {
    let mut title: String = title
        .chars()
        .filter(|c| !c.is_control() && *c != '/')
        .take(80)
        .collect::<String>()
        .trim()
        .to_owned();
    if title.is_empty()
        || matches!(title.as_str(), "self" | "." | "..")
        || title.starts_with("ag_")
        || title.starts_with("app_")
    {
        title = format!("Pockymoe {title}").trim().to_owned();
    }
    format!("{title}-{thread}")
}

/// Default deny. Collection responses are supplied separately by the scoped hub middleware.
pub fn gate(method: &str, path: &str, allowed_thread: impl Fn(&str) -> bool) -> bool {
    let read = method == "GET" || method == "HEAD";
    if method == "POST" && path == "/api/threads/start" {
        return true; // The scoped handler creates the corresponding Treer Agent.
    }
    if let Some(tail) = path.strip_prefix("/api/threads/") {
        let mut parts = tail.split('/');
        let id = parts.next().unwrap_or_default();
        if !allowed_thread(id) {
            return false;
        }
        let action = parts.collect::<Vec<_>>().join("/");
        if read {
            return true;
        }
        // Do not allow fork/import, arbitrary harness commands, shell or global settings.
        return match method {
            "POST" => {
                matches!(
                    action.as_str(),
                    "prompt"
                        | "interrupt"
                        | "release"
                        | "resume"
                        | "disconnect"
                        | "compact"
                        | "goal"
                ) || action.starts_with("requests/")
                    || action.starts_with("pending-steers/")
            }
            "PATCH" => action.is_empty() || action == "settings" || action == "goal",
            "DELETE" => {
                action.is_empty() || action.starts_with("pending-steers/") || action == "goal"
            }
            _ => false,
        };
    }
    read && (matches!(
        path,
        "/" | "/index.html"
            | "/ws"
            | "/healthz"
            | "/readyz"
            | "/api/version"
            | "/api/auth/session"
            | "/api/config/runtime"
            | "/api/config/model-pricing"
            | "/api/agent-runtimes"
            | "/api/plugins"
            | "/threads"
    ) || path.starts_with("/assets/")
        || path.starts_with("/fonts/")
        || path == "/favicon.ico"
        || path
            .strip_prefix("/threads/")
            .is_some_and(|id| id == "new" || allowed_thread(id)))
}

pub fn event_allowed(event: &Value, allowed_thread: impl Fn(&str) -> bool) -> bool {
    event
        .get("threadId")
        .and_then(Value::as_str)
        .is_some_and(allowed_thread)
        || matches!(
            event["type"].as_str(),
            Some("supervisor.connected" | "supervisor.pong")
        )
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Queue,
    Steer,
}
impl Default for Delivery {
    fn default() -> Self {
        Self::Queue
    }
}
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Sender {
    pub agent_id: String,
    pub path: String,
    pub name: String,
}
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Notify {
    pub on: Vec<String>,
    pub to: Option<String>,
}
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Prompt {
    pub operation_id: String,
    pub text: String,
    pub mode: String,
    #[serde(default)]
    pub delivery: Delivery,
    pub sender: Option<Sender>,
    pub notify: Option<Notify>,
}
impl Prompt {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.operation_id.trim().is_empty()
                && self.operation_id.len() <= 128
                && !self.text.trim().is_empty()
                && self.text.len() <= 128 * 1024
                && self.mode == "prompt",
            "invalid_prompt"
        );
        if let Some(notify) = &self.notify {
            anyhow::ensure!(
                notify
                    .on
                    .iter()
                    .all(|v| ["done", "failed", "interrupted"].contains(&v.as_str())),
                "invalid_notify"
            );
            anyhow::ensure!(
                notify.on.is_empty()
                    || notify
                        .to
                        .as_ref()
                        .or(self.sender.as_ref().map(|s| &s.agent_id))
                        .is_some_and(|s| !s.is_empty()),
                "invalid_notify_recipient"
            );
        }
        Ok(())
    }
}
#[derive(Serialize)]
pub struct Manifest {
    pub protocol: &'static str,
    pub instance_id: String,
    pub capabilities: Vec<&'static str>,
    pub ui_path: &'static str,
}
#[derive(Serialize)]
pub struct Status {
    pub agent_id: String,
    pub interface_instance_id: String,
    pub status: &'static str,
    pub busy: bool,
    pub queue_length: usize,
    pub error: Option<String>,
}
pub fn status(running: bool, blocked: bool) -> &'static str {
    if blocked {
        "blocked"
    } else if running {
        "working"
    } else {
        "idle"
    }
}
pub fn submission_state(status: &pockymoe_protocol::TurnStatus) -> &'static str {
    use pockymoe_protocol::TurnStatus::*;
    match status {
        Completed => "done",
        Failed => "failed",
        Interrupted | Unknown(_) => "interrupted",
        InProgress | Recovering => "running",
    }
}

#[derive(Serialize)]
pub struct Entry {
    pub id: String,
    pub kind: String,
    pub role: Option<&'static str>,
    pub content: Value,
    pub created_at: Option<String>,
}
/// Exhaustive match keeps additions to the typed Pockymoe schema reviewable.
pub fn item_role(kind: &ThreadItemKind) -> Option<&'static str> {
    use ThreadItemKind::*;
    match kind {
        UserMessage => Some("user"),
        AgentMessage => Some("assistant"),
        Artifact | Image | ImageView | Plan | ContextCompaction | Reasoning | CommandExecution
        | WebSearch | FileRead | FileChange | Hook | AgentToolCall | SkillToolCall | ToolCall
        | Generic | Other | Unknown(_) => None,
    }
}
#[derive(Serialize)]
pub struct Transcript {
    pub agent_id: String,
    pub interface_instance_id: String,
    pub page: usize,
    pub page_count: usize,
    pub next_page: Option<usize>,
    pub cursor: String,
    pub next_cursor: Option<String>,
    pub entries: Vec<Entry>,
}
pub fn transcript(
    agent: &str,
    instance: &str,
    turns: &[ThreadTurnDto],
    page: usize,
    limit: usize,
) -> Transcript {
    let selected = turns
        .iter()
        .skip(page)
        .take(limit.clamp(1, 1000))
        .collect::<Vec<_>>();
    transcript_page(
        agent,
        instance,
        &selected.into_iter().cloned().collect::<Vec<_>>(),
        turns.len(),
        page,
    )
}

/// Map only the selected turns; page_count is the total turn count, not ceil(total/limit).
pub fn transcript_page(
    agent: &str,
    instance: &str,
    turns: &[ThreadTurnDto],
    total: usize,
    page: usize,
) -> Transcript {
    let selected = turns.iter().collect::<Vec<_>>();
    let next = page.saturating_add(selected.len());
    let next_page = (next < total).then_some(next);
    let entries = selected
        .into_iter()
        .flat_map(|turn| {
            turn.items.iter().map(|item| Entry {
                id: item.id.clone(),
                kind: item.kind.to_string(),
                role: item_role(&item.kind),
                content: json!(item.text),
                created_at: turn.started_at.clone(),
            })
        })
        .collect();
    Transcript {
        agent_id: agent.into(),
        interface_instance_id: instance.into(),
        page,
        page_count: total,
        next_page,
        cursor: page.to_string(),
        next_cursor: next_page.map(|p| p.to_string()),
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bound_workspace_scope_matrix_and_labels() {
        for (root, bound, creating, expected) in [
            (None, true, false, true),
            (Some("other"), true, false, true),
            (None, false, false, false),
            (Some("unbound"), false, false, false),
            (None, false, true, true),
        ] {
            assert_eq!(
                thread_in_scope(Source::ProxyUi, "x", "thread", root, bound, creating),
                expected
            );
        }
        assert!(thread_in_scope(
            Source::Controller,
            "x",
            "child",
            Some("x"),
            true,
            false
        ));
        assert!(!thread_in_scope(
            Source::Controller,
            "x",
            "other",
            None,
            true,
            false
        ));
        for title in ["self", ".", "..", "ag_bad", "app_bad", " /\n", ""] {
            let label = agent_label(title, "thread-id");
            assert!(!label.contains('/') && !label.chars().any(char::is_control));
            assert!(!label.starts_with("ag_") && !label.starts_with("app_"));
            assert!(label.ends_with("-thread-id"));
        }
    }
    #[test]
    fn gate_matrix_and_events() {
        for (m, p, ok) in [
            ("GET", "/api/threads/root", true),
            ("GET", "/api/threads/child", true),
            ("GET", "/api/threads/other", false),
            ("GET", "/api/management/supervisor", false),
            ("PATCH", "/api/config/workspace-settings", false),
            ("GET", "/api/workspaces", false),
            ("POST", "/api/threads/root/fork", false),
            ("POST", "/api/threads/start", true),
            ("PATCH", "/api/threads/root", true),
            ("DELETE", "/api/threads/root", true),
            ("PATCH", "/api/threads/root/settings", true),
            ("PATCH", "/api/threads/root/goal", true),
            ("POST", "/api/threads/root/harness", false),
            ("POST", "/api/agent-runtimes/codex/install", false),
            ("POST", "/api/threads/root/prompt", true),
            ("GET", "/ws", true),
            ("POST", "/api/cli", false),
        ] {
            assert_eq!(
                gate(m, p, |id| ["root", "child"].contains(&id)),
                ok,
                "{m} {p}"
            );
        }
        assert!(event_allowed(
            &json!({"type":"thread.updated","threadId":"root"}),
            |id| id == "root"
        ));
        assert!(!event_allowed(
            &json!({"type":"thread.updated","threadId":"other"}),
            |id| id == "root"
        ));
        assert!(!event_allowed(
            &json!({"type":"device.ports.changed"}),
            |_| true
        ));
        assert!(source("controller-api", "agent-interface", 1, false, false).is_some());
        assert!(source("controller-api", "agent-interface", 1, false, true).is_none());
        assert!(source("proxy-ui", "evil.example", 1, true, true).is_none());
    }
    #[test]
    fn transcript_golden() {
        let turns: Vec<ThreadTurnDto> = serde_json::from_value(json!([
            {"id":"a","status":"completed","error":null,"startedAt":"2026-01-01T00:00:00Z","items":[{"id":"u","kind":"userMessage","text":"hello"},{"id":"a","kind":"agentMessage","text":"hi"}]},
            {"id":"b","status":"inProgress","error":null,"items":[{"id":"p","kind":"plan","text":"plan"}]}
        ])).unwrap();
        assert_eq!(
            serde_json::to_value(transcript("ag", "i", &turns, 0, 1)).unwrap(),
            json!({"agent_id":"ag","interface_instance_id":"i","page":0,"page_count":2,"next_page":1,"cursor":"0","next_cursor":"1","entries":[{"id":"u","kind":"userMessage","role":"user","content":"hello","created_at":"2026-01-01T00:00:00Z"},{"id":"a","kind":"agentMessage","role":"assistant","content":"hi","created_at":"2026-01-01T00:00:00Z"}]})
        );
        assert_eq!(transcript("ag", "i", &turns, 1, 100).entries.len(), 1);
        assert!(transcript("ag", "i", &turns, 9, 1).entries.is_empty());
        for kind in ThreadItemKind::KNOWN_VALUES {
            let _ = item_role(&ThreadItemKind::from(*kind));
        }
        assert_eq!(
            [
                status(true, true),
                status(true, false),
                status(false, false)
            ],
            ["blocked", "working", "idle"]
        );
    }
}
