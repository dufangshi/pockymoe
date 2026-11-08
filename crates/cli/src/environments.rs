use anyhow::{ensure, Context, Result};
use clap::Subcommand;
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum EnvironmentCommand {
    /// Discover non-secret connections, limits and supported authentication modes.
    Connections,
    /// Submit metadata only and return a receipt; only the owner can approve in Web.
    #[command(after_long_help = r#"Rules:
  The current thread comes from the authenticated CLI credential; no --thread or
  --from override can grant access to another thread. This command returns before
  migration. The owner reviews exact immutable paths, permissions, resources and
  HTTPS hosts. Approval waits for the current turn and admitted operations to end.
  Do not include keys, OAuth tokens, credential contents or secret environment values.
  NativeFile maps the original file store directly, not a copy. Writable refresh
  requires explicit approval of the whole config directory; read-only auth.json or
  .credentials.json cannot persist refresh. System keyrings/Keychain and SSH are
  unsupported. All other credentials stay unmapped unless explicitly proposed.

Example metadata (replace paths and connection ID):
  {"connectionId":"ID","authentication":{"codex":"apiBroker","claude":"apiBroker"},
   "egressHosts":["github.com","api.github.com"],"credentialMounts":[],
   "handoff":"Current goal, progress, working tree, remaining work and constraints"}

The original Pockymoe threadId/model/effort are retained. A fresh native session
receives a bounded journal projection and the handoff; neither replays a tool.
Changed specs need a new request and approval. Use --request-id for safe retries."#)]
    Request {
        #[arg(long)]
        spec: PathBuf,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// List this authenticated thread's requests and persistent states.
    Requests,
    /// Inspect this thread's immutable proposal and migration receipt.
    Show { id: String },
    /// Revoke a pending/waiting proposal before the Host fence is established.
    Cancel { id: String },
}

impl EnvironmentCommand {
    fn request(self) -> Result<Value> {
        Ok(match self {
            Self::Connections => json!({"operation":"environmentConnections"}),
            Self::Requests => json!({"operation":"environmentRequests"}),
            Self::Show { id } => json!({"operation":"environmentRequestShow","requestId":id}),
            Self::Cancel { id } => json!({"operation":"environmentRequestCancel","requestId":id}),
            Self::Request { spec, request_id } => {
                let bytes =
                    std::fs::read(spec).context("cannot read metadata-only environment spec")?;
                ensure!(
                    bytes.len() <= 128 * 1024,
                    "environment spec exceeds 128 KiB"
                );
                let mut spec:pockymoe_protocol::ProposeEnvironmentRequest=serde_json::from_slice(&bytes).map_err(|_|anyhow::anyhow!("invalid metadata-only environment spec (unknown fields, including secret values, are rejected)"))?;
                if let Some(id) = request_id {
                    spec.client_request_id = Some(id);
                }
                json!({"operation":"environmentRequest","spec":spec})
            }
        })
    }
}

impl crate::threads::Client {
    pub async fn environment(&self, command: EnvironmentCommand) -> Result<Value> {
        let mut input = command.request()?;
        // The server uses the credential's identity. `from` is advisory only.
        input["fromThreadId"] = json!(self.from);
        self.request(input).await
    }
}
