//! Dokploy lifecycle and private resource ownership. Execution stays in the
//! Supervisor's worker transport; this module never falls back to host work.
mod compose;
pub mod docker;
mod dokploy;
mod storage;

use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use anyhow::{bail, ensure, Context, Result};
use pockymoe_protocol::*;
use serde::{Deserialize, Serialize};

use docker::{
    CliEnvironmentDocker, DockerHostInfo, EnvironmentDocker, ExpectedContainer, LocalDockerContext,
    QuiesceTarget,
};
use dokploy::{find_owned_compose, verify_owned_compose, DokployClient};

#[derive(Clone, Serialize, Deserialize)]
struct ConnectionRecord {
    public: EnvironmentConnectionDto,
    api_key: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct EnvironmentRecord {
    dto: EnvironmentDto,
    /// Exact deterministic resource name; Dokploy appends a suffix to appName.
    app_name: String,
    #[serde(default)]
    dokploy_app_name: Option<String>,
    /// Persist before create so an unavailable response cannot cause duplicates.
    #[serde(default)]
    create_pending: bool,
    /// Persist before deploy, including the window before the full ID is known.
    #[serde(default)]
    deploy_pending: bool,
    uid_gid: String,
    image: String,
    worker_binary: String,
    managed_worktree_root: String,
    #[serde(default)]
    daemon_id: Option<String>,
    /// Conservatively reserve capacity for errors and uncertain remote work.
    #[serde(default)]
    resource_active: bool,
    #[serde(default)]
    pending_quiesce: Option<QuiesceTarget>,
    /// False in legacy records: a lost writer ID requires an owned Dokploy Stop.
    #[serde(default)]
    quiesce_confirmed: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Store {
    version: u32,
    connections: BTreeMap<String, ConnectionRecord>,
    environments: BTreeMap<String, EnvironmentRecord>,
    bindings: BTreeMap<String, String>,
    budget: EnvironmentBudgetDto,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: 1,
            connections: BTreeMap::new(),
            environments: BTreeMap::new(),
            bindings: BTreeMap::new(),
            budget: EnvironmentBudgetDto::default(),
        }
    }
}

/// One manager per Supervisor database. The sidecar is process-locked, private,
/// and atomically replaced; initialization errors remain fail-closed.
pub struct EnvironmentManager {
    directory: PathBuf,
    store: Mutex<std::result::Result<Store, String>>,
    _process_lock: Option<File>,
    operation: tokio::sync::Mutex<()>,
    docker: RwLock<Arc<dyn EnvironmentDocker>>,
}

impl EnvironmentManager {
    pub fn new(database_path: &Path) -> Self {
        let mut name = database_path.as_os_str().to_os_string();
        name.push(".environments");
        let directory = PathBuf::from(name);
        let (store, process_lock) = match open_store(&directory) {
            Ok((store, lock)) => (Ok(store),Some(lock)),
            Err(_) => (Err("environment private sidecar could not be opened or is already owned by another Supervisor".into()),None),
        };
        let directory = directory.canonicalize().unwrap_or(directory);
        Self {
            directory,
            store: Mutex::new(store),
            _process_lock: process_lock,
            operation: tokio::sync::Mutex::new(()),
            docker: RwLock::new(Arc::new(CliEnvironmentDocker::default())),
        }
    }

    pub fn set_docker_driver(&self, driver: Arc<dyn EnvironmentDocker>) -> Result<()> {
        *self
            .docker
            .write()
            .map_err(|_| anyhow::anyhow!("Docker driver lock is unavailable"))? = driver;
        Ok(())
    }

    fn driver(&self) -> Result<Arc<dyn EnvironmentDocker>> {
        Ok(self
            .docker
            .read()
            .map_err(|_| anyhow::anyhow!("Docker driver lock is unavailable"))?
            .clone())
    }

