use super::*;
use std::sync::atomic::Ordering;

/// Do not canonicalize argv[0]: current_exe resolves Unix symlinks and therefore
/// reports a release path even when the owner correctly uses native/current.
fn versioned_path(path: &Path) -> bool {
    let text = path.to_string_lossy().replace('\\', "/");
    let parts: Vec<_> = text.split('/').filter(|p| *p != ".").collect();
    parts.windows(3).any(|p| {
        p[0].eq_ignore_ascii_case("native")
            && p[1].eq_ignore_ascii_case("releases")
            && !p[2].is_empty()
    })
}

fn launch_path(context: &ContextData) -> PathBuf {
    let launch = &context.launch_executable;
    if launch.components().count() == 1 {
        if let Some(path) = context.environment.get("PATH") {
            for directory in std::env::split_paths(path) {
                let candidate = directory.join(launch);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }
    launch.clone()
}

pub(super) fn add_restart_status(context: &ContextData, status: &mut Value) {
    status["restartStrategy"] = json!(context.restart_strategy);
    if context.restart_strategy == RestartStrategy::HostManaged {
        let launch = launch_path(context);
        status["restartExitCode"] = json!(pockymoe_runtime::HOST_MANAGED_RESTART_EXIT_CODE);
        status["launchPath"] = json!(launch);
        if versioned_path(&launch)
            || (launch.as_os_str().is_empty() && versioned_path(&context.executable))
        {
            status["restartDiagnostic"] = json!("The Supervisor launch command uses a versioned native/releases path. Configure the owning process manager to relaunch from ~/.local/share/remote-codex/native/current/remote-codex or ~/.local/bin/pockymoe. On Windows use a stable launcher that resolves native/current.json. A fixed release path will relaunch the old version after an update.");
            status["restartDiagnosticCode"] = json!("versioned-launch-path");
        } else if versioned_path(&context.executable) {
            status["restartDiagnostic"] = json!("The running executable resolves to a versioned native/releases path. Keep the owning process manager launch command on native/current/remote-codex or ~/.local/bin/pockymoe (Windows: a stable launcher resolving native/current.json), so the next launch follows the installed version.");
            status["restartDiagnosticCode"] = json!("resolved-release-path");
        }
    }
}

/// Called only after prepare_update_restart has durably journaled paused turns
/// and drained their history writes. No independent worker or service manager is
/// involved. The caller retains its maintenance guard until process exit.
pub(super) async fn prepare_handoff(plan: &Plan) -> Result<Value> {
    let _cleanup = WorkerCleanup {
        plan: plan.directory.join("plan.json"),
        lock: plan.lock.clone(),
    };
    set_status(plan, "preparing", json!({}))?;
    let previous = releases::current().ok();
    let candidate = if plan.restart {
        previous
            .clone()
            .filter(|i| i.version == plan.version)
            .context("Native installation is not available for restart")?
    } else {
        set_status(plan, "installing", json!({}))?;
        releases::install(&plan.version, None).await?
    };
    // Persist the startup-verification intent before switching current. A crash
    // after activation is recoverable even if the explicit exit is not reached.
    persist_handoff(plan)?;
    if !plan.restart {
        if let Err(error) = releases::activate(&candidate) {
            if let Some(previous) = previous {
                releases::activate(&previous).context("Restore installation before exit")?;
            }
            return Err(error);
        }
    }
    Ok(read(&plan.status_file)?)
}

fn persist_handoff(plan: &Plan) -> Result<()> {
    set_status(
        plan,
        "restarting",
        json!({
            "restartExitCode": pockymoe_runtime::HOST_MANAGED_RESTART_EXIT_CODE,
            "verificationOwner": "next-startup"
        }),
    )
}

pub(crate) fn host_managed_pending(state: &Supervisor) -> bool {
    context(state).ok().is_some_and(|context| {
        let status = job(&context);
        pending(&status)
    })
}

fn pending(status: &Value) -> bool {
    status["restartStrategy"] == "host-managed"
        && (active(status) || status["recoveryDeferred"] == true)
}

/// The listener is already bound; relay readiness is established by its normal
/// startup path. Return false on failure so paused-turn markers remain intact.
pub(crate) async fn complete_host_managed_startup(state: &Supervisor) -> Result<bool> {
    let context = context(state)?;
    let status = job(&context);
    if !pending(&status) {
        return Ok(true);
    }
    let mut status = status;
    // Container relaunches may reuse a PID; include the process start identity.
    if status["previousProcessId"].as_u64() == Some(context.pid as u64)
        && status["previousProcessStartedAt"] == context.process_started_at
    {
        bail!("Host-managed restart is still owned by the preparing process");
    }
    status["phase"] = json!("verifying");
    status["updatedAt"] = json!(now());
    write(&status_path(&context), &status)?;
    for _ in 0..90 {
        let relay_ready = !context.relay || state.relay_connected.load(Ordering::SeqCst);
        if relay_ready || status["targetVersion"] != context.running_version {
            return finish_startup(&context, status, relay_ready);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    finish_startup(&context, status, false)
}

fn finish_startup(context: &ContextData, mut status: Value, relay_ready: bool) -> Result<bool> {
    let version_matches = status["targetVersion"] == context.running_version;
    let success = version_matches && relay_ready;
    status["phase"] = json!(if success { "completed" } else { "failed" });
    status["updatedAt"] = json!(now());
    status["runningVersion"] = json!(context.running_version);
    status["processId"] = json!(context.pid);
    if success {
        status.as_object_mut().unwrap().remove("recoveryDeferred");
        status.as_object_mut().unwrap().remove("error");
        status
            .as_object_mut()
            .unwrap()
            .remove("keptInstalledVersion");
    }
    if !success {
        status["keptInstalledVersion"] = json!(true);
        status["recoveryDeferred"] = json!(true);
        status["error"] = json!(if !version_matches {
            "The owning process manager relaunched a different runtime version. Use a stable launch path that follows native/current (Windows: native/current.json). The installation and paused-task journal were retained; fix the launch command and relaunch the installed runtime."
        } else {
            "The restarted Supervisor did not reconnect to the relay. The installation and paused-task journal were retained; restore connectivity and explicitly restart to retry verification."
        });
    }
    write(&status_path(context), &status)?;
    let _ = std::fs::remove_dir(lock_path(context));
    Ok(success)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path) -> ContextData {
        ContextData {
            restart_strategy: RestartStrategy::HostManaged,
            launch_executable: root.join("native/current/remote-codex"),
            process_started_at: "new-start".into(),
            running_version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            executable: root.join("native/releases/0.13.0/remote-codex"),
            database: root.join("device.sqlite"),
            port: 1,
            host: "127.0.0.1".into(),
            relay: false,
            device: true,
            config: root.join("device.json"),
            manager: Some("must-not-be-called".into()),
            environment: BTreeMap::new(),
        }
    }

    fn state(root: &Path) -> Supervisor {
        let mut config = pockymoe_runtime::RuntimeConfig::from_env().unwrap();
        config.supervisor_restart_strategy = RestartStrategy::HostManaged;
        config.database_url = root.join("device.sqlite");
        config.workspace_root = root.join("workspaces");
        config.mode = pockymoe_protocol::Mode::Local;
        config.auth_required = false;
        config.enabled_providers = vec![pockymoe_protocol::Provider::Codex];
        let db = pockymoe_runtime::Database::open(&config.database_url).unwrap();
        Supervisor::new(
            config,
            db,
            vec![std::sync::Arc::new(
                pockymoe_runtime::fake::FakeRuntime::new(pockymoe_protocol::Provider::Codex),
            )],
        )
    }

    #[test]
    fn host_managed_versioned_path_diagnostic_preserves_stable_symlink_launch() {
        let root = tempfile::tempdir().unwrap();
        let mut context = fixture(root.path());
        let mut status = json!({});
        add_restart_status(&context, &mut status);
        assert_eq!(status["restartStrategy"], "host-managed");
        assert_eq!(status["restartDiagnosticCode"], "resolved-release-path");
        context.launch_executable = context.executable.clone();
        add_restart_status(&context, &mut status);
        assert_eq!(status["restartDiagnosticCode"], "versioned-launch-path");
        assert!(status["restartDiagnostic"]
            .as_str()
            .unwrap()
            .contains("stable"));
        assert!(versioned_path(Path::new(
            r"C:\Users\u\native\releases\0.13.0\remote-codex.exe"
        )));
        assert!(!versioned_path(Path::new(
            r"C:\Users\u\native\current\remote-codex.exe"
        )));
        context.restart_strategy = RestartStrategy::ExternalUpdater;
        status = json!({});
        add_restart_status(&context, &mut status);
        assert!(status.get("restartDiagnostic").is_none());
    }

    #[tokio::test]
    async fn host_managed_next_start_completes_even_without_paused_turns_and_with_reused_pid() {
        let root = tempfile::tempdir().unwrap();
        let state = std::sync::Arc::new(state(root.path()));
        let context = fixture(root.path());
        let plan = Plan {
            version: context.running_version.clone(),
            restart: true,
            status_file: status_path(&context),
            lock: lock_path(&context),
            directory: root.path().join("worker"),
            context,
        };
        persist_handoff(&plan).unwrap();
        assert!(!state.defer_update_recovery());
        assert!(host_managed_pending(&state));
        // A handoff has no worker to mark stale, even after arbitrary downtime.
        let mut status = read::<Value>(&plan.status_file).unwrap();
        status["updatedAt"] = json!(0);
        write(&plan.status_file, &status).unwrap();
        assert_eq!(job(&plan.context)["phase"], "restarting");
        assert_eq!(status["restartExitCode"], 0);
        crate::management::recover_after_update(state.clone()).await;
        let axum::Json(status) =
            crate::management::supervisor_status(axum::extract::State(state.clone())).await;
        assert_eq!(status["job"]["phase"], "completed");
        assert_eq!(status["job"]["processId"], std::process::id());
        assert!(!host_managed_pending(&state));
        assert!(complete_host_managed_startup(&state).await.unwrap());
    }

    #[tokio::test]
    async fn host_managed_wrong_version_defers_recovery_across_startups_until_corrected() {
        let root = tempfile::tempdir().unwrap();
        let state = std::sync::Arc::new(state(root.path()));
        let workspace = state
            .create_workspace(pockymoe_protocol::CreateWorkspaceInput {
                abs_path: Some(root.path().to_string_lossy().into_owned()),
                git_url: None,
                label: None,
            })
            .unwrap();
        let thread = state
            .create_thread(pockymoe_protocol::CreateThreadInput {
                workspace_id: workspace.id,
                title: None,
                provider: Some(pockymoe_protocol::Provider::Codex),
                agent_id: None,
                model: "fixture".into(),
                reasoning_effort: None,
                approval_mode: "yolo".into(),
                parent_thread_id: None,
            })
            .await
            .unwrap();
        state.db.with(|conn| {
            conn.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES ('paused-turn',?1,'interrupted',1)", [&thread.id])?;
            conn.execute("UPDATE threads SET status='interrupted' WHERE id=?1", [&thread.id])?;
            conn.execute("INSERT INTO thread_pending_steers(id,thread_id,turn_id,display_prompt,submitted_prompt,delivery,created_at,updated_at) VALUES ('resume',?1,'paused-turn','resume','resume','update-resume','now','now')", [&thread.id])?;
            Ok(())
        }).unwrap();
        let context = fixture(root.path());
        let status = json!({"restartStrategy":"host-managed","phase":"restarting", "targetVersion":"9.1.0", "previousProcessStartedAt":"old"});
        write(&status_path(&context), &status).unwrap();
        crate::management::recover_after_update(state.clone()).await;
        let status = job(&context);
        assert_eq!(status["phase"], "failed");
        assert_eq!(status["recoveryDeferred"], true);
        assert!(host_managed_pending(&state));
        // A second startup must not resume just because the job already failed.
        crate::management::recover_after_update(state.clone()).await;
        let remaining: i64 = state
            .db
            .with(|conn| {
                Ok(conn.query_row(
                    "SELECT count(*) FROM thread_pending_steers WHERE delivery='update-resume'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(
            remaining, 1,
            "Failed verification never consumes the paused-turn journal"
        );
        let mut corrected = context.clone();
        corrected.running_version = "9.1.0".into();
        assert!(finish_startup(&corrected, job(&context), true).unwrap());
        assert_eq!(job(&context)["phase"], "completed");
        assert!(!host_managed_pending(&state));
    }

    #[test]
    fn host_managed_relay_failure_is_visible_and_keeps_recovery_deferred() {
        let root = tempfile::tempdir().unwrap();
        let context = fixture(root.path());
        let status = json!({"restartStrategy":"host-managed","phase":"verifying", "targetVersion":context.running_version});
        assert!(!finish_startup(&context, status, false).unwrap());
        let status = job(&context);
        assert_eq!(status["phase"], "failed");
        assert!(pending(&status));
        assert!(status["error"].as_str().unwrap().contains("relay"));
    }

    // An isolated subprocess exercises the real HTTP orchestration and exit.
    // Its HOME and database are fixtures; the running device is never touched.
    #[test]
    fn host_managed_plain_restart_exits_after_durable_handoff_without_spawning_worker() {
        let root = tempfile::tempdir().unwrap();
        let output = run_child(root.path(), "restart");
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let context = fixture(root.path());
        let status = job(&context);
        assert_eq!(status["phase"], "restarting");
        assert_eq!(status["action"], "restart");
        assert_eq!(status["verificationOwner"], "next-startup");
        assert_eq!(status["restartExitCode"], 0);
        assert!(status.get("workerPid").is_none());
        assert!(!lock_path(&context).exists());
        for entry in std::fs::read_dir(status_path(&context).parent().unwrap()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                assert_eq!(
                    std::fs::read_dir(path).unwrap().count(),
                    0,
                    "No copied worker, plan or launch script"
                );
            }
        }
    }

    fn run_child(root: &Path, action: &str) -> std::process::Output {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "distribution::update::host_managed::tests::host_managed_restart_child",
                "--nocapture",
            ])
            .env("POCKYMOE_TEST_HOST_MANAGED_CHILD", action)
            .env("HOME", root)
            .env("USERPROFILE", root)
            .env("POCKYMOE_MODE", "local")
            .env("POCKYMOE_DATABASE_PATH", root.join("device.sqlite"))
            .env("POCKYMOE_SUPERVISOR_RESTART_STRATEGY", "host-managed")
            .output()
            .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn host_managed_update_verifies_staged_release_switches_current_and_exits() {
        let root = tempfile::tempdir().unwrap();
        let output = run_child(root.path(), "update");
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let context = fixture(root.path());
        let status = job(&context);
        assert_eq!(status["phase"], "restarting");
        assert_eq!(status["action"], "update");
        assert_eq!(status["targetVersion"], "9.1.0");
        assert!(!lock_path(&context).exists());
        let install = root.path().join(".local/share/remote-codex/native");
        assert_eq!(
            read::<releases::Installed>(&install.join("current.json"))
                .unwrap()
                .version,
            "9.1.0"
        );
        assert_eq!(
            std::fs::read_link(install.join("current")).unwrap(),
            install.join("releases/9.1.0")
        );
    }

    #[tokio::test]
    async fn host_managed_restart_child() {
        let Ok(action) = std::env::var("POCKYMOE_TEST_HOST_MANAGED_CHILD") else {
            return;
        };
        let root = home();
        assert_eq!(
            pockymoe_runtime::RuntimeConfig::from_env()
                .unwrap()
                .supervisor_restart_strategy,
            RestartStrategy::HostManaged
        );
        let installed = releases::Installed {
            version: env!("CARGO_PKG_VERSION").into(),
            executable: std::env::current_exe().unwrap(),
            web_dist: root.join("web"),
        };
        write(&install_root().join("current.json"), &installed).unwrap();
        let mut state = state(&root);
        #[cfg(unix)]
        if action == "update" {
            let target = install_root().join("releases/9.1.0");
            let candidate = releases::Installed {
                version: "9.1.0".into(),
                executable: target.join(binary_name()),
                web_dist: target.join("web"),
            };
            private_dir(&candidate.web_dist).unwrap();
            std::fs::write(candidate.web_dist.join("index.html"), "fixture").unwrap();
            std::fs::write(&candidate.executable, "#!/bin/sh\nprintf '9.1.0\\n'\n").unwrap();
            executable(&candidate.executable).unwrap();
            write(&target.join("installed.json"), &candidate).unwrap();
            let context = context(&state).unwrap();
            let plan = Plan {
                version: "9.1.0".into(),
                restart: false,
                status_file: status_path(&context),
                lock: lock_path(&context),
                directory: root.join("worker"),
                context,
            };
            private_dir(plan.status_file.parent().unwrap()).unwrap();
            std::fs::create_dir(&plan.lock).unwrap();
            let _guard = state.prepare_update_restart().await.unwrap();
            let status = prepare_handoff(&plan).await.unwrap();
            assert_eq!(status["phase"], "restarting");
            assert_eq!(releases::current().unwrap().version, "9.1.0");
            std::process::exit(pockymoe_runtime::HOST_MANAGED_RESTART_EXIT_CODE);
        }
        state.config.port = 0;
        state.config.host = "127.0.0.1".into();
        assert_eq!(
            state.config.supervisor_restart_strategy,
            RestartStrategy::HostManaged
        );
        let state = std::sync::Arc::new(state);
        let serving = state.clone();
        tokio::spawn(async move {
            crate::serve(serving).await.unwrap();
        });
        tokio::time::timeout(Duration::from_secs(10), async {
            let cli = loop {
                if let Ok(cli) =
                    read::<Value>(&state.config.database_url.with_extension("cli.json"))
                {
                    break cli;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            let response = reqwest::Client::new()
                .post(format!(
                    "{}/api/management/supervisor/restart",
                    cli["url"].as_str().unwrap()
                ))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
            let accepted: Value = response.json().await.unwrap();
            assert_eq!(accepted["job"]["phase"], "preparing");
            // Production orchestration should terminate this subprocess.
            std::future::pending::<()>().await;
        })
        .await
        .expect("Host-managed orchestration did not exit");
    }
}
