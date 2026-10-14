//! Local protocol between `pockymoe attach` (one Treer Agent per top-level
//! thread) and the hub Supervisor that owns the threads on the same device.
//!
//! The attach process opens `GET /api/treer/attach` as a WebSocket with
//! `Authorization: Bearer <attach token>` and sends one [`AttachHello`]. The hub
//! answers with one [`AttachReply`] and keeps the socket open; the binding lives
//! exactly as long as the socket. Text frames carry JSON. Either side may send
//! WebSocket pings. The hub accepts every `protocolVersion` from
//! [`TREER_ATTACH_MIN_PROTOCOL`] to [`TREER_ATTACH_PROTOCOL`], so updating the
//! hub never requires restarting attach processes (each is a Treer Agent).

use serde::{Deserialize, Serialize};

pub const TREER_ATTACH_PROTOCOL: u32 = 1;
pub const TREER_ATTACH_MIN_PROTOCOL: u32 = 1;
pub const TREER_ATTACH_PATH: &str = "/api/treer/attach";
/// File (mode 0600) in the hub's data directory holding the attach bearer token.
pub const TREER_ATTACH_TOKEN_FILE: &str = "treer-attach.token";
/// Agent Interface protocol the hub implements for each bound Agent.
pub const TREER_AGENT_INTERFACE_PROTOCOL: &str = "treer.agent-interface/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachHello {
    pub protocol_version: u32,
    /// `TREER_AGENT_ID` of the attach process.
    pub agent_id: String,
    /// Fresh per attach process start; registered as the interface instance.
    pub instance_id: String,
    /// Existing thread to bind. Absent on first launch: the hub creates one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Used only when `thread_id` is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<AttachCreateThread>,
    /// Treer identity of this Agent, injected into the bound thread's harness
    /// environment. Held in hub memory only; never persisted or logged.
    pub treer: AttachTreerIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachCreateThread {
    /// Absolute working directory; the hub resolves or creates its workspace.
    pub cwd: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Values of the `TREER_*` environment Treer gives the attach process.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachTreerIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_server_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_credential: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
}

impl std::fmt::Debug for AttachTreerIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachTreerIdentity")
            .field("agent_server_url", &self.agent_server_url)
            .field(
                "workload_credential",
                &self.workload_credential.as_ref().map(|_| "<redacted>"),
            )
            .field("workspace_id", &self.workspace_id)
            .field("server_id", &self.server_id)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AttachReply {
    #[serde(rename_all = "camelCase")]
    Bound {
        thread_id: String,
        /// True when the hub created the thread for this hello.
        created: bool,
        /// Loopback port to register with `treer interface register --port`.
        interface_port: u16,
        /// Value for `--ui-path`.
        ui_path: String,
        /// Values for repeated `--capability`.
        capabilities: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    Error { code: String, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_and_reply_use_camel_case_and_redact_credentials() {
        let hello = AttachHello {
            protocol_version: TREER_ATTACH_PROTOCOL,
            agent_id: "ag_1".into(),
            instance_id: "i1".into(),
            thread_id: None,
            create: Some(AttachCreateThread {
                cwd: "/w".into(),
                provider: "codex".into(),
                agent: None,
                model: None,
                reasoning_effort: Some("high".into()),
                title: None,
            }),
            treer: AttachTreerIdentity {
                agent_server_url: Some("http://127.0.0.1:1".into()),
                workload_credential: Some("secret".into()),
                workspace_id: None,
                server_id: None,
            },
        };
        let json = serde_json::to_value(&hello).unwrap();
        assert_eq!(json["protocolVersion"], 1);
        assert_eq!(json["create"]["reasoningEffort"], "high");
        assert_eq!(json["treer"]["workloadCredential"], "secret");
        assert!(!format!("{hello:?}").contains("secret"));

        let reply: AttachReply = serde_json::from_value(serde_json::json!({
            "type": "bound", "threadId": "t", "created": true,
            "interfacePort": 45679, "uiPath": "/", "capabilities": ["abort"]
        }))
        .unwrap();
        assert!(matches!(reply, AttachReply::Bound { created: true, .. }));
    }
}