    fn read<T>(&self, f: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        let guard = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("environment store lock is unavailable"))?;
        match &*guard {
            Ok(store) => f(store),
            Err(message) => bail!("{message}"),
        }
    }

    fn write<T>(&self, f: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("environment store lock is unavailable"))?;
        let store = guard
            .as_ref()
            .map_err(|message| anyhow::anyhow!("{message}"))?;
        let mut next = store.clone();
        let result = f(&mut next)?;
        if persist(&self.directory, &next).is_err() {
            let message="environment state could not be persisted; this manager is unavailable until Supervisor recovery";
            *guard = Err(message.into());
            bail!("{message}");
        }
        *guard = Ok(next);
        Ok(result)
    }

    pub async fn snapshot(&self) -> Result<EnvironmentsDto> {
        self.read(|store| {
            Ok(EnvironmentsDto {
                connections: store
                    .connections
                    .values()
                    .map(|c| c.public.clone())
                    .collect(),
                environments: store.environments.values().map(|e| e.dto.clone()).collect(),
                budget: store.budget.clone(),
            })
        })
    }

    /// Observe current execution authority without adopting a replacement
    /// container, reconnecting old sessions, or replaying work.
    pub async fn reconcile(&self, id: &str) -> Result<EnvironmentDto> {
        let _operation = self.operation.lock().await;
        let record = self.record(id)?;
        if record.deploy_pending {
            let _ = self.quiesce_deployment(id).await;
            return self.get(id);
        }
        if record.pending_quiesce.is_some() {
            // Preserve the original interrupted reason and capacity if stopping
            // fails. The next background observation retries the same full ID.
            let _ = self.quiesce_inner(id).await;
            return self.get(id);
        }
        if record.dto.state != "running" {
            return Ok(record.dto);
        }
        let docker = self.driver()?;
        let observation=async {
            let values=docker.inspect_environment(&self.context(&record.dto.connection_id)?,id).await
                .map_err(|_|anyhow::anyhow!("local Docker is unavailable; retained environment resources require explicit recovery"))?;
            ensure!(values.len()==1,"environment container is missing or its ownership is ambiguous");
            let container=&values[0];
            if container["State"]["Running"]!=true {
                if container["State"]["OOMKilled"]==true { bail!("environment container exited after an out-of-memory kill"); }
                if let Some(code)=container["State"]["ExitCode"].as_i64() { bail!("environment container exited with code {code}"); }
                bail!("environment container is no longer running");
            }
            ensure!(record.dto.container_id.as_deref()==container["Id"].as_str(),"environment container was replaced outside its generation");
            docker::verify_container(container,&expected(&record))
        }.await;
        match observation {
            Ok(ref observed) if record.dto.observed_limits.as_ref() == Some(observed) => {
                Ok(record.dto)
            }
            Ok(observed) => self.write(|store| {
                let record = store
                    .environments
                    .get_mut(id)
                    .context("environment not found")?;
                record.dto.observed_limits = Some(observed);
                Ok(record.dto.clone())
            }),
            Err(error) => {
                self.write(|store| {
                    let record = store
                        .environments
                        .get_mut(id)
                        .context("environment not found")?;
                    remember_writer(record);
                    record.dto.generation = record
                        .dto
                        .generation
                        .checked_add(1)
                        .context("environment generation exhausted")?;
                    record.dto.state = "interrupted".into();
                    record.dto.container_id = None;
                    record.dto.observed_limits = None;
                    record.dto.last_error = Some(error.to_string());
                    record.dto.updated_at = now_rfc3339();
                    record.resource_active = true;
                    Ok(())
                })?;
                let _ = self.quiesce_inner(id).await;
                self.get(id)
            }
        }
    }

    pub async fn reconcile_all(&self) -> Result<EnvironmentsDto> {
        let ids = self.read(|store| {
            Ok(store
                .environments
                .values()
                .filter(|record| {
                    record.dto.state == "running"
                        || record.pending_quiesce.is_some()
                        || record.deploy_pending
                })
                .map(|record| record.dto.id.clone())
                .collect::<Vec<_>>())
        })?;
        for id in ids {
            self.reconcile(&id).await?;
        }
        self.snapshot().await
    }

    /// Quiesce a fenced writer without releasing capacity, changing its
    /// interrupted reason, replaying a session, or deleting provider/data state.
    pub async fn quiesce(&self, id: &str) -> Result<EnvironmentDto> {
        let _operation = self.operation.lock().await;
        ensure!(
            self.record(id)?.dto.state != "running",
            "running environment must be fenced before quiescence"
        );
        if self.record(id)?.deploy_pending {
            self.quiesce_deployment(id).await?;
        } else {
            self.quiesce_inner(id).await?;
        }
        self.get(id)
    }

    async fn quiesce_deployment(&self, id: &str) -> Result<()> {
        let record = self.record(id)?;
        ensure!(
            record.dto.compose_id.is_some() && record.dokploy_app_name.is_some(),
            "uncertain deployment has no pinned Dokploy identity; explicit owned Stop is required"
        );
        self.write(|store| {
            let record = store
                .environments
                .get_mut(id)
                .context("environment not found")?;
            if record.dto.state!="interrupted" {
                remember_writer(record);
                record.dto.generation=record.dto.generation.checked_add(1).context("environment generation exhausted")?;
                record.dto.container_id=None;
                record.dto.observed_limits=None;
            }
            record.dto.state = "interrupted".into();
            record.dto.last_error.get_or_insert_with(||"Dokploy deployment outcome is uncertain; reconciling the owned resource before new execution".into());
            record.resource_active = true;
            Ok(())
        })?;
        // A queued deployment can create a container after an empty Docker
        // inspection. Only the owned Dokploy stop path also clears those jobs.
        self.stop_inner(id, false, true).await
    }

    async fn quiesce_inner(&self, id: &str) -> Result<()> {
        let record = self.record(id)?;
        if let Some(target) = &record.pending_quiesce {
            self.driver()?
                .quiesce(&self.context(&record.dto.connection_id)?, target)
                .await?;
            self.write(|store| {
                let current = store
                    .environments
                    .get_mut(id)
                    .context("environment not found")?;
                ensure!(
                    current.pending_quiesce.as_ref() == Some(target),
                    "writer quiescence target changed"
                );
                current.pending_quiesce = None;
                current.quiesce_confirmed = true;
                Ok(())
            })?;
        } else {
            ensure!(record.quiesce_confirmed || (record.dto.state=="stopped" && !record.resource_active),
                "old writer identity is unavailable; an explicit Dokploy-owned Stop is required before Start");
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<EnvironmentDto> {
        Ok(self.record(id)?.dto)
    }

    pub fn get_connection(&self, id: &str) -> Result<EnvironmentConnectionDto> {
        Ok(self.connection(id)?.public)
    }

    pub fn managed_worktree_root(&self, id: &str) -> Result<PathBuf> {
        Ok(PathBuf::from(self.record(id)?.managed_worktree_root))
    }

    pub fn inherit_thread(&self, parent: &str, child: &str) -> Result<()> {
        ensure!(
            !child.trim().is_empty(),
            "child thread identity is required"
        );
        self.write(|store| {
            ensure!(
                !store.bindings.contains_key(child),
                "child already has an environment binding"
            );
            if let Some(id) = store.bindings.get(parent).cloned() {
                ensure!(
                    store
                        .environments
                        .get(&id)
                        .is_some_and(|e| e.dto.state != "destroyed"),
                    "parent environment is unavailable"
                );
                store.bindings.insert(child.into(), id);
            }
            Ok(())
        })
    }

    fn record(&self, id: &str) -> Result<EnvironmentRecord> {
        self.read(|store| {
            store
                .environments
                .get(id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("environment not found"))
        })
    }

    fn connection(&self, id: &str) -> Result<ConnectionRecord> {
        self.read(|store| {
            store
                .connections
                .get(id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("environment connection not found"))
        })
    }

    pub fn for_thread(&self, thread_id: &str) -> Result<Option<EnvironmentDto>> {
        self.read(|store| match store.bindings.get(thread_id) {
            None => Ok(None),
            Some(id) => Ok(Some(
                store
                    .environments
                    .get(id)
                    .ok_or_else(|| anyhow::anyhow!("thread environment binding is invalid"))?
                    .dto
                    .clone(),
            )),
        })
    }

    /// The caller enforces idle/native-session and thread-family authorization.
    /// A missing/destroyed target is an error, never a host-execution request.
    pub fn bind_thread(&self, thread_id: &str, environment_id: Option<&str>) -> Result<()> {
        ensure!(!thread_id.trim().is_empty(), "thread identity is required");
        self.write(|store| {
            if let Some(id) = environment_id {
                let record = store
                    .environments
                    .get(id)
                    .ok_or_else(|| anyhow::anyhow!("environment not found"))?;
                ensure!(
                    record.dto.state != "destroyed",
                    "destroyed environment cannot be bound"
                );
                store.bindings.insert(thread_id.into(), id.into());
            } else {
                store.bindings.remove(thread_id);
            }
            Ok(())
        })
    }

    /// No credential is returned. Explicitly pin the local socket instead of
    /// inheriting DOCKER_HOST, contexts, SSH configuration, or client secrets.
    pub fn docker_host(&self, connection_id: &str) -> Result<Option<String>> {
        let value = self.connection(connection_id)?.public.docker_host;
        docker::validate_docker_host(value.as_deref())?;
        Ok(Some(
            value.unwrap_or_else(|| "unix:///var/run/docker.sock".into()),
        ))
    }

    fn context(&self, connection_id: &str) -> Result<LocalDockerContext> {
        Ok(LocalDockerContext {
            docker_host: self.docker_host(connection_id)?,
            probe_user: None,
        })
    }

    fn validate_project_source(&self, path: &Path) -> Result<()> {
        ensure!(
            !path.starts_with(&self.directory) && !self.directory.starts_with(path),
            "project/Git storage must not expose the Supervisor private sidecar"
        );
        if let Some(home) =
            std::env::var_os("HOME").and_then(|home| PathBuf::from(home).canonicalize().ok())
        {
            ensure!(
                path != home,
                "the Supervisor home directory cannot be mounted as project storage"
            );
            for name in [
                ".config", ".ssh", ".gnupg", ".aws", ".docker", ".kube", ".codex", ".claude",
            ] {
                ensure!(!path.starts_with(home.join(name)),"host authentication/configuration storage cannot be mounted as project storage");
            }
        }
        Ok(())
    }

    pub fn validate_generation(&self, id: &str, generation: u64) -> Result<EnvironmentDto> {
        let dto = self.get(id)?;
        ensure!(
            dto.generation == generation && dto.state == "running" && dto.container_id.is_some(),
            "environment generation is stale or unavailable"
        );
        Ok(dto)
    }

    pub fn bridge_directory(&self, id: &str, generation: u64) -> Result<PathBuf> {
        let dto = self.get(id)?;
        ensure!(
            dto.generation == generation && dto.state != "destroyed",
            "environment bridge generation is stale"
        );
        Ok(self
            .directory
            .join("bridges")
            .join(&dto.id)
            .join(generation.to_string()))
    }

    pub async fn save_connection(
        &self,
        id: &str,
        input: &UpdateEnvironmentConnectionRequest,
    ) -> Result<EnvironmentConnectionDto> {
        let _operation = self.operation.lock().await;
        validate_connection(input)?;
        let id = if id == "new" {
            uuid::Uuid::new_v4().to_string()
        } else {
            ensure!(
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')),
                "invalid connection identity"
            );
            id.to_owned()
        };
        let roots = storage::allowed_roots(&input.allowed_roots)?;
        self.write(|store| {
            let old = store.connections.get(&id);
            let key = input
                .api_key
                .as_ref()
                .or_else(|| old.map(|v| &v.api_key))
                .cloned()
                .unwrap_or_default();
            ensure!(
                !key.trim().is_empty() && key.len() <= 8192 && !key.contains(['\r', '\n', '\0']),
                "a valid Dokploy API key is required"
            );
            let public = EnvironmentConnectionDto {
                id: id.clone(),
                name: input.name.trim().into(),
                base_url: input.base_url.trim_end_matches('/').into(),
                configured: true,
                dokploy_environment_id: input.dokploy_environment_id.clone(),
                image: input.image.clone(),
                storage_node: "local".into(),
                docker_host: input.docker_host.clone(),
                allowed_roots: roots
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect(),
                allowed_egress_hosts: input
                    .allowed_egress_hosts
                    .iter()
                    .map(|s| s.to_ascii_lowercase())
                    .collect(),
                cpu_limit: input.cpu_limit,
                memory_limit_bytes: input.memory_limit_bytes,
                pids_limit: input.pids_limit,
                nofile_limit: input.nofile_limit,
            };
            if let Some(old) = old {
                let mut before = serde_json::to_value(&old.public)?;
                let mut after = serde_json::to_value(&public)?;
                before["name"] = serde_json::Value::Null;
                after["name"] = serde_json::Value::Null;
                ensure!(
                    before == after
                        || !store
                            .environments
                            .values()
                            .any(|e| e.dto.connection_id == id && e.dto.state != "destroyed"),
                    "connection storage, image and limits are immutable while it owns environments"
                );
            }
            store.connections.insert(
                id.clone(),
                ConnectionRecord {
                    public: public.clone(),
                    api_key: key,
                },
            );
            Ok(public)
        })
    }

    pub async fn test_connection(&self, id: &str) -> Result<TestEnvironmentConnectionResponse> {
        let connection = self.connection(id)?;
        let result = async {
            let api = DokployClient::new(&connection)?;
            let projects = api.projects().await?;
            ensure!(
                has_environment(&projects, &connection.public.dokploy_environment_id),
                "configured Dokploy environment was not found in project.all"
            );
            let docker = self.driver()?;
            docker.host_info(&self.context(id)?).await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        Ok(match result {
            Ok(()) => TestEnvironmentConnectionResponse { ok:true,detail:"Dokploy authentication and local Linux Docker resource inspection succeeded; storage is verified per environment before deployment".into() },
            Err(error) => TestEnvironmentConnectionResponse { ok:false,detail:public_error(&error,&connection.api_key) },
        })
    }

    pub async fn save_budget(&self, input: &EnvironmentBudgetDto) -> Result<EnvironmentBudgetDto> {
        let _operation = self.operation.lock().await;
        validate_budget(input)?;
        self.write(|store| {
            ensure_budget(store, None, None, input)?;
            store.budget = input.clone();
            Ok(input.clone())
        })
    }

    pub async fn create(
        &self,
        input: &CreateEnvironmentRequest,
        trusted_workspace: &Path,
        trusted_working: &Path,
    ) -> Result<EnvironmentDto> {
        let _operation = self.operation.lock().await;
        ensure!(
            !input.thread_id.is_empty()
                && input.thread_id.len() <= 128
                && input
                    .thread_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')),
            "invalid thread identity"
        );
        let connection = self.connection(&input.connection_id)?;
        let workspace = storage::canonical_directory(trusted_workspace)?;
        let working = storage::canonical_directory(trusted_working)?;
        let roots = storage::allowed_roots(&connection.public.allowed_roots)?;
        for path in storage::git_closure(&workspace, &working, &roots)? {
            self.validate_project_source(&path)?;
        }
        let uid_gid = owner(&working)?;
        let managed_parent = workspace
            .parent()
            .context("workspace parent is unavailable")?
            .join(format!(
                "{}.worktrees",
                workspace
                    .file_name()
                    .context("workspace name is unavailable")?
                    .to_str()
                    .context("workspace name is not UTF-8")?
            ));
        private_dir(&managed_parent)?;
        let managed_root = managed_parent.join(&input.thread_id);
        private_dir(&managed_root)?;
        self.validate_project_source(&managed_root)?;
        set_owner(&managed_root, &uid_gid)?;
        ensure!(
            managed_root.canonicalize()? == managed_root,
            "managed worktree storage must not traverse symlinks"
        );
        let binary = std::env::current_exe()?.canonicalize()?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_rfc3339();
        let name = input
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Dokploy environment");
        ensure!(
            name.len() <= 200 && !name.contains(['\0', '\r', '\n']),
            "invalid environment name"
        );
        let dto = EnvironmentDto {
            id: id.clone(),
            name: name.into(),
            connection_id: input.connection_id.clone(),
            provider: "dokploy".into(),
            thread_id: input.thread_id.clone(),
            generation: 1,
            state: "stopped".into(),
            container_id: None,
            compose_id: None,
            workspace_path: workspace.to_string_lossy().into_owned(),
            worktree_path: (working != workspace).then(|| working.to_string_lossy().into_owned()),
            mounts: Vec::new(),
            limits: EnvironmentLimitsDto {
                cpu: connection.public.cpu_limit,
                memory_bytes: connection.public.memory_limit_bytes,
                pids: connection.public.pids_limit,
                nofile: connection.public.nofile_limit,
            },
            observed_limits: None,
            last_error: None,
            disk_quota_supported: false,
            created_at: now.clone(),
            updated_at: now,
        };
        self.write(|store| {
            ensure!(
                !store
                    .environments
                    .values()
                    .any(|e| e.dto.thread_id == input.thread_id && e.dto.state != "destroyed"),
                "thread already owns an environment"
            );
            store.environments.insert(
                id.clone(),
                EnvironmentRecord {
                    dto: dto.clone(),
                    app_name: format!("pockymoe-{id}"),
                    dokploy_app_name: None,
                    create_pending: false,
                    deploy_pending: false,
                    uid_gid,
                    image: connection.public.image.clone(),
                    worker_binary: binary.to_string_lossy().into_owned(),
                    managed_worktree_root: managed_root.to_string_lossy().into_owned(),
                    daemon_id: None,
                    resource_active: false,
                    pending_quiesce: None,
                    quiesce_confirmed: true,
                },
            );
            Ok(dto)
        })
    }

    pub async fn action(&self, id: &str, action: EnvironmentAction) -> Result<EnvironmentDto> {
        let _operation = self.operation.lock().await;
        let record = self.record(id)?;
        if action == EnvironmentAction::Destroy && record.dto.state == "destroyed" {
            return Ok(record.dto);
        }
        ensure!(
            record.dto.state != "destroyed",
            "environment has been destroyed; its data is retained"
        );
        let connection = self.connection(&record.dto.connection_id)?;
        let preserve_interrupted = record.dto.state == "interrupted";
        let result = match action {
            EnvironmentAction::Start => self.start(id, false).await,
            EnvironmentAction::Rebuild => self.start(id, true).await,
            EnvironmentAction::Stop => self.stop(id, false).await,
            EnvironmentAction::Destroy => self.stop(id, true).await,
        };
        if let Err(error) = result {
            let message = public_error(&error, &connection.api_key);
            self.write(|store| {
                let record = store
                    .environments
                    .get_mut(id)
                    .context("environment not found")?;
                let interrupted = record.dto.state == "interrupted"
                    || (preserve_interrupted && !record.quiesce_confirmed);
                if record.dto.container_id.is_some() || record.dto.state == "running" {
                    remember_writer(record);
                    record.dto.generation = record
                        .dto
                        .generation
                        .checked_add(1)
                        .context("environment generation exhausted")?;
                }
                record.dto.state = if interrupted { "interrupted" } else { "error" }.into();
                record.dto.container_id = None;
                record.dto.observed_limits = None;
                if !interrupted {
                    record.dto.last_error = Some(message.clone());
                }
                record.dto.updated_at = now_rfc3339();
                Ok(())
            })?;
            if self.record(id)?.pending_quiesce.is_some() {
                let _ = self.quiesce_inner(id).await;
            }
            bail!("{message}");
        }
        self.get(id)
    }

    async fn start(&self, id: &str, rebuild: bool) -> Result<()> {
        ensure!(
            !self.record(id)?.deploy_pending,
            "Dokploy deployment is unconfirmed; owned Stop must complete before Start"
        );
        if self.record(id)?.dto.state != "running" {
            self.quiesce_inner(id).await?;
        }
        let record = self.record(id)?;
        let connection = self.connection(&record.dto.connection_id)?;
        let context = self.context(&record.dto.connection_id)?;
        let docker = self.driver()?;
        let host = docker.host_info(&context).await?;
        if record.dto.state == "running" && !rebuild {
            ensure!(
                record.daemon_id.as_deref() == Some(&host.daemon_id),
                "Docker daemon identity changed"
            );
            let values = docker
                .inspect_environment(&context, id)
                .await
                .map_err(|_| anyhow::anyhow!("environment inspect failed"))?;
            ensure!(
                values.len() == 1,
                "environment container identity is ambiguous"
            );
            ensure!(
                record.dto.container_id.as_deref() == values[0]["Id"].as_str(),
                "environment container was replaced outside its generation"
            );
            docker::verify_container(&values[0], &expected(&record))?;
            return Ok(());
        }
        if rebuild || record.resource_active || record.dto.compose_id.is_some() {
            self.stop(id, false).await?;
        }
        self.read(|store| ensure_budget(store, Some(id), Some(&record.dto.limits), &store.budget))?;
        self.read(|store| {
            let (cpu, memory, _) = totals(store, Some(id));
            ensure!(
                cpu + record.dto.limits.cpu + store.budget.reserved_cpu <= host.cpus + 0.000001,
                "host CPU reserve would be exceeded"
            );
            ensure!(
                memory
                    .checked_add(record.dto.limits.memory_bytes)
                    .and_then(|v| v.checked_add(store.budget.reserved_memory_bytes))
                    .is_some_and(|v| v <= host.memory_bytes),
                "host memory reserve would be exceeded"
            );
            Ok(())
        })?;
        self.write(|store| {
            let record = store
                .environments
                .get_mut(id)
                .context("environment not found")?;
            if let Some(daemon) = &record.daemon_id {
                ensure!(
                    daemon == &host.daemon_id,
                    "Docker daemon identity changed; an explicit storage migration is required"
                );
            }
            record.dto.generation = record
                .dto
                .generation
                .checked_add(1)
                .context("environment generation exhausted")?;
            record.dto.state = "starting".into();
            record.dto.container_id = None;
            record.dto.observed_limits = None;
            record.dto.last_error = None;
            record.dto.updated_at = now_rfc3339();
            record.daemon_id = Some(host.daemon_id.clone());
            record.resource_active = true;
            record.quiesce_confirmed = true;
            Ok(())
        })?;
        let record = self.record(id)?;
        let mounts = self
            .mounts(&record, &connection, &host, docker.as_ref(), &context)
            .await?;
        self.write(|store| {
            store
                .environments
                .get_mut(id)
                .context("environment not found")?
                .dto
                .mounts = mounts;
            Ok(())
        })?;
        let record = self.record(id)?;
        let yaml = compose::render(&record.dto, &record.image, &record.uid_gid)?;
        let api = DokployClient::new(&connection)?;
        let compose_id = match &record.dto.compose_id {
            Some(id) => id.clone(),
            None => match find_owned_compose(
                &api.environment(&connection.public.dokploy_environment_id)
                    .await?,
                &record.app_name,
                &connection.public.dokploy_environment_id,
            )? {
                Some(id) => id,
                None => {
                    ensure!(!record.create_pending,"Dokploy creation outcome is uncertain and no owned resource is visible; refusing to create a duplicate");
                    self.write(|store| {
                        store
                            .environments
                            .get_mut(id)
                            .context("environment not found")?
                            .create_pending = true;
                        Ok(())
                    })?;
                    api.create(
                        &connection.public.dokploy_environment_id,
                        &record.app_name,
                        &yaml,
                    )
                    .await?
                }
            },
        };
        // Persist before querying detail or mutating the created resource. A
        // lost create response is recovered by exact name through environment.one.
        self.write(|store| {
            let record = store
                .environments
                .get_mut(id)
                .context("environment not found")?;
            record.dto.compose_id = Some(compose_id.clone());
            record.create_pending = false;
            Ok(())
        })?;
        let app_name = verify_owned_compose(
            &api.compose(&compose_id).await?,
            &record.app_name,
            &connection.public.dokploy_environment_id,
            record.dokploy_app_name.as_deref(),
        )?;
        self.write(|store| {
            store
                .environments
                .get_mut(id)
                .context("environment not found")?
                .dokploy_app_name = Some(app_name);
            Ok(())
        })?;
        api.update(&compose_id, &yaml).await?;
        self.write(|store| {
            let record = store
                .environments
                .get_mut(id)
                .context("environment not found")?;
            record.deploy_pending = true;
            record.quiesce_confirmed = false;
            Ok(())
        })?;
        api.deploy(&compose_id).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        loop {
            let values = docker
                .inspect_environment(&context, id)
                .await
                .map_err(|_| anyhow::anyhow!("deployment container inspect failed"))?;
            let running: Vec<_> = values
                .iter()
                .filter(|v| v["State"]["Running"] == true)
                .collect();
            ensure!(
                running.len() <= 1,
                "multiple running containers claim the environment"
            );
            if !running.is_empty() && api.jobs_idle(&compose_id).await? {
                let container = running[0];
                let observed = docker::verify_container(container, &expected(&record))?;
                let container_id = container["Id"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .context("container identity is unavailable")?
                    .to_owned();
                self.write(|store| {
                    let record = store
                        .environments
                        .get_mut(id)
                        .context("environment not found")?;
                    record.dto.state = "running".into();
                    record.dto.container_id = Some(container_id);
                    record.deploy_pending = false;
                    record.quiesce_confirmed = false;
                    record.dto.observed_limits = Some(observed);
                    record.dto.updated_at = now_rfc3339();
                    Ok(())
                })?;
                return Ok(());
            }
            ensure!(tokio::time::Instant::now()<deadline,"Dokploy deployment did not produce a verified container; resources remain reserved until explicit stop/destroy");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn mounts(
        &self,
        record: &EnvironmentRecord,
        connection: &ConnectionRecord,
        host: &DockerHostInfo,
        docker: &dyn EnvironmentDocker,
        context: &LocalDockerContext,
    ) -> Result<Vec<EnvironmentMountDto>> {
        let context = &LocalDockerContext {
            docker_host: context.docker_host.clone(),
            probe_user: Some(record.uid_gid.clone()),
        };
        let workspace = storage::canonical_directory(Path::new(&record.dto.workspace_path))?;
        let working = storage::canonical_directory(Path::new(
            record
                .dto
                .worktree_path
                .as_ref()
                .unwrap_or(&record.dto.workspace_path),
        ))?;
        ensure!(
            workspace == Path::new(&record.dto.workspace_path)
                && working
                    == Path::new(
                        record
                            .dto
                            .worktree_path
                            .as_ref()
                            .unwrap_or(&record.dto.workspace_path)
                    ),
            "stored workspace/working-copy path changed; recreate the environment explicitly"
        );
        let roots = storage::allowed_roots(&connection.public.allowed_roots)?;
        let paths = storage::git_closure(&workspace, &working, &roots)?;
        ensure!(
            owner(&working)? == record.uid_gid,
            "working-copy ownership changed"
        );
        let state = self.directory.join("state").join(&record.dto.id);
        let bridge = self.bridge_directory(&record.dto.id, record.dto.generation)?;
        private_dir(&state)?;
        private_dir(&bridge)?;
        set_owner(&state, &record.uid_gid)?;
        set_owner(&bridge, &record.uid_gid)?;
        let mut mounts = Vec::new();
        for path in paths {
            self.validate_project_source(&path)?;
            ensure!(
                ![Path::new("/pockymoe"), Path::new("/opt/pockymoe")]
                    .iter()
                    .any(|reserved| path.starts_with(reserved) || reserved.starts_with(&path))
                    && path != Path::new("/tmp")
                    && !path.starts_with("/run"),
                "project path collides with the worker/state namespace"
            );
            mounts.push(
                storage::verified_mount(
                    docker,
                    context,
                    host,
                    &path,
                    path.to_str().context("project path is not UTF-8")?,
                    false,
                )
                .await?,
            );
        }
        let managed = Path::new(&record.managed_worktree_root);
        self.validate_project_source(managed)?;
        ensure!(
            managed.canonicalize()? == managed,
            "managed worktree storage source changed"
        );
        mounts.push(
            storage::verified_mount(
                docker,
                context,
                host,
                managed,
                managed
                    .to_str()
                    .context("managed worktree path is not UTF-8")?,
                false,
            )
            .await?,
        );
        mounts.push(
            storage::verified_mount(docker, context, host, &state, compose::STATE_PATH, false)
                .await?,
        );
        mounts.push(
            // Workers only connect to these sockets. Read-only directory access
            // prevents replacing paths while the Supervisor sets socket ownership.
            storage::verified_mount(docker, context, host, &bridge, compose::BRIDGE_PATH, true)
                .await?,
        );
        // Never accept a caller-provided executable or persist an obsolete
        // executable after an installed runtime update.
        let binary = std::env::current_exe()?.canonicalize()?;
        ensure!(
            !mounts.iter().any(|mount| !mount.read_only
                && (binary.starts_with(&mount.target) || binary.starts_with(&mount.source))),
            "worker executable must be outside writable project/state mounts"
        );
        mounts.push(
            storage::verified_mount(docker, context, host, &binary, compose::WORKER_PATH, true)
                .await?,
        );
        Ok(mounts)
    }

    async fn stop(&self, id: &str, destroy: bool) -> Result<()> {
        self.stop_inner(id, destroy, false).await
    }

    async fn stop_inner(&self, id: &str, destroy: bool, retain_interrupted: bool) -> Result<()> {
        let record = self.record(id)?;
        let connection = self.connection(&record.dto.connection_id)?;
        let context = self.context(&record.dto.connection_id)?;
        let docker = self.driver()?;
        let host = docker.host_info(&context).await?;
        ensure!(
            record
                .daemon_id
                .as_ref()
                .is_none_or(|daemon| daemon == &host.daemon_id),
            "Docker daemon identity changed; retained resources require explicit storage recovery"
        );
        self.write(|store| {
            let record = store
                .environments
                .get_mut(id)
                .context("environment not found")?;
            remember_writer(record);
            if !retain_interrupted {
                record.dto.generation = record
                    .dto
                    .generation
                    .checked_add(1)
                    .context("environment generation exhausted")?;
                record.dto.state = "stopping".into();
            }
            record.dto.container_id = None;
            record.dto.observed_limits = None;
            record.dto.updated_at = now_rfc3339();
            Ok(())
        })?;
        if self.record(id)?.pending_quiesce.is_some() {
            let _ = self.quiesce_inner(id).await;
        }
        let api = DokployClient::new(&connection)?;
        let mut compose_id = match record.dto.compose_id.clone() {
            Some(id) => Some(id),
            None => find_owned_compose(
                &api.environment(&connection.public.dokploy_environment_id)
                    .await?,
                &record.app_name,
                &connection.public.dokploy_environment_id,
            )?,
        };
        ensure!(compose_id.is_some() || !record.create_pending,"Dokploy creation outcome is uncertain and no owned resource is visible; resources are retained for recovery");
        if let Some(id) = &compose_id {
            if let Some(value) = api.optional_compose(id).await? {
                let app_name = verify_owned_compose(
                    &value,
                    &record.app_name,
                    &connection.public.dokploy_environment_id,
                    record.dokploy_app_name.as_deref(),
                )?;
                self.write(|store| {
                    let record = store
                        .environments
                        .get_mut(&record.dto.id)
                        .context("environment not found")?;
                    record.dto.compose_id = Some(id.clone());
                    record.dokploy_app_name = Some(app_name.clone());
                    record.create_pending = false;
                    Ok(())
                })?;
                // Delete also clears queued jobs in Dokploy. Stop retains the record,
                // and explicitly clears queued deploys before attempting to stop.
                api.clean_queues(id).await?;
                let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
                while !api.jobs_idle(id).await? {
                    ensure!(tokio::time::Instant::now()<deadline,"Dokploy deployment remains active; stop/destroy cannot safely release its resources");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                // Re-apply our own safe raw spec before the fixed stop/down
                // command. The runner supplies the correct Compose cwd.
                api.update(
                    id,
                    &compose::render(&record.dto, &record.image, &record.uid_gid)?,
                )
                .await?;
                api.stop_or_down(id, &app_name, destroy).await?;
                let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
                while !api.jobs_idle(id).await? {
                    ensure!(
                        tokio::time::Instant::now() < deadline,
                        "Dokploy lifecycle command remains active; resources are retained"
                    );
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                if destroy {
                    let remaining = docker
                        .inspect_environment(&context, &record.dto.id)
                        .await
                        .map_err(|_| anyhow::anyhow!("destroyed environment inspection failed"))?;
                    ensure!(remaining.is_empty(),"Dokploy did not remove the owned environment containers; data and capacity are retained for recovery");
                    api.delete(id).await?;
                }
            } else {
                compose_id = None;
            }
        }
        let values = docker
            .inspect_environment(&context, id)
            .await
            .map_err(|_| anyhow::anyhow!("stopped environment inspection failed"))?;
        ensure!(
            !values.iter().any(|v| v["State"]["Running"] == true),
            "Dokploy did not stop the actual environment container"
        );
        if destroy {
            ensure!(values.is_empty(),"Dokploy deleted its record but left environment containers; data and capacity are retained for recovery");
        }
        ensure!(
            docker.host_info(&context).await?.daemon_id == host.daemon_id,
            "Docker daemon identity changed during lifecycle confirmation; resources are retained"
        );
        self.write(|store| {
            let record = store
                .environments
                .get_mut(id)
                .context("environment not found")?;
            record.dto.state = if retain_interrupted {
                "interrupted"
            } else if destroy {
                "destroyed"
            } else {
                "stopped"
            }
            .into();
            record.dto.compose_id = if destroy { None } else { compose_id };
            if record.dto.compose_id.is_none() {
                record.dokploy_app_name = None;
            }
            record.create_pending = false;
            record.deploy_pending = false;
            if !retain_interrupted {
                record.dto.last_error = None;
            }
            record.dto.updated_at = now_rfc3339();
            record.resource_active = retain_interrupted;
            record.pending_quiesce = None;
            record.quiesce_confirmed = true;
            Ok(())
        })
    }
}

fn remember_writer(record: &mut EnvironmentRecord) {
    if record.pending_quiesce.is_none() && !record.quiesce_confirmed {
        if let (Some(container_id), Some(daemon_id)) = (&record.dto.container_id, &record.daemon_id)
        {
            record.pending_quiesce = Some(QuiesceTarget {
                container_id: container_id.clone(),
                environment_id: record.dto.id.clone(),
                generation: record.dto.generation,
                daemon_id: daemon_id.clone(),
            });
        }
    }
}

fn expected(record: &EnvironmentRecord) -> ExpectedContainer<'_> {
    ExpectedContainer {
        id: &record.dto.id,
        generation: record.dto.generation,
        uid_gid: &record.uid_gid,
        image: &record.image,
        mounts: &record.dto.mounts,
        limits: &record.dto.limits,
    }
}

fn public_error(error: &anyhow::Error, key: &str) -> String {
    let message = error.to_string();
    if key.is_empty() {
        message
    } else {
        message.replace(key, "[redacted]")
    }
}

fn has_environment(value: &serde_json::Value, id: &str) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            value["environmentId"] == id || map.values().any(|value| has_environment(value, id))
        }
        serde_json::Value::Array(values) => values.iter().any(|value| has_environment(value, id)),
        _ => false,
    }
}

fn validate_connection(input: &UpdateEnvironmentConnectionRequest) -> Result<()> {
    ensure!(
        input.storage_node == "local",
        "remote Dokploy storage without an explicit replication adapter is unsupported"
    );
    let url = url::Url::parse(&input.base_url)
        .map_err(|_| anyhow::anyhow!("invalid Dokploy base URL"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Dokploy base URL must be HTTP(S) without credentials, query or fragment"
    );
    ensure!(
        !input.name.trim().is_empty()
            && input.name.len() <= 200
            && !input.name.contains(['\0', '\r', '\n']),
        "invalid connection name"
    );
    ensure!(
        !input.dokploy_environment_id.is_empty()
            && input.dokploy_environment_id.len() <= 256
            && !input.dokploy_environment_id.contains(['\0', '\r', '\n']),
        "invalid Dokploy environment identity"
    );
    ensure!(
        !input.image.is_empty()
            && input.image.len() <= 512
            && !input
                .image
                .contains(|c: char| c.is_whitespace() || matches!(c, '$' | '\0')),
        "configure an explicit compatible Linux worker image"
    );
    ensure!(
        input.image.contains('@')
            || input
                .image
                .rsplit('/')
                .next()
                .is_some_and(|part| part.contains(':') && !part.ends_with(":latest")),
        "worker image must have an explicit version tag or digest"
    );
    docker::validate_docker_host(input.docker_host.as_deref())?;
    ensure!(
        input.allowed_egress_hosts.len() <= 128
            && input.allowed_egress_hosts.iter().all(|host| {
                host.len() <= 253
                    && host.contains('.')
                    && !host.ends_with('.')
                    && host.parse::<std::net::IpAddr>().is_err()
                    && host.split('.').all(|part| {
                        !part.is_empty()
                            && part.len() <= 63
                            && !part.starts_with('-')
                            && !part.ends_with('-')
                            && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    })
            }),
        "egress hosts must be exact DNS names without wildcards, addresses, credentials or ports"
    );
    validate_limits(&EnvironmentLimitsDto {
        cpu: input.cpu_limit,
        memory_bytes: input.memory_limit_bytes,
        pids: input.pids_limit,
        nofile: input.nofile_limit,
    })
}

fn validate_limits(limits: &EnvironmentLimitsDto) -> Result<()> {
    ensure!(
        limits.cpu.is_finite() && (0.1..=1024.0).contains(&limits.cpu),
        "CPU limit must be finite and between 0.1 and 1024"
    );
    ensure!(
        (64 * 1024 * 1024..=1024 * 1024 * 1024 * 1024_u64).contains(&limits.memory_bytes),
        "memory limit must be between 64 MiB and 1 TiB"
    );
    ensure!(
        (16..=65536).contains(&limits.pids),
        "PID limit must be between 16 and 65536"
    );
    ensure!(
        (64..=1048576).contains(&limits.nofile),
        "nofile limit must be between 64 and 1048576"
    );
    Ok(())
}

fn validate_budget(budget: &EnvironmentBudgetDto) -> Result<()> {
    ensure!(
        (1..=1024).contains(&budget.max_running),
        "running environment budget must be between 1 and 1024"
    );
    ensure!(
        budget.max_cpu.is_finite()
            && budget.max_cpu > 0.0
            && budget.max_cpu <= 65536.0
            && budget.reserved_cpu.is_finite()
            && budget.reserved_cpu >= 0.0
            && budget.reserved_cpu <= 65536.0,
        "invalid CPU budget/reserve"
    );
    ensure!(
        budget.max_memory_bytes >= 64 * 1024 * 1024
            && budget.max_memory_bytes <= 1024 * 1024 * 1024 * 1024 * 1024_u64
            && budget.reserved_memory_bytes <= 1024 * 1024 * 1024 * 1024 * 1024_u64,
        "invalid memory budget/reserve"
    );
    Ok(())
}

fn totals(store: &Store, exclude: Option<&str>) -> (f64, u64, u32) {
    store
        .environments
        .values()
        .filter(|e| e.resource_active && Some(e.dto.id.as_str()) != exclude)
        .fold((0.0, 0, 0), |(cpu, memory, count), e| {
            (
                cpu + e.dto.limits.cpu,
                memory.saturating_add(e.dto.limits.memory_bytes),
                count + 1,
            )
        })
}

fn ensure_budget(
    store: &Store,
    exclude: Option<&str>,
    additional: Option<&EnvironmentLimitsDto>,
    budget: &EnvironmentBudgetDto,
) -> Result<()> {
    let (mut cpu, mut memory, mut count) = totals(store, exclude);
    if let Some(limits) = additional {
        cpu += limits.cpu;
        memory = memory.saturating_add(limits.memory_bytes);
        count += 1;
    }
    ensure!(
        count <= budget.max_running
            && cpu <= budget.max_cpu + 0.000001
            && memory <= budget.max_memory_bytes,
        "environment resource budget would be exceeded"
    );
    Ok(())
}

fn owner(path: &Path) -> Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = path.metadata()?;
        ensure!(metadata.uid()!=0,"working-copy owner is root; provision non-root project storage before using an environment");
        Ok(format!("{}:{}", metadata.uid(), metadata.gid()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        bail!("execution environments require Linux same-storage semantics")
    }
}

fn set_owner(path: &Path, uid_gid: &str) -> Result<()> {
    #[cfg(unix)]
    {
        let (uid, gid) = uid_gid
            .split_once(':')
            .context("invalid environment owner")?;
        std::os::unix::fs::chown(path, Some(uid.parse()?), Some(gid.parse()?))
            .context("private environment storage owner could not be set")?;
    }
    Ok(())
}

fn private_dir(path: &Path) -> Result<()> {
    if let Ok(metadata) = path.symlink_metadata() {
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "private environment storage is not a real directory"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        bail!("private environment storage requires Unix permissions");
    }
    Ok(())
}

fn open_store(directory: &Path) -> Result<(Store, File)> {
    private_dir(directory)?;
    let lock_path = directory.join("owner.lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let lock = options.open(lock_path)?;
    lock.try_lock()?;
    let path = directory.join("resources.json");
    let mut store: Store = if path.exists() {
        ensure!(
            path.symlink_metadata()?.file_type().is_file(),
            "environment store is not a regular private file"
        );
        serde_json::from_reader(File::open(path)?)?
    } else {
        Store::default()
    };
    ensure!(store.version == 1, "unsupported environment store version");
    for record in store.environments.values_mut() {
        if matches!(
            record.dto.state.as_str(),
            "creating" | "starting" | "running" | "stopping"
        ) {
            remember_writer(record);
            record.dto.state = "interrupted".into();
            record.dto.container_id = None;
            record.dto.observed_limits = None;
            record.dto.generation = record
                .dto
                .generation
                .checked_add(1)
                .context("environment generation exhausted")?;
            record.dto.last_error=Some("Supervisor restarted; explicitly stop or rebuild to reconcile retained provider resources".into());
            record.dto.updated_at = now_rfc3339();
            record.resource_active = true;
        }
        if matches!(record.dto.state.as_str(), "interrupted" | "error") && !record.quiesce_confirmed
        {
            remember_writer(record);
            record.dto.container_id = None;
            record.dto.observed_limits = None;
            record.resource_active = true;
        }
    }
    persist(directory, &store)?;
    Ok((store, lock))
}

fn persist(directory: &Path, store: &Store) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(file.as_file_mut(), store)?;
    file.as_file_mut().sync_all()?;
    file.persist(directory.join("resources.json"))
        .map_err(|_| anyhow::anyhow!("environment store could not be replaced"))?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;
