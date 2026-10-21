//! DSH bridge. A plugin inserted with DSH's `--patch` into the ACP-owned process
//! reverse-connects after `appReady`, before ACP initialize/new, so the provider
//! catalog is complete. ACP keeps prompts, tools, permissions and cancellation;
//! the bridge carries metadata, allowlisted DSH Remote calls, projection views,
//! live assistant text and human questions. It never writes to the ACP stdio.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use pockymoe_protocol::{AgentProviderCapabilitiesDto, ModelOptionDto};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{tcp::OwnedWriteHalf, TcpListener},
    sync::{broadcast, oneshot, Mutex},
};

use super::modes::ProductSessionPolicy;

/// DSH reads its process-wide sandbox default from this variable at boot.
pub(super) const PERMISSION_ENV: &str = "DSH_PERMISSION_MODE";
const PROTOCOL: u64 = 1;
const MAX_LINE: u64 = 2 * 1024 * 1024;
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// `/compact` summarizes with a model call; mode commands return at once.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(600);
/// Commands that only switch session state; callers may hold session locks.
const MODE_COMMANDS: [&str; 2] = ["/permission", "/plan"];

pub(super) struct Launch {
    listener: TcpListener,
    token: String,
    directory: tempfile::TempDir,
}

/// DSH's own Web bundle patches (host plane, run modes, native console),
/// taken from the installed release so they always match its version.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Composition {
    pub patches: Vec<PathBuf>,
}

/// Find the Web bundle that ships with the `dsh` executable. Any doubt
/// (unknown layout, version skew) keeps the plain ACP composition.
pub(super) fn native_composition(executable: &Path) -> Option<Composition> {
    let web_app = std::env::var_os("POCKYMOE_DSH_WEB_APP")
        .map(PathBuf::from)
        .or_else(|| {
            let package = dsh_package(executable)?;
            [
                package.join("node_modules/@deepseek-ai/dsh-web-app"),
                package.parent()?.join("dsh-web-app"),
            ]
            .into_iter()
            .find(|candidate| candidate.join("package.json").is_file())
            .filter(|candidate| same_release(&package, candidate))
        })?;
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(web_app.join("package.json")).ok()?).ok()?;
    let patches: Vec<PathBuf> = manifest["dsh"]["bundle"]["patch"]
        .as_array()?
        .iter()
        .map(|patch| Some(web_app.join(patch.as_str()?)))
        .collect::<Option<_>>()?;
    (!patches.is_empty() && patches.iter().all(|patch| patch.is_file()))
        .then_some(Composition { patches })
}

fn dsh_package(executable: &Path) -> Option<PathBuf> {
    let resolved = std::fs::canonicalize(executable).ok()?;
    // npm links bin/dsh to <package>/lib/bin.js; Windows shims sit beside node_modules.
    [
        resolved.parent()?.parent().map(Path::to_path_buf),
        resolved
            .parent()
            .map(|dir| dir.join("node_modules/@deepseek-ai/dsh")),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| package_name(candidate).as_deref() == Some("@deepseek-ai/dsh"))
}

fn package_name(dir: &Path) -> Option<String> {
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(dir.join("package.json")).ok()?).ok()?;
    manifest["name"].as_str().map(str::to_string)
}

fn same_release(dsh: &Path, web_app: &Path) -> bool {
    let version = |dir: &Path| -> Option<String> {
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(dir.join("package.json")).ok()?).ok()?;
        manifest["version"].as_str().map(str::to_string)
    };
    version(dsh).is_some() && version(dsh) == version(web_app)
}

impl Launch {
    pub async fn prepare(command: &mut String, composition: Option<&Composition>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let token = uuid::Uuid::new_v4().to_string();
        let directory = tempfile::tempdir()?;
        let plugin = directory.path().join("remote-codex-bridge.mjs");
        std::fs::write(&plugin, include_str!("deepseek-bridge.mjs"))?;
        let mut patches: Vec<PathBuf> = Vec::new();
        if let Some(composition) = composition {
            patches.extend(composition.patches.iter().cloned());
            // The Web host stays private to this process: an OS-chosen loopback
            // port, no URL on stdout (ACP framing) and no browser launch.
            let overrides = directory.path().join("native.json");
            std::fs::write(
                &overrides,
                serde_json::to_vec(&json!([
                    {"id":"webserver","config":{"host":"127.0.0.1","port":0,"compression":"gzip",
                        "compressionLevel":1,"compressionThresholdBytes":1024}},
                    {"id":"web-runtime","config":{"openBrowser":false,"printUrl":false,
                        "surfaceContext":false,"trustedHosts":[]}},
                    {"id":"connection","config":{"trustedHosts":[]}}
                ]))?,
            )?;
            patches.push(overrides);
        }
        let patch = directory.path().join("patch.json");
        std::fs::write(
            &patch,
            serde_json::to_vec(&json!([{"insert":[{
                "id":"remote-codex-bridge", "name":plugin,
                "config":{"port":listener.local_addr()?.port(),"token":token}
            }]}]))?,
        )?;
        patches.push(patch);
        for patch in patches {
            command.push_str(" --patch ");
            if cfg!(windows) {
                // cmd.exe does not interpret POSIX single-quoted arguments.
                command.push_str(&format!("\"{}\"", patch.to_string_lossy()));
            } else {
                command.push_str(&shell_words::quote(&patch.to_string_lossy()));
            }
        }
        Ok(Self {
            listener,
            token,
            directory,
        })
    }

