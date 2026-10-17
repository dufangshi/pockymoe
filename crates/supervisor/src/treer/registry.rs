//! Strict durable thread/Agent registry. Credentials only live in Binding values.
use super::*;
use pockymoe_protocol::ThreadDto;
use pockymoe_runtime::host_hooks::{ThreadHost, ThreadOrigin};
use std::{
    io::Read,
    path::Path,
    process::Stdio,
    sync::Weak,
    time::{Duration, Instant},
};

pub(super) struct Registry {
    hub: Weak<Hub>,
}
impl Registry {
    pub(super) fn new(hub: &Arc<Hub>) -> Self {
        Self {
            hub: Arc::downgrade(hub),
        }
    }
    fn hub(&self) -> anyhow::Result<Arc<Hub>> {
        self.hub
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("host registry unavailable"))
    }
}

pub(super) fn ensure_schema(state: &Supervisor) -> anyhow::Result<()> {
    state.db.with(|c| {
        c.execute_batch(include_str!("schema.sql"))?;
        // Upgrade hubs created before notification retry scheduling existed.
        let columns = c
            .prepare("PRAGMA table_info(treer_submissions)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        for column in ["notification_attempts", "notification_next_attempt"] {
            if !columns.iter().any(|existing| existing == column) {
                c.execute_batch(&format!(
                    "ALTER TABLE treer_submissions ADD COLUMN {column} INTEGER NOT NULL DEFAULT 0"
                ))?;
            }
        }
        Ok(())
    })
}

pub(super) fn app_identity() -> Option<Binding> {
    Some(Binding {
        agent: std::env::var("TREER_AGENT_ID")
            .ok()
            .filter(|s| !s.is_empty())?,
        identity: AttachTreerIdentity {
            workload_credential: std::env::var("TREER_WORKLOAD_CREDENTIAL").ok(),
            agent_server_url: std::env::var("TREER_AGENT_SERVER_URL").ok(),
            workspace_id: std::env::var("TREER_WORKSPACE_ID").ok(),
            server_id: std::env::var("TREER_SERVER_ID").ok(),
            http_proxy: std::env::var("TREER_HTTP_PROXY").ok(),
        },
        thread: String::new(),
        instance: String::new(),
        cancelled: Default::default(),
    })
}

/// Inspect only a short, known protocol error code; never include CLI output in
/// errors or logs (whoami and diagnostic messages may contain credentials).
fn protocol_error(bytes: &[u8]) -> &'static str {
    let value = serde_json::from_slice::<Value>(bytes).unwrap_or_default();
    match value["error"]["code"].as_str() {
        Some("agent_name_conflict") => "agent_name_conflict",
        Some("agent_not_found") => "agent_not_found",
        Some("startup_not_found" | "agent_startup_not_found") => "startup_not_found",
        _ => "treer_command_failed",
    }
}
fn command(executable: &Path, binding: &Binding, args: &[String]) -> anyhow::Result<Value> {
    let mut cmd = std::process::Command::new(executable);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in pockymoe_treer::IDENTITY_KEYS {
        cmd.env_remove(key);
    }
    cmd.env_remove("TREER_OPERATOR_CREDENTIAL");
    cmd.envs(pockymoe_treer::identity_env(
        &binding.agent,
        &binding.identity,
    ));
    let mut child = cmd
        .spawn()
        .map_err(|_| anyhow::anyhow!("Cannot start the Treer CLI on this hub"))?;
    let deadline = Instant::now() + Duration::from_secs(15);
    fn reader(
        stream: impl Read + Send + 'static,
    ) -> std::sync::mpsc::Receiver<std::io::Result<Vec<u8>>> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stream
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = tx.send(result);
        });
        rx
    }
    let stdout = reader(child.stdout.take().unwrap());
    let stderr = reader(child.stderr.take().unwrap());
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("Treer operation timed out after 15 seconds");
            }
        }
    };
    let read =
        |rx: std::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>| -> anyhow::Result<Vec<u8>> {
            let bytes = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| anyhow::anyhow!("Treer reply timed out"))?
                .map_err(|_| anyhow::anyhow!("Cannot read Treer reply"))?;
            anyhow::ensure!(
                bytes.len() <= 1024 * 1024,
                "Treer returned an oversized reply"
            );
            Ok(bytes)
        };
    let out = read(stdout)?;
    let err = read(stderr)?;
    if !status.success() {
        anyhow::bail!("{}", protocol_error(&err));
    }
    if out.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&out).map_err(|_| anyhow::anyhow!("Treer returned an invalid reply"))
}

