use anyhow::{ensure, Context, Result};
use clap::{Args, Subcommand};
use serde_json::{json, Value};
use std::{io::Read, path::PathBuf};

#[derive(Args)]
pub struct Connection {
    #[arg(long, global = true, env = "POCKYMOE_URL")]
    pub url: Option<String>,
    #[arg(long, global = true, env = "POCKYMOE_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
    #[arg(long, global = true, env = "POCKYMOE_THREAD_ID")]
    pub from: Option<String>,
    #[arg(long, global = true, env = "POCKYMOE_CLI_CONFIG")]
    pub cli_config: Option<PathBuf>,
}
#[derive(Args)]
pub struct Body {
    /// inbox is passive (send default); queue assigns a task (create default); direct/steer require an urgent correction or unblock request plus --interrupt-reason for peer sends.
    #[arg(long, value_parser=["inbox","direct","queue","steer"])]
    pub delivery: Option<String>,
    /// Completion notifications are passive inbox mail; they do not immediately wake or queue the caller.
    #[arg(long, default_value="inbox", value_parser=["inbox"], requires="notify_on_complete")]
    pub notify_delivery: String,
    /// One-line summary, shown in `inbox list` and in the unread notice so the
    /// receiver can triage without opening the message. Strongly recommended.
    #[arg(long)]
    pub subject: Option<String>,
    /// What this message is for. `question` is the only kind that implies you are
    /// waiting on a reply; the rest are informational.
    #[arg(long, value_parser=["result","question","status","task"])]
    pub kind: Option<String>,
    /// Message id this answers, so the exchange correlates.
    #[arg(long, value_name = "MESSAGE_ID")]
    pub in_reply_to: Option<String>,
    /// Why waiting for the next checkpoint would cause harm or wasted work.
    /// Required for peer direct/steer; ordinary results and progress stay in inbox.
    #[arg(long)]
    pub interrupt_reason: Option<String>,
    /// Replace older full inbox status snapshots on this topic, from you only.
    /// Never use for results, questions, replies or incremental patches.
    #[arg(long)]
    pub topic_key: Option<String>,
    #[arg(long, conflicts_with = "text_file")]
    pub text: Option<String>,
    #[arg(long)]
    pub text_file: Option<PathBuf>,
    #[arg(long)]
    pub notify_on_complete: bool,
    #[arg(long)]
    pub request_id: Option<String>,
}
impl Body {
    fn text(&self) -> Result<Option<String>> {
        Ok(if let Some(path) = &self.text_file {
            Some(if path.as_os_str() == "-" {
                let mut s = String::new();
                std::io::stdin().take(262145).read_to_string(&mut s)?;
                s
            } else {
                std::fs::read_to_string(path)?
            })
        } else {
            self.text.clone()
        })
    }
}
#[derive(Subcommand)]
pub enum PreviewCommand {
    /// Reserve a stable private preview address BEFORE starting an HTTP service.
    #[command(after_long_help = r#"Rules:
  - Reserve the address BEFORE starting the HTTP service; bind the service to 127.0.0.1
    on this port (Vite: --strictPort).
  - Add only the returned hostname/origin to a framework allowlist, and only if the
    framework needs it; never `*` or disabled host checks.
  - Give the user `openUrl`. Never copy tokens or launch tickets into reports.

More: pockymoe guide preview"#)]
    Create {
        #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
        #[arg(long, default_value = "")]
        label: String,
        #[arg(long, default_value = "/")]
        path: String,
    },
    /// Show enabled mappings, exact allowlist hostnames and browser entry URLs.
    List,
    /// Probe local HTTP and optionally a known WebSocket/HMR endpoint.
    Check {
        /// Mapping ID or local port.
        target: String,
        #[arg(long, default_value = "/")]
        path: String,
        /// Exact local endpoint; handshake acceptance alone does not verify browser HMR.
        #[arg(long)]
        websocket_path: Option<String>,
    },
    /// Revoke a mapping and terminate its preview connections.
    Stop { target: String },
}

#[derive(Subcommand)]
pub enum ThreadCommand {
    /// Current Pockymoe thread identity and status.
    #[command(name = "self")]
    SelfInfo,
    /// Threads a person started, with a count of the agent threads under each.
    List {
        /// Relay device ID or unique device name; defaults to this device.
        #[arg(long)]
        device: Option<String>,
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// Include agent-created threads flat, instead of only lineage roots.
        #[arg(long)]
        all: bool,
        /// List the descendants of one root instead of the roots themselves.
        #[arg(long, value_name = "THREAD_ID")]
        group: Option<String>,
    },
    Show {
        id: String,
        #[arg(long)]
        device: Option<String>,
    },
    Status {
        id: String,
        #[arg(long)]
        device: Option<String>,
    },
    /// Delete your own finished or unused direct child. Running/queued children and children with descendants are refused.
    Delete { id: String },
    Backends {
        #[arg(long)]
        device: Option<String>,
    },
    Models {
        #[arg(long)]
        device: Option<String>,
        /// Workspace used for model discovery on the target device.
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long, default_value = "acp")]
        provider: String,
        #[arg(long)]
        agent: Option<String>,
    },
    #[command(after_long_help = r#"Rules:
  - Label the initial prompt with --kind and --subject. It defaults to queue delivery, so
    the new thread starts working; --delivery inbox makes it passive.
  - Give delegates a --name, wait with `thread wait NAME`, and `thread close NAME` once
    collected. Limits: depth 3 and 20 open threads per root; do not retry a refusal.
  - The model defaults to `default`, not yours. Pass explicitly requested models as given.
  - --worktree branches from COMMITTED HEAD; commit what the delegate builds on first.
  - If sending fails after creation, reuse the returned thread ID instead of creating again.