    /// Wait for the plugin's hello. Strangers without the token are dropped.
    pub async fn connect(self) -> Result<(Bridge, Value)> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            let (read, writer) = stream.into_split();
            let mut reader = BufReader::new(read);
            let mut line = String::new();
            if !matches!(
                tokio::time::timeout(
                    Duration::from_secs(3),
                    (&mut reader).take(MAX_LINE).read_line(&mut line)
                )
                .await,
                Ok(Ok(_))
            ) {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value["token"].as_str() != Some(self.token.as_str()) {
                continue;
            }
            if let Some(error) = value["error"].as_str() {
                bail!("DSH bridge: {error}");
            }
            let hello = value
                .get("hello")
                .cloned()
                .context("DSH bridge sent no hello")?;
            if hello["protocol"].as_u64() != Some(PROTOCOL) {
                bail!("DSH bridge protocol {} is not supported", hello["protocol"]);
            }
            let (events, _) = broadcast::channel(1024);
            let bridge = Bridge {
                writer: Arc::new(Mutex::new(writer)),
                pending: Default::default(),
                next_id: AtomicU64::new(1),
                events,
                closed: Default::default(),
                _directory: self.directory,
            };
            // Keep the buffered reader: events may already follow the hello.
            bridge.spawn_reader(reader);
            return Ok((bridge, hello));
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum BridgeEvent {
    Stream {
        session_id: String,
        attempt_id: String,
        kind: String,
        text: Option<String>,
        outcome: Option<String>,
    },
    Status {
        session_id: String,
        status: String,
    },
    Projection {
        session_id: String,
        key: String,
        value: Value,
    },
    Question {
        id: u64,
        session_id: String,
        questions: Value,
    },
    QuestionCancelled {
        id: u64,
    },
    /// DSH asks to approve a tool call (sandbox escalation) for the session
    /// or one of its subagents.
    Approval {
        id: u64,
        session_id: String,
        request: Value,
    },
    /// One model call's tokens (the session's or a descendant subagent's),
    /// as a delta report for the turn's usage accumulator.
    Usage {
        session_id: String,
        report: Value,
    },
    /// A slow subscriber missed events; re-read the session state.
    Lagged,
    Closed,
}

impl BridgeEvent {
    fn parse(message: &Value) -> Option<Self> {
        let session_id = || message["sessionId"].as_str().map(str::to_string);
        let text = |key: &str| message[key].as_str().map(str::to_string);
        Some(match message["event"].as_str()? {
            "stream" => Self::Stream {
                session_id: session_id()?,
                attempt_id: text("attemptId")?,
                kind: text("kind")?,
                text: text("text"),
                outcome: text("outcome"),
            },
            "status" => Self::Status {
                session_id: session_id()?,
                status: text("status")?,
            },
            "projection" => Self::Projection {
                session_id: session_id()?,
                key: text("key")?,
                value: message["value"].clone(),
            },
            "question" => Self::Question {
                id: message["questionId"].as_u64()?,
                session_id: session_id()?,
                questions: message["questions"].clone(),
            },
            "question-cancelled" => Self::QuestionCancelled {
                id: message["questionId"].as_u64()?,
            },
            "approval" => Self::Approval {
                id: message["questionId"].as_u64()?,
                session_id: session_id()?,
                request: message["request"].clone(),
            },
            "usage" => Self::Usage {
                session_id: session_id()?,
                report: usage_report(message)?,
            },
            _ => return None,
        })
    }
}

/// A DSH approval as an ACP-shaped permission request: one-shot choices only,
/// like DSH's own ACP bridge offers.
pub(super) fn approval_permission(asked: &Value) -> Value {
    let tool = asked["toolName"].as_str().unwrap_or("tool");
    let reason = asked["reason"]
        .as_str()
        .filter(|reason| !reason.trim().is_empty());
    let who = if asked["subagent"] == true {
        "A DSH subagent"
    } else {
        "DSH"
    };
    json!({
        "toolCall": {
            "toolCallId": asked["callId"],
            "title": format!("{who} asks to run {tool} with more access"),
            "kind": "execute",
            "rawInput": {"command": reason.unwrap_or(tool)},
        },
        "options": [
            {"optionId":"allow-once","name":"Allow once","kind":"allow_once"},
            {"optionId":"reject-once","name":"Reject","kind":"reject_once"},
        ],
    })
}

/// DSH counts input disjointly from cache reads and writes; the ACP-style
/// `cached*` names tell the usage parser to add them back into input.
fn usage_report(message: &Value) -> Option<Value> {
    let usage = &message["usage"];
    let number = |key: &str| usage[key].as_u64();
    let mut total = json!({
        "inputTokens": number("inputTokens")?,
        "outputTokens": number("outputTokens")?,
        "cachedReadTokens": number("cacheReadTokens").unwrap_or(0),
        "cachedWriteTokens": number("cacheWriteTokens").unwrap_or(0),
        "reasoningOutputTokens": number("reasoningTokens").unwrap_or(0),
    });
    if let Some(sum) = number("totalTokens") {
        total["totalTokens"] = json!(sum);
    }
    let mut report =
        json!({"total": total, "reportKind": "delta", "reportId": message["reportId"]});
    if let Some(model) = message["model"].as_str() {
        report["model"] = json!(model);
    }
    Some(report)
}

type Pending = std::sync::Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>;

pub(super) struct Bridge {
    writer: Arc<Mutex<OwnedWriteHalf>>,
    pending: Arc<Pending>,
    next_id: AtomicU64,
    events: broadcast::Sender<BridgeEvent>,
    closed: Arc<AtomicBool>,
    _directory: tempfile::TempDir,
}

impl Bridge {
    fn spawn_reader(&self, mut reader: BufReader<tokio::net::tcp::OwnedReadHalf>) {
        let pending = self.pending.clone();
        let events = self.events.clone();
        let closed = self.closed.clone();
        let writer = self.writer.clone();
        tokio::spawn(async move {
            loop {
                let mut line = String::new();
                match (&mut reader).take(MAX_LINE).read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) if !line.ends_with('\n') => break,
                    Ok(_) => {}
                }
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if let Some(id) = message.get("id").and_then(Value::as_u64) {
                    let result = match message.get("error") {
                        Some(error) => Err(error["message"]
                            .as_str()
                            .unwrap_or("DSH bridge call failed")
                            .to_string()),
                        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    if let Some(sender) = pending.lock().unwrap().remove(&id) {
                        let _ = sender.send(result);
                    }
                } else if let Some(event) = BridgeEvent::parse(&message) {
                    let _ = events.send(event);
                }
            }
            // New calls fail at once; closing our side stops DSH from writing
            // questions and events that nobody reads.
            closed.store(true, Ordering::SeqCst);
            for (_, sender) in pending.lock().unwrap().drain() {
                let _ = sender.send(Err("DSH bridge disconnected".into()));
            }
            let _ = events.send(BridgeEvent::Closed);
            let _ = writer.lock().await.shutdown().await;
        });
    }

    async fn write(&self, message: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(message)?;
        line.push(b'\n');
        let mut writer = self.writer.lock().await;
        tokio::time::timeout(CALL_TIMEOUT, writer.write_all(&line))
            .await
            .map_err(|_| anyhow!("DSH bridge write timed out"))??;
        Ok(())
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_with_timeout(method, params, CALL_TIMEOUT).await
    }

    async fn call_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        if self.closed.load(Ordering::SeqCst) {
            bail!("DSH bridge disconnected");
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);
        let result = async {
            self.write(&json!({"id":id,"method":method,"params":params}))
                .await?;
            tokio::time::timeout(timeout, receiver)
                .await
                .map_err(|_| anyhow!("DSH bridge {method} timed out"))?
                .map_err(|_| anyhow!("DSH bridge disconnected"))?
                .map_err(|error| anyhow!("{error}"))
        }
        .await;
        self.pending.lock().unwrap().remove(&id);
        result
    }

