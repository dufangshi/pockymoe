//! Durable Core Message transport. No credentials or raw subprocess errors enter storage/logs.
use super::*;
use anyhow::{ensure, Context};
use pockymoe_runtime::host_messaging::{store_inbound, HostMessaging, InboundMail};
use pockymoe_treer::messages::{self as core, Delivery};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, process::Stdio, sync::Weak, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS treer_message_outbound (
 id TEXT PRIMARY KEY, sender_thread TEXT NOT NULL, recipient_thread TEXT NOT NULL,
 recipient_agent TEXT NOT NULL, request_id TEXT, fingerprint TEXT NOT NULL,
 delivery TEXT NOT NULL, notify INTEGER NOT NULL, receipt_json TEXT NOT NULL,
 core_id TEXT UNIQUE, submission_id TEXT, dispatched INTEGER NOT NULL DEFAULT 0,
 UNIQUE(sender_thread,recipient_thread,request_id)
);
CREATE TABLE IF NOT EXISTS treer_message_inbound (
 thread_id TEXT NOT NULL, core_id TEXT NOT NULL, message_id TEXT NOT NULL,
 PRIMARY KEY(thread_id,core_id)
);
CREATE TABLE IF NOT EXISTS treer_message_acks (
 thread_id TEXT NOT NULL, delivery_id TEXT NOT NULL, operation_id TEXT NOT NULL,
 PRIMARY KEY(thread_id,delivery_id)
);
CREATE TABLE IF NOT EXISTS treer_message_wake_rate (
 thread_id TEXT NOT NULL, sender_kind TEXT NOT NULL, sender_id TEXT NOT NULL,
 window_start INTEGER NOT NULL, count INTEGER NOT NULL,
 PRIMARY KEY(thread_id,sender_kind,sender_id)
);
"#;

pub(super) struct Bridge(pub Weak<Hub>);

pub(crate) fn interaction_text(state: &Supervisor, text: &str) -> String {
    if state.config.treer_enabled
        && state.config.host_mode == pockymoe_runtime::host_mode::HostMode::Hosted
    {
        format!("{text}{}", include_str!("message-guide.md"))
    } else {
        text.to_owned()
    }
}

