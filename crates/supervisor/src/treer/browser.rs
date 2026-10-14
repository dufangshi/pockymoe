//! Browser mutations use only the requesting Agent's current, in-memory identity.
use super::*;
use pockymoe_protocol::ThreadDto;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

#[derive(Clone)]
pub(super) struct Commands {
    pub treer: PathBuf,
    pub pockymoe: Option<PathBuf>,
}
impl Default for Commands {
    fn default() -> Self {
        Self {
            treer: "treer".into(),
            pockymoe: None,
        }
    }
}
pub(crate) struct Error {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: &'static str,
}
fn error(code: &'static str, message: &'static str) -> Error {
    Error {
        status: StatusCode::BAD_GATEWAY,
        code,
        message,
    }
}
fn forbidden() -> Error {
    Error {
        status: StatusCode::FORBIDDEN,
        code: "thread_scope_forbidden",
        message: "This thread is not available through this Treer hub.",
    }
}
fn runtime_error() -> Error {
    error(
        "thread_operation_failed",
        "The hub could not complete the thread operation.",
    )
}

impl Hub {
    fn browser_identity(&self, scope: &Scope) -> Result<Binding, Error> {
        if scope.controller {
            return Err(forbidden());
        }
        self.live
            .lock()
            .unwrap()
            .get(&scope.agent)
            .filter(|b| {
                b.instance == scope.instance
                    && b.thread == scope.thread
                    && !b.cancelled.is_cancelled()
            })
            .cloned()
            .ok_or_else(forbidden)
    }

