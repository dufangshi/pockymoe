//! Execution data plane. Environment identities are selected by the Supervisor,
//! never inferred from a caller-supplied cwd. Isolated operations fail closed.
use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
};

pub mod worker;

#[derive(Clone)]
pub struct ExecutionContext {
    pub manager: Arc<crate::environment::EnvironmentManager>,
    pub environment_id: String,
    pub generation: u64,
    pub container_id: Option<String>,
    pub thread_id: String,
    pub upstream_directory: PathBuf,
    pub workspace_path: PathBuf,
    pub working_directory: PathBuf,
}

tokio::task_local! { static EXECUTION: Option<ExecutionContext>; }
thread_local! { static BLOCKING: std::cell::RefCell<Option<ExecutionContext>> = const { std::cell::RefCell::new(None) }; }

pub fn current() -> Option<ExecutionContext> {
    EXECUTION
        .try_with(Clone::clone)
        .ok()
        .flatten()
        .or_else(|| BLOCKING.with(|c| c.borrow().clone()))
}
pub async fn scope<T>(
    context: Option<ExecutionContext>,
    future: impl std::future::Future<Output = T>,
) -> T {
    EXECUTION.scope(context, future).await
}
pub fn blocking_scope<T>(context: Option<ExecutionContext>, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<ExecutionContext>);
    impl Drop for Restore {
        fn drop(&mut self) {
            BLOCKING.with(|c| {
                c.replace(self.0.take());
            });
        }
    }
    let _restore = Restore(BLOCKING.with(|c| c.replace(context)));
    action()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    pub id: String,
    pub argv: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