    pub async fn command(&self, session_id: &str, line: &str) -> Result<Value> {
        let timeout = if MODE_COMMANDS.iter().any(|mode| line.starts_with(mode)) {
            CALL_TIMEOUT
        } else {
            COMMAND_TIMEOUT
        };
        let response = self
            .call_with_timeout(
                "command",
                json!({"sessionId":session_id,"line":line}),
                timeout,
            )
            .await?;
        command_result(line, response)
    }

    pub async fn answer(&self, question: u64, result: Result<Value, String>) -> Result<()> {
        self.write(&match result {
            Ok(value) => json!({"answer":question,"result":value}),
            Err(error) => json!({"answer":question,"error":error}),
        })
        .await
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.events.subscribe()
    }
}

/// DSH answers an unregistered command with no result; that is not success.
fn command_result(line: &str, response: Value) -> Result<Value> {
    match response["result"]["kind"].as_str() {
        Some("success") => Ok(response),
        Some("error") => bail!(
            "DSH {line}: {}",
            response["result"]["text"].as_str().unwrap_or("failed")
        ),
        _ => bail!(
            "This DSH session has no command for {line} (its run mode or profile leaves it out)"
        ),
    }
}

/// ACP options are the selectable truth, including an uncatalogued current
/// model. Discovery adds each model's own reasoning levels and provider name;
/// ACP's reasoning option only describes the current model.
pub(super) fn catalog(
    acp: Vec<ModelOptionDto>,
    discovered: &Value,
    current_effort: Option<&str>,
) -> Vec<ModelOptionDto> {
    let discovered: Vec<ModelOptionDto> =
        serde_json::from_value(discovered.clone()).unwrap_or_default();
    acp.into_iter()
        .map(|mut model| {
            if let Some(known) = discovered.iter().find(|known| known.model == model.model) {
                model.display_name = known.display_name.clone();
                if !known.description.is_empty() {
                    model.description = known.description.clone();
                }
                model.supported_reasoning_efforts = known.supported_reasoning_efforts.clone();
                model.default_reasoning_effort = known.default_reasoning_effort.clone();
            } else if model.is_default {
                model.default_reasoning_effort = current_effort.map(str::to_string);
            } else {
                model.supported_reasoning_efforts.clear();
                model.default_reasoning_effort = None;
            }
            model
        })
        .collect()
}