More: pockymoe guide delegate"#)]
    Create {
        /// Other device; requires --workspace and creates no local lineage.
        #[arg(long)]
        device: Option<String>,
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, default_value = "acp")]
        provider: String,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, default_value = "default")]
        model: String,
        #[arg(long)]
        reasoning_effort: Option<String>,
        #[arg(long,value_parser=["guarded","yolo"])]
        approval_mode: Option<String>,
        /// Address for this delegate, unique among open threads in your lineage
        /// (`reviewer`). Every command that takes a thread id accepts it.
        #[arg(long)]
        name: Option<String>,
        /// Start from `.pockymoe/agents/ROLE.md` (workspace, then home): its
        /// model/effort/agent become defaults and its body prefixes the first prompt.
        #[arg(long)]
        role: Option<String>,
        /// Work in an isolated git worktree (sibling `REPO.worktrees/NAME`, branch
        /// `agent/NAME`) checked out from HEAD. Merge its branch when done.
        #[arg(long)]
        worktree: bool,
        /// Branch for --worktree; an existing branch is checked out as is.
        #[arg(long, value_name = "BRANCH")]
        worktree_branch: Option<String>,
        #[command(flatten)]
        body: Body,
    },
    /// Block until the threads settle (finish, fail, or go idle), one blocks on an
    /// approval, or the timeout passes. Returns each one's closing message.
    #[command(after_long_help = r#"Rules:
  - Block here when you will act on the result; your shell may return early, so keep
    waiting on the same command.
  - With nothing else to do for a long time, use --wake and end your turn saying what you
    wait for: exactly one turn is queued on you when they settle. Unread inbox mail has a separate 10-minute idle reminder.
  - `blocked: true` means a delegate waits on an approval or your answer (`waitingOn`).
  - Settled is not success: check the evidence in each closing message.

More: pockymoe guide delegate"#)]
    Wait {
        #[arg(required = true, num_args = 1..)]
        ids: Vec<String>,
        /// Return as soon as any one settles instead of all of them.
        #[arg(long)]
        any: bool,
        /// Seconds to wait (max 1800). Keep it below your shell tool's own timeout.
        #[arg(long, default_value_t = 300)]
        timeout: u64,
        /// Do not block: queue one turn on yourself when they all settle, so you can
        /// end your turn now.
        #[arg(long, conflicts_with_all = ["any", "timeout"])]
        wake: bool,
    },
    /// Your lineage at a glance: every delegate's state, unread mail, current task
    /// and worktree, plus task counts.
    Tree {
        /// A root thread id; defaults to your own lineage.
        root: Option<String>,
        /// Include closed threads.
        #[arg(long)]
        all: bool,
    },
    /// Close a finished delegate: frees its slot under the 20-thread cap and its
    /// name. History stays readable; prompting it again reopens it.
    Close {
        #[arg(required = true, num_args = 1..)]
        ids: Vec<String>,
        /// Also `git worktree remove` its worktree (fails if it has uncommitted changes).
        #[arg(long)]
        remove_worktree: bool,
    },
    /// Role templates available to `thread create --role`.
    Roles,
    /// Send passive mail or request execution; direct/steer also await the steering acknowledgement when running.
    #[command(after_long_help = r#"Choosing --delivery:
  inbox (default)  results, progress, ready inputs, questions, acknowledgements
  queue            distinct work (--kind task) that can wait for the peer's whole turn
  steer / direct   correct, stop or reprioritize ACTIVE work, with --interrupt-reason
                   naming the concrete harm; direct when the peer's state is uncertain
Never queue a correction, and never use direct because a result is important or the
peer is idle. Always pass --kind and --subject; use --in-reply-to when answering.
Read the whole JSON receipt: `queued` is acceptance, `steered` is acknowledgement,
`held` needs inspection rather than retries. Use --request-id for a send you may retry.

