//! Browser admission feeds the shared registry; creation carries the live page identity.
use super::*;
use pockymoe_protocol::ThreadDto;
use std::path::PathBuf;

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
        self.state.create_thread_with_origin(input, Default::default(), pockymoe_runtime::host_hooks::ThreadOrigin::Browser { caller_thread_id: binding.thread }).await.map_err(|e| {
            if e.to_string().contains("attach did not bind") {
                error("treer_attach_timeout", "Treer attach did not bind before the deadline; the new thread was rolled back.")
            } else { error("treer_agent_operation_failed", "Treer could not register the Agent; the new thread was rolled back.") }
        })
    }
    pub(crate) async fn rename_browser_thread(
        &self,
        scope: &Scope,
        id: &str,
        title: &str,
    ) -> Result<ThreadDto, Error> {
        let _operation = self.browser_operations.lock().await;
        self.browser_identity(scope)?;
        if !scope.allows(id) {
            return Err(forbidden());
        }
        if title.trim().is_empty() {
            return Err(runtime_error());
        }
        // Metadata reconciliation may still be finishing the previous attach.
        // Wait before entering the synchronous runtime lifecycle boundary.
        drop(self.registry_operations.lock().await);
        self.state
            .rename_thread(id, title)
            .map_err(|_| runtime_error())
    }
    pub(super) fn retire_browser_binding(&self, id: &str) {
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
        self.browser_identity(scope)?;
        if !scope.allows(id) {
            return Err(forbidden());
        }
        let target = self.state.get_thread(id).map_err(|_| runtime_error())?;
        if let Some(parent) = target.parent_thread_id.as_deref() {
            drop(self.registry_operations.lock().await);
            self.state
                .delete_child_thread(parent, id)
                .await
                .map_err(|_| runtime_error())?;
            return Ok(());
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
            drop(self.registry_operations.lock().await);
            self.state
                .delete_thread_releasing(&child.id)
                .await
                .map_err(|_| runtime_error())?;
        }
        drop(self.registry_operations.lock().await);
        self.state
            .delete_thread_releasing(id)
            .await
            .map_err(|_| runtime_error())?;
        self.retire_browser_binding(id);
        Ok(())
    }
}

pub(super) fn stable_executable() -> Result<PathBuf, Error> {
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