struct Registering<'a> {
    hub: &'a Hub,
    thread: &'a str,
}
impl Drop for Registering<'_> {
    fn drop(&mut self) {
        self.hub.creating.lock().unwrap().remove(self.thread);
    }
}

impl Hub {
    fn registry_command(&self, binding: &Binding, args: Vec<String>) -> anyhow::Result<Value> {
        let executable = self.browser_commands.lock().unwrap().treer.clone();
        // These runtime APIs are synchronous. Let Tokio replace the worker
        // while a bounded CLI operation runs, so attach and detach keep moving.
        if tokio::runtime::Handle::try_current()
            .is_ok_and(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
        {
            tokio::task::block_in_place(|| command(&executable, binding, &args))
        } else {
            command(&executable, binding, &args)
        }
    }
    pub(super) async fn registry_command_async(
        &self,
        binding: &Binding,
        args: Vec<String>,
    ) -> anyhow::Result<Value> {
        let executable = self.browser_commands.lock().unwrap().treer.clone();
        let binding = binding.clone();
        tokio::task::spawn_blocking(move || command(&executable, &binding, &args)).await?
    }
    fn paused_mutation(
        &self,
        thread: &ThreadDto,
        operation: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let paused = self.state.integration_thread_paused(&thread.id);
        self.state.pause_integration_thread(&thread.id);
        let result = operation();
        if result.is_err() && !paused {
            self.state.resume_integration_thread(&thread.id);
        }
        result
    }
    fn registry_caller(&self, origin: &ThreadOrigin) -> anyhow::Result<Binding> {
        let thread = match origin {
            ThreadOrigin::Delegate { caller_thread_id }
            | ThreadOrigin::Browser { caller_thread_id } => Some(caller_thread_id),
            ThreadOrigin::Fork { source_thread_id } => Some(source_thread_id),
            _ => None,
        };
        if let Some(thread) = thread {
            return self
                .live
                .lock()
                .unwrap()
                .values()
                .find(|b| &b.thread == thread && !b.cancelled.is_cancelled())
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "The creator's Treer Agent is detached; reconnect before creating a thread"
                    )
                });
        }
        self.hub_identity
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("The hub App's Treer identity is unavailable"))
    }
    fn registry_agent(&self, thread: &str) -> anyhow::Result<Option<String>> {
        self.state.db.with(|c| {
            Ok(c.query_row(
                "SELECT agent_id FROM treer_bindings WHERE thread_id=?1",
                [thread],
                |r| r.get(0),
            )
            .optional()?)
        })
    }
    fn registry_name(&self, thread: &str) -> anyhow::Result<String> {
        self.state.db.with(|c| {
            Ok(c.query_row(
                "SELECT name FROM treer_registry WHERE thread_id=?1",
                [thread],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default())
        })
    }
    fn save_registry(&self, thread: &str, status: &str, name: &str) -> anyhow::Result<()> {
        self.state.db.with(|c| { c.execute("INSERT INTO treer_registry(thread_id,state,name) VALUES(?1,?2,?3) ON CONFLICT(thread_id) DO UPDATE SET state=excluded.state,name=excluded.name",params![thread,status,name])?; Ok(()) })
    }
    fn save_registry_binding(&self, thread: &str, agent: &str) -> anyhow::Result<()> {
        self.state.db.with(|c| {
            c.execute("INSERT INTO treer_bindings(thread_id,agent_id,created_at,last_instance_id,last_seen_at) VALUES(?1,?2,?3,'',?3) ON CONFLICT(thread_id) DO NOTHING",params![thread,agent,now_rfc3339()])?;
            anyhow::ensure!(c.query_row("SELECT agent_id FROM treer_bindings WHERE thread_id=?1",[thread],|r|r.get::<_,String>(0))? == agent, "Treer binding identity mismatch");
            Ok(())
        })
    }
    async fn wait_registry_binding(&self, thread: &str, agent: &str) -> anyhow::Result<()> {
        #[cfg(test)]
        let timeout = Duration::from_secs(2);
        #[cfg(not(test))]
        let timeout = Duration::from_secs(30);
        tokio::time::timeout(timeout, async {
            loop {
                if self
                    .live
                    .lock()
                    .unwrap()
                    .get(agent)
                    .is_some_and(|b| b.thread == thread && !b.cancelled.is_cancelled())
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "Treer Agent was created or started but attach did not bind before the deadline"
            )
        })
    }
    /// Recover the lost-response crash window using the saved startup's stable
    /// thread ID. A matching name alone never authorizes adopting an Agent.
    async fn recover_registration(
        &self,
        thread: &ThreadDto,
        caller: &Binding,
    ) -> anyhow::Result<bool> {
        let interrupted = self.state.db.with(|c| Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM treer_registry WHERE thread_id=?1 AND state='registering' AND name!='')",[&thread.id],|r|r.get::<_,bool>(0))?))?;
        if !interrupted {
            return Ok(false);
        }
        let listing = self
            .registry_command_async(
                caller,
                vec![
                    "agent".into(),
                    "ls".into(),
                    "/".into(),
                    "--recursive".into(),
                ],
            )
            .await?;
        let agents = listing["agents"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Treer returned an invalid Agent listing"))?;
        let mut found = None;
        for agent in agents {
            let Some(id) = agent["agent_id"].as_str().filter(|s| s.starts_with("ag_")) else {
                continue;
            };
            let startup = self
                .registry_command_async(
                    caller,
                    vec![
                        "agent".into(),
                        "admin".into(),
                        "startup".into(),
                        "show".into(),
                        id.into(),
                    ],
                )
                .await;
            let startup = match startup {
                Ok(startup) => startup,
                Err(e) if e.to_string() == "startup_not_found" => continue,
                Err(e) => return Err(e),
            };
            let Some(args) = startup["args"].as_array() else {
                continue;
            };
            let flag = |key: &str| {
                args.windows(2)
                    .find(|pair| pair[0] == key)
                    .and_then(|pair| pair[1].as_str())
            };
            if args.first().is_none_or(|arg| arg != "attach")
                || flag("--thread") != Some(&thread.id)
                || flag("--hub") != Some(&format!("http://127.0.0.1:{}", self.port))
            {
                continue;
            }
            let cleanup_pending = self.state.db.with(|c| {
                Ok(c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM treer_registry_cleanup WHERE agent_id=?1)",
                    [id],
                    |r| r.get::<_, bool>(0),
                )?)
            })?;
            anyhow::ensure!(
                !cleanup_pending,
                "Treer Agent cleanup is pending; registration deferred"
            );
            anyhow::ensure!(
                found.is_none(),
                "Multiple Treer Agents claim this thread; registration refused"
            );
            found = Some((
                id.to_owned(),
                agent["name"].as_str().unwrap_or_default().to_owned(),
            ));
        }
        let Some((agent, name)) = found else {
            return Ok(false);
        };
        self.save_registry_binding(&thread.id, &agent)?;
        self.registry_command_async(caller, vec!["agent".into(), "start".into(), agent.clone()])
            .await?;
        self.wait_registry_binding(&thread.id, &agent).await?;
        self.save_registry(&thread.id, "bound", &name)?;
        if thread.closed_at.is_some() {
            self.registry_command_async(caller, vec!["agent".into(), "stop".into(), agent])
                .await?;
            self.state.pause_integration_thread(&thread.id);
            self.retire_browser_binding(&thread.id);
        }
        Ok(true)
    }

    async fn register_thread(
        &self,
        thread: &ThreadDto,
        origin: &ThreadOrigin,
    ) -> anyhow::Result<()> {
        let caller = self.registry_caller(origin)?;
        self.creating.lock().unwrap().insert(thread.id.clone());
        let _registering = Registering {
            hub: self,
            thread: &thread.id,
        };
        self.state.pause_integration_thread(&thread.id);
        if self.recover_registration(thread, &caller).await? {
            return Ok(());
        }
        self.save_registry(&thread.id, "registering", "")?;
        let mut created_agent = None;
        let result = async {
            self.state
                .release_thread_harness(&thread.id, true, None)
                .await?;
            let who = self
                .registry_command_async(&caller, vec!["whoami".into()])
                .await?;
            let machine = who["machine"]["server_id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Treer did not identify this machine"))?;
            anyhow::ensure!(
                caller
                    .identity
                    .server_id
                    .as_deref()
                    .is_none_or(|id| id == machine),
                "Treer creator is on another machine"
            );
            let root = who["machine"]["root"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Treer did not identify the machine root"))?;
            anyhow::ensure!(
                Path::new(root).is_absolute(),
                "Treer machine root must be absolute"
            );
            let root = Path::new(root).canonicalize()?;
            let cwd = self
                .state
                .session_cwd(thread)
                .ok_or_else(|| anyhow::anyhow!("Thread workspace unavailable"))?;
            let cwd = Path::new(&cwd).canonicalize()?;
            let relative = cwd.strip_prefix(root).map_err(|_| {
                anyhow::anyhow!("Thread workspace is outside this Treer machine's root")
            })?;
            let cwd = if relative.as_os_str().is_empty() {
                ".".into()
            } else {
                relative.to_string_lossy().into_owned()
            };
            let executable = self
                .browser_commands
                .lock()
                .unwrap()
                .pockymoe
                .clone()
                .map(Ok)
                .unwrap_or_else(browser::stable_executable)
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Install a stable pockymoe/current launcher before creating a Treer Agent"
                    )
                })?;
            let token_file = self
                .state
                .config
                .database_url
                .parent()
                .unwrap_or(Path::new("."))
                .join(TREER_ATTACH_TOKEN_FILE)
                .canonicalize()?;
            let title = if thread.parent_thread_id.is_some() {
                thread.agent_name.as_deref().unwrap_or(&thread.title)
            } else {
                &thread.title
            };
            let mut label = pockymoe_treer::agent_label(title, "");
            if let Some(parent) = &thread.parent_thread_id {
                let parent_agent = self
                    .registry_agent(parent)?
                    .ok_or_else(|| anyhow::anyhow!("Parent thread has no Treer Agent"))?;
                let show = self
                    .registry_command_async(
                        &caller,
                        vec!["agent".into(), "show".into(), parent_agent],
                    )
                    .await?;
                let name = show["name"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Treer parent Agent name unavailable"))?;
                self.save_registry(parent, "bound", name)?;
                label = format!("{name}.{label}");
            }
            for suffix in 1..=100 {
                let name = if suffix == 1 {
                    label.clone()
                } else {
                    format!("{label}-{suffix}")
                };
                self.save_registry(&thread.id, "registering", &name)?;
                anyhow::ensure!(
                    !caller.cancelled.is_cancelled(),
                    "The creator's Treer Agent disconnected during creation"
                );
                let reply = self
                    .registry_command_async(
                        &caller,
                        vec![
                            "agent".into(),
                            "admin".into(),
                            "create".into(),
                            "--machine".into(),
                            machine.into(),
                            "--kind".into(),
                            "command".into(),
                            "--name".into(),
                            name.clone(),
                            "--cwd".into(),
                            cwd.clone(),
                            "--".into(),
                            executable.to_string_lossy().into_owned(),
                            "attach".into(),
                            "--thread".into(),
                            thread.id.clone(),
                            "--hub".into(),
                            format!("http://127.0.0.1:{}", self.port),
                            "--hub-token-file".into(),
                            token_file.to_string_lossy().into_owned(),
                        ],
                    )
                    .await;
                match reply {
                    Err(e) if e.to_string() == "agent_name_conflict" => continue,
                    Err(e) => return Err(e),
                    Ok(reply) => {
                        let agent = reply["agent_id"]
                            .as_str()
                            .filter(|s| {
                                !s.is_empty()
                                    && s.len() <= 256
                                    && s.bytes()
                                        .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
                            })
                            .ok_or_else(|| {
                                anyhow::anyhow!("Treer did not return the created Agent ID")
                            })?;
                        created_agent = Some(agent.to_owned());
                        self.save_registry_binding(&thread.id, agent)?;
                        self.wait_registry_binding(&thread.id, agent).await?;
                        self.save_registry(&thread.id, "bound", &name)?;
                        if thread.closed_at.is_some() {
                            self.registry_command_async(
                                &caller,
                                vec!["agent".into(), "stop".into(), agent.into()],
                            )
                            .await?;
                            self.state.pause_integration_thread(&thread.id);
                            self.retire_browser_binding(&thread.id);
                        }
                        return Ok(());
                    }
                }
            }
            anyhow::bail!("Treer Agent name conflicts exhausted 100 candidate names")
        }
        .await;
        if result.is_err() {
            if let Some(agent) = created_agent {
                // Cleanup uses the hub App identity when the creator disconnected.
                let cleanup = self
                    .hub_identity
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| caller.clone());
                let _ = self.state.db.with(|c| {
                    c.execute(
                        "INSERT OR IGNORE INTO treer_registry_cleanup(agent_id) VALUES(?1)",
                        [&agent],
                    )?;
                    Ok(())
                });
                if self
                    .registry_command_async(
                        &cleanup,
                        vec![
                            "agent".into(),
                            "admin".into(),
                            "delete".into(),
                            agent.clone(),
                        ],
                    )
                    .await
                    .is_ok()
                {
                    let _ = self.state.db.with(|c| {
                        c.execute(
                            "DELETE FROM treer_registry_cleanup WHERE agent_id=?1",
                            [&agent],
                        )?;
                        Ok(())
                    });
                } else {
                    tracing::warn!(thread_id=%thread.id,"Treer registration cleanup deferred");
                }
            }
            self.retire_browser_binding(&thread.id);
            // Leave interrupted migrations retryable; new creates are rolled back
            // by the runtime after this error returns.
            let _ = self.state.db.with(|c| {
                c.execute(
                    "DELETE FROM treer_bindings WHERE thread_id=?1",
                    [&thread.id],
                )?;
                Ok(())
            });
        }
        result
    }

    pub(super) async fn reconcile_registry(&self) {
        let Some(caller) = self.hub_identity.lock().unwrap().clone() else {
            return;
        };
        let Ok(_operation) = self.registry_operations.try_lock() else {
            return;
        };
        let pending = self
            .state
            .db
            .with(|c| {
                let mut stmt = c.prepare("SELECT agent_id FROM treer_registry_cleanup")?;
                let rows = stmt
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .unwrap_or_default();
        for agent in pending {
            let result = self
                .registry_command_async(
                    &caller,
                    vec![
                        "agent".into(),
                        "admin".into(),
                        "delete".into(),
                        agent.clone(),
                    ],
                )
                .await;
            if result.is_ok()
                || result
                    .err()
                    .is_some_and(|e| e.to_string() == "agent_not_found")
            {
                let _ = self.state.db.with(|c| {
                    c.execute(
                        "DELETE FROM treer_registry_cleanup WHERE agent_id=?1",
                        [&agent],
                    )?;
                    Ok(())
                });
            }
        }
        let Ok(threads) = self.state.list_threads(None, true) else {
            return;
        };
        let mut threads = threads;
        threads.sort_by_key(|t| t.lineage_depth);
        for thread in threads {
            if self.creating.lock().unwrap().contains(&thread.id) {
                continue;
            }
            let Ok(agent) = self.registry_agent(&thread.id) else {
                continue;
            };
            let Some(agent) = agent else {
                // Old unbound children and interrupted creates migrate with the
                // hub App identity, even when their original caller is offline.
                if self
                    .register_thread(&thread, &ThreadOrigin::Local)
                    .await
                    .is_err()
                {
                    tracing::warn!(thread_id=%thread.id,"Treer thread registration deferred");
                }
                continue;
            };
            match self
                .registry_command_async(&caller, vec!["agent".into(), "show".into(), agent])
                .await
            {
                Err(e) if e.to_string() == "agent_not_found" => {
                    self.retire_browser_binding(&thread.id);
                    self.state.pause_integration_thread(&thread.id);
                    let _ = self.state.interrupt(&thread.id).await;
                    let _ = self
                        .state
                        .release_thread_harness(&thread.id, true, None)
                        .await;
                    let _ = self.save_registry(
                        &thread.id,
                        "archived",
                        &self.registry_name(&thread.id).unwrap_or_default(),
                    );
                    self.registry_event(&thread.id, "archived");
                }
                Ok(show) => {
                    let Some(name) = show["name"].as_str() else {
                        continue;
                    };
                    let old = self.registry_name(&thread.id).unwrap_or_default();
                    if old != name {
                        if let Ok(changes) =
                            self.registry_descendant_renames(&thread.id, &old, name)
                        {
                            for (id, agent, _, renamed) in changes {
                                if self
                                    .registry_command_async(
                                        &caller,
                                        vec![
                                            "agent".into(),
                                            "admin".into(),
                                            "rename".into(),
                                            agent,
                                            renamed.clone(),
                                        ],
                                    )
                                    .await
                                    .is_ok()
                                {
                                    let _ = self.save_registry(&id, "bound", &renamed);
                                }
                            }
                        }
                        let prefix = thread
                            .parent_thread_id
                            .as_ref()
                            .and_then(|p| self.registry_name(p).ok())
                            .map(|p| format!("{p}."));
                        let title = prefix
                            .as_deref()
                            .and_then(|p| name.strip_prefix(p))
                            .unwrap_or(name);
                        let _ = self.state.db.with(|c| {
                            c.execute(
                                "UPDATE threads SET title=?2,updated_at=?3 WHERE id=?1",
                                params![thread.id, title, now_rfc3339()],
                            )?;
                            Ok(())
                        });
                        self.registry_event(&thread.id, "renamed");
                    }
                    let _ = self.save_registry(&thread.id, "bound", name);
                    if matches!(
                        show["status"].as_str(),
                        Some("exited" | "failed" | "stopped")
                    ) && !self
                        .live
                        .lock()
                        .unwrap()
                        .values()
                        .any(|b| b.thread == thread.id && !b.cancelled.is_cancelled())
                    {
                        let _ = self.state.db.with(|c| {
                            c.execute(
                                "UPDATE threads SET closed_at=COALESCE(closed_at,?2) WHERE id=?1",
                                params![thread.id, now_rfc3339()],
                            )?;
                            Ok(())
                        });
                    }
                }
                Err(_) => { /* An outage or policy denial is never deletion. */ }
            }
        }
    }
    fn registry_event(&self, id: &str, reason: &str) {
        self.state.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
            thread_id: id.into(),
            timestamp: now_rfc3339(),
            payload: json!({"reason":reason}),
        });
    }
    fn registry_descendant_renames(
        &self,
        parent: &str,
        old: &str,
        name: &str,
    ) -> anyhow::Result<Vec<(String, String, String, String)>> {
        let mut changes = Vec::new();
        if old.is_empty() {
            return Ok(changes);
        }
        let prefix = format!("{old}.");
        let mut children = self.state.list_threads(None, true)?;
        children.sort_by_key(|t| t.lineage_depth);
        let mut descendants = HashSet::from([parent.to_owned()]);
        for child in children {
            if !child
                .parent_thread_id
                .as_ref()
                .is_some_and(|p| descendants.contains(p))
            {
                continue;
            }
            descendants.insert(child.id.clone());
            let child_name = self.registry_name(&child.id)?;
            if let Some(suffix) = child_name.strip_prefix(&prefix) {
                let Some(agent) = self.registry_agent(&child.id)? else {
                    continue;
                };
                changes.push((
                    child.id,
                    agent,
                    child_name.clone(),
                    format!("{name}.{suffix}"),
                ));
            }
        }
        Ok(changes)
    }

    fn rename_registry(&self, thread: &ThreadDto, title: &str) -> anyhow::Result<()> {
        let caller = self.registry_caller(&ThreadOrigin::Local)?;
        let agent = self
            .registry_agent(&thread.id)?
            .ok_or_else(|| anyhow::anyhow!("Thread has no Treer Agent"))?;
        let mut old = self.registry_name(&thread.id)?;
        if old.is_empty() {
            let show =
                self.registry_command(&caller, vec!["agent".into(), "show".into(), agent.clone()])?;
            old = show["name"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Treer Agent name unavailable"))?
                .to_owned();
        }
        let mut name = pockymoe_treer::agent_label(title, "");
        if let Some(parent) = &thread.parent_thread_id {
            name = format!("{}.{}", self.registry_name(parent)?, name);
        }
        let mut changes = vec![(thread.id.clone(), agent, old.clone(), name.clone())];
        changes.extend(self.registry_descendant_renames(&thread.id, &old, &name)?);
        for (index, (_, agent, _, name)) in changes.iter().enumerate() {
            if let Err(error) = self.registry_command(
                &caller,
                vec![
                    "agent".into(),
                    "admin".into(),
                    "rename".into(),
                    agent.clone(),
                    name.clone(),
                ],
            ) {
                for (_, agent, old, _) in changes[..index].iter().rev() {
                    let _ = self.registry_command(
                        &caller,
                        vec![
                            "agent".into(),
                            "admin".into(),
                            "rename".into(),
                            agent.clone(),
                            old.clone(),
                        ],
                    );
                }
                return Err(error);
            }
        }
        self.state.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            for (id, _, _, name) in changes {
                tx.execute(
                    "UPDATE treer_registry SET name=?2 WHERE thread_id=?1",
                    params![id, name],
                )?;
            }
            tx.commit()?;
            Ok(())
        })?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ThreadHost for Registry {
    async fn created(&self, thread: &ThreadDto, origin: &ThreadOrigin) -> anyhow::Result<()> {
        let hub = self.hub()?;
        match origin {
            ThreadOrigin::Hidden => hub.save_registry(&thread.id, "hidden", ""),
            ThreadOrigin::Attached { external_id } => {
                hub.save_registry_binding(&thread.id, external_id)?;
                hub.save_registry(&thread.id, "bound", "")
            }
            _ => {
                let _operation = hub.registry_operations.lock().await;
                hub.register_thread(thread, origin).await
            }
        }
    }
    fn before_delete(&self, thread: &ThreadDto) -> anyhow::Result<()> {
        let hub = self.hub()?;
        let _operation = hub
            .registry_operations
            .try_lock()
            .map_err(|_| anyhow::anyhow!("conflict: host thread registry is busy; retry"))?;
        if let Some(agent) = hub.registry_agent(&thread.id)? {
            let caller = hub.registry_caller(&ThreadOrigin::Local)?;
            hub.paused_mutation(thread, || {
                hub.registry_command(
                    &caller,
                    vec!["agent".into(), "admin".into(), "delete".into(), agent],
                )?;
                Ok(())
            })?;
            hub.retire_browser_binding(&thread.id);
        }
        Ok(())
    }
    fn before_close(&self, thread: &ThreadDto) -> anyhow::Result<()> {
        if thread.closed_at.is_some() {
            return Ok(());
        }
        let hub = self.hub()?;
        let _operation = hub
            .registry_operations
            .try_lock()
            .map_err(|_| anyhow::anyhow!("conflict: host thread registry is busy; retry"))?;
        let caller = hub.registry_caller(&ThreadOrigin::Local)?;
        let agent = hub
            .registry_agent(&thread.id)?
            .ok_or_else(|| anyhow::anyhow!("Thread has no Treer Agent"))?;
        hub.paused_mutation(thread,|| {
            let pending=hub.state.db.with(|c|Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM thread_pending_steers WHERE thread_id=?1) OR EXISTS(SELECT 1 FROM thread_turns WHERE thread_id=?1 AND status IN ('inProgress','recovering'))",[&thread.id],|r|r.get::<_,bool>(0))?))?;
            anyhow::ensure!(!pending,"conflict: thread acquired active or queued work; wait before closing");
            hub.registry_command(&caller,vec!["agent".into(),"stop".into(),agent])?;
            Ok(())
        })?;
        hub.retire_browser_binding(&thread.id);
        Ok(())
    }
    async fn before_prompt(&self, thread: &ThreadDto) -> anyhow::Result<()> {
        let hub = self.hub()?;
        let _operation = hub.registry_operations.lock().await;
        let agent = hub
            .registry_agent(&thread.id)?
            .ok_or_else(|| anyhow::anyhow!("Thread has no Treer Agent"))?;
        if hub
            .live
            .lock()
            .unwrap()
            .get(&agent)
            .is_some_and(|b| !b.cancelled.is_cancelled())
        {
            return Ok(());
        }
        let caller = hub.registry_caller(&ThreadOrigin::Local)?;
        hub.registry_command_async(&caller, vec!["agent".into(), "start".into(), agent.clone()])
            .await?;
        hub.wait_registry_binding(&thread.id, &agent).await
    }
    fn before_rename(&self, thread: &ThreadDto, title: &str) -> anyhow::Result<()> {
        let hub = self.hub()?;
        let _operation = hub
            .registry_operations
            .try_lock()
            .map_err(|_| anyhow::anyhow!("conflict: host thread registry is busy; retry"))?;
        hub.rename_registry(thread, title)
    }
    fn reserve_closed_names(&self) -> bool {
        true
    }
    fn visible(&self, id: &str) -> bool {
        self.hub().is_ok_and(|hub| hub.state.db.with(|c| Ok(!c.query_row("SELECT EXISTS(SELECT 1 FROM treer_registry WHERE thread_id=?1 AND state IN ('archived','hidden'))",[id],|r|r.get::<_,bool>(0))?)).unwrap_or(false))
    }
}
