//! Device MCP servers passed to ACP sessions (`session/new`, `load`, `resume`, `fork`).
//!
//! Harnesses with their own MCP configuration (Codex, Claude Code) keep it; this
//! registry adds servers for harnesses that only receive them through ACP, such
//! as DeepSeek Harness. Each server can be limited to some agent ids. The file
//! holds header and env values that may be credentials: it is 0600 and listings
//! redact them.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::capabilities::NegotiatedCaps;

static REGISTRY: OnceLock<PathBuf> = OnceLock::new();
const MAX_SERVERS: usize = 32;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServer {
    pub name: String,
    /// `stdio`, `http` or `sse`.
    #[serde(default = "stdio")]
    pub transport: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// ACP agent ids (for example `deepseek`); empty means every ACP agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn stdio() -> String {
    "stdio".into()
}
fn enabled() -> bool {
    true
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registry {
    #[serde(default)]
    servers: Vec<McpServer>,
}

/// Called once at Supervisor start with its database path.
pub fn configure(database: &Path) {
    let _ = REGISTRY.set(database.with_extension("mcp-servers.json"));
}

fn path() -> Option<&'static PathBuf> {
    REGISTRY.get()
}

fn read() -> Result<Registry> {
    let Some(path) = path() else {
        return Ok(Registry::default());
    };
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(serde_json::from_str(&text).context("invalid MCP server registry")?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
        Err(error) => Err(error.into()),
    }
}

fn write(registry: &Registry) -> Result<()> {
    let path = path().context("MCP server registry is not configured")?;
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(registry)?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

impl McpServer {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            valid_name(&self.name),
            "MCP server name must be 1..64 letters, digits, - or _"
        );
        match self.transport.as_str() {
            "stdio" => {
                ensure!(
                    !self.command.trim().is_empty(),
                    "a stdio MCP server needs command"
                );
                ensure!(
                    self.url.is_empty() && self.headers.is_empty(),
                    "stdio takes no url or headers"
                );
            }
            "http" | "sse" => {
                let url = url::Url::parse(&self.url).context("invalid MCP server url")?;
                ensure!(
                    url.scheme() == "https"
                        || (url.scheme() == "http"
                            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))),
                    "MCP server url must be https, or http on loopback"
                );
                ensure!(
                    self.command.is_empty() && self.args.is_empty() && self.env.is_empty(),
                    "http/sse takes no command, args or env"
                );
            }
            _ => anyhow::bail!("transport must be stdio, http or sse"),
        }
        ensure!(
            self.args.len() <= 64 && self.env.len() <= 64 && self.headers.len() <= 32,
            "too many args, env or headers"
        );
        for value in self
            .args
            .iter()
            .chain(self.env.values())
            .chain(self.headers.values())
            .chain([&self.command, &self.url])
        {
            ensure!(
                value.len() <= 16384 && !value.contains('\0'),
                "invalid MCP server value"
            );
        }
        for key in self.env.keys().chain(self.headers.keys()) {
            ensure!(
                !key.is_empty() && key.len() <= 128 && !key.contains(['\0', '\n', '\r', ':', '=']),
                "invalid env or header name"
            );
        }
        ensure!(
            self.agents.iter().all(|a| valid_name(a)),
            "invalid agent id"
        );
        Ok(())
    }

    fn redacted(&self) -> McpServer {
        let hide = |map: &BTreeMap<String, String>| {
            map.keys()
                .map(|k| (k.clone(), "<redacted>".to_string()))
                .collect()
        };
        McpServer {
            env: hide(&self.env),
            headers: hide(&self.headers),
            ..self.clone()
        }
    }

    fn applies_to(&self, agent_id: &str, caps: &NegotiatedCaps) -> bool {
        self.enabled
            && (self.agents.is_empty() || self.agents.iter().any(|a| a == agent_id))
            && match self.transport.as_str() {
                "stdio" => true,
                "http" => caps.mcp_http,
                "sse" => caps.mcp_sse,
                _ => false,
            }
    }

    /// ACP `McpServer` wire shape.
    fn to_acp(&self) -> Value {
        let pairs = |map: &BTreeMap<String, String>| {
            map.iter()
                .map(|(name, value)| json!({"name": name, "value": value}))
                .collect::<Vec<_>>()
        };
        match self.transport.as_str() {
            "stdio" => json!({
                "name": self.name,
                "command": self.command,
                "args": self.args,
                "env": pairs(&self.env),
            }),
            kind => json!({
                "type": kind,
                "name": self.name,
                "url": self.url,
                "headers": pairs(&self.headers),
            }),
        }
    }
}

