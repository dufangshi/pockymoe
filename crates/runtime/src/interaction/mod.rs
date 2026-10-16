//! Provider-independent local thread operations. Uses the existing prompt queue and KV store.
mod agents;
pub(crate) mod inbox;
mod isolation;
mod peer;
mod tasks;
mod transcript;
pub use agents::{
    clamp_wait, create_worktree, list_roles, load_role, remove_worktree, AgentOptions, RoleTemplate,
};
pub(crate) use agents::{stage_role, take_role};
pub use isolation::{AgentIsolation, AgentRequester};
pub use peer::RemoteSender;
pub use transcript::TranscriptQuery;

use crate::Supervisor;
use anyhow::{ensure, Result};
use pockymoe_protocol::{now_rfc3339, ThreadEventEnvelope};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use uuid::Uuid;

#[derive(Clone)]
pub struct CliContext {
    pub url: String,
    pub token: String,
    /// Holds `pockymoe` and `remote-codex` links to this executable; first on managed PATHs.
    pub bin_dir: Option<std::path::PathBuf>,
}

/// Agents run `pockymoe`, or `remote-codex` from skills, prompts and hook
/// scripts written before the rename, but release executables carry a platform
/// suffix (`remote-codex-linux-x64-gnu`) and native installs keep the
/// pre-rename file name. Prepending only the executable's directory let PATH
/// fall through to an older global install that lacks newer commands.
fn cli_bin_dir(database: &std::path::Path) -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = database.with_extension("cli-bin");
    let linked = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        for command in ["pockymoe", "remote-codex"] {
            let name = if cfg!(windows) {
                format!("{command}.exe")
            } else {
                command.to_string()
            };
            let staged = dir.join(format!(".{name}.{}", Uuid::new_v4().simple()));
            #[cfg(unix)]
            std::os::unix::fs::symlink(&exe, &staged)?;
            #[cfg(windows)]
            std::fs::hard_link(&exe, &staged)
                .or_else(|_| std::fs::copy(&exe, &staged).map(|_| ()))?;
            // Replaces the link an earlier version left, without a window where it is missing.
            std::fs::rename(&staged, dir.join(&name)).inspect_err(|_| {
                let _ = std::fs::remove_file(&staged);
            })?;
        }
        Ok(())
    })();
    match linked {
        Ok(()) => Some(dir),
        Err(error) => {
            tracing::warn!(%error, "managed agents may resolve another pockymoe on PATH");
            None
        }
    }
}

tokio::task_local! { static CLI_ENV: Vec<(String, String)>; }
pub fn launch_env() -> Vec<(String, String)> {
    CLI_ENV.try_with(Clone::clone).unwrap_or_default()
}

#[derive(Default)]
pub struct InteractionState {
    pub(crate) host_messaging:
        std::sync::RwLock<Option<Arc<dyn crate::host_messaging::HostMessaging>>>,
    pub context: std::sync::RwLock<Option<CliContext>>,
    thread_tokens: std::sync::RwLock<std::collections::HashMap<String, String>>,
    started: AtomicBool,
    paused_threads: std::sync::RwLock<HashSet<String>>,
    thread_env: std::sync::RwLock<std::collections::HashMap<String, Vec<(String, String)>>>,
}

pub use pockymoe_protocol::ThreadSendInput as SendInput;