/// Typed panel actions. The HTTP body never reaches DSH as a raw Remote call.
#[derive(Debug, PartialEq)]
pub(super) enum PanelAction {
    Refresh,
    Settings,
    UpdateSetting {
        ns: String,
        key: String,
        value: Value,
        revision: u64,
    },
    SetPluginEnabled {
        id: String,
        enabled: bool,
    },
    SetBundleEnabled {
        name: String,
        enabled: bool,
    },
    /// Stop autonomous work (goal rounds, background wakeups) outside a turn.
    Stop,
    /// Restart the idle session process so saved profile changes load.
    Restart,
    /// Switch the run mode (agent preset) before the first turn.
    SelectRunMode {
        id: String,
    },
    /// Address of DSH's native Web UI in this session's process.
    Console,
    /// Run a DSH command (built-in or plugin-registered) without a model turn.
    Command {
        line: String,
    },
}

impl PanelAction {
    pub fn parse(action: &Value) -> Result<Self> {
        let text = |key: &str| {
            action[key]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .ok_or_else(|| anyhow!("{key} is required"))
        };
        let enabled = || {
            action["enabled"]
                .as_bool()
                .ok_or_else(|| anyhow!("enabled must be a boolean"))
        };
        Ok(match action["kind"].as_str() {
            Some("refresh") => Self::Refresh,
            Some("settings") => Self::Settings,
            Some("updateSetting") => {
                let value = action.get("value").cloned().unwrap_or(Value::Null);
                if value.is_array() || value.is_object() {
                    bail!("DSH settings accept JSON scalars only");
                }
                Self::UpdateSetting {
                    ns: text("ns")?,
                    key: text("key")?,
                    value,
                    revision: action["revision"]
                        .as_u64()
                        .ok_or_else(|| anyhow!("revision is required"))?,
                }
            }
            Some("setPluginEnabled") => Self::SetPluginEnabled {
                id: text("id")?,
                enabled: enabled()?,
            },
            Some("setBundleEnabled") => Self::SetBundleEnabled {
                name: text("name")?,
                enabled: enabled()?,
            },
            Some("stop") => Self::Stop,
            Some("restart") => Self::Restart,
            Some("selectRunMode") => Self::SelectRunMode { id: text("id")? },
            Some("console") => Self::Console,
            Some("command") => {
                let line = text("line")?;
                let name = line.trim().split_whitespace().next().unwrap_or_default();
                if !name.starts_with('/') {
                    bail!("A DSH command starts with /");
                }
                // Pockymoe owns these session controls; the panel must not desync them.
                if PRODUCT_COMMANDS.contains(&&name[1..]) {
                    bail!("Use the thread's own control for {name}");
                }
                Self::Command {
                    line: line.trim().to_string(),
                }
            }
            other => bail!("Unknown DSH panel action {other:?}"),
        })
    }
}

/// Controls the bridge provides through DSH's own commands and goal service.
pub(super) fn patch_capabilities(
    caps: &mut AgentProviderCapabilitiesDto,
    hello: &Value,
    meta: &Value,
) {
    let features = &hello["features"];
    let commands = features["commands"].as_bool().unwrap_or(false);
    // Once the session is known, its run mode decides: Minimal has no plan
    // mode, goals or compaction. Before that the process features apply.
    let has = |name: &str| {
        meta["commands"].as_array().map_or(true, |list| {
            list.iter().any(|command| command["name"] == name)
        })
    };
    caps.controls.plan_mode = commands && has("plan");
    caps.turns.compact = commands && has("compact");
    caps.controls.goals = commands && features["goals"].as_bool().unwrap_or(false) && has("goal");
    // The bridge forks through DSH's own session controller, at any turn.
    if features["fork"].as_bool().unwrap_or(false) {
        caps.branching.fork = true;
        caps.branching.fork_at = true;
    }
}

