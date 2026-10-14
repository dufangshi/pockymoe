use super::{releases::Installed, *};
use anyhow::{bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    process::{Command, Stdio},
};

pub fn command(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
fn uid() -> Result<String> {
    let out = Command::new("id").arg("-u").output()?;
    ensure!(out.status.success(), "Cannot determine service user");
    Ok(String::from_utf8(out.stdout)?.trim().into())
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
/// Environment= values expand specifiers (%) but not variables ($).
fn environment_value(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('\n', "\\n")
    )
}
fn unit(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}
pub fn definition(manager: &str, installed: &Installed, config: &Path) -> String {
    let args = [
        installed.executable.to_string_lossy().into_owned(),
        "device-run".into(),
        "--config".into(),
        config.to_string_lossy().into_owned(),
    ];
    // The PATH the Supervisor runs with, already merged with the user's shell
    // PATH, so agent CLIs stay discoverable under the service manager.
    let path = std::env::var("PATH").unwrap_or_default();
    if manager == "launchd" {
        format!("<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>Label</key><string>com.remote-codex.supervisor</string><key>ProgramArguments</key><array>{}</array><key>EnvironmentVariables</key><dict><key>PATH</key><string>{}</string><key>POCKYMOE_MANAGED_SERVICE</key><string>launchd</string></dict><key>WorkingDirectory</key><string>{}</string><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>StandardOutPath</key><string>{}</string><key>StandardErrorPath</key><string>{}</string></dict></plist>", args.iter().map(|a| format!("<string>{}</string>", xml(a))).collect::<String>(), xml(&path), xml(&home().to_string_lossy()), xml(&log_path().to_string_lossy()), xml(&log_path().to_string_lossy()))
    } else {
        // Interactive shells from Web terminals ignore SIGTERM; without a short
        // stop timeout systemd waits 90s before killing them, and the device
        // stays offline for the whole update restart.
        format!("[Unit]\nDescription=Pockymoe Supervisor\nAfter=network-online.target\n[Service]\nType=simple\nExecStart={}\nWorkingDirectory=%h\nEnvironment={}\nEnvironment=POCKYMOE_MANAGED_SERVICE=systemd-user\nRestart=always\nRestartSec=5\nTimeoutStopSec=15\nUnsetEnvironment=POCKYMOE_ENROLL_SECRET POCKYMOE_ENROLL\n[Install]\nWantedBy=default.target\n", args.iter().map(|a| unit(a)).collect::<Vec<_>>().join(" "), environment_value(&format!("PATH={path}")))
    }
}
pub fn log_path() -> PathBuf {
    home().join(".remote-codex/logs/relay-supervisor.log")
}
pub fn detect() -> Option<String> {
    if cfg!(target_os = "macos") {
        Some("launchd".into())
    } else if cfg!(target_os = "linux") && command("systemctl", &["--user", "show-environment"]) {
        Some("systemd-user".into())
    } else {
        None
    }
}
fn service_path(manager: &str) -> PathBuf {
    if manager == "launchd" {
        home().join("Library/LaunchAgents/com.remote-codex.supervisor.plist")
    } else {
        home().join(".config/systemd/user/remote-codex-supervisor.service")
    }
}
pub fn install(manager: &str, installed: &Installed, config: &Path) -> Result<()> {
    let file = service_path(manager);
    private_dir(file.parent().unwrap())?;
    std::fs::write(&file, definition(manager, installed, config))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
    }
    private_dir(log_path().parent().unwrap())?;
    if manager == "systemd-user" {
        ensure!(
            command("systemctl", &["--user", "daemon-reload"]),
            "Unable to reload systemd user service"
        );
    }
    Ok(())
}
/// Updates are run by the previous runtime's worker, which writes its own unit
/// definition. Bring a systemd user unit up to this runtime's definition at
/// startup so changes such as the stop timeout apply from the next stop
/// instead of one update later.
pub fn refresh(manager: &str, installed: &Installed, config: &Path) -> Result<bool> {
    if manager != "systemd-user" {
        return Ok(false);
    }
    let changed = rewrite_if_changed(
        &service_path(manager),
        &definition(manager, installed, config),
    )?;
    if changed {
        ensure!(
            command("systemctl", &["--user", "daemon-reload"]),
            "Unable to reload systemd user service"
        );
    }
    Ok(changed)
}
fn rewrite_if_changed(file: &Path, wanted: &str) -> Result<bool> {
    if !file.is_file() || std::fs::read_to_string(file)? == wanted {
        return Ok(false);
    }
    write_bytes(file, wanted.as_bytes())?;
    Ok(true)
}
pub fn manage(manager: &str, action: &str) -> Result<()> {
    let ok = match manager {
        "systemd-user" => {
            if action == "start" {
                command(
                    "systemctl",
                    &[
                        "--user",
                        "enable",
                        "--now",
                        "remote-codex-supervisor.service",
                    ],
                )
            } else {
                command(
                    "systemctl",
                    &[
                        "--user",
                        "stop",
                        "--no-block",
                        "remote-codex-supervisor.service",
                    ],
                )
            }
        }
        "launchd" => {
            let domain = format!("gui/{}", uid()?);
            if action == "stop" {
                command(
                    "/bin/launchctl",
                    &["bootout", &format!("{domain}/com.remote-codex.supervisor")],
                )
            } else {
                command(
                    "/bin/launchctl",
                    &[
                        "bootstrap",
                        &domain,
                        &service_path(manager).to_string_lossy(),
                    ],
                )
            }
        }
        _ => bail!("Unknown service manager"),
    };
    ensure!(ok, "Unable to {action} managed Supervisor service");
    Ok(())
}
pub fn alive(pid: u32) -> bool {
    if cfg!(windows) {
        Command::new("tasklist.exe")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
    } else {
        command("kill", &["-0", &pid.to_string()])
    }
}
pub fn stop(pid: u32) -> Result<()> {
    let ok = if cfg!(windows) {
        command("taskkill.exe", &["/PID", &pid.to_string(), "/F"])
    } else {
        command("kill", &["-TERM", &pid.to_string()])
    };
    ensure!(ok || !alive(pid), "Unable to stop the original Supervisor");
    Ok(())
}
fn worker_environment(
    mut inherited: BTreeMap<String, String>,
    environment: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    inherited.extend(environment.iter().map(|(k, v)| (k.clone(), v.clone())));
    inherited.remove("POCKYMOE_ENROLL_SECRET");
    inherited.remove("POCKYMOE_ENROLL");
    inherited
}
// Windows PowerShell 5.1 treats redirected native stderr as a PowerShell
// error. Keep setup failures strict, but do not terminate the native worker on
// ordinary tracing output; its exit code remains authoritative.
fn windows_launcher_source(payload: &Path) -> String {
    let payload = payload.to_string_lossy().replace('\'', "''");
    format!("$ErrorActionPreference='Stop'\n$p=Get-Content -LiteralPath '{payload}' -Raw | ConvertFrom-Json\nRemove-Item -LiteralPath '{payload}', $PSCommandPath -Force\nRemove-Item Env:POCKYMOE_ENROLL_SECRET -ErrorAction SilentlyContinue\nforeach($v in $p.environment.PSObject.Properties){{[Environment]::SetEnvironmentVariable($v.Name,[string]$v.Value,'Process')}}\n$a=@($p.args)\n$ErrorActionPreference='Continue'\n& $p.binary @a *> $p.log\nexit $LASTEXITCODE\n")
}
/// Detach from the Supervisor's cgroup/job before it can be stopped.
pub fn independent(
    binary: &Path,
    args: &[String],
    environment: &BTreeMap<String, String>,
    log: &Path,
) -> Result<()> {
    private_dir(log.parent().unwrap())?;
    let inherited = worker_environment(std::env::vars().collect(), environment);
    let environment = &inherited;
    if cfg!(target_os = "linux") && command("systemctl", &["--user", "show-environment"]) {
        let mut command = Command::new("systemd-run");
        command.args([
            "--user",
            "--collect",
            "--property=Type=exec",
            "--property=KillMode=process",
            "--property=UnsetEnvironment=POCKYMOE_ENROLL_SECRET POCKYMOE_ENROLL",
        ]);
        // Pass secrets through a private file rather than exposing --setenv
        // values in process arguments. The user manager has a separate env.
        let env_file = log.with_extension(format!("{}.env", uuid::Uuid::new_v4()));
        let contents = environment
            .iter()
            .map(|(key, value)| {
                format!(
                    "{key}=\"{}\"\n",
                    value.replace('\\', "\\\\").replace('\"', "\\\"")
                )
            })
            .collect::<String>();
        write_bytes(&env_file, contents.as_bytes())?;
        command.arg(format!(
            "--property=EnvironmentFile={}",
            env_file.to_string_lossy().replace('%', "%%")
        ));
        command
            .arg(format!(
                "--property=StandardOutput={}",
                format!("append:{}", log.display()).replace('%', "%%")
            ))
            .arg("--property=StandardError=inherit");
        command
            .arg("--")
            .arg(binary)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = command.status();
        let _ = std::fs::remove_file(env_file);
        let status = status?;
        ensure!(
            status.success(),
            "Unable to start independent systemd worker"
        );
    } else if cfg!(target_os = "macos") {
        let label = format!("com.remotecodex.update.{}", uuid::Uuid::new_v4());
        let plist = log.with_extension("plist");
        let mut argv = vec![binary.to_string_lossy().into_owned()];
        argv.extend_from_slice(args);
        let data = format!("<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array>{}</array><key>EnvironmentVariables</key><dict>{}</dict><key>RunAtLoad</key><true/><key>AbandonProcessGroup</key><true/><key>StandardOutPath</key><string>{}</string><key>StandardErrorPath</key><string>{}</string></dict></plist>", argv.iter().map(|s| format!("<string>{}</string>", xml(s))).collect::<String>(), environment.iter().map(|(k,v)| format!("<key>{}</key><string>{}</string>", xml(k), xml(v))).collect::<String>(), xml(&log.to_string_lossy()), xml(&log.to_string_lossy()));
        std::fs::write(&plist, data)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&plist, std::fs::Permissions::from_mode(0o600))?;
        }
        ensure!(
            command(
                "/bin/launchctl",
                &[
                    "bootstrap",
                    &format!("gui/{}", uid()?),
                    &plist.to_string_lossy()
                ]
            ),
            "Unable to start independent launchd worker"
        );
    } else if cfg!(windows) {
        // WMI detaches the process from a Supervisor/Manager job. A protected
        // launcher carries the environment without putting credentials in argv.
        let id = uuid::Uuid::new_v4();
        let payload = log.with_extension(format!("{id}.launch.json"));
        let launcher = log.with_extension(format!("{id}.launch.ps1"));
        write(
            &payload,
            &serde_json::json!({"binary":binary,"args":args,"environment":environment,"log":log}),
        )?;
        let source = windows_launcher_source(&payload);
        write_bytes(&launcher, source.as_bytes())?;
        let line = format!("powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{}\"", launcher.display());
        let script = format!("$r=Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{{CommandLine='{}'}}; exit $r.ReturnValue", line.replace('\'', "''"));
        let started = command(
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command", &script],
        );
        if !started {
            let _ = std::fs::remove_file(payload);
            let _ = std::fs::remove_file(launcher);
        }
        ensure!(started, "Unable to start independent Windows worker");
    } else {
        let out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)?;
        let status = Command::new("setsid")
            .arg("--fork")
            .arg(binary)
            .args(args)
            .env_clear()
            .envs(environment)
            .stdin(Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(out)
            .status()?;
        ensure!(status.success(), "Unable to detach native worker");
    }
    Ok(())
}
pub fn start(installed: &Installed, config: &Path, manager: Option<&str>) -> Result<()> {
    if let Some(manager) = manager {
        install(manager, installed, config)?;
        manage(manager, "start")
    } else {
        independent(
            &installed.executable,
            &[
                "device-run".into(),
                "--config".into(),
                config.to_string_lossy().into_owned(),
            ],
            &BTreeMap::new(),
            &log_path(),
        )
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct TmuxOwner {
    socket: String,
    session: String,
    pane: String,
    pane_pid: u32,
}
fn tmux(socket: &str, args: &[&str]) -> Result<String> {
    let out = Command::new("tmux")
        .args(["-S", socket])
        .args(args)
        .output()?;
    ensure!(out.status.success(), "Unable to verify owned tmux session");
    Ok(String::from_utf8(out.stdout)?.trim().into())
}
pub fn capture_tmux(pid: u32, env: &BTreeMap<String, String>) -> Result<Option<TmuxOwner>> {
    let (Some(raw), Some(pane)) = (env.get("TMUX"), env.get("TMUX_PANE")) else {
        return Ok(None);
    };
    let socket = raw.rsplitn(3, ',').last().context("Invalid tmux socket")?;
    let info = tmux(
        socket,
        &[
            "display-message",
            "-p",
            "-t",
            pane,
            "#{session_id}\t#{session_name}\t#{pane_id}\t#{pane_pid}",
        ],
    )?;
    let values: Vec<_> = info.split('\t').collect();
    ensure!(values.len() == 4, "Invalid tmux ownership response");
    if values[1]
        != env
            .get("POCKYMOE_RELAY_SUPERVISOR_TMUX_SESSION")
            .map(String::as_str)
            .unwrap_or("remote-codex-relay-supervisor")
    {
        return Ok(None);
    }
    let pane_pid = values[3].parse::<u32>()?;
    let mut parent = pid;
    for _ in 0..64 {
        if parent == pane_pid || parent <= 1 {
            break;
        }
        let out = Command::new("ps")
            .args(["-p", &parent.to_string(), "-o", "ppid="])
            .output()?;
        parent = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
    }
    ensure!(
        parent == pane_pid
            && tmux(
                socket,
                &["list-panes", "-s", "-t", values[0], "-F", "#{pane_id}"]
            )? == pane.as_str(),
        "Supervisor tmux session contains unrelated processes/panes"
    );
    Ok(Some(TmuxOwner {
        socket: socket.into(),
        session: values[0].into(),
        pane: pane.clone(),
        pane_pid,
    }))
}
pub fn retire_tmux(owner: &Option<TmuxOwner>) -> Result<()> {
    let Some(o) = owner else {
        return Ok(());
    };
    if !command("tmux", &["-S", &o.socket, "has-session", "-t", &o.session]) {
        return Ok(());
    }
    ensure!(
        tmux(
            &o.socket,
            &[
                "list-panes",
                "-s",
                "-t",
                &o.session,
                "-F",
                "#{pane_id}\t#{pane_pid}"
            ]
        )? == format!("{}\t{}", o.pane, o.pane_pid),
        "tmux session ownership changed during update"
    );
    ensure!(
        command("tmux", &["-S", &o.socket, "kill-session", "-t", &o.session]),
        "Unable to retire old tmux session"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detached_worker_does_not_inherit_enrollment_from_parent_or_overrides() {
        let inherited = BTreeMap::from([
            ("POCKYMOE_ENROLL_SECRET".into(), "synthetic-secret".into()),
            ("SAFE".into(), "retained".into()),
        ]);
        let environment = worker_environment(
            inherited,
            &BTreeMap::from([(
                "POCKYMOE_ENROLL_SECRET".into(),
                "synthetic-other-secret".into(),
            )]),
        );
        assert!(!environment.contains_key("POCKYMOE_ENROLL_SECRET"));
        #[cfg(unix)]
        assert!(Command::new("/bin/sh")
            .args([
                "-c",
                "test -z \"$POCKYMOE_ENROLL_SECRET\" && test \"$SAFE\" = retained"
            ])
            .env_clear()
            .envs(environment)
            .status()
            .unwrap()
            .success());
    }
    #[test]
    fn windows_launch_uses_native_exit_code_after_stderr() {
        let source = windows_launcher_source(Path::new("C:/User's home/payload.json"));
        assert!(source.starts_with("$ErrorActionPreference='Stop'\n"));
        assert!(source.contains("User''s home"));
        assert!(source.contains(
            "$ErrorActionPreference='Continue'\n& $p.binary @a *> $p.log\nexit $LASTEXITCODE"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_native_stderr_survives() {
        use std::time::{Duration, Instant};
        const MARKER: &str = "POCKYMOE_WINDOWS_LAUNCH_TEST_MARKER";
        if let Ok(marker) = std::env::var(MARKER) {
            eprintln!("INFO ordinary native stderr must not terminate this worker");
            std::thread::sleep(Duration::from_millis(500));
            std::fs::write(marker, std::process::id().to_string()).unwrap();
            std::thread::sleep(Duration::from_secs(60));
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("space folder");
        std::fs::create_dir(&root).unwrap();
        let marker = root.join("started");
        independent(
            &std::env::current_exe().unwrap(),
            &[
                "--exact".into(),
                "distribution::service::tests::windows_native_stderr_survives".into(),
                "--nocapture".into(),
            ],
            &BTreeMap::from([(MARKER.into(), marker.to_string_lossy().into_owned())]),
            &root.join("worker.log"),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            marker.exists(),
            "normal native stderr terminated the detached worker"
        );
        let pid = std::fs::read_to_string(&marker).unwrap().parse().unwrap();
        stop(pid).unwrap();
        assert!(!alive(pid), "headless native worker was not terminated");
        std::thread::sleep(Duration::from_millis(500));
        let bytes = std::fs::read(root.join("worker.log")).unwrap();
        let output = if bytes.starts_with(&[0xff, 0xfe]) {
            String::from_utf16(
                &bytes[2..]
                    .chunks_exact(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect::<Vec<_>>(),
            )
            .unwrap()
        } else {
            String::from_utf8(bytes).unwrap()
        };
        assert!(output.contains("ordinary native stderr"));
    }
    #[test]
    fn unit_refresh_rewrites_only_a_changed_existing_unit() {
        let dir = tempfile::tempdir().unwrap();
        let unit = dir.path().join("remote-codex-supervisor.service");
        assert!(!rewrite_if_changed(&unit, "new").unwrap());
        assert!(!unit.exists());
        std::fs::write(&unit, "old").unwrap();
        assert!(rewrite_if_changed(&unit, "new").unwrap());
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), "new");
        assert!(!rewrite_if_changed(&unit, "new").unwrap());
    }
    #[test]
    fn native_service_quotes_paths_and_omits_device_credentials() {
        let installed = Installed {
            version: "1.2.3".into(),
            executable: "/User $Name/100%/a\"b".into(),
            web_dist: "/web".into(),
        };
        let linux = definition(
            "systemd-user",
            &installed,
            Path::new("/User $Name/cfg.json"),
        );
        assert!(linux.contains("WorkingDirectory=%h\n"));
        assert!(linux.contains("$$Name/100%%"));
        assert!(!linux.contains("--token"));
        assert!(linux.contains("device-run"));
        assert!(linux.contains(
            "TimeoutStopSec=15\nUnsetEnvironment=POCKYMOE_ENROLL_SECRET POCKYMOE_ENROLL\n"
        ));
        assert!(linux.contains("\nEnvironment=\"PATH="));
        let mac = definition("launchd", &installed, Path::new("/cfg"));
        assert!(mac.contains("a&quot;b"));
        assert!(mac.contains("<key>PATH</key>"));
    }
}