    fn bound_agent(&self, id: &str) -> Result<Option<String>, Error> {
        self.state
            .db
            .with(|c| {
                Ok(c.query_row(
                    "SELECT agent_id FROM treer_bindings WHERE thread_id=?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?)
            })
            .map_err(|_| runtime_error())
    }
    async fn treer_command(
        &self,
        binding: &Binding,
        args: Vec<String>,
        deleting_self: bool,
    ) -> Result<Value, Error> {
        if binding.cancelled.is_cancelled() {
            return Err(error(
                "treer_binding_closed",
                "The requesting Treer Agent disconnected. Retry from a live Agent page.",
            ));
        }
        let executable = self.browser_commands.lock().unwrap().treer.clone();
        let mut cmd = Command::new(executable);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for key in pockymoe_treer::IDENTITY_KEYS {
            cmd.env_remove(key);
        }
        cmd.env_remove("TREER_OPERATOR_CREDENTIAL");
        cmd.envs(pockymoe_treer::identity_env(
            &binding.agent,
            &binding.identity,
        ));
        let mut child = cmd.spawn().map_err(|_| {
            error(
                "treer_command_unavailable",
                "Cannot start the Treer CLI on this hub.",
            )
        })?;
        let mut stdout = child.stdout.take().unwrap();
        let operation = async {
            let mut bytes = Vec::new();
            // Never log CLI output: whoami can include identity details.
            (&mut stdout)
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| runtime_error())?;
            if bytes.len() > 1024 * 1024 {
                return Err(error(
                    "treer_invalid_reply",
                    "Treer returned an oversized reply.",
                ));
            }
            let status = child.wait().await.map_err(|_| runtime_error())?;
            if !status.success() {
                return Err(error(
                    "treer_agent_operation_failed",
                    "Treer could not update the Agent. The thread was not changed.",
                ));
            }
            if bytes.is_empty() {
                Ok(Value::Null)
            } else {
                serde_json::from_slice(&bytes)
                    .map_err(|_| error("treer_invalid_reply", "Treer returned an invalid reply."))
            }
        };
        tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(60), operation) => result.unwrap_or_else(|_| Err(error("treer_operation_timeout", "Treer did not reply within 60 seconds."))),
            // Deleting the caller closes its attach socket before the CLI can
            // return. Let that already-authorized deletion finish (bounded above).
            _ = binding.cancelled.cancelled(), if !deleting_self => Err(error("treer_binding_closed", "The requesting Treer Agent disconnected. Retry from a live Agent page.")),
        }
    }
    pub(crate) async fn create_browser_thread(
        &self,
        scope: &Scope,
        input: CreateThreadInput,
    ) -> Result<ThreadDto, Error> {
        let _operation = self.browser_operations.lock().await;
        let binding = self.browser_identity(scope)?;
        let visible_workspace = self
            .state
            .list_threads(None, true)
            .map_err(|_| runtime_error())?
            .iter()
            .any(|t| t.workspace_id == input.workspace_id && scope.allows(&t.id));
        if !visible_workspace
            || input
                .parent_thread_id
                .as_ref()
                .is_some_and(|id| !scope.allows(id))
        {
            return Err(forbidden());
        }
        if input.parent_thread_id.is_some() {
            return self
                .state
                .create_thread(input)
                .await
                .map_err(|_| runtime_error());
        }
        let workspace = self
            .state
            .get_workspace(&input.workspace_id)
            .map_err(|_| runtime_error())?;
        let identity = self
            .treer_command(&binding, vec!["whoami".into()], false)
            .await?;
        let machine = identity["machine"]["server_id"]
            .as_str()
            .filter(|id| Some(*id) == binding.identity.server_id.as_deref())
            .ok_or_else(|| {
                error(
                    "treer_machine_unavailable",
                    "Treer did not identify the requesting Agent's machine.",
                )
            })?;
        let root = identity["machine"]["root"]
            .as_str()
            .filter(|p| Path::new(p).is_absolute())
            .ok_or_else(|| {
                error(
                    "treer_machine_unavailable",
                    "Treer did not provide this machine's root directory.",
                )
            })?;
        let root = Path::new(root)
            .canonicalize()
            .map_err(|_| runtime_error())?;
        let cwd = Path::new(&workspace.abs_path)
            .canonicalize()
            .map_err(|_| runtime_error())?;
        let relative = cwd.strip_prefix(&root).map_err(|_| {
            error(
                "treer_cwd_outside_machine",
                "The selected workspace is outside this Treer machine's root.",
            )
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
            .unwrap_or_else(stable_executable)?;
        self.browser_identity(scope)?;
        // Do not pass X's identity into a new thread. Its own Agent attach will
        // install its credentials and resume it after the binding is established.
        let thread = self
            .state
            .create_thread(input)
            .await
            .map_err(|_| runtime_error())?;
        self.creating.lock().unwrap().insert(thread.id.clone());
        self.state.pause_integration_thread(&thread.id);
        let mut created_agent = None;
        let result = async {
            self.state.release_thread_harness(&thread.id, true, None).await.map_err(|_| runtime_error())?;
            let token_file = self.state.config.database_url.parent().unwrap_or(Path::new(".")).join(TREER_ATTACH_TOKEN_FILE).canonicalize().map_err(|_| runtime_error())?;
            let reply = self.treer_command(&binding, vec![
                "agent".into(), "admin".into(), "create".into(), "--machine".into(), machine.into(),
                "--kind".into(), "command".into(), "--name".into(), pockymoe_treer::agent_label(&thread.title, &thread.id),
                "--cwd".into(), cwd, "--".into(), executable.to_string_lossy().into_owned(), "attach".into(),
                "--thread".into(), thread.id.clone(), "--hub".into(), format!("http://127.0.0.1:{}", self.port),
                "--hub-token-file".into(), token_file.to_string_lossy().into_owned(),
            ], false).await?;
            let agent = reply["agent_id"].as_str().filter(|id| !id.is_empty() && id.len() <= 256 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b)))
                .ok_or_else(|| error("treer_invalid_reply", "Treer did not return the new Agent's ID."))?;
            created_agent = Some(agent.to_owned());
            // Attach may already have completed while admin create was returning.
            let now = now_rfc3339();
            self.state.db.with(|c| {
                c.execute("INSERT INTO treer_bindings(thread_id,agent_id,created_at,last_instance_id,last_seen_at) VALUES(?1,?2,?3,'',?3) ON CONFLICT(thread_id) DO NOTHING", params![thread.id,agent,now])?;
                anyhow::ensure!(c.query_row("SELECT agent_id FROM treer_bindings WHERE thread_id=?1", [&thread.id], |r| r.get::<_,String>(0))? == agent, "binding mismatch");
                Ok(())
            }).map_err(|_| runtime_error())?;
            self.state.get_thread(&thread.id).map_err(|_| runtime_error())
        }.await;
        if result.is_err() {
            if let Some(agent) = created_agent {
                let _ = self
                    .treer_command(
                        &binding,
                        vec!["agent".into(), "admin".into(), "delete".into(), agent],
                        false,
                    )
                    .await;
            }
            self.retire_browser_binding(&thread.id);
            let _ = self.state.delete_thread_releasing(&thread.id).await;
        }
        self.creating.lock().unwrap().remove(&thread.id);
        result
    }
    pub(crate) async fn rename_browser_thread(
        &self,
        scope: &Scope,
        id: &str,
        title: &str,
    ) -> Result<ThreadDto, Error> {
        let _operation = self.browser_operations.lock().await;
        let _lifecycle = self.lifecycle.lock().await;
        let binding = self.browser_identity(scope)?;
        if !scope.allows(id) {
            return Err(forbidden());
        }
        if title.trim().is_empty() {
            return Err(Error {
                status: StatusCode::BAD_REQUEST,
                code: "invalid_title",
                message: "Enter a thread title.",
            });
        }
        if let Some(agent) = self.bound_agent(id)? {
            self.treer_command(
                &binding,
                vec![
                    "agent".into(),
                    "admin".into(),
                    "rename".into(),
                    agent,
                    pockymoe_treer::agent_label(title, id),
                ],
                false,
            )
            .await?;
        }
        self.state
            .rename_thread(id, title)
            .map_err(|_| runtime_error())
    }
    fn retire_browser_binding(&self, id: &str) {
        let mut live = self.live.lock().unwrap();
        for b in live.values_mut().filter(|b| b.thread == id) {
            b.cancelled.cancel();
            b.identity = AttachTreerIdentity {
                agent_server_url: None,
                workload_credential: None,
                workspace_id: None,
                server_id: None,
            };
        }
        // Keep the cancelled reservation until attach-loop cleanup finishes;
        // a replacement instance must not race the old detach's harness release.
        self.state.clear_thread_cli_identity(id);
    }
    pub(crate) async fn delete_browser_thread(&self, scope: &Scope, id: &str) -> Result<(), Error> {
        let _operation = self.browser_operations.lock().await;
        let _lifecycle = self.lifecycle.lock().await;
        let binding = self.browser_identity(scope)?;
        if !scope.allows(id) {
            return Err(forbidden());
        }
        if let Some(agent) = self.bound_agent(id)? {
            self.treer_command(
                &binding,
                vec!["agent".into(), "admin".into(), "delete".into(), agent],
                id == binding.thread,
            )
            .await?;
        }
        // Release descendants before removing the root, so no invisible harness
        // remains running with an orphaned lineage after a top-level deletion.
        let mut descendants = self
            .state
            .list_threads(None, true)
            .map_err(|_| runtime_error())?;
        descendants.retain(|t| t.root_thread_id.as_deref() == Some(id));
        descendants.sort_by_key(|t| std::cmp::Reverse(t.lineage_depth));
        for child in descendants {
            self.state
                .delete_thread_releasing(&child.id)
                .await
                .map_err(|_| runtime_error())?;
        }
        self.state
            .delete_thread_releasing(id)
            .await
            .map_err(|_| runtime_error())?;
        self.retire_browser_binding(id);
        Ok(())
    }
}

fn stable_executable() -> Result<PathBuf, Error> {
    let running = std::env::current_exe().map_err(|_| runtime_error())?;
    let target = running.canonicalize().map_err(|_| runtime_error())?;
    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        candidates.push(home.join(".local/bin/pockymoe"));
        candidates.push(home.join(".local/share/remote-codex/native/current/remote-codex"));
    }
    for parent in running.ancestors() {
        if parent.file_name().is_some_and(|n| n == "releases") {
            if let Some(root) = parent.parent() {
                candidates.push(
                    root.join("current")
                        .join(running.file_name().unwrap_or_default()),
                );
            }
        }
    }
    candidates.push(running);
    candidates
        .into_iter()
        .find(|p| {
            !p.components().any(|c| c.as_os_str() == "releases")
                && p.canonicalize().ok().as_ref() == Some(&target)
        })
        .ok_or_else(|| {
            error(
                "stable_launcher_required",
                "Install a stable pockymoe/current launcher before creating a Treer Agent.",
            )
        })
}