/// Prepare text-only host mail before native device routing rejects local attachments.
pub(crate) fn prepare_send(
    state: &Supervisor,
    caller: Option<&str>,
    mut input: Value,
) -> anyhow::Result<Value> {
    if !state.config.treer_enabled
        || state.config.host_mode != pockymoe_runtime::host_mode::HostMode::Hosted
        || input["operation"] != "send"
    {
        return Ok(input);
    }
    let Some(paths) = input.get("attachments").cloned() else {
        return Ok(input);
    };
    ensure!(input["deviceId"].as_str().is_none_or(|id| id == "local" || id == state.db.host_id), "Core Messages are text-only across machines; use treer machine put, then send the destination paths");
    let sender = caller
        .or_else(|| input["fromThreadId"].as_str())
        .context("Treer attachments require a sender Agent binding")?;
    ensure!(
        caller.is_none_or(|caller| input["fromThreadId"]
            .as_str()
            .is_none_or(|from| from == caller)),
        "forbidden: --from cannot override the authenticated caller"
    );
    let recipient = input["threadId"].as_str().context("threadId required")?;
    state.db.with(|c| {
        for thread in [sender, recipient] {
            ensure!(
                c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM treer_bindings WHERE thread_id=?1)",
                    [thread],
                    |r| r.get::<_, bool>(0)
                )?,
                "Treer attachments require bound sender and recipient Agents on this hub"
            );
        }
        Ok(())
    })?;
    let paths: Vec<PathBuf> =
        serde_json::from_value(paths).context("attachments must be an array of local paths")?;
    ensure!(
        !paths.is_empty() && paths.len() <= 20,
        "attach 1 to 20 local files"
    );
    let request = input["clientRequestId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let key = format!(
        "treer:attachments:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&(
            sender, recipient, &request
        ))?))
    );
    let original = serde_json::to_string(&input)?;
    let saved = state.db.with(|c| {
        Ok(
            c.query_row("SELECT value FROM kv WHERE key=?1", [&key], |r| {
                r.get::<_, String>(0)
            })
            .optional()?,
        )
    })?;
    let text = if let Some(saved) = saved {
        let saved: Value = serde_json::from_str(&saved)?;
        ensure!(
            saved["original"] == original,
            "conflict: clientRequestId was used with different attachments or input"
        );
        saved["text"]
            .as_str()
            .context("invalid attachment snapshot")?
            .to_owned()
    } else {
        let dir = state
            .config
            .database_url
            .with_extension("message-files")
            .join(recipient)
            .join(uuid::Uuid::new_v4().to_string());
        let copied = crate::peer_files::stage(&paths, &dir)?;
        let mut text = input["text"].as_str().context("text required")?.to_owned();
        text.push_str("\n\nAttachments (local copies):");
        for path in copied {
            text.push_str(&format!("\n- {}", path.display()));
        }
        state.db.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO kv(key,value) VALUES(?1,?2)",
                params![key, json!({"original":original,"text":text}).to_string()],
            )?;
            let raw = c.query_row("SELECT value FROM kv WHERE key=?1", [&key], |r| {
                r.get::<_, String>(0)
            })?;
            let saved: Value = serde_json::from_str(&raw)?;
            ensure!(
                saved["original"] == original,
                "conflict: clientRequestId was used with different attachments or input"
            );
            Ok(saved["text"]
                .as_str()
                .context("invalid attachment snapshot")?
                .to_owned())
        })?
    };
    input["text"] = json!(text);
    input["clientRequestId"] = json!(request);
    input.as_object_mut().unwrap().remove("attachments");
    input.as_object_mut().unwrap().remove("deviceId");
    Ok(input)
}
impl HostMessaging for Bridge {
    fn send(
        &self,
        _state: &Supervisor,
        recipient: &str,
        input: ThreadSendInput,
    ) -> anyhow::Result<Value> {
        let hub = self.0.upgrade().context("host messaging is unavailable")?;
        let recipient = recipient.to_owned();
        // The runtime's send choke point is synchronous. Isolate subprocess I/O in a
        // dedicated runtime; never nest block_on in the Supervisor's Tokio runtime.
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(hub.send_host_message(&recipient, input))
        })
        .join()
        .map_err(|_| anyhow::anyhow!("host messaging worker failed"))?
    }
}

impl Hub {
    fn message_binding(&self, thread: &str) -> anyhow::Result<Binding> {
        self.live
            .lock()
            .unwrap()
            .values()
            .find(|b| b.thread == thread && !b.cancelled.is_cancelled())
            .cloned()
            .context("Treer mode requires the sender thread's own live Agent binding")
    }
    fn message_agent(&self, thread: &str) -> anyhow::Result<String> {
        self.state.db.with(|c| {
            c.query_row("SELECT agent_id FROM treer_bindings WHERE thread_id=?1", [thread], |r| r.get(0))
                .optional()?.context("Treer mode requires a recipient Agent binding; create children with pockymoe thread create")
        })
    }