/// DSH commands mirrored by thread controls (plan mode, sandbox, goals,
/// compaction). Running them from the panel would desync the thread.
pub(super) const PRODUCT_COMMANDS: [&str; 4] = ["plan", "permission", "goal", "compact"];

/// Run modes and features for thread creation forms, without session data.
pub(super) fn catalog_view(hello: &Value) -> Value {
    json!({
        "kind": "dsh",
        "version": hello["version"],
        "profile": hello["profile"],
        "composition": hello["composition"],
        "compositionError": hello["compositionError"],
        "runModes": hello["runModes"],
        "features": hello["features"],
    })
}

/// Panel metadata from a bridge `session` reply: the session's commands
/// (built-in and plugin-registered) and whether its run mode is fixed.
pub(super) fn session_meta(state: &Value) -> Value {
    json!({
        "presetLocked": state["presetLocked"].as_bool().unwrap_or(true),
        // Null when DSH could not list them: unknown, not "none".
        "commands": state["commands"].as_array().map(|commands| Value::Array(commands.clone())).unwrap_or(Value::Null),
    })
}

/// Panel metadata: the startup snapshot without the model catalog (served by
/// the model list) plus this session's live projection views.
pub(super) fn harness_view(
    hello: &Value,
    projections: &serde_json::Map<String, Value>,
    meta: &Value,
    running: bool,
) -> Value {
    let mut view = hello.clone();
    if let Some(object) = view.as_object_mut() {
        object.remove("models");
        object.insert("kind".into(), json!("dsh"));
        object.insert(
            "session".into(),
            json!({
                "projections": projections,
                "running": running,
                "presetLocked": meta["presetLocked"].as_bool().unwrap_or(true),
                "commands": meta["commands"].clone(),
            }),
        );
    }
    view
}

/// DSH permission preset (sandbox + approval) for the product policy.
pub(super) fn permission_preset(policy: &ProductSessionPolicy) -> &'static str {
    if policy.allows_writes_outside_workspace() {
        "danger-full-access"
    } else if policy.sandbox_mode.as_deref() == Some("read-only") {
        "read-only"
    } else {
        "workspace-write"
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum PermissionStep {
    Applied,
    Switch,
    /// Never run a session with a wider sandbox than the thread asked for.
    Refuse,
}

/// How to reach the wanted preset. Without the permission plugin only the
/// launch environment confines the process, so it must already match.
pub(super) fn permission_step(
    current: Option<&str>,
    wanted: &str,
    can_switch: bool,
    launch: Option<&str>,
) -> PermissionStep {
    if current == Some(wanted) || current.is_none() && !can_switch && launch == Some(wanted) {
        PermissionStep::Applied
    } else if can_switch {
        PermissionStep::Switch
    } else {
        PermissionStep::Refuse
    }
}

pub(super) fn wants_plan(policy: &ProductSessionPolicy) -> bool {
    policy.collaboration_mode.as_deref() == Some("plan")
}

/// DSH questions in the product's native question shape. A plan review keeps
/// its plan in `detail`; the reviewer must see it beside the choice.
pub(super) fn questions(questions: &Value) -> Vec<Value> {
    questions
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|question| {
            let id = question["id"].as_str()?;
            let prompt = question["question"].as_str().unwrap_or(id);
            let text = match question["detail"].as_str() {
                Some(detail) if !detail.trim().is_empty() => format!("{prompt}\n\n{detail}"),
                _ => prompt.to_string(),
            };
            let options = question["options"]
                .as_array()
                .filter(|options| !options.is_empty());
            Some(json!({
                "id": id,
                "header": question["header"].as_str().unwrap_or("Question"),
                "question": text,
                // Plan reviews accept feedback; generic questions accept free text.
                "isOther": true,
                "isSecret": false,
                "multiSelect": question["multiSelect"].as_bool().unwrap_or(false),
                "options": options.map(|options| json!(options.iter().map(|option| json!({
                    "label": option["label"],
                    "description": option["description"].as_str().unwrap_or(""),
                })).collect::<Vec<_>>())).unwrap_or(Value::Null),
            }))
        })
        .collect()
}