/// The servers for one session of `agent_id`. A registry that cannot be read
/// yields none, so a broken file never blocks starting a thread.
pub(crate) fn for_session(agent_id: &str, caps: &NegotiatedCaps) -> Value {
    match read() {
        Ok(registry) => Value::Array(
            registry
                .servers
                .iter()
                .filter(|s| s.applies_to(agent_id, caps))
                .map(McpServer::to_acp)
                .collect(),
        ),
        Err(error) => {
            tracing::warn!(%error, "ignoring the MCP server registry");
            json!([])
        }
    }
}

pub fn list_redacted() -> Result<Vec<McpServer>> {
    Ok(read()?.servers.iter().map(McpServer::redacted).collect())
}

/// Add or replace a server by name. Takes effect for sessions started or loaded later.
pub fn upsert(server: McpServer) -> Result<McpServer> {
    server.validate()?;
    let mut registry = read()?;
    registry.servers.retain(|s| s.name != server.name);
    ensure!(
        registry.servers.len() < MAX_SERVERS,
        "at most {MAX_SERVERS} MCP servers"
    );
    let shown = server.redacted();
    registry.servers.push(server);
    registry.servers.sort_by(|a, b| a.name.cmp(&b.name));
    write(&registry)?;
    Ok(shown)
}

pub fn remove(name: &str) -> Result<()> {
    let mut registry = read()?;
    let before = registry.servers.len();
    registry.servers.retain(|s| s.name != name);
    ensure!(registry.servers.len() < before, "MCP server not found");
    write(&registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(http: bool) -> NegotiatedCaps {
        NegotiatedCaps {
            mcp_http: http,
            ..Default::default()
        }
    }

    #[test]
    fn filters_by_agent_and_transport_and_uses_the_acp_shape() {
        let stdio = McpServer {
            name: "files".into(),
            transport: "stdio".into(),
            command: "files-mcp".into(),
            env: [("TOKEN".to_string(), "t".to_string())].into(),
            agents: vec!["deepseek".into()],
            enabled: true,
            ..Default::default()
        };
        let http = McpServer {
            name: "slack".into(),
            transport: "http".into(),
            url: "https://mcp.example.com/mcp".into(),
            headers: [("Authorization".to_string(), "Bearer x".to_string())].into(),
            enabled: true,
            ..Default::default()
        };
        stdio.validate().unwrap();
        http.validate().unwrap();
        assert!(stdio.applies_to("deepseek", &caps(false)));
        assert!(!stdio.applies_to("codex", &caps(true)));
        assert!(
            !http.applies_to("deepseek", &caps(false)),
            "http needs mcpCapabilities.http"
        );
        assert!(http.applies_to("grok", &caps(true)));
        assert_eq!(
            stdio.to_acp(),
            json!({"name":"files","command":"files-mcp","args":[],"env":[{"name":"TOKEN","value":"t"}]})
        );
        assert_eq!(http.to_acp()["type"], "http");
        assert_eq!(http.to_acp()["headers"][0]["value"], "Bearer x");
        assert_eq!(http.redacted().headers["Authorization"], "<redacted>");
        assert!(McpServer {
            url: "http://example.com".into(),
            ..http.clone()
        }
        .validate()
        .is_err());
        assert!(McpServer {
            name: "bad name".into(),
            ..stdio
        }
        .validate()
        .is_err());
    }
}
