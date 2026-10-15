use super::{
    releases::{self},
    service, setup, *,
};
use anyhow::{bail, ensure};
use pockymoe_runtime::{RestartStrategy, Supervisor};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod host_managed;
pub(crate) use host_managed::{complete_host_managed_startup, host_managed_pending};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContextData {
    #[serde(default)]
    restart_strategy: RestartStrategy,
    #[serde(default)]
    launch_executable: PathBuf,
    #[serde(default)]
    process_started_at: String,
    running_version: String,
    pid: u32,
    executable: PathBuf,
    database: PathBuf,
    port: u16,
    host: String,
    relay: bool,
    config: PathBuf,
    manager: Option<String>,
    environment: BTreeMap<String, String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    context: ContextData,
    version: String,
    restart: bool,
    directory: PathBuf,
    status_file: PathBuf,
    lock: PathBuf,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn active(job: &Value) -> bool {
    matches!(
        job["phase"].as_str(),
        Some("scheduled" | "preparing" | "installing" | "restarting" | "verifying")
    )
}
fn status_path(context: &ContextData) -> PathBuf {
    context
        .database
        .parent()
        .unwrap_or(Path::new("."))
        .join("updates/supervisor-update.json")
}
fn lock_path(context: &ContextData) -> PathBuf {
    status_path(context).with_file_name("supervisor-update.lock")
}
fn job(context: &ContextData) -> Value {
    let file = status_path(context);
    let mut job = read::<Value>(&file).unwrap_or(Value::Null);
    let scheduled_stale = job["phase"] == "scheduled"
        && now().saturating_sub(job["updatedAt"].as_u64().unwrap_or(0)) > 30_000;
    let stopped = job["workerPid"]
        .as_u64()
        .is_some_and(|p| !service::alive(p as u32));
    if active(&job) && job["restartStrategy"] != "host-managed" && (scheduled_stale || stopped) {
        job["phase"] = json!("failed");
        job["error"] = json!("The independent native update worker stopped unexpectedly. Check its log before retrying.");
        let _ = write(&file, &job);
        let _ = std::fs::remove_dir(lock_path(context));
    }
    job
}
fn context(state: &Supervisor) -> Result<ContextData> {
    Ok(ContextData {
        restart_strategy: state.config.supervisor_restart_strategy,
        launch_executable: std::env::args_os().next().unwrap_or_default().into(),
        process_started_at: state.started_at.clone(),
        running_version: env!("CARGO_PKG_VERSION").into(),
        pid: std::process::id(),
        executable: std::env::current_exe()?,
        database: state.config.database_url.clone(),
        port: state.config.port,
        host: state.config.host.clone(),
        relay: state.config.mode == pockymoe_protocol::Mode::Relay,
        config: config_path(),
        manager: std::env::var("POCKYMOE_MANAGED_SERVICE").ok(),
        environment: std::env::vars().collect(),
    })
}
fn compatible_context() -> Result<ContextData> {
    let value = |key: &str| {
        std::env::var(key).with_context(|| format!("Missing maintenance context {key}"))
    };
    Ok(ContextData {
        restart_strategy: std::env::var("POCKYMOE_SUPERVISOR_RESTART_STRATEGY")
            .unwrap_or_default()
            .parse()?,
        launch_executable: std::env::args_os().next().unwrap_or_default().into(),
        process_started_at: String::new(),
        running_version: value("POCKYMOE_UPDATE_RUNNING_VERSION")?,
        pid: value("POCKYMOE_UPDATE_PID")?.parse()?,
        executable: value("POCKYMOE_UPDATE_EXECUTABLE")?.into(),
        database: value("POCKYMOE_UPDATE_DATABASE")?.into(),
        port: value("POCKYMOE_UPDATE_PORT")?.parse()?,
        host: value("POCKYMOE_UPDATE_HOST")?,
        relay: value("POCKYMOE_UPDATE_MODE")? == "relay",
        config: config_path(),
        manager: std::env::var("POCKYMOE_MANAGED_SERVICE").ok(),
        environment: std::env::vars().collect(),
    })
}
async fn action(context: ContextData, action: &str) -> Result<Value> {
    let current = releases::current().ok();
    let installed = current
        .as_ref()
        .map(|i| i.version.as_str())
        .unwrap_or(&context.running_version);
    let release_binary = current
        .as_ref()
        .is_some_and(|i| i.executable == context.executable)
        || std::env::current_exe()?.starts_with(install_root());
    // The last npm release runs this binary from the retired npm launcher.
    // Its Update moves the device to the native GitHub runtime; Device
    // Manager devices keep their bootstrap and migrate through setup.ps1.
    let npm_launcher = !release_binary && npm_launched(&context.environment);
    let device_manager = context
        .environment
        .contains_key("POCKYMOE_WINDOWS_DEVICE_MANAGER");
    let migrate = npm_launcher && !device_manager;
    let allowed = (release_binary || migrate) && context.database.is_absolute();
    let mut base = json!({"runningVersion":context.running_version,"installedVersion":installed,"canUpdate":allowed,"canRestart":allowed && !migrate,"manager":if migrate {"npm"} else {"github-release"},"nativeMigration":migrate,"path":context.executable,"job":job(&context),"releaseUrl":releases::REPO});
    host_managed::add_restart_status(&context, &mut base);
    if migrate && context.restart_strategy == RestartStrategy::HostManaged {
        base["canUpdate"] = json!(false);
        base["reason"] = json!("Host-managed updates require a native installation and a stable native launch command. Run the GitHub setup command first.");
        return Ok(base);
    }
    if !allowed {
        base["reason"] = json!(if npm_launcher && device_manager {
            "This Windows Device Manager device runs the retired npm package. Run the Windows setup command from the Devices page to move it to the native GitHub runtime."
        } else {
            "This is a source or unmanaged executable. Run the GitHub setup command to install a managed native runtime."
        });
        return Ok(base);
    }
    if migrate {
        base["reason"] = json!("This device still runs from the retired npm package. Update installs the native GitHub runtime and moves its service to it.");
        ensure!(
            action != "restart",
            "Update this device to the native runtime before restarting it"
        );
    }
    if action == "status" {
        return Ok(base);
    }
    let version = if action == "restart" {
        context.running_version.clone()
    } else {
        releases::latest().await?
    };
    base["latestVersion"] = json!(version);
    if action != "restart" && version_parts(&version)? < version_parts(&context.running_version)? {
        base["canUpdate"] = json!(false);
        base["reason"] = json!(
            "The installed runtime is newer than GitHub latest; automatic downgrade is disabled."
        );
        return Ok(base);
    }
    if action == "check" {
        return Ok(base);
    }
    ensure!(
        matches!(action, "launch" | "restart"),
        "Unknown native maintenance action"
    );
    if action != "restart" && !migrate && version == context.running_version && version == installed
    {
        return Ok(base);
    }
    ensure!(!active(&base["job"]), "A native update is already running");
    let status_file = status_path(&context);
    private_dir(status_file.parent().unwrap())?;
    let lock = lock_path(&context);
    std::fs::create_dir(&lock).context("An update is already in progress")?;
    let failed_status = status_file.clone();
    let result = async {
        let directory = status_file
            .parent()
            .unwrap()
            .join(format!("supervisor-{}", uuid::Uuid::new_v4()));
        private_dir(&directory)?;
        let plan = Plan {
            context,
            version: version.clone(),
            restart: action == "restart",
            directory: directory.clone(),
            status_file,
            lock: lock.clone(),
        };
        if plan.context.restart_strategy == RestartStrategy::HostManaged {
            base["job"] = host_managed::prepare_handoff(&plan).await?;
            return Ok(base);
        }
        let worker = directory.join(binary_name());
        std::fs::copy(std::env::current_exe()?, &worker)?;
        executable(&worker)?;
        let plan_file = directory.join("plan.json");
        write(&plan_file, &plan)?;
        let scheduled = json!({"phase":"scheduled","action":if plan.restart {"restart"} else {"update"},"targetVersion":version,"updatedAt":now(),"logPath":directory.join("update.log")});
        write(&plan.status_file, &scheduled)?;
        service::independent(
            &worker,
            &[
                "internal-update".into(),
                "--plan".into(),
                plan_file.to_string_lossy().into_owned(),
            ],
            &BTreeMap::new(),
            &directory.join("update.log"),
        )?;
        base["job"] = scheduled;
        Ok(base)
    }.await;
    if let Err(error) = &result {
        let _ = write(
            &failed_status,
            &json!({"phase":"failed","error":format!("{error:#}"),"updatedAt":now()}),
        );
        let _ = std::fs::remove_dir(&lock);
    }
    result
}
fn npm_launched(environment: &BTreeMap<String, String>) -> bool {
    environment
        .get("POCKYMOE_LAUNCHER_PATH")
        .and_then(|path| Path::new(path).file_name()?.to_str())
        == Some("remote-codex.mjs")
}
pub async fn updater(state: &Supervisor, action_name: &str) -> Result<Value> {
    action(context(state)?, action_name).await
}
pub async fn maintenance_cli(action_name: &str) -> Result<Value> {
    action(compatible_context()?, action_name).await
}
fn set_status(plan: &Plan, phase: &str, extra: Value) -> Result<()> {
    let mut status = json!({"phase":phase,"action":if plan.restart {"restart"} else {"update"},"targetVersion":plan.version,"runningVersion":plan.context.running_version,"workerPid":std::process::id(),"updatedAt":now(),"logPath":plan.directory.join("update.log")});
    status["restartStrategy"] = json!(plan.context.restart_strategy);
    if plan.context.restart_strategy == RestartStrategy::HostManaged {
        status.as_object_mut().unwrap().remove("workerPid");
        status.as_object_mut().unwrap().remove("logPath");
        status["previousProcessId"] = json!(plan.context.pid);
        status["previousProcessStartedAt"] = json!(plan.context.process_started_at);
    }
    for (key, value) in extra.as_object().unwrap() {
        status[key] = value.clone();
    }
    write(&plan.status_file, &status)
}
async fn verify(plan: &Plan) -> Result<Value> {
    for _ in 0..90 {
        if let Some(health) = setup::health(plan.context.port, &plan.context.host).await {
            if health["status"] == "ok"
                && health["runningVersion"] == plan.version
                && health["processId"].as_u64() != Some(plan.context.pid as u64)
                && (!plan.context.relay || health["relayConnected"] == true)
            {
                return Ok(health);
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    bail!("Updated Supervisor did not become healthy and reconnect. Check the service log.")
}
fn version_parts(version: &str) -> Result<(u32, u32, u32)> {
    let normalized = releases::stable_version(version)?;
    let parts = normalized
        .split('.')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok((parts[0], parts[1], parts[2]))
}
struct WorkerCleanup {
    plan: PathBuf,
    lock: PathBuf,
}
impl Drop for WorkerCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.plan);
        let _ = std::fs::remove_dir(&self.lock);
    }
}
pub async fn run_worker(path: PathBuf) -> Result<()> {
    let plan: Plan = read(&path)?;
    ensure!(
        plan.context.restart_strategy == RestartStrategy::ExternalUpdater,
        "Host-managed restarts must not launch an independent worker"
    );
    ensure!(
        path.parent() == Some(plan.directory.as_path()) && plan.context.database.is_absolute(),
        "Invalid update plan"
    );
    let _cleanup = WorkerCleanup {
        plan: path.clone(),
        lock: plan.lock.clone(),
    };
    let previous = releases::current().ok();
    let mut stopped = false;
    let mut activated = false;
    let mut started = false;
    let result = async {
        set_status(&plan, "preparing", json!({}))?;
        let original = setup::health(plan.context.port, &plan.context.host)
            .await
            .context("Original Supervisor is unavailable")?;
        ensure!(
            original["processId"].as_u64() == Some(plan.context.pid as u64),
            "The running Supervisor changed"
        );
        let tmux = service::capture_tmux(plan.context.pid, &plan.context.environment)?;
        let candidate = if plan.restart {
            previous
                .clone()
                .filter(|i| i.version == plan.version)
                .context("Native installation is not available for restart")?
        } else {
            set_status(&plan, "installing", json!({}))?;
            releases::install(&plan.version, None).await?
        };
        set_status(&plan, "restarting", json!({}))?;
        let original = setup::health(plan.context.port, &plan.context.host)
            .await
            .context("Original Supervisor is unavailable")?;
        ensure!(
            original["processId"].as_u64() == Some(plan.context.pid as u64)
                && original["activeTurnCount"].as_u64().unwrap_or(1) == 0,
            "Supervisor changed or a turn started during update preparation"
        );
        if let Some(manager) = &plan.context.manager {
            service::manage(manager, "stop", &plan.context.config)?;
        } else {
            service::stop(plan.context.pid)?;
        }
        stopped = true;
        for _ in 0..60 {
            if !service::alive(plan.context.pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        ensure!(
            !service::alive(plan.context.pid),
            "Original Supervisor did not stop; refusing to replace a running process"
        );
        service::retire_tmux(&tmux)?;
        releases::activate(&candidate)?;
        activated = true;
        started = true;
        if plan.context.relay {
            // A legacy npm/tmux device has no native manager marker yet. Adopt
            // the available persistent manager instead of leaving migration
            // in a transient worker that cannot start the device after reboot.
            let manager = plan.context.manager.clone().or_else(service::detect);
            service::start(&candidate, &plan.context.config, manager.as_deref())?;
        } else {
            let mut env = plan.context.environment.clone();
            env.retain(|key, _| {
                !key.starts_with("POCKYMOE_UPDATE_")
                    && !matches!(
                        key.as_str(),
                        "POCKYMOE_NATIVE_BINARY"
                            | "POCKYMOE_LAUNCHER_PATH"
                            | "POCKYMOE_LAUNCHER_NODE"
                            | "TMUX"
                            | "TMUX_PANE"
                    )
            });
            env.insert(
                "POCKYMOE_WEB_DIST_DIR".into(),
                candidate.web_dist.to_string_lossy().into_owned(),
            );
            service::independent(
                &candidate.executable,
                &["supervisor".into()],
                &env,
                &service::log_path(&plan.context.config),
            )?;
        }
        set_status(&plan, "verifying", json!({"runningVersion":plan.version}))?;
        let health = verify(&plan).await?;
        set_status(
            &plan,
            "completed",
            json!({"runningVersion":plan.version,"processId":health["processId"]}),
        )?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        eprintln!("Native update failed: {error:#}");
        if started {
            // The new executable may already have migrated SQLite. Never start
            // an older binary or restore the old database after this point.
            set_status(
                &plan,
                "failed",
                json!({"keptInstalledVersion":true,"error":format!("{error:#}. Kept the current installation to avoid a database downgrade; check the service log.")}),
            )?;
        } else {
            let rollback = (|| {
                if activated {
                    if let Some(previous) = &previous {
                        releases::activate(previous)?;
                    }
                }
                if stopped && !service::alive(plan.context.pid) {
                    if let Some(manager) = &plan.context.manager {
                        service::manage(manager, "start", &plan.context.config)?;
                    } else if let Some(previous) = &previous {
                        service::start(previous, &plan.context.config, None)?;
                    } else {
                        let mut env = plan.context.environment.clone();
                        env.remove("TMUX");
                        env.remove("TMUX_PANE");
                        service::independent(
                            &plan.context.executable,
                            &[if plan.context.relay {
                                "relay-supervisor"
                            } else {
                                "supervisor"
                            }
                            .into()],
                            &env,
                            &service::log_path(&plan.context.config),
                        )?;
                    }
                }
                Ok::<_, anyhow::Error>(())
            })();
            set_status(
                &plan,
                if rollback.is_err() {
                    "rollback-failed"
                } else if stopped {
                    "rolled-back"
                } else {
                    "failed"
                },
                json!({"error":format!("{error:#}{}", rollback.err().map(|e| format!("; rollback: {e:#}")).unwrap_or_default())}),
            )?;
        }
    }
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir(plan.lock);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_retired_npm_launcher_counts_as_an_npm_install() {
        let env =
            |path: &str| BTreeMap::from([("POCKYMOE_LAUNCHER_PATH".to_string(), path.to_string())]);
        assert!(npm_launched(&env(
            "/usr/lib/node_modules/remote-codex/bin/remote-codex.mjs"
        )));
        assert!(!npm_launched(&env(
            "/home/u/.local/share/remote-codex/native/current/remote-codex"
        )));
        assert!(!npm_launched(&BTreeMap::new()));
    }
    #[test]
    fn native_update_recovers_stale_worker_and_releases_private_lock() {
        let root = tempfile::tempdir().unwrap();
        let context = ContextData {
            restart_strategy: RestartStrategy::ExternalUpdater,
            launch_executable: PathBuf::new(),
            process_started_at: String::new(),
            running_version: "1.0.0".into(),
            pid: std::process::id(),
            executable: root.path().join(binary_name()),
            database: root.path().join("device.sqlite"),
            port: 1,
            host: "127.0.0.1".into(),
            relay: true,
            config: root.path().join("device.json"),
            manager: None,
            environment: BTreeMap::new(),
        };
        let file = status_path(&context);
        private_dir(file.parent().unwrap()).unwrap();
        std::fs::create_dir(lock_path(&context)).unwrap();
        write(&file, &json!({"phase":"scheduled","updatedAt":0})).unwrap();
        let recovered = job(&context);
        assert_eq!(recovered["phase"], "failed");
        assert!(!lock_path(&context).exists());
        write(
            &file,
            &json!({"phase":"verifying","workerPid":std::process::id(),"updatedAt":now()}),
        )
        .unwrap();
        assert_eq!(job(&context)["phase"], "verifying");
    }
    #[tokio::test]
    async fn native_update_fails_before_stopping_an_unavailable_original_supervisor() {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let context = ContextData {
            restart_strategy: RestartStrategy::ExternalUpdater,
            launch_executable: PathBuf::new(),
            process_started_at: String::new(),
            running_version: "1.0.0".into(),
            pid: std::process::id(),
            executable: root.path().join(binary_name()),
            database: root.path().join("device.sqlite"),
            port,
            host: "127.0.0.1".into(),
            relay: true,
            config: root.path().join("device.json"),
            manager: None,
            environment: BTreeMap::new(),
        };
        let directory = root.path().join("worker");
        private_dir(&directory).unwrap();
        let plan = Plan {
            version: "9.1.0".into(),
            restart: false,
            status_file: status_path(&context),
            lock: lock_path(&context),
            context,
            directory: directory.clone(),
        };
        private_dir(plan.status_file.parent().unwrap()).unwrap();
        std::fs::create_dir(&plan.lock).unwrap();
        let file = directory.join("plan.json");
        write(&file, &plan).unwrap();
        run_worker(file.clone()).await.unwrap();
        assert_eq!(read::<Value>(&plan.status_file).unwrap()["phase"], "failed");
        assert!(!file.exists());
        assert!(!plan.lock.exists());
        assert!(service::alive(std::process::id()));
    }
}