More: pockymoe guide messaging"#)]
    Send {
        id: String,
        #[arg(long)]
        device: Option<String>,
        /// Copy a local file or zip a directory to the remote thread. Repeatable, up to 20.
        #[arg(long, value_name = "PATH")]
        attach: Vec<PathBuf>,
        #[command(flatten)]
        body: Body,
    },
}
#[derive(Args)]
pub struct Transcript {
    pub id: String,
    #[arg(long)]
    pub device: Option<String>,
    #[arg(long, default_value_t = 3)]
    pub limit: u32,
    #[arg(long, conflicts_with = "turn")]
    pub before_turn: Option<String>,
    #[arg(long)]
    pub turn: Option<String>,
    #[arg(long, requires = "turn")]
    pub item: Option<String>,
    #[arg(long,value_parser=["overview","turn"])]
    pub view: Option<String>,
    #[arg(long, default_value_t = 0)]
    pub offset: u32,
    #[arg(long, default_value_t = 0)]
    pub text_offset: u32,
    #[arg(long, requires = "item")]
    pub raw: bool,
}

#[derive(Subcommand)]
pub enum DeviceCommand {
    /// Devices belonging to this relay owner, including this device.
    List,
    /// Inspect access, or enable/disable it using the local machine credential.
    Access {
        #[arg(value_parser = ["on", "off"])]
        enabled: Option<String>,
    },
    /// Forget a peer's pinned identity after verifying its new fingerprint.
    Trust {
        device: String,
        #[arg(long, required = true)]
        reset: bool,
    },
    /// Workspaces available on another device.
    Workspaces { device: String },
}