/// Product answers (`{id:{answers:[…]}}`) as a DSH answer batch. Values that
/// are not offered options become the single free-text `custom` reply.
pub(super) fn answer(questions: &Value, answers: &Value) -> Result<Value> {
    let mut batch = Vec::new();
    for question in questions.as_array().into_iter().flatten() {
        let id = question["id"]
            .as_str()
            .ok_or_else(|| anyhow!("DSH question has no id"))?;
        let labels: Vec<&str> = question["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|option| option["label"].as_str())
            .collect();
        let values: Vec<&str> = answers[id]["answers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .collect();
        if values.is_empty() {
            bail!("Answer required for {id}");
        }
        let (selected, custom): (Vec<&str>, Vec<&str>) =
            values.into_iter().partition(|value| labels.contains(value));
        let mut item = json!({"id":id,"selected":selected});
        if !custom.is_empty() {
            item["custom"] = json!(custom.join("\n"));
        }
        batch.push(item);
    }
    Ok(json!({"answers":batch}))
}

/// DSH `todos` projection as product plan steps.
pub(super) fn todo_steps(value: &Value) -> Option<Vec<(String, String)>> {
    let todos = value.as_array()?;
    Some(
        todos
            .iter()
            .filter_map(|todo| {
                let step = todo["content"].as_str()?.to_string();
                let status = match todo["status"].as_str() {
                    Some("completed") => "completed",
                    Some("in_progress") => "inProgress",
                    _ => "pending",
                };
                Some((step, status.to_string()))
            })
            .collect(),
    )
}

/// DSH `goal` projection as the product goal state. `blocked` keeps the goal
/// live but stopped, which the product represents as paused.
pub(super) fn goal_state(value: &Value) -> Option<crate::actor::GoalState> {
    let goal = &value["goal"];
    let objective = goal["objective"].as_str()?;
    let status = match goal["phase"].as_str() {
        Some("complete") => "complete",
        Some("paused" | "blocked") => "paused",
        _ => "active",
    };
    Some(crate::actor::GoalState {
        objective: objective.into(),
        status: status.into(),
        tokens_used: 0,
        time_used_seconds: 0,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_bridge_fork_feature_enables_fork_at_any_turn() {
        let mut caps = AgentProviderCapabilitiesDto::conversational();
        patch_capabilities(&mut caps, &json!({"features":{}}), &Value::Null);
        assert!(!caps.branching.fork && !caps.branching.fork_at);
        patch_capabilities(&mut caps, &json!({"features":{"fork":true}}), &Value::Null);
        assert!(caps.branching.fork && caps.branching.fork_at);
    }

    use super::*;

    #[test]
    fn product_policy_selects_the_matching_dsh_preset() {
        let policy = |sandbox: Option<&str>, approval: &str| ProductSessionPolicy {
            collaboration_mode: None,
            sandbox_mode: sandbox.map(str::to_string),
            approval_mode: Some(approval.into()),
        };
        assert_eq!(
            permission_preset(&policy(Some("read-only"), "guarded")),
            "read-only"
        );
        assert_eq!(
            permission_preset(&policy(Some("workspace-write"), "yolo")),
            "workspace-write"
        );
        assert_eq!(
            permission_preset(&policy(None, "guarded")),
            "workspace-write"
        );
        assert_eq!(
            permission_preset(&policy(None, "yolo")),
            "danger-full-access"
        );
        assert_eq!(
            permission_preset(&policy(Some("danger-full-access"), "guarded")),
            "danger-full-access"
        );
    }

    #[test]
    fn permission_changes_fail_closed_without_the_preset_plugin() {
        use PermissionStep::*;
        assert_eq!(
            permission_step(Some("read-only"), "read-only", true, None),
            Applied
        );
        assert_eq!(
            permission_step(Some("workspace-write"), "read-only", true, None),
            Switch
        );
        // No presets plugin: the launch environment is the only confinement.
        assert_eq!(
            permission_step(None, "read-only", false, Some("read-only")),
            Applied
        );
        assert_eq!(
            permission_step(None, "read-only", false, Some("workspace-write")),
            Refuse
        );
        // A presets service without its command cannot switch live either.
        assert_eq!(
            permission_step(
                Some("workspace-write"),
                "read-only",
                false,
                Some("read-only")
            ),
            Refuse
        );
    }

    #[test]
    fn plan_review_keeps_the_plan_and_round_trips_feedback() {
        let asked = json!([{
            "id":"plan-review","header":"Plan review",
            "question":"Approve this plan and leave plan mode?","detail":"# Plan\n- ship",
            "options":[{"label":"Approve","description":"Leave plan mode"},{"label":"Keep planning"}],
            "intent":{"kind":"plan-review","approve":"Approve"}
        }]);
        let shown = questions(&asked);
        assert_eq!(
            shown[0]["question"],
            "Approve this plan and leave plan mode?\n\n# Plan\n- ship"
        );
        assert_eq!(shown[0]["options"][1]["description"], "");
        assert_eq!(shown[0]["isOther"], true);
        assert_eq!(
            answer(&asked, &json!({"plan-review":{"answers":["Approve"]}})).unwrap(),
            json!({"answers":[{"id":"plan-review","selected":["Approve"]}]})
        );
        assert_eq!(
            answer(
                &asked,
                &json!({"plan-review":{"answers":["Add tests first"]}})
            )
            .unwrap(),
            json!({"answers":[{"id":"plan-review","selected":[],"custom":"Add tests first"}]})
        );
        assert!(answer(&asked, &json!({"plan-review":{"answers":[" "]}})).is_err());
    }

    #[test]
    fn projections_map_to_product_plan_and_goal() {
        assert_eq!(
            todo_steps(&json!([
                {"content":"a","status":"completed"},
                {"content":"b","status":"in_progress"},
                {"content":"c","status":"pending"}
            ])),
            Some(vec![
                ("a".into(), "completed".into()),
                ("b".into(), "inProgress".into()),
                ("c".into(), "pending".into()),
            ])
        );
        assert_eq!(todo_steps(&Value::Null), None);
        let goal =
            goal_state(&json!({"goal":{"objective":"ship","phase":"blocked"},"roundsStarted":2}))
                .unwrap();
        assert_eq!(
            (goal.objective.as_str(), goal.status.as_str()),
            ("ship", "paused")
        );
        assert!(goal_state(&Value::Null).is_none());
    }

    #[test]
    fn acp_options_stay_authoritative_and_discovery_adds_per_model_reasoning() {
        // DSH 0.2.0-rc.2: the acp row's default is not in the provider catalog.
        let options = json!([
            {"id":"model","category":"model","type":"select",
             "currentValue":"[\"deepseek-official\",\"deepseek-v4-flash\"]",
             "options":[{"group":"deepseek-official","name":"DeepSeek","options":[
                {"value":"[\"deepseek-official\",\"deepseek-v4-flash\"]","name":"deepseek-v4-flash"},
                {"value":"[\"deepseek-official\",\"deepseek-v4-pro\"]","name":"DeepSeek-V4-Pro"},
                {"value":"[\"custom\",\"plain\"]","name":"Plain"}]}]},
            {"id":"reasoning_effort","category":"thought_level","type":"select","currentValue":"high",
             "options":[{"value":"low","name":"Low"},{"value":"high","name":"High"}]}
        ]);
        let discovered = json!([{
            "id":"[\"deepseek-official\",\"deepseek-v4-pro\"]","model":"[\"deepseek-official\",\"deepseek-v4-pro\"]",
            "displayName":"DeepSeek · DeepSeek-V4-Pro","description":"Stronger","isDefault":false,"hidden":false,
            "supportedReasoningEfforts":[{"reasoningEffort":"off","description":"Off"},{"reasoningEffort":"max","description":"Max"}],
            "defaultReasoningEffort":"max","selectionKind":"model","acpAgent":null
        }]);
        let models = catalog(
            super::super::runtime::models_from_config_options(&options),
            &discovered,
            Some("high"),
        );
        assert_eq!(models.len(), 3);
        assert!(models[0].is_default);
        assert_eq!(models[0].default_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(models[0].supported_reasoning_efforts.len(), 2);
        assert_eq!(models[1].display_name, "DeepSeek · DeepSeek-V4-Pro");
        assert_eq!(models[1].default_reasoning_effort.as_deref(), Some("max"));
        assert!(models[2].supported_reasoning_efforts.is_empty());
        assert_eq!(models[2].default_reasoning_effort, None);
    }

    #[test]
    fn native_composition_comes_from_the_matching_installed_release() {
        let root = tempfile::tempdir().unwrap();
        let scope = root.path().join("lib/node_modules/@deepseek-ai");
        let dsh = scope.join("dsh");
        let web_app = dsh.join("node_modules/@deepseek-ai/dsh-web-app");
        std::fs::create_dir_all(dsh.join("lib")).unwrap();
        std::fs::create_dir_all(web_app.join("presets")).unwrap();
        std::fs::write(dsh.join("lib/bin.js"), "").unwrap();
        std::fs::write(
            dsh.join("package.json"),
            r#"{"name":"@deepseek-ai/dsh","version":"0.2.0"}"#,
        )
        .unwrap();
        std::fs::write(web_app.join("cordis.patch.yml"), "[]").unwrap();
        std::fs::write(web_app.join("presets/minimal.patch.yml"), "[]").unwrap();
        let manifest = |version: &str| {
            format!(
                r#"{{"name":"@deepseek-ai/dsh-web-app","version":"{version}","dsh":{{"bundle":{{"patch":["./cordis.patch.yml","./presets/minimal.patch.yml"]}}}}}}"#
            )
        };
        std::fs::write(web_app.join("package.json"), manifest("0.2.0")).unwrap();
        let composition = native_composition(&dsh.join("lib/bin.js")).unwrap();
        assert_eq!(composition.patches.len(), 2);
        assert!(composition.patches[1].ends_with("presets/minimal.patch.yml"));
        // A different release would mix incompatible host rows.
        std::fs::write(web_app.join("package.json"), manifest("0.2.1")).unwrap();
        assert_eq!(native_composition(&dsh.join("lib/bin.js")), None);
    }

    #[test]
    fn session_run_mode_decides_plan_goal_and_compact_controls() {
        let hello = json!({"features":{"commands":true,"goals":true}});
        let session = |names: &[&str]| {
            session_meta(&json!({
                "presetLocked": false,
                "commands": names.iter().map(|name| json!({"name": name})).collect::<Vec<_>>(),
            }))
        };
        let caps = |meta: &Value| {
            let mut caps = AgentProviderCapabilitiesDto::conversational();
            patch_capabilities(&mut caps, &hello, meta);
            (
                caps.controls.plan_mode,
                caps.controls.goals,
                caps.turns.compact,
            )
        };
        // Before the session is read, the process features apply.
        assert_eq!(caps(&Value::Null), (true, true, true));
        assert_eq!(
            caps(&session(&["plan", "goal", "compact", "permission"])),
            (true, true, true)
        );
        // Minimal mode registers none of them.
        assert_eq!(
            caps(&session(&["export", "feedback", "permission"])),
            (false, false, false)
        );
    }

    #[test]
    fn unknown_or_failed_commands_are_errors() {
        let ok = json!({"commandId":"c","result":{"kind":"success","text":"preset read-only"}});
        assert!(command_result("/permission read-only", ok).is_ok());
        // A profile without the permission plugin answers with no result.
        let missing = json!(null);
        assert!(command_result("/permission read-only", missing)
            .unwrap_err()
            .to_string()
            .contains("no command"));
        let failed = json!({"result":{"kind":"error","text":"unknown preset"}});
        assert!(command_result("/permission x", failed)
            .unwrap_err()
            .to_string()
            .contains("unknown preset"));
    }

    #[test]
    fn panel_actions_are_typed_and_reject_structured_setting_values() {
        assert_eq!(
            PanelAction::parse(
                &json!({"kind":"setPluginEnabled","id":"include:web","enabled":false})
            )
            .unwrap(),
            PanelAction::SetPluginEnabled {
                id: "include:web".into(),
                enabled: false
            }
        );
        assert!(
            PanelAction::parse(&json!({"kind":"setPluginEnabled","id":"x","enabled":"no"}))
                .is_err()
        );
        assert!(
            PanelAction::parse(&json!({"kind":"updateSetting","ns":"agent-loop","key":"k",
            "value":{"__jsExpr":"process.exit()"}}))
            .is_err()
        );
        assert!(PanelAction::parse(&json!({"kind":"invoke","namespace":"credentials"})).is_err());
        // DSH skips its conflict check for an absent revision; require one.
        assert!(PanelAction::parse(
            &json!({"kind":"updateSetting","ns":"agent-loop","key":"k","value":1})
        )
        .is_err());
        assert_eq!(
            PanelAction::parse(&json!({"kind":"restart"})).unwrap(),
            PanelAction::Restart
        );
        assert_eq!(
            PanelAction::parse(&json!({"kind":"command","line":" /export now "})).unwrap(),
            PanelAction::Command {
                line: "/export now".into()
            }
        );
        // Product-owned controls stay with Pockymoe.
        assert!(PanelAction::parse(
            &json!({"kind":"command","line":"/permission danger-full-access"})
        )
        .is_err());
        assert!(PanelAction::parse(&json!({"kind":"command","line":"compact"})).is_err());
        assert!(PanelAction::parse(&json!({"kind":"command","line":"/compact"})).is_err());
        assert_eq!(
            PanelAction::parse(&json!({"kind":"selectRunMode","id":"ptc"})).unwrap(),
            PanelAction::SelectRunMode { id: "ptc".into() }
        );
        // A reply without the lock bit never offers a run-mode switch.
        assert_eq!(session_meta(&json!({}))["presetLocked"], true);
    }

    #[test]
    fn dsh_usage_becomes_a_delta_report_with_cache_in_input() {
        let event = BridgeEvent::parse(&json!({
            "event":"usage","sessionId":"s","reportId":"s:7",
            "usage":{"inputTokens":100,"outputTokens":20,"cacheReadTokens":900,"totalTokens":1020}
        }))
        .unwrap();
        let BridgeEvent::Usage { session_id, report } = event else {
            panic!("usage event")
        };
        assert_eq!(session_id, "s");
        assert_eq!(report["reportKind"], "delta");
        let tokens = crate::usage::normalize_usage(&report).unwrap();
        assert_eq!(tokens["total"]["inputTokens"], 1000);
        assert_eq!(tokens["total"]["cachedInputTokens"], 900);
        assert_eq!(tokens["total"]["totalTokens"], 1020);
        assert!(BridgeEvent::parse(&json!({"event":"usage","sessionId":"s","usage":{}})).is_none());
    }

    #[test]
    fn bridge_events_require_their_identity_fields() {
        assert_eq!(
            BridgeEvent::parse(&json!({"event":"status","sessionId":"s","status":"running"})),
            Some(BridgeEvent::Status {
                session_id: "s".into(),
                status: "running".into()
            })
        );
        assert_eq!(
            BridgeEvent::parse(&json!({"event":"status","status":"running"})),
            None
        );
        assert_eq!(
            BridgeEvent::parse(&json!({"event":"unknown","sessionId":"s"})),
            None
        );
    }
}