impl Supervisor {
    /// Managed callers receive an opaque identity-bound credential. A shell's
    /// machine credential still supports discovery/messaging, but cannot assert
    /// an arbitrary parent identity for destructive CLI operations.
    pub fn cli_thread_token(&self, thread_id: &str) -> String {
        let mut tokens = self.interaction.thread_tokens.write().unwrap();
        tokens
            .entry(thread_id.into())
            .or_insert_with(|| format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()))
            .clone()
    }

    pub fn cli_token_thread(&self, token: &str) -> Option<String> {
        self.interaction
            .thread_tokens
            .read()
            .unwrap()
            .iter()
            .find(|(_, saved)| saved.as_str() == token)
            .map(|(id, _)| id.clone())
    }

    pub(crate) fn revoke_cli_thread_token(&self, id: &str) {
        self.interaction.thread_tokens.write().unwrap().remove(id);
    }

    pub fn configure_cli(&self, url: String) -> CliContext {
        let mut context = self.interaction.context.write().unwrap();
        context
            .get_or_insert_with(|| CliContext {
                url,
                token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
                bin_dir: cli_bin_dir(&self.config.database_url),
            })
            .clone()
    }

    pub async fn with_cli_context<T>(&self, thread_id: &str, future: impl Future<Output = T>) -> T {
        let context = self.interaction.context.read().unwrap().clone();
        let mut env = context
            .map(|c| {
                let token = self.cli_thread_token(thread_id);
                let mut env = vec![
                    ("POCKYMOE_URL".into(), c.url.clone()),
                    ("POCKYMOE_TOKEN".into(), token.clone()),
                    ("POCKYMOE_THREAD_ID".into(), thread_id.into()),
                    // Skills, prompts and scripts written before the rename.
                    ("REMOTE_CODEX_URL".into(), c.url),
                    ("REMOTE_CODEX_TOKEN".into(), token),
                    ("REMOTE_CODEX_THREAD_ID".into(), thread_id.into()),
                ];
                if let Ok(exe) = std::env::current_exe() {
                    if let Some(dir) = exe.parent() {
                        let paths = c
                            .bin_dir
                            .into_iter()
                            .chain(std::iter::once(dir.to_path_buf()))
                            .chain(std::env::split_paths(
                                &std::env::var_os("PATH").unwrap_or_default(),
                            ))
                            .collect::<Vec<_>>();
                        if let Ok(path) = std::env::join_paths(paths) {
                            env.push(("PATH".into(), path.to_string_lossy().into()));
                        }
                    }
                }
                env
            })
            .unwrap_or_default();
        env.extend(
            self.interaction
                .thread_env
                .read()
                .unwrap()
                .get(thread_id)
                .cloned()
                .unwrap_or_default(),
        );
        CLI_ENV.scope(env, future).await
    }

    /// Every live integration binding owns its own external credentials.
    pub fn set_thread_cli_identity(&self, id: &str, env: Vec<(String, String)>) -> Result<()> {
        ensure!(
            self.thread_host.get().is_some() || self.get_thread(id)?.parent_thread_id.is_none(),
            "identity requires a top-level thread"
        );
        self.get_thread(id)?;
        self.interaction
            .thread_env
            .write()
            .unwrap()
            .insert(id.into(), env);
        Ok(())
    }
    pub fn pause_integration_thread(&self, id: &str) {
        self.interaction
            .paused_threads
            .write()
            .unwrap()
            .insert(id.into());
    }
    pub fn resume_integration_thread(&self, id: &str) {
        self.interaction.paused_threads.write().unwrap().remove(id);
    }
    pub fn integration_thread_paused(&self, id: &str) -> bool {
        self.interaction.paused_threads.read().unwrap().contains(id)
    }
    pub fn clear_thread_cli_identity(&self, id: &str) {
        self.interaction.thread_env.write().unwrap().remove(id);
    }
    pub(crate) fn stage_thread_cli_identity(&self, id: &str, env: Vec<(String, String)>) {
        self.interaction
            .thread_env
            .write()
            .unwrap()
            .insert(id.into(), env);
    }

    pub fn send_to_thread(&self, id: &str, input: SendInput) -> Result<Value> {
        self.send_to_thread_inner(id, input, None)
    }

    /// Delivery already accepted by the host, or generated locally by its inbox worker.
    /// Never routes back through the outbound host transport.
    pub fn send_to_thread_locally(&self, id: &str, input: SendInput) -> Result<Value> {
        self.send_to_thread_impl(id, input, None, false)
    }

    fn send_to_thread_inner(
        &self,
        id: &str,
        input: SendInput,
        remote: Option<RemoteSender>,
    ) -> Result<Value> {
        self.send_to_thread_impl(id, input, remote, true)
    }

    fn send_to_thread_impl(
        &self,
        id: &str,
        input: SendInput,
        remote: Option<RemoteSender>,
        consult_host: bool,
    ) -> Result<Value> {
        let host_hook = consult_host
            .then(|| self.interaction.host_messaging.read().unwrap().clone())
            .flatten();
        let thread = self.get_thread(id)?;
        ensure!(
            ["inbox", "direct", "queue", "steer"].contains(&input.delivery.as_str()),
            "delivery must be inbox, direct, queue or steer"
        );
        ensure!(
            input.notify_delivery == "inbox",
            "notifyDelivery must be inbox; completion notifications are passive and cannot wake the caller. Read them with pockymoe inbox."
        );
        if input.delivery != "inbox" {
            if host_hook.is_none() {
                self.ensure_prompt_allowed(&thread)?;
            }
            // Prompting a closed delegate reopens it, so it must fit under the cap again.
            if let (Some(_), Some(root)) = (&thread.closed_at, &thread.root_thread_id) {
                ensure!(
                    self.open_agent_threads(root)? < crate::service::MAX_OPEN_AGENT_THREADS,
                    "this lineage is at its open-thread limit; close another delegate before reopening this one"
                );
            }
        }
        ensure!(input.delivery != "inbox" || !input.notify_on_complete, "passive inbox messages have no execution turn; use direct, queue or steer with notifyOnComplete");
        ensure!(
            !input.text.trim().is_empty() && input.text.len() <= 256 * 1024,
            "text must be nonempty and at most 256 KiB"
        );
        if remote.is_none() {
            if let Some(from) = &input.from_thread_id {
                self.get_thread(from)?;
            }
        }
        ensure!(
            !input.notify_on_complete || input.from_thread_id.is_some(),
            "notifyOnComplete requires fromThreadId"
        );
        if let Some(key) = &input.client_request_id {
            ensure!(
                !key.is_empty() && key.len() <= 128,
                "invalid clientRequestId"
            );
        }
        if let Some(subject) = &input.subject {
            ensure!(
                !subject.trim().is_empty() && subject.chars().count() <= 120,
                "subject must be nonempty and at most 120 characters"
            );
        }
        let kind = input.kind.as_deref().unwrap_or("status");
        ensure!(
            pockymoe_protocol::MESSAGE_KINDS.contains(&kind),
            "kind must be one of {}",
            pockymoe_protocol::MESSAGE_KINDS.join(", ")
        );
        if let Some(parent) = &input.in_reply_to {
            ensure!(!parent.trim().is_empty(), "inReplyTo must be a message id");
        }
        if let Some(reason) = &input.interrupt_reason {
            ensure!(
                !reason.trim().is_empty() && reason.chars().count() <= 500,
                "interruptReason must be nonempty and at most 500 characters"
            );
            ensure!(
                matches!(input.delivery.as_str(), "direct" | "steer"),
                "interruptReason is only for direct/steer corrections; use inbox for ordinary reports"
            );
        }
        if let Some(topic) = &input.topic_key {
            ensure!(
                !topic.trim().is_empty() && topic.chars().count() <= 120,
                "topicKey must be nonempty and at most 120 characters"
            );
            ensure!(
                input.delivery == "inbox" && kind == "status" && input.in_reply_to.is_none(),
                "topicKey only replaces full inbox status snapshots; results, questions, tasks and replies must stay distinct"
            );
        }
        if let Some(hook) = host_hook {
            ensure!(
                remote.is_none(),
                "host messaging does not accept native peer relay delivery"
            );
            return hook.send(self, id, input);
        }
        let pending_id = Uuid::new_v4().to_string();
        let now = now_rfc3339();
        // Derive a subject when the sender omitted one. Observed agents label a
        // `create` (which defaults it from --title) but routinely skip it on a plain
        // send, and an unlabelled message is exactly the one a receiver cannot triage.
        // Doing it here rather than at display time means `inbox list` is labelled too.
        let derived_subject = input.subject.clone().or_else(|| {
            input
                .text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(|line| line.chars().take(72).collect())
        });
        let sender = remote
            .as_ref()
            .map(|sender| {
                format!(
                    "{}/{} (device \"{}\")",
                    sender.device_id,
                    sender.thread_id.as_deref().unwrap_or("local"),
                    sender.device_name
                )
            })
            .or_else(|| {
                input
                    .from_thread_id
                    .as_ref()
                    .map(|from| format!("thread {from}"))
            });
        let peer_prompt = sender.is_some();
        let prompt = match sender {
            Some(from) => {
                let subject = derived_subject
                    .as_deref()
                    .map(|value| format!(" | {value}"))
                    .unwrap_or_default();
                let reply = input
                    .in_reply_to
                    .as_deref()
                    .map(|value| format!("\nIn reply to message {value}"))
                    .unwrap_or_default();
                let reason = input
                    .interrupt_reason
                    .as_deref()
                    .map(|value| format!("\nImmediate handling needed: {value}"))
                    .unwrap_or_default();
                format!(
                    "[Pockymoe {kind} from {from}{subject}]{reply}{reason}\n{}",
                    input.text
                )
            }
            None => input.text.clone(),
        };
        let encoded = match &remote {
            Some(sender) => serde_json::to_vec(&(&input, sender))?,
            None => serde_json::to_vec(&input)?,
        };
        let fingerprint = hex::encode(Sha256::digest(encoded));
        let result = self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            let retry_key = input.client_request_id.as_ref().map(|key| {
                match &remote {
                    Some(sender) => format!(
                        "cli:request:{id}:peer:{}:{}:{key}",
                        sender.device_id,
                        sender.thread_id.as_deref().unwrap_or("local")
                    ),
                    None => format!(
                        "cli:request:{id}:{}:{key}",
                        input.from_thread_id.as_deref().unwrap_or("local")
                    ),
                }
            });
            if let Some(key) = &retry_key {
                if let Some(raw) = tx
                    .query_row("SELECT value FROM kv WHERE key=?1", [key], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()?
                {
                    let saved: Value = serde_json::from_str(&raw)?;
                    ensure!(
                        saved["fingerprint"] == fingerprint,
                        "conflict: clientRequestId was used with different input"
                    );
                    return Ok(saved["receipt"].clone());
                }
            }
            // Apply the peer policy after retry lookup: work already accepted by an
            // older runtime must retain its receipt, without dispatching it again.
            if input.from_thread_id.is_some() {
                match input.delivery.as_str() {
                    "queue" => ensure!(
                        kind == "task",
                        "peer queue requires kind=task for a concrete assignment; send reports, results and questions to inbox"
                    ),
                    "direct" | "steer" => {
                        ensure!(
                            matches!(kind, "task" | "question"),
                            "peer direct/steer is only for urgent corrections or unblock requests (kind=task/question); send ordinary status/results to inbox"
                        );
                        ensure!(
                            input.interrupt_reason.is_some(),
                            "peer direct/steer requires interruptReason (--interrupt-reason): explain why waiting for the next checkpoint would cause harm or wasted work; otherwise use inbox or queue a task"
                        );
                    }
                    _ => {}
                }
            }
            // Resolve only after deduplication, inside the acceptance transaction.
            // A retry must keep its original route even when the peer changes state.
            let (status, active_turn): (String, Option<String>) = tx.query_row(
                "SELECT status,(SELECT id FROM thread_turns WHERE thread_id=?1 AND status='inProgress' ORDER BY ordinal DESC LIMIT 1) FROM threads WHERE id=?1", [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let delivery = if input.delivery == "direct" {
                match status.as_str() {
                    "idle" if active_turn.is_none() => "queue",
                    "running" => "steer",
                    _ => anyhow::bail!("conflict: direct requires an idle or running thread; inspect its status before retrying"),
                }
            } else {
                input.delivery.as_str()
            };
            if delivery == "steer" {
                ensure!(
                    status == "running" && active_turn.is_some(),
                    "conflict: steering requires an active turn"
                );
                ensure!(
                    self.runtime(thread.provider)?
                        .negotiated_caps(thread.agent_id.as_deref())
                        .turns
                        .steer,
                    "conflict: this backend does not support steering"
                );
            }
            let mut receipt = json!({"threadId":id,"pendingSteerId":if delivery == "inbox" {None} else {Some(&pending_id)},"clientRequestId":input.client_request_id,"delivery":if delivery == "queue" {"queued"} else {delivery},"requestedDelivery":input.delivery,"acceptedAt":now,"messageId":pending_id,"kind":kind});
            if let Some(reason) = &input.interrupt_reason {
                receipt["interruptReason"] = json!(reason);
            }
            if delivery == "inbox" {
                let envelope = inbox::Envelope {
                    subject: derived_subject.as_deref(),
                    kind,
                    in_reply_to: input.in_reply_to.as_deref(),
                    topic_key: input.topic_key.as_deref(),
                };
                match &remote {
                    Some(sender) => inbox::store_from_peer(
                        &tx, id, &pending_id, sender, &input.text, &now, envelope,
                    )?,
                    None => inbox::store(
                        &tx, id, &pending_id, input.from_thread_id.as_deref(),
                        &input.text, &now, envelope,
                    )?,
                }
                if let Some(topic) = &input.topic_key {
                    receipt["topicKey"] = json!(topic);
                    receipt["supersededMessageCount"] = json!(inbox::supersede_status(
                        &tx, id, &pending_id, input.from_thread_id.as_deref(), topic,
                    )?);
                }
            } else {
                enqueue(
                    &tx,
                    &pending_id,
                    id,
                    &prompt,
                    input.client_request_id.as_deref(),
                    &now,
                )?;
                if peer_prompt {
                    tx.execute("INSERT INTO kv(key,value) VALUES(?1,'peer')",
                        [format!("cli:prompt-origin:{pending_id}")])?;
                }
                if delivery == "steer" {
                    tx.execute(
                        "UPDATE thread_pending_steers SET delivery='cli-steer',turn_id=?2 WHERE id=?1",
                        params![pending_id, active_turn],
                    )?;
                }
            }
            if input.notify_on_complete {
                let mut subscription = json!({"threadId":input.from_thread_id,"delivery":input.notify_delivery});
                if let Some(sender) = &remote {
                    subscription["deviceId"] = json!(sender.device_id);
                    subscription["deviceName"] = json!(sender.device_name);
                }
                tx.execute(
                    "INSERT INTO kv(key,value) VALUES(?1,?2)",
                    params![
                        format!("cli:notify:pending:{pending_id}"),
                        subscription.to_string()
                    ],
                )?;
            }
            if let Some(key) = retry_key {
                tx.execute(
                    "INSERT INTO kv(key,value) VALUES(?1,?2)",
                    params![
                        key,
                        json!({"fingerprint":fingerprint,"receipt":receipt}).to_string()
                    ],
                )?;
            }
            tx.commit()?;
            Ok(receipt)
        })?;
        self.bus.emit(ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
            thread_id: id.into(),
            timestamp: now,
            payload: json!({"reason":"pending_steer_updated"}),
        });
        Ok(result)
    }

    pub async fn interaction_status(&self, id: &str) -> Result<Value> {
        let thread = self.get_thread(id)?;
        let pending = if let Ok(runtime) = self.runtime(thread.provider) {
            runtime.pending_requests(id).await
        } else {
            vec![]
        };
        let queued: i64 = self.db.with(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM thread_pending_steers WHERE thread_id=?1",
                [id],
                |r| r.get(0),
            )?)
        })?;
        Ok(
            json!({"threadId":id,"name":thread.agent_name,"role":thread.agent_role,"parentThreadId":thread.parent_thread_id,"rootThreadId":thread.root_thread_id,"worktreePath":thread.worktree_path,"closedAt":thread.closed_at,"title":thread.title,"workspaceId":thread.workspace_id,"provider":thread.provider,"agentId":thread.agent_id,"model":thread.model,"reasoningEffort":thread.reasoning_effort,"status":thread.status,"activeTurnId":thread.active_turn_id,"updatedAt":thread.updated_at,"lastError":thread.last_error,"waitingForInput":!pending.is_empty(),"queuedCount":queued,"unreadMessageCount":self.inbox_unread_count(id)?}),
        )
    }

    pub fn start_interaction_worker(self: &Arc<Self>) {
        if self.interaction.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let running = Arc::new(std::sync::Mutex::new(HashSet::<String>::new()));
            let mut next_inbox_reminder = std::time::Instant::now();
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let Some(state) = weak.upgrade() else {
                    break;
                };
                if let Err(error) = state.automation_tick(&now_rfc3339()).await {
                    tracing::warn!(%error, "automation tick failed");
                }
                if let Err(error) = state.recover_cli_notifications() {
                    tracing::warn!(%error,"CLI notification recovery failed");
                }
                if let Err(error) = state.fire_wakes().await {
                    tracing::warn!(%error,"delegate wake failed");
                }
                if std::time::Instant::now() >= next_inbox_reminder {
                    if let Err(error) = state.inbox_reminder_tick(&now_rfc3339()).await {
                        tracing::warn!(%error, "unread inbox reminder failed");
                    }
                    next_inbox_reminder =
                        std::time::Instant::now() + std::time::Duration::from_secs(30);
                }
                let ids = state.db.with(|c| {
                    let mut stmt = c.prepare("SELECT DISTINCT p.thread_id FROM thread_pending_steers p JOIN threads t ON t.id=p.thread_id WHERE p.delivery='continuation' AND t.status!='running'")?;
                    let ids=stmt.query_map([],|r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
                    Ok(ids)
                });
                let Ok(ids) = ids else {
                    continue;
                };
                for id in ids {
                    if state.integration_thread_paused(&id)
                        && !(state.thread_host.get().is_some()
                            && state.get_thread(&id).is_ok_and(|t| t.closed_at.is_some()))
                    {
                        continue;
                    }
                    if !running.lock().unwrap().insert(id.clone()) {
                        continue;
                    }
                    let state = state.clone();
                    let running = running.clone();
                    tokio::spawn(async move {
                        if let Err(error) = state.drain_steers(&id).await {
                            tracing::warn!(%error,thread_id=%id,"queued prompt deferred");
                            if !error.to_string().starts_with("conflict:") {
                                let _ = state.db.with(|c| {
                                    c.execute(
                                        "UPDATE threads SET last_error=?1 WHERE id=?2",
                                        params![format!("{error:#}"), id],
                                    )?;
                                    Ok(())
                                });
                                // A missing/unavailable harness should not be restarted four times per second.
                                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            }
                        }
                        running.lock().unwrap().remove(&id);
                    });
                }
            }
        });
    }

    fn recover_cli_notifications(&self) -> Result<()> {
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            // The prefix uses KV's primary-key index, rather than scanning old turns on every tick.
            let turns = {
                let mut stmt = tx.prepare("SELECT DISTINCT substr(key,17,36) FROM kv WHERE key GLOB 'cli:notify:turn:*'")?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
                rows
            };
            for turn in turns {
                let ended = tx.query_row("SELECT thread_id,status FROM thread_turns WHERE id=?1 AND status!='inProgress'", [&turn], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).optional()?;
                if let Some((thread,status)) = ended { finish_notification(&tx, &thread, &turn, &status, &now_rfc3339())?; }
            }
            tx.commit()?;
            Ok(())
        })
    }
}