#[derive(Subcommand)]
pub enum FsCommand {
    /// List one directory in a remote workspace.
    Ls {
        device: String,
        #[arg(long)]
        workspace: String,
        #[arg(default_value = ".")]
        path: String,
    },
    /// Download a remote workspace file and verify its hash.
    Get {
        device: String,
        #[arg(long)]
        workspace: String,
        path: String,
        /// Local destination; defaults to the caller's .temp downloads directory.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, PartialEq)]
struct ThreadTarget {
    thread_id: String,
    device_id: Option<String>,
}
impl ThreadTarget {
    fn apply(&self, input: &mut Value) {
        input["threadId"] = json!(self.thread_id);
        if let Some(device) = &self.device_id {
            input["deviceId"] = json!(device);
        }
    }
}

fn thread_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
}

fn parse_target(value: &str, device: Option<&str>) -> Result<Option<ThreadTarget>> {
    ensure!(
        device.is_none_or(|d| !d.trim().is_empty()),
        "device is required"
    );
    let (target_device, thread) = if thread_url(value) {
        let url = reqwest::Url::parse(value)?;
        let parts = url
            .path_segments()
            .context("invalid thread URL")?
            .collect::<Vec<_>>();
        ensure!(
            parts.len() == 4
                && parts[0] == "devices"
                && parts[2] == "threads"
                && !parts[1].is_empty(),
            "expected /devices/DEVICE/threads/THREAD URL"
        );
        (Some(parts[1].to_owned()), parts[3].to_owned())
    } else if let Some((device, thread)) = value.split_once('/') {
        ensure!(
            !device.is_empty() && !thread.contains('/'),
            "expected DEVICE/THREAD with a thread UUID"
        );
        (Some(device.to_owned()), thread.to_owned())
    } else if let Ok(id) = uuid::Uuid::parse_str(value) {
        return Ok(Some(ThreadTarget {
            thread_id: id.to_string(),
            device_id: device.map(str::to_owned),
        }));
    } else {
        ensure!(device.is_none(), "thread names, self, parent and root only resolve locally; use a thread UUID on another device");
        return Ok(None);
    };
    ensure!(
        device.is_none_or(|d| target_device
            .as_deref()
            .is_some_and(|target| target.eq_ignore_ascii_case(d))),
        "--device conflicts with the device in the target"
    );
    Ok(Some(ThreadTarget {
        thread_id: uuid::Uuid::parse_str(&thread)
            .context("remote thread target must be a UUID")?
            .to_string(),
        device_id: target_device,
    }))
}

fn same_device(info: &Value, device: &str) -> bool {
    ["relayDeviceId", "deviceId"].iter().any(|key| {
        info[key]
            .as_str()
            .is_some_and(|id| id.eq_ignore_ascii_case(device))
    })
}

pub struct Client {
    url: String,
    token: String,
    pub from: Option<String>,
    http: reqwest::Client,
}
impl Client {
    pub fn new(c: Connection) -> Result<Self> {
        let path = match c.cli_config {
            Some(path) => path,
            None => {
                // The configured Supervisor, else a local-mode install, else
                // the relay device installed by setup.
                let configured = pockymoe_runtime::RuntimeConfig::from_env()?
                    .database_url
                    .with_extension("cli.json");
                let home = std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(std::path::PathBuf::from)
                    .unwrap_or_default()
                    .join(".remote-codex");
                [
                    configured.clone(),
                    home.join("supervisor.cli.json"),
                    home.join("relay-supervisor.cli.json"),
                ]
                .into_iter()
                .find(|path| path.is_file())
                .unwrap_or(configured)
            }
        };
        let saved: Value = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null);
        let url=c.url.or_else(||saved["url"].as_str().map(str::to_owned)).context("No local Supervisor connection. Set POCKYMOE_URL and POCKYMOE_TOKEN, or --cli-config PATH.")?;
        let parsed = reqwest::Url::parse(&url)?;
        ensure!(
            matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
                && parsed.scheme() == "http",
            "CLI requires a local loopback HTTP Supervisor URL"
        );
        let token = c
            .token
            .or_else(|| saved["token"].as_str().map(str::to_owned))
            .context("Local CLI token is missing")?;
        Ok(Self {
            url,
            token,
            from: c.from,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }
    pub(crate) async fn request(&self, input: Value) -> Result<Value> {
        self.request_for(input, 0).await
    }
    pub async fn skill(&self) -> Result<String> {
        let value = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.request(json!({"operation":"skill"})),
        )
        .await??;
        Ok(value["text"]
            .as_str()
            .context("Supervisor did not return a skill")?
            .to_owned())
    }
    pub async fn guide(&self, topic: &str) -> Result<String> {
        let value = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.request(json!({"operation":"guide","topic":topic})),
        )
        .await??;
        Ok(value["text"]
            .as_str()
            .context("Supervisor did not return a guide")?
            .to_owned())
    }
    /// `wait_seconds` extends the HTTP timeout for operations that block server-side.
    pub(crate) async fn request_for(&self, input: Value, wait_seconds: u64) -> Result<Value> {
        let response = self
            .http
            .post(format!("{}/api/cli", self.url.trim_end_matches('/')))
            .bearer_auth(&self.token)
            .timeout(std::time::Duration::from_secs(120 + wait_seconds.min(1800)))
            .json(&input)
            .send()
            .await?;
        let status = response.status();
        let value: Value = response.json().await?;
        ensure!(status.is_success(), "HTTP {status}: {value}");
        Ok(value)
    }
    async fn target(&self, value: &str, device: Option<&str>) -> Result<ThreadTarget> {
        if let Some(mut target) = parse_target(value, device)? {
            if thread_url(value) {
                let info = self.request(json!({"operation":"info"})).await?;
                if target
                    .device_id
                    .as_deref()
                    .is_some_and(|d| same_device(&info, d))
                {
                    target.device_id = None;
                }
            }
            return Ok(target);
        }
        let found = self
            .request(json!({"operation":"resolve","name":value,"fromThreadId":self.from}))
            .await?;
        Ok(ThreadTarget {
            thread_id: found["threadId"]
                .as_str()
                .context("resolve returned no thread")?
                .into(),
            device_id: None,
        })
    }
    pub(crate) async fn id(&self, value: &str) -> Result<String> {
        let target = self.target(value, None).await?;
        ensure!(
            target.device_id.is_none(),
            "this operation is not available across devices"
        );
        Ok(target.thread_id)
    }
    async fn send(
        &self,
        target: &ThreadTarget,
        attachments: &[PathBuf],
        body: &Body,
        default_delivery: &str,
    ) -> Result<Value> {
        let text = body
            .text()?
            .context("send requires --text or --text-file")?;
        let mut input = json!({"operation":"send","text":text,"delivery":body.delivery.as_deref().unwrap_or(default_delivery),"notifyDelivery":body.notify_delivery,"fromThreadId":self.from,"notifyOnComplete":body.notify_on_complete,"clientRequestId":body.request_id,"subject":body.subject,"kind":body.kind,"inReplyTo":body.in_reply_to,"interruptReason":body.interrupt_reason,"topicKey":body.topic_key});
        target.apply(&mut input);
        if !attachments.is_empty() {
            ensure!(
                attachments.len() <= 20,
                "at most 20 attachments are allowed"
            );
            input["attachments"] = json!(attachments
                .iter()
                .map(std::fs::canonicalize)
                .collect::<std::io::Result<Vec<_>>>()?);
        }
        self.request(input).await
    }
    pub async fn thread(&self, command: ThreadCommand) -> Result<Value> {
        match command {
            ThreadCommand::SelfInfo => self.request(json!({"operation":"status","threadId":self.from.as_ref().context("Current thread is unknown; use --from ID")?})).await,
            ThreadCommand::List { device, workspace, limit, all, group } => {
                let mut input = json!({"operation":"list","workspaceId":workspace,"limit":limit,"includeAgentThreads":all,"groupId":group});
                if let Some(device) = device { input["deviceId"] = json!(device); }
                self.request(input).await
            }
            ThreadCommand::Show { id, device } | ThreadCommand::Status { id, device } => {
                let mut input = json!({"operation":"status"});
                self.target(&id, device.as_deref()).await?.apply(&mut input);
                self.request(input).await
            }
            ThreadCommand::Delete { id } => self.request(json!({"operation":"delete","threadId":self.id(&id).await?,"fromThreadId":self.from})).await,
            ThreadCommand::Backends { device } => {
                let mut input = json!({"operation":"backends"});
                if let Some(device) = device { input["deviceId"] = json!(device); }
                self.request(input).await
            }
            ThreadCommand::Models { device, workspace, provider, agent } => {
                let mut input = json!({"operation":"models","provider":provider,"agentId":agent,"fromThreadId":self.from});
                if let Some(device) = device { input["deviceId"] = json!(device); }
                if let Some(workspace) = workspace { input["workspaceId"] = json!(workspace); }
                self.request(input).await
            }
            ThreadCommand::Send { id, device, attach, body } => self.send(&self.target(&id, device.as_deref()).await?, &attach, &body, "inbox").await,
            ThreadCommand::Wait { ids, any, timeout, wake } => {
                let mut resolved = Vec::new();
                for id in &ids { resolved.push(self.id(id).await?); }
                if wake {
                    return self.request(json!({"operation":"wake","threadIds":resolved,"fromThreadId":self.from})).await;
                }
                self.request_for(json!({"operation":"wait","threadIds":resolved,"any":any,"timeoutSeconds":timeout,"fromThreadId":self.from}), timeout).await
            }
            ThreadCommand::Tree { root, all } => {
                let root = match root { Some(r) => Some(self.id(&r).await?), None => None };
                self.request(json!({"operation":"tree","rootThreadId":root,"all":all,"fromThreadId":self.from})).await
            }
            ThreadCommand::Close { ids, remove_worktree } => {
                let (mut closed, mut failed) = (Vec::new(), Vec::new());
                for id in &ids {
                    let result = match self.id(id).await {
                        Ok(resolved) => self.request(json!({"operation":"close","threadId":resolved,"removeWorktree":remove_worktree,"fromThreadId":self.from})).await,
                        Err(e) => Err(e),
                    };
                    match result { Ok(v) => closed.push(v), Err(e) => failed.push(json!({"thread":id,"error":e.to_string()})) }
                }
                Ok(json!({"closed":closed,"failed":failed}))
            }
            ThreadCommand::Roles => self.request(json!({"operation":"roles","fromThreadId":self.from})).await,
            ThreadCommand::Create { device, workspace, title, provider, agent, model, reasoning_effort, approval_mode, name, role, worktree, worktree_branch, body } => {
                ensure!(device.is_none() || workspace.is_some(), "cross-device create requires --workspace");
                ensure!(!body.notify_on_complete || body.text.is_some() || body.text_file.is_some(), "notification requires an initial prompt");
                ensure!(!body.notify_on_complete || self.from.is_some(), "notification requires --from ID or managed thread context");
                let mut input = json!({"operation":"create","title":title.clone().or_else(||name.clone()),"provider":provider,"agentId":agent,"model":model,"reasoningEffort":reasoning_effort,"fromThreadId":self.from,"name":name,"role":role,"worktree":worktree,"worktreeBranch":worktree_branch});
                if let Some(device) = &device { input["deviceId"] = json!(device); }
                if let Some(ws) = workspace { input["workspaceId"] = json!(ws); }
                if let Some(mode) = approval_mode { input["approvalMode"] = json!(mode); }
                let mut result = self.request(input).await?;
                if body.text.is_some() || body.text_file.is_some() {
                    let id = result["threadId"].as_str().context("create returned no thread ID")?.to_string();
                    let target = ThreadTarget { thread_id: id.clone(), device_id: device };
                    let mut body = body;
                    if body.kind.is_none() { body.kind = Some("task".into()); }
                    if body.subject.is_none() { body.subject = title.clone(); }
                    result["send"] = self.send(&target, &[], &body, "queue").await.with_context(||format!("Thread {} was created, but initial send failed; reuse this thread", target.device_id.as_ref().map(|d| format!("{d}/{id}")).unwrap_or(id)))?;
                }
                Ok(result)
            }
        }
    }
    pub async fn transcript(&self, q: Transcript) -> Result<Value> {
        let mut input = json!({"operation":"transcript","limit":q.limit,"beforeTurnId":q.before_turn,"turnId":q.turn,"itemId":q.item,"view":q.view,"offset":q.offset,"textOffset":q.text_offset,"raw":q.raw});
        self.target(&q.id, q.device.as_deref())
            .await?
            .apply(&mut input);
        self.request(input).await
    }
    pub async fn preview(&self, command: PreviewCommand) -> Result<Value> {
        let input = match command {
            PreviewCommand::Create { port, label, path } => {
                json!({"operation":"previewCreate","port":port,"label":label,"path":path})
            }
            PreviewCommand::List => json!({"operation":"previewList"}),
            PreviewCommand::Check {
                target,
                path,
                websocket_path,
            } => {
                json!({"operation":"previewCheck","target":target,"path":path,"websocketPath":websocket_path})
            }
            PreviewCommand::Stop { target } => json!({"operation":"previewStop","target":target}),
        };
        self.request(input).await.context("Preview command requires a Supervisor and Relay with CLI preview support; update older installations")
    }
    pub async fn device(&self, command: DeviceCommand) -> Result<Value> {
        let input = match command {
            DeviceCommand::List => json!({"operation":"devices"}),
            DeviceCommand::Access { enabled } => {
                let mut input = json!({"operation":"peerAccess"});
                if let Some(enabled) = enabled {
                    input["enabled"] = json!(enabled == "on");
                }
                input
            }
            DeviceCommand::Trust { device, reset } => {
                json!({"operation":"peerTrust","deviceId":device,"reset":reset})
            }
            DeviceCommand::Workspaces { device } => {
                json!({"operation":"workspaces","deviceId":device})
            }
        };
        self.request(input).await
    }
    pub async fn fs(&self, command: FsCommand) -> Result<Value> {
        let input = match command {
            FsCommand::Ls {
                device,
                workspace,
                path,
            } => {
                json!({"operation":"fsList","deviceId":device,"workspaceId":workspace,"path":path})
            }
            FsCommand::Get {
                device,
                workspace,
                path,
                out,
            } => {
                let out = out
                    .map(|p| -> Result<PathBuf> {
                        Ok(if p.is_absolute() {
                            p
                        } else {
                            std::env::current_dir()?.join(p)
                        })
                    })
                    .transpose()?;
                json!({"operation":"fsGet","deviceId":device,"workspaceId":workspace,"path":path,"out":out,"fromThreadId":self.from})
            }
        };
        self.request(input).await
    }
    pub async fn outbox(&self) -> Result<Value> {
        self.request(json!({"operation":"outbox"})).await
    }
}