    async fn message_command(
        &self,
        binding: &Binding,
        args: Vec<String>,
        body: Option<String>,
    ) -> anyhow::Result<Value> {
        let executable = self.message_executable.lock().unwrap().clone();
        let mut command = tokio::process::Command::new(executable);
        command
            .args(&args)
            .stdin(if body.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for key in pockymoe_treer::IDENTITY_KEYS {
            command.env_remove(key);
        }
        command.env_remove("TREER_OPERATOR_CREDENTIAL");
        command.envs(pockymoe_treer::identity_env(
            &binding.agent,
            &binding.identity,
        ));
        let mut child = command
            .spawn()
            .map_err(|_| anyhow::anyhow!("Cannot start the Treer message CLI"))?;
        let operation = async {
            let stdin = child.stdin.take();
            let mut stdout = child.stdout.take().unwrap();
            // Read concurrently with stdin: a malicious/failed CLI cannot deadlock pipes.
            let write = async {
                if let (Some(mut stdin), Some(body)) = (stdin, body) {
                    stdin.write_all(body.as_bytes()).await?;
                    stdin.shutdown().await?;
                }
                Ok::<_, std::io::Error>(())
            };
            let read = async {
                let mut bytes = Vec::new();
                (&mut stdout)
                    .take(16 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .await?;
                Ok::<_, std::io::Error>(bytes)
            };
            let (_, bytes) = tokio::try_join!(write, read)
                .map_err(|_| anyhow::anyhow!("Treer message I/O failed"))?;
            ensure!(
                bytes.len() <= 16 * 1024 * 1024,
                "Treer message reply too large"
            );
            let status = child
                .wait()
                .await
                .map_err(|_| anyhow::anyhow!("Treer message command failed"))?;
            ensure!(
                status.success(),
                "Treer message command failed; inspect Treer Agent availability and policy"
            );
            serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("Invalid Treer message reply"))
        };
        tokio::select! {
            _ = binding.cancelled.cancelled() => anyhow::bail!("Treer message binding detached"),
            result = tokio::time::timeout(Duration::from_secs(45), operation) => result.map_err(|_| anyhow::anyhow!("Treer message command timed out"))?,
        }
    }

    async fn send_host_message(
        &self,
        recipient: &str,
        input: ThreadSendInput,
    ) -> anyhow::Result<Value> {
        let _operation = self.message_operations.lock().await;
        let sender = input
            .from_thread_id
            .as_deref()
            .context("Treer mode requires a sender thread Agent identity")?;
        let binding = self.message_binding(sender)?;
        let agent = self.message_agent(recipient)?;
        let fingerprint = hex::encode(Sha256::digest(serde_json::to_vec(&input)?));
        let saved = self.state.db.with(|c| {
            Ok(c.query_row("SELECT id,fingerprint,receipt_json,dispatched FROM treer_message_outbound WHERE sender_thread=?1 AND recipient_thread=?2 AND request_id=?3", params![sender,recipient,input.client_request_id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,bool>(3)?))).optional()?)
        })?;
        let id = if let Some((id, hash, receipt, dispatched)) = saved {
            ensure!(
                hash == fingerprint,
                "conflict: clientRequestId was used with different input"
            );
            if dispatched {
                return Ok(serde_json::from_str(&receipt)?);
            }
            id
        } else {
            match input.delivery.as_str() {
                "queue" => ensure!(
                    input.kind.as_deref() == Some("task"),
                    "peer queue requires kind=task"
                ),
                "direct" | "steer" => {
                    ensure!(
                        matches!(input.kind.as_deref(), Some("task" | "question")),
                        "peer direct/steer requires kind=task/question"
                    );
                    ensure!(
                        input.interrupt_reason.is_some(),
                        "peer direct/steer requires interruptReason (--interrupt-reason)"
                    );
                }
                _ => {}
            }
            let id = uuid::Uuid::new_v4().to_string();
            let receipt = json!({"threadId":recipient,"messageId":id,"pendingSteerId":if input.delivery=="inbox" {None} else {Some(&id)},"clientRequestId":input.client_request_id,"delivery":"held","requestedDelivery":input.delivery,"kind":input.kind.as_deref().unwrap_or("status"),"acceptedAt":now_rfc3339(),"error":"Treer dispatch has no confirmed receipt; inspect before retrying"});
            self.state.db.with(|c| {
                c.execute("INSERT INTO treer_message_outbound(id,sender_thread,recipient_thread,recipient_agent,request_id,fingerprint,delivery,notify,receipt_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![id,sender,recipient,agent,input.client_request_id,fingerprint,input.delivery,input.notify_on_complete,receipt.to_string()])?;
                Ok(())
            })?;
            id
        };
        let subject = input.subject.clone().or_else(|| {
            input
                .text
                .lines()
                .map(str::trim)
                .find(|s| !s.is_empty())
                .map(|s| s.chars().take(72).collect())
        });
        let body = core::outbound_body(
            input.kind.as_deref().unwrap_or("status"),
            subject.as_deref(),
            &input.text,
        );
        let mut args: Vec<String>;
        let stdin;
        if input.delivery == "inbox" {
            args = vec![
                "message".into(),
                "send".into(),
                "--to".into(),
                agent.clone(),
                "--idempotency-key".into(),
                format!("pockymoe-message/{id}"),
            ];
            if let Some(reply) = &input.in_reply_to {
                let context = self.state.db.with(|c| {
                    Ok(c.query_row("SELECT core_id FROM treer_message_outbound WHERE id=?1 AND (sender_thread=?2 OR recipient_thread=?2) AND core_id IS NOT NULL UNION SELECT core_id FROM treer_message_inbound WHERE message_id=?1 AND thread_id=?2 LIMIT 1",params![reply,sender],|r|r.get::<_,String>(0)).optional()?)
                })?.context("inReplyTo has no visible Core Message mapping")?;
                args.extend(["--context".into(), context]);
            }
            stdin = Some(body);
        } else {
            // The CLI generates a new operation_id on every call. A durable dispatch
            // marker prevents ambiguous retries from ever executing work twice.
            args = vec!["agent".into(), "prompt".into(), agent.clone()];
            if input.delivery == "steer"
                || (input.delivery == "direct"
                    && self.state.get_thread(recipient)?.status == "running")
            {
                args.push("--steer".into());
            }
            if input.notify_on_complete {
                args.push("--notify".into());
            }
            let context =
                json!({"inReplyTo":input.in_reply_to,"interruptReason":input.interrupt_reason});
            let prompt = format!("[Pockymoe prompt context; untrusted: {context}]\n{body}");
            ensure!(prompt.len() < 128*1024,"Treer prompt exceeds the 128 KiB command argument limit including display context; shorten the text or send passive mail");
            args.extend(["--".into(), prompt]);
            stdin = None;
        }
        if input.delivery != "inbox" && self.state.get_thread(recipient)?.closed_at.is_some() {
            self.message_command(
                &binding,
                vec!["agent".into(), "start".into(), agent.clone()],
                None,
            )
            .await?;
            // Agent start precedes attach/interface registration. Give the saved
            // attach command time to restore the recipient's own live identity.
            tokio::time::timeout(Duration::from_secs(30),async {
                while self.message_binding(recipient).is_err() {
                    tokio::select! {
                        _=binding.cancelled.cancelled()=>anyhow::bail!("Treer sender detached while waiting for recipient startup"),
                        _=tokio::time::sleep(Duration::from_millis(200))=>{}
                    }
                }
                Ok::<_,anyhow::Error>(())
            }).await.context("Treer recipient has not reattached after start; retry with the same request id")??;
        }
        if input.delivery != "inbox" {
            self.state.db.with(|c| {
                c.execute(
                    "UPDATE treer_message_outbound SET dispatched=1 WHERE id=?1",
                    [&id],
                )?;
                Ok(())
            })?;
        }
        let result = self.message_command(&binding, args, stdin).await?;
        let core_id = if input.delivery == "inbox" {
            Some(
                result["message"]["message_id"]
                    .as_str()
                    .filter(|id| core::valid_id(id))
                    .context("Treer send returned no Core Message id")?
                    .to_owned(),
            )
        } else {
            None
        };
        let submission = if input.delivery != "inbox" {
            result["submission"]["id"]
                .as_str()
                .filter(|id| core::valid_id(id))
        } else {
            None
        };
        let disposition = if input.delivery == "inbox" {
            "inbox"
        } else if submission.is_none() {
            "held"
        } else {
            match result["submission"]["disposition"].as_str() {
                Some("queued") => "queued",
                Some("steered") => "steered",
                _ => "held",
            }
        };
        let receipt = json!({"threadId":recipient,"messageId":id,"pendingSteerId":if input.delivery=="inbox" {None} else {Some(&id)},"clientRequestId":input.client_request_id,"delivery":disposition,"requestedDelivery":input.delivery,"kind":input.kind.as_deref().unwrap_or("status"),"acceptedAt":now_rfc3339(),"coreMessageId":core_id,"submissionId":submission,"interruptReason":input.interrupt_reason});
        self.state.db.with(|c| {c.execute("UPDATE treer_message_outbound SET core_id=?2,submission_id=?3,receipt_json=?4,dispatched=1 WHERE id=?1",params![id,core_id,submission,receipt.to_string()])?;Ok(())})?;
        Ok(receipt)
    }

    /// One atomic acceptance: inbox, Core-id deduplication, wake and ack checkpoint.
    fn store_delivery(&self, binding: &Binding, delivery: &Delivery) -> anyhow::Result<bool> {
        ensure!(
            delivery.recipient.kind == "agent" && delivery.recipient.id == binding.agent,
            "Treer delivery recipient mismatch"
        );
        ensure!(
            core::valid_id(&delivery.delivery_id)
                && core::valid_id(&delivery.message.message_id)
                && core::valid_id(&delivery.message.sender.id),
            "Invalid Treer delivery identity"
        );
        ensure!(
            delivery.message.body.len() <= 1024 * 1024,
            "Treer message too large"
        );
        let message = &delivery.message;
        let fresh = self.state.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            let seen = tx.query_row("SELECT EXISTS(SELECT 1 FROM treer_message_inbound WHERE thread_id=?1 AND core_id=?2)",params![binding.thread,message.message_id],|r|r.get::<_,bool>(0))?;
            if !seen {
                let mapped = tx.query_row("SELECT id FROM treer_message_outbound WHERE core_id=?1",[&message.message_id],|r|r.get::<_,String>(0)).optional()?;
                let id = mapped.unwrap_or_else(||uuid::Uuid::new_v4().to_string());
                let from = if message.sender.kind == "agent" {tx.query_row("SELECT thread_id FROM treer_bindings WHERE agent_id=?1",[&message.sender.id],|r|r.get::<_,String>(0)).optional()?} else {None};
                let mut reply = None;
                for context in &message.context_ids {
                    reply = tx.query_row("SELECT id FROM treer_message_outbound WHERE core_id=?1 AND (sender_thread=?2 OR recipient_thread=?2) UNION SELECT message_id FROM treer_message_inbound WHERE core_id=?1 AND thread_id=?2 LIMIT 1",params![context,binding.thread],|r|r.get::<_,String>(0)).optional()?;
                    if reply.is_some() {break}
                }
                let (mut kind, mut subject) = core::display_labels(&message.body);
                // Body data may correlate a passive completion only with an actual,
                // opt-in submission and its attested Agent sender. Never used to wake.
                let completion = serde_json::from_str::<Value>(&message.body).ok().filter(|v|v["type"]=="treer.prompt.completed");
                if message.sender.kind == "agent" && message.external_source.is_none() {
                    if let Some(completion) = &completion {
                        let mut ids = completion["submission_ids"].as_array().cloned().unwrap_or_default();
                        if let Some(id) = completion["submission_id"].as_str() {ids.push(json!(id));}
                        for submission in ids.iter().filter_map(Value::as_str) {
                            let correlated = tx.query_row("SELECT id FROM treer_message_outbound WHERE sender_thread=?1 AND recipient_agent=?2 AND submission_id=?3 AND notify=1",params![binding.thread,message.sender.id,submission],|r|r.get::<_,String>(0)).optional()?;
                            if let Some(correlated) = correlated {reply=Some(correlated);kind="result".into();subject=Some("Task completion".into());break}
                        }
                    }
                }
                store_inbound(&tx,InboundMail {thread_id:&binding.thread,message_id:&id,from_thread_id:from.as_deref(),text:&message.body,created_at:&message.created_at,subject:subject.as_deref(),kind:&kind,in_reply_to:reply.as_deref(),metadata:json!({"provider":"treer","coreMessageId":message.message_id,"senderKind":message.sender.kind,"senderId":message.sender.id,"senderPath":message.sender.path,"contextIds":message.context_ids,"externalSource":message.external_source})})?;
                tx.execute("INSERT INTO treer_message_inbound(thread_id,core_id,message_id) VALUES(?1,?2,?3)",params![binding.thread,message.message_id,id])?;
                if core::wakes(message) {
                    let now = chrono::Utc::now().timestamp();
                    // Fixed one-minute per authenticated sender window, maximum three wakes.
                    tx.execute("INSERT INTO treer_message_wake_rate(thread_id,sender_kind,sender_id,window_start,count) VALUES(?1,?2,?3,?4,0) ON CONFLICT(thread_id,sender_kind,sender_id) DO UPDATE SET count=CASE WHEN window_start<=?4-60 THEN 0 ELSE count END,window_start=CASE WHEN window_start<=?4-60 THEN ?4 ELSE window_start END",params![binding.thread,message.sender.kind,message.sender.id,now])?;
                    let count: u32 = tx.query_row("SELECT count FROM treer_message_wake_rate WHERE thread_id=?1 AND sender_kind=?2 AND sender_id=?3",params![binding.thread,message.sender.kind,message.sender.id],|r|r.get(0))?;
                    if count < 3 {
                        let pending = uuid::Uuid::new_v4().to_string();
                        // Queued in this same transaction, so restart or ack retries cannot duplicate wakes.
                        tx.execute("INSERT INTO thread_pending_steers(id,thread_id,turn_id,client_request_id,display_prompt,submitted_prompt,delivery,created_at,updated_at) VALUES(?1,?2,'',?3,?4,?4,'continuation',?5,?5)",params![pending,binding.thread,format!("treer-message/{}",message.message_id),core::message_prompt(message),now_rfc3339()])?;
                        tx.execute("UPDATE treer_message_wake_rate SET count=count+1 WHERE thread_id=?1 AND sender_kind=?2 AND sender_id=?3",params![binding.thread,message.sender.kind,message.sender.id])?;
                    }
                }
            }
            tx.execute("INSERT OR IGNORE INTO treer_message_acks(thread_id,delivery_id,operation_id) VALUES(?1,?2,?3)",params![binding.thread,delivery.delivery_id,format!("pockymoe-message-ack/{}",delivery.delivery_id)])?;
            tx.commit()?;
            Ok(!seen)
        })?;
        if fresh {
            self.state.host_inbox_committed(&binding.thread);
        }
        Ok(fresh)
    }

    async fn retry_message_acks(&self, binding: &Binding) -> anyhow::Result<()> {
        let acks = self.state.db.with(|c| {
            let mut stmt = c.prepare(
                "SELECT delivery_id,operation_id FROM treer_message_acks WHERE thread_id=?1",
            )?;
            let rows = stmt
                .query_map([&binding.thread], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        for (delivery, operation) in acks {
            let ack = self
                .message_command(
                    binding,
                    vec![
                        "message".into(),
                        "ack".into(),
                        delivery.clone(),
                        "--operation-id".into(),
                        operation,
                    ],
                    None,
                )
                .await?;
            ensure!(
                [
                    "acknowledged_delivery_ids",
                    "already_acknowledged_delivery_ids"
                ]
                .iter()
                .any(|key| ack[key]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id == &delivery))),
                "Treer ack did not confirm the delivery"
            );
            self.state.db.with(|c| {
                c.execute(
                    "DELETE FROM treer_message_acks WHERE thread_id=?1 AND delivery_id=?2",
                    params![binding.thread, delivery],
                )?;
                Ok(())
            })?;
        }
        Ok(())
    }

    pub(super) fn start_message_workers(self: &Arc<Self>) {
        let bindings = self
            .live
            .lock()
            .unwrap()
            .values()
            .filter(|b| !b.cancelled.is_cancelled())
            .cloned()
            .collect::<Vec<_>>();
        for binding in bindings {
            let key = (binding.agent.clone(), binding.instance.clone());
            if !self.message_workers.lock().unwrap().insert(key.clone()) {
                continue;
            }
            let weak = Arc::downgrade(self);
            tokio::spawn(async move {
                loop {
                    let Some(hub) = weak.upgrade() else { break };
                    if binding.cancelled.is_cancelled() {
                        hub.message_workers.lock().unwrap().remove(&key);
                        break;
                    }
                    let result = async {
                        hub.retry_message_acks(&binding).await?;
                        let reply = hub
                            .message_command(
                                &binding,
                                vec![
                                    "message".into(),
                                    "receive".into(),
                                    "--wait".into(),
                                    "30000".into(),
                                    "--limit".into(),
                                    "50".into(),
                                ],
                                None,
                            )
                            .await?;
                        let deliveries =
                            serde_json::from_value::<Vec<Delivery>>(reply["deliveries"].clone())
                                .map_err(|_| anyhow::anyhow!("Invalid Treer deliveries"))?;
                        ensure!(deliveries.len() <= 50, "Invalid Treer batch size");
                        // Serialize against outbound mapping commits, after polling.
                        {
                            let _operation = hub.message_operations.lock().await;
                            for delivery in deliveries {
                                hub.store_delivery(&binding, &delivery)?;
                            }
                        }
                        hub.retry_message_acks(&binding).await
                    }
                    .await;
                    drop(hub);
                    if binding.cancelled.is_cancelled() {
                        break;
                    }
                    // No upstream diagnostics: CLI replies/errors can contain secrets.
                    if result.is_err() {
                        tracing::warn!("Treer inbox delivery deferred");
                    }
                    tokio::select! {_ = binding.cancelled.cancelled()=>break,_ = tokio::time::sleep(Duration::from_secs(2))=>{}}
                }
                if let Some(hub) = weak.upgrade() {
                    hub.message_workers.lock().unwrap().remove(&key);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests;