pub(crate) fn enqueue(
    conn: &Connection,
    id: &str,
    thread: &str,
    text: &str,
    request: Option<&str>,
    now: &str,
) -> Result<()> {
    conn.execute("INSERT INTO thread_pending_steers(id,thread_id,turn_id,client_request_id,display_prompt,submitted_prompt,delivery,created_at,updated_at) VALUES(?1,?2,'',?3,?4,?4,'continuation',?5,?5)", params![id,thread,request,text,now])?;
    Ok(())
}

pub(crate) fn bind_notification(conn: &Connection, pending_id: &str, turn_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO kv(key,value) VALUES(?1,?2)",
        params![format!("cli:submission:turn:{pending_id}"), turn_id],
    )?;
    conn.execute(
        "UPDATE kv SET key=?1 WHERE key=?2",
        params![
            format!("cli:peer-work:turn:{turn_id}"),
            format!("cli:peer-work:pending:{pending_id}")
        ],
    )?;
    conn.execute(
        "UPDATE kv SET key=?1 WHERE key=?2",
        params![
            format!("cli:notify:turn:{turn_id}:{pending_id}"),
            format!("cli:notify:pending:{pending_id}")
        ],
    )?;
    Ok(())
}

/// The last nonempty agent message of a turn: what the delegate said it did.
pub(crate) fn closing_message(
    conn: &Connection,
    thread: &str,
    turn: &str,
) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT json_extract(item_json,'$.text') FROM thread_history_items
             WHERE thread_id=?1 AND turn_id=?2
               AND json_extract(item_json,'$.kind')='agentMessage'
               AND trim(coalesce(json_extract(item_json,'$.text'),'')) <> ''
             ORDER BY CASE json_extract(item_json,'$.responsePhase') WHEN 'final' THEN 0 ELSE 1 END, created_at DESC, rowid DESC LIMIT 1",
            params![thread, turn],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