#[derive(Args)]
pub struct Inbox {
    /// Defaults to the managed caller's Pockymoe identity.
    #[arg(long, global = true)]
    pub thread: Option<String>,
    #[command(subcommand)]
    pub command: Option<InboxCommand>,
}
#[derive(Subcommand)]
pub enum InboxCommand {
    /// List bounded previews. Reading never marks mail as acknowledged.
    List {
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        before: Option<String>,
        #[arg(long)]
        all: bool,
        /// Only mail from these threads (ids or names). Repeatable.
        #[arg(long = "from-thread", value_name = "ID_OR_NAME")]
        from_threads: Vec<String>,
        /// Only these kinds. Repeatable; include question while waiting on results.
        #[arg(long, value_parser=["result","question","status","task"])]
        kind: Vec<String>,
    },
    /// Read one message, expanding long text in bounded chunks.
    Read {
        id: String,
        #[arg(long, default_value_t = 0)]
        text_offset: u32,
    },
    /// Acknowledge messages after handling them; keep them accessible with list --all.
    Ack {
        #[arg(required=true,num_args=1..)]
        ids: Vec<String>,
    },
    /// Block until unacknowledged mail (optionally from given threads or of given
    /// kinds) is waiting, and return it with text inline. Stays in your turn
    /// without polling.
    Wait {
        /// Only mail from these threads (ids or names). Repeatable.
        #[arg(long = "from-thread", value_name = "ID_OR_NAME")]
        from_threads: Vec<String>,
        /// Only these kinds (result, question, status, task). Repeatable.
        #[arg(long, value_parser=["result","question","status","task"])]
        kind: Vec<String>,
        /// Ignore mail already waiting when the wait starts.
        #[arg(long)]
        new: bool,
        /// Seconds to wait (max 1800). Keep it below your shell tool's own timeout.
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
}
impl Client {
    pub async fn inbox(&self, args: Inbox) -> Result<Value> {
        let thread = args
            .thread
            .or_else(|| self.from.clone())
            .context("Current thread is unknown; pass --thread ID")?;
        let thread = self.id(&thread).await?;
        let mut input = match args.command.unwrap_or(InboxCommand::List {
            limit: 20,
            before: None,
            all: false,
            from_threads: vec![],
            kind: vec![],
        }) {
            InboxCommand::List {
                limit,
                before,
                all,
                from_threads,
                kind,
            } => {
                let mut from = Vec::new();
                for id in &from_threads {
                    from.push(self.id(id).await?);
                }
                json!({"operation":"inbox","limit":limit,"before":before,"all":all,"fromThreadIds":from,"kinds":kind})
            }
            InboxCommand::Read { id, text_offset } => {
                json!({"operation":"inboxRead","messageId":id,"textOffset":text_offset})
            }
            InboxCommand::Ack { ids } => json!({"operation":"inboxAck","messageIds":ids}),
            InboxCommand::Wait {
                from_threads,
                kind,
                new,
                timeout,
            } => {
                let mut from = Vec::new();
                for id in &from_threads {
                    from.push(self.id(id).await?);
                }
                let input = json!({"operation":"inboxWait","threadId":thread,"fromThreadIds":from,"kinds":kind,"onlyNew":new,"timeoutSeconds":timeout});
                return self.request_for(input, timeout).await;
            }
        };
        input["threadId"] = json!(thread);
        self.request(input).await
    }
}

/// A task board shared by every thread in one lineage.
#[derive(Subcommand)]
pub enum TaskCommand {
    /// Add a task. Use --after for dependencies and --assign to give it to a delegate.
    Add {
        title: String,
        #[arg(long, conflicts_with = "detail_file")]
        detail: Option<String>,
        #[arg(long)]
        detail_file: Option<PathBuf>,
        /// Task numbers that must complete first. Repeatable or comma-separated.
        #[arg(long, value_delimiter = ',')]
        after: Vec<i64>,
        /// Thread id or name that should do it; it gets passive mail.
        #[arg(long)]
        assign: Option<String>,
    },
    /// Open tasks with owner, dependencies and whether each is ready.
    List {
        /// Include completed and failed tasks.
        #[arg(long)]
        all: bool,
    },
    /// One task with its detail and result.
    Show { number: i64 },
    /// Take a task: the given one, or the lowest ready one assigned to you or nobody.
    /// `claimed: null` with `finished: false` means work is only blocked; use --wait.
    Claim {
        number: Option<i64>,
        /// Block while pending tasks are blocked on in-progress work, until one
        /// becomes ready or the board is finished.
        #[arg(long)]
        wait: bool,
        /// Seconds to wait (max 1800). Keep it below your shell tool's own timeout.
        #[arg(long, default_value_t = 300, requires = "wait")]
        timeout: u64,
    },
    /// Finish a task you own or created. Its creator gets the result as mail, and
    /// dependents become ready.
    Done {
        number: i64,
        #[arg(long, conflicts_with = "result_file")]
        result: Option<String>,
        #[arg(long)]
        result_file: Option<PathBuf>,
        /// Record failure instead; dependents stay blocked.
        #[arg(long)]
        failed: bool,
    },
    /// Hand an in-progress task back to the pool.
    Release { number: i64 },
}

fn read_text(inline: Option<String>, file: Option<PathBuf>) -> Result<Option<String>> {
    Ok(match file {
        Some(path) if path.as_os_str() == "-" => {
            let mut s = String::new();
            std::io::stdin().take(65537).read_to_string(&mut s)?;
            Some(s)
        }
        Some(path) => Some(std::fs::read_to_string(path)?),
        None => inline,
    })
}

impl Client {
    pub async fn task(&self, command: TaskCommand) -> Result<Value> {
        let from = self
            .from
            .clone()
            .context("Current thread is unknown; tasks belong to a lineage, pass --from ID")?;
        let input = match command {
            TaskCommand::Add {
                title,
                detail,
                detail_file,
                after,
                assign,
            } => {
                let assign = match assign {
                    Some(a) => Some(self.id(&a).await?),
                    None => None,
                };
                json!({"operation":"taskAdd","title":title,"detail":read_text(detail,detail_file)?,"after":after,"assignThreadId":assign})
            }
            TaskCommand::List { all } => json!({"operation":"taskList","all":all}),
            TaskCommand::Show { number } => json!({"operation":"taskShow","number":number}),
            TaskCommand::Claim {
                number,
                wait,
                timeout,
            } => {
                let input = json!({"operation":"taskClaim","number":number,"wait":wait,"timeoutSeconds":timeout,"fromThreadId":from});
                return self
                    .request_for(input, if wait { timeout } else { 0 })
                    .await;
            }
            TaskCommand::Done {
                number,
                result,
                result_file,
                failed,
            } => {
                json!({"operation":"taskDone","number":number,"result":read_text(result,result_file)?,"failed":failed})
            }
            TaskCommand::Release { number } => json!({"operation":"taskRelease","number":number}),
        };
        let mut input = input;
        input["fromThreadId"] = json!(from);
        self.request(input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const THREAD: &str = "00000000-0000-0000-0000-000000000001";
    const RELAY_DEVICE: &str = "00000000-0000-0000-0000-000000000002";
    const HOST: &str = "00000000-0000-0000-0000-000000000003";

    #[test]
    fn peer_cli_parses_qualified_ids_urls_and_local_names() {
        assert_eq!(
            parse_target(THREAD, None).unwrap().unwrap(),
            ThreadTarget {
                thread_id: THREAD.into(),
                device_id: None
            }
        );
        assert_eq!(
            parse_target(THREAD, Some("Treer"))
                .unwrap()
                .unwrap()
                .device_id
                .as_deref(),
            Some("Treer")
        );
        for device in ["Treer", RELAY_DEVICE] {
            let target = parse_target(&format!("{device}/{THREAD}"), None)
                .unwrap()
                .unwrap();
            assert_eq!(target.thread_id, THREAD);
            assert_eq!(target.device_id.as_deref(), Some(device));
        }
        let target = parse_target(
            &format!(
                "https://remote.example/devices/{RELAY_DEVICE}/threads/{THREAD}?view=full#item"
            ),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(target.device_id.as_deref(), Some(RELAY_DEVICE));
        for local in ["reviewer", "self", "parent", "root"] {
            assert!(parse_target(local, None).unwrap().is_none());
            assert!(parse_target(local, Some("Treer")).is_err());
            assert!(parse_target(&format!("Treer/{local}"), None).is_err());
        }
        for invalid in [
            "/00000000-0000-0000-0000-000000000001",
            "Treer/a/b",
            "https://remote.example/threads/00000000-0000-0000-0000-000000000001",
        ] {
            assert!(parse_target(invalid, None).is_err(), "{invalid}");
        }
        assert!(parse_target(&format!("Treer/{THREAD}"), Some("Desktop")).is_err());
        assert!(parse_target(THREAD, Some("")).is_err());
    }

    #[test]
    fn peer_cli_identifies_relay_and_legacy_host_ids_as_this_device() {
        let info = json!({"deviceId":HOST,"relayDeviceId":RELAY_DEVICE,"deviceName":"Treer"});
        assert!(same_device(&info, RELAY_DEVICE));
        assert!(same_device(&info, HOST));
        assert!(!same_device(&info, THREAD));
        assert!(same_device(&json!({"deviceId":HOST}), HOST));
        assert!(!same_device(&json!({"deviceId":HOST}), RELAY_DEVICE));
    }

    async fn mock_client(responses: Vec<Value>) -> (Client, tokio::task::JoinHandle<Vec<Value>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (body_start, body_len) = loop {
                    let mut chunk = [0; 4096];
                    let count = stream.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
                        let headers = std::str::from_utf8(&bytes[..end]).unwrap();
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while bytes.len() < body_start + body_len {
                    let mut chunk = [0; 4096];
                    let count = stream.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                }
                requests.push(
                    serde_json::from_slice(&bytes[body_start..body_start + body_len]).unwrap(),
                );
                let body = response.to_string();
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            }
            requests
        });
        (
            Client {
                url,
                token: "test-token".into(),
                from: Some(THREAD.into()),
                http: reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()
                    .unwrap(),
            },
            task,
        )
    }

    #[tokio::test]
    async fn peer_cli_routes_local_web_urls_locally_and_remote_web_urls_by_device() {
        for (url_device, expected_device) in
            [(RELAY_DEVICE, None), (HOST, None), (THREAD, Some(THREAD))]
        {
            let (client, task) = mock_client(vec![
                json!({"deviceId":HOST,"relayDeviceId":RELAY_DEVICE}),
                json!({"threadId":THREAD}),
            ])
            .await;
            let url = format!("https://remote.example/devices/{url_device}/threads/{THREAD}");
            client
                .thread(ThreadCommand::Status {
                    id: url,
                    device: None,
                })
                .await
                .unwrap();
            let requests = task.await.unwrap();
            assert_eq!(requests[0]["operation"], "info");
            assert_eq!(requests[1]["operation"], "status");
            assert_eq!(requests[1]["threadId"], THREAD);
            assert_eq!(requests[1]["deviceId"].as_str(), expected_device);
        }
        let (client, task) =
            mock_client(vec![json!({"deviceId":HOST}), json!({"threadId":THREAD})]).await;
        client
            .thread(ThreadCommand::Show {
                id: format!("http://old.example/devices/{HOST}/threads/{THREAD}"),
                device: None,
            })
            .await
            .unwrap();
        assert!(task.await.unwrap()[1].get("deviceId").is_none());
    }

    #[tokio::test]
    async fn peer_cli_keeps_remote_create_initial_prompt_on_the_selected_device() {
        let (client, task) = mock_client(vec![
            json!({"threadId":THREAD}),
            json!({"delivery":"queued"}),
        ])
        .await;
        let body = Body {
            delivery: None,
            notify_delivery: "inbox".into(),
            subject: None,
            kind: None,
            in_reply_to: None,
            interrupt_reason: None,
            topic_key: None,
            text: Some("do the task".into()),
            text_file: None,
            notify_on_complete: true,
            request_id: Some("stable".into()),
        };
        let result = client
            .thread(ThreadCommand::Create {
                device: Some("Treer".into()),
                workspace: Some("remote-ws".into()),
                title: Some("Task".into()),
                provider: "acp".into(),
                agent: None,
                model: "default".into(),
                reasoning_effort: None,
                approval_mode: None,
                name: None,
                role: None,
                worktree: false,
                worktree_branch: None,
                body,
            })
            .await
            .unwrap();
        assert_eq!(result["send"]["delivery"], "queued");
        let requests = task.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["operation"], "create");
        assert_eq!(requests[0]["workspaceId"], "remote-ws");
        assert_eq!(requests[1]["operation"], "send");
        assert_eq!(requests[1]["delivery"], "queue");
        assert_eq!(requests[1]["kind"], "task");
        assert_eq!(requests[1]["subject"], "Task");
        assert_eq!(requests[1]["notifyOnComplete"], true);
        assert!(requests
            .iter()
            .all(|r| r["deviceId"] == "Treer" && r["fromThreadId"] == THREAD));
    }
}