impl ExecutionContext {
    pub fn checked(&self) -> Result<pockymoe_protocol::EnvironmentDto> {
        let env = self.manager.get(&self.environment_id)?;
        ensure!(
            env.provider == "dokploy",
            "unsupported_environment_provider: {}",
            env.provider
        );
        ensure!(
            env.generation == self.generation,
            "stale_environment_generation"
        );
        ensure!(
            env.state == "running",
            "environment_not_running: {}",
            env.state
        );
        let bound = self
            .manager
            .for_thread(&self.thread_id)?
            .context("thread_environment_unbound")?;
        ensure!(
            bound.id == env.id && bound.generation == self.generation,
            "thread_environment_changed"
        );
        Ok(env)
    }
    pub fn docker(&self) -> Result<Command> {
        let environment = self.checked()?;
        let mut cmd = Command::new("docker");
        cmd.arg("--config")
            .arg(empty_docker_config(&self.manager, &environment)?);
        let host = self
            .manager
            .docker_host(&environment.connection_id)?
            .unwrap_or_else(|| "unix:///var/run/docker.sock".into());
        cmd.args(["--host", &host]);
        cmd.env_clear().env("PATH", "/usr/local/bin:/usr/bin:/bin");
        Ok(cmd)
    }
    pub fn std_docker(&self) -> Result<std::process::Command> {
        let environment = self.checked()?;
        let mut cmd = std::process::Command::new("docker");
        cmd.arg("--config")
            .arg(empty_docker_config(&self.manager, &environment)?);
        let host = self
            .manager
            .docker_host(&environment.connection_id)?
            .unwrap_or_else(|| "unix:///var/run/docker.sock".into());
        cmd.args(["--host", &host]);
        cmd.env_clear().env("PATH", "/usr/local/bin:/usr/bin:/bin");
        Ok(cmd)
    }
    pub async fn spawn(&self, request: &RunRequest) -> Result<Child> {
        let environment = self.checked()?;
        let container = environment
            .container_id
            .context("environment_container_missing")?;
        let mut cmd = self.docker()?;
        cmd.args([
            "exec",
            "-i",
            &container,
            "/opt/pockymoe/bin/pockymoe",
            "environment-worker",
            "run",
            &request.id,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
        let mut child = cmd.spawn().context("environment exec failed")?;
        let input = serde_json::to_vec(request)?;
        let stdin = child.stdin.as_mut().context("missing environment stdin")?;
        stdin.write_all(&input).await?;
        stdin.write_all(b"\n").await?;
        Ok(child)
    }
    pub fn request(&self, request: &Value) -> Result<Value> {
        use std::io::{Read, Write};
        let environment = self.checked()?;
        let mut cmd = self.std_docker()?;
        cmd.args([
            "exec",
            "-i",
            environment
                .container_id
                .as_deref()
                .context("environment_container_missing")?,
            "/opt/pockymoe/bin/pockymoe",
            "environment-worker",
            "request",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
        let mut child = cmd
            .spawn()
            .context("environment file transport unavailable")?;
        child
            .stdin
            .take()
            .context("missing worker input")?
            .write_all(&serde_json::to_vec(request)?)?;
        let stdout = child.stdout.take().context("missing worker output")?;
        let reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
            let mut output = Vec::new();
            stdout.take(16 * 1024 * 1024 + 1).read_to_end(&mut output)?;
            Ok(output)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("environment operation timed out; outcome may be uncertain")
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let output = reader
            .join()
            .map_err(|_| anyhow!("environment reader failed"))??;
        ensure!(
            output.len() <= 16 * 1024 * 1024,
            "environment result too large"
        );
        ensure!(status.success(), "environment worker request failed");
        self.checked()?;
        let value: Value =
            serde_json::from_slice(&output).context("invalid environment response")?;
        if value["ok"] != true {
            bail!(
                "environment operation failed: {}",
                value["error"].as_str().unwrap_or("unknown")
            );
        }
        Ok(value["value"].clone())
    }
    fn cleanup_docker(&self) -> Result<std::process::Command> {
        let env = self.manager.get(&self.environment_id)?;
        let mut command = std::process::Command::new("docker");
        command
            .arg("--config")
            .arg(empty_docker_config(&self.manager, &env)?);
        let host = self
            .manager
            .docker_host(&env.connection_id)?
            .unwrap_or_else(|| "unix:///var/run/docker.sock".into());
        command.args(["--host", &host]);
        command
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin");
        Ok(command)
    }
    fn cleanup_target(&self) -> Result<Option<&str>> {
        let Some(container) = self.container_id.as_deref() else {
            return Ok(None);
        };
        let output = bounded_cleanup(self.cleanup_docker()?.args(["inspect", container]))?;
        let values: Value = serde_json::from_slice(&output)?;
        let value = &values[0];
        ensure!(
            value["Id"].as_str() == Some(container)
                && value["Config"]["Labels"]["ca.pockymoe.environment"].as_str()
                    == Some(self.environment_id.as_str())
                && value["Config"]["Labels"]["ca.pockymoe.generation"].as_str()
                    == Some(self.generation.to_string().as_str()),
            "foreign cleanup target"
        );
        Ok((value["State"]["Running"] == true).then_some(container))
    }
    pub async fn kill(&self, id: &str) -> Result<()> {
        let context = self.clone();
        let id = id.to_owned();
        let result = tokio::task::spawn_blocking(move || context.kill_sync(&id)).await?;
        if result.is_err() {
            // A lost worker cannot prove that its adopted descendants exited.
            // Fence and stop its entire retained container before admitting work.
            let _ = self
                .manager
                .action(
                    &self.environment_id,
                    pockymoe_protocol::EnvironmentAction::Stop,
                )
                .await;
        }
        result
    }
    /// Cleanup retains only authority to stop a captured process domain in its
    /// original, label-checked container. It cannot execute work in a new generation.
    pub fn kill_sync(&self, id: &str) -> Result<()> {
        ensure!(
            uuid::Uuid::parse_str(id).is_ok(),
            "invalid process identity"
        );
        let Some(container) = self.cleanup_target()? else {
            return Ok(());
        };
        bounded_cleanup(self.cleanup_docker()?.args([
            "exec",
            container,
            "/opt/pockymoe/bin/pockymoe",
            "environment-worker",
            "kill",
            id,
        ]))?;
        Ok(())
    }
    pub async fn open_tcp(&self, port: u16) -> Result<tokio::io::DuplexStream> {
        ensure!(port > 0, "invalid port");
        let env = self.checked()?;
        let mut child = self
            .docker()?
            .args([
                "exec",
                "-i",
                env.container_id.as_deref().context("missing container")?,
                "/opt/pockymoe/bin/pockymoe",
                "environment-worker",
                "tcp",
                &port.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdin = child.stdin.take().context("missing TCP input")?;
        let mut stdout = child.stdout.take().context("missing TCP output")?;
        let (user, io) = tokio::io::duplex(64 * 1024);
        let (mut read, mut write) = tokio::io::split(io);
        let context = self.clone();
        tokio::spawn(async move {
            tokio::select! {
                _=tokio::io::copy(&mut read,&mut stdin)=>{},
                _=tokio::io::copy(&mut stdout,&mut write)=>{},
                _=async { loop {tokio::time::sleep(std::time::Duration::from_millis(250)).await;if context.checked().is_err(){break}} }=>{}
            }
            let _ = child.kill().await;
        });
        Ok(user)
    }
    pub fn launch_env(&self) -> Vec<(String, String)> {
        let token = issue_capability(self);
        let proxy = format!("http://{token}:sandbox@127.0.0.1:45681");
        vec![
            ("POCKYMOE_TOKEN".into(), token.clone()),
            ("REMOTE_CODEX_TOKEN".into(), token),
            ("POCKYMOE_THREAD_ID".into(), self.thread_id.clone()),
            ("REMOTE_CODEX_THREAD_ID".into(), self.thread_id.clone()),
            ("POCKYMOE_URL".into(), "http://127.0.0.1:45680".into()),
            ("REMOTE_CODEX_URL".into(), "http://127.0.0.1:45680".into()),
            ("HTTPS_PROXY".into(), proxy.clone()),
            ("https_proxy".into(), proxy),
            ("NO_PROXY".into(), "127.0.0.1,localhost".into()),
        ]
    }
}

fn empty_docker_config(
    manager: &crate::environment::EnvironmentManager,
    env: &pockymoe_protocol::EnvironmentDto,
) -> Result<PathBuf> {
    let path = manager
        .bridge_directory(&env.id, env.generation)?
        .join("engine-client");
    std::fs::create_dir_all(&path)?;
    ensure!(
        std::fs::symlink_metadata(&path)?.is_dir() && std::fs::read_dir(&path)?.next().is_none(),
        "environment Docker client configuration is not empty"
    );
    Ok(path)
}

fn bounded_cleanup(command: &mut std::process::Command) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("environment cleanup transport unavailable")?;
    let stdout = child.stdout.take().context("missing cleanup response")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("environment cleanup timed out; stop the retained environment");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let bytes = reader
        .join()
        .map_err(|_| anyhow!("environment cleanup reader failed"))??;
    ensure!(
        status.success(),
        "environment cleanup target unavailable; stop the retained environment"
    );
    ensure!(
        bytes.len() <= 1024 * 1024,
        "environment cleanup response too large"
    );
    Ok(bytes)
}

pub fn file_request(op: &str, root: &Path, path: &str, extra: Value) -> Result<Option<Value>> {
    let Some(context) = current() else {
        return Ok(None);
    };
    let environment = context.checked()?;
    let root = if root == context.workspace_path || root == Path::new(&environment.workspace_path) {
        context.working_directory.as_path()
    } else {
        root
    };
    let mut request = json!({"op":op,"root":root,"path":path});
    if let Some(fields) = extra.as_object() {
        request.as_object_mut().unwrap().extend(fields.clone());
    }
    context.request(&request).map(Some)
}

impl crate::Supervisor {
    pub fn execution_context(&self, thread: &str) -> Result<Option<ExecutionContext>> {
        Ok(self.environments.for_thread(thread)?.map(|env| {
            let saved = self.get_thread(thread).ok();
            let working_directory = saved
                .as_ref()
                .and_then(|t| self.session_cwd(t))
                .unwrap_or_else(|| {
                    env.worktree_path
                        .clone()
                        .unwrap_or_else(|| env.workspace_path.clone())
                });
            ExecutionContext {
                manager: self.environments.clone(),
                environment_id: env.id,
                generation: env.generation,
                container_id: env.container_id.clone(),
                thread_id: thread.into(),
                upstream_directory: crate::upstreams::directory(&self.config.database_url),
                workspace_path: PathBuf::from(&env.workspace_path),
                working_directory: PathBuf::from(working_directory),
            }
        }))
    }
}

pub(crate) fn isolated_harness_environment(harness: &str) -> Result<Vec<(&'static str, String)>> {
    let context = current().context("missing environment context")?;
    context.checked()?;
    ensure!(crate::acp::mcp_servers::list_redacted()?.is_empty(),"unsupported_environment_mcp: configure stdio MCP in the environment profile; host registry credentials are not projected");
    if harness == "custom" || harness.starts_with("env-fixture") {
        return Ok(Vec::new());
    }
    let profile=crate::upstreams::active_profile(&context.upstream_directory,harness)?
        .context("unsupported_environment_auth: select an API upstream; native subscription credentials are not projected")?;
    ensure!(
        !profile.api_key.is_empty(),
        "unsupported_environment_auth: API credential required"
    );
    let token = context
        .launch_env()
        .into_iter()
        .find(|(k, _)| k == "POCKYMOE_TOKEN")
        .context("missing thread gateway capability")?
        .1;
    let base = format!("http://127.0.0.1:45680/model/{harness}");
    match harness {
        "codex" => Ok(vec![
            ("OPENAI_API_KEY", token),
            ("OPENAI_BASE_URL", base),
            ("POCKYMOE_ENV_CODEX", "1".into()),
        ]),
        "claude" => Ok(vec![
            ("ANTHROPIC_BASE_URL", base),
            ("ANTHROPIC_AUTH_TOKEN", token),
        ]),
        _ => bail!("unsupported_isolated_harness"),
    }
}

#[derive(Clone)]
pub struct Capability {
    pub environment_id: String,
    pub generation: u64,
    pub thread_id: String,
    pub expires: std::time::Instant,
}
fn capabilities() -> &'static std::sync::Mutex<std::collections::HashMap<String, Capability>> {
    static TOKENS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Capability>>,
    > = std::sync::OnceLock::new();
    TOKENS.get_or_init(Default::default)
}
pub fn issue_capability(context: &ExecutionContext) -> String {
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut all = capabilities().lock().unwrap();
    all.retain(|_, c| c.expires > std::time::Instant::now());
    if let Some((token, cap)) = all.iter_mut().find(|(_, c)| {
        c.environment_id == context.environment_id
            && c.generation == context.generation
            && c.thread_id == context.thread_id
    }) {
        cap.expires = std::time::Instant::now() + std::time::Duration::from_secs(3600);
        return token.clone();
    }
    all.insert(
        token.clone(),
        Capability {
            environment_id: context.environment_id.clone(),
            generation: context.generation,
            thread_id: context.thread_id.clone(),
            expires: std::time::Instant::now() + std::time::Duration::from_secs(3600),
        },
    );
    token
}
pub fn validate_capability(token: &str, environment: &str, generation: u64) -> Result<String> {
    let all = capabilities().lock().unwrap();
    let cap = all.get(token).context("invalid_environment_capability")?;
    ensure!(
        cap.expires > std::time::Instant::now(),
        "expired_environment_capability"
    );
    ensure!(
        cap.environment_id == environment && cap.generation == generation,
        "foreign_environment_capability"
    );
    Ok(cap.thread_id.clone())
}
pub fn revoke_capabilities(environment: &str) {
    capabilities()
        .lock()
        .unwrap()
        .retain(|_, c| c.environment_id != environment);
}

pub fn spawn_blocking<T: Send + 'static>(
    action: impl FnOnce() -> T + Send + 'static,
) -> tokio::task::JoinHandle<T> {
    let context = current();
    tokio::task::spawn_blocking(move || blocking_scope(context, action))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context(directory: &Path) -> ExecutionContext {
        ExecutionContext {
            manager: Arc::new(crate::environment::EnvironmentManager::new(
                &directory.join("test.sqlite"),
            )),
            environment_id: uuid::Uuid::new_v4().to_string(),
            generation: 3,
            container_id: None,
            thread_id: uuid::Uuid::new_v4().to_string(),
            upstream_directory: directory.join("upstreams"),
            workspace_path: directory.join("project"),
            working_directory: directory.join("project"),
        }
    }
    #[test]
    fn capability_is_bound_to_thread_environment_and_lifetime() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path());
        let token = issue_capability(&context);
        assert!(
            validate_capability(&token, &context.environment_id, context.generation)
                .is_ok_and(|t| t == context.thread_id)
        );
        assert!(
            validate_capability(&token, &context.environment_id, context.generation + 1).is_err()
        );
        assert!(validate_capability(
            &token,
            &uuid::Uuid::new_v4().to_string(),
            context.generation
        )
        .is_err());
        capabilities()
            .lock()
            .unwrap()
            .get_mut(&token)
            .unwrap()
            .expires = std::time::Instant::now();
        assert!(validate_capability(&token, &context.environment_id, context.generation).is_err());
        let renewed = issue_capability(&context);
        assert!(validate_capability(&renewed, &context.environment_id, context.generation).is_ok());
        revoke_capabilities(&context.environment_id);
        assert!(
            validate_capability(&renewed, &context.environment_id, context.generation).is_err()
        );
    }
    #[tokio::test]
    async fn blocking_work_preserves_the_isolation_requirement_and_restores_scope() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path());
        let id = context.environment_id.clone();
        assert!(current().is_none());
        scope(Some(context), async {
            let (inside, refused) = spawn_blocking(|| {
                let c = current().unwrap();
                (
                    c.environment_id.clone(),
                    file_request("readText", Path::new("/"), "etc/hostname", json!({})).is_err(),
                )
            })
            .await
            .unwrap();
            assert_eq!(inside, id);
            assert!(refused);
        })
        .await;
        assert!(current().is_none());
    }
}