pub(crate) fn finish_notification(
    conn: &Connection,
    thread: &str,
    turn: &str,
    status: &str,
    now: &str,
) -> Result<()> {
    let key = format!("cli:notify:turn:{turn}");
    let watchers = {
        let mut stmt = conn.prepare("SELECT key,value FROM kv WHERE key=?1 OR key GLOB ?2")?;
        let rows = stmt
            .query_map(params![key, format!("{key}:*")], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    for (key, target) in watchers {
        // Completion is status, not a new task. Even subscriptions saved by
        // older versions with queue delivery must remain passive after upgrade.
        let parsed: Value = serde_json::from_str(&target).unwrap_or(Value::Null);
        let from = parsed["threadId"].as_str().unwrap_or(&target).to_owned();
        let remote_device = parsed["deviceId"].as_str();
        let exists = remote_device.is_some()
            || conn
                .query_row("SELECT 1 FROM threads WHERE id=?1", [&from], |_| Ok(()))
                .optional()?
                .is_some();
        if exists {
            // Carry the delegate's own closing message instead of only pointing at it.
            // A bare "it ended" forced the caller into a second transcript call and
            // prose-parsing before it could act on anything.
            let reply = closing_message(conn, thread, turn)?;
            let summary = match reply.as_deref() {
                // Bounded: the notification lands in a context window, and a delegate
                // that wrote an essay should not evict the caller's own work.
                Some(text) if text.chars().count() > 4000 => {
                    let head: String = text.chars().take(4000).collect();
                    format!("\n\nIts closing message (truncated, full text via the transcript command above):\n{head}")
                }
                Some(text) => format!("\n\nIts closing message:\n{text}"),
                None => String::new(),
            };
            // The caller on another device must qualify the thread with this device;
            // runtime does not know its relay ID, but the delivered mail carries it.
            let target = if remote_device.is_some() {
                format!("DEVICE/{thread}")
            } else {
                thread.to_string()
            };
            let device_hint = if remote_device.is_some() {
                " (DEVICE is this message's fromDeviceId)"
            } else {
                ""
            };
            let text = format!("[Pockymoe turn notification]\nThread {thread}, turn {turn} ended with status {status} at {now}. This describes execution status, not business success - check the result below before acting on it. Full detail: pockymoe transcript {target} --turn {turn} --view overview{device_hint}{summary}");
            let message_id = Uuid::new_v4().to_string();
            let subject = format!("Delegate turn {status}");
            if let Some(device) = remote_device {
                // The supervisor owns transport/retries; runtime only persists the result.
                peer::completion_outbox(conn, device, &from, thread, &text, &subject, now)?;
            } else {
                inbox::store(
                    conn,
                    &from,
                    &message_id,
                    Some(thread),
                    &text,
                    now,
                    inbox::Envelope {
                        subject: Some(&subject),
                        kind: "result",
                        in_reply_to: None,
                        topic_key: None,
                    },
                )?;
            }
        }
        conn.execute("DELETE FROM kv WHERE key=?1", [key])?;
    }
    Ok(())
}
