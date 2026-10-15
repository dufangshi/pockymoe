use super::{releases, service, *};
use anyhow::{bail, ensure};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{BufRead, Write},
    net::TcpListener,
    time::Duration,
};

pub struct SetupOptions {
    pub relay: String,
    pub web_origin: Option<String>,
    pub account_key: Option<String>,
    pub enroll: Option<String>,
    pub token: Option<String>,
    pub code: Option<String>,
    pub port: u16,
    /// Move an existing database that cannot be upgraded aside without asking.
    pub reset_database: bool,
}
fn digest(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}
fn identity(origin: &str, options: &SetupOptions) -> String {
    digest(
        &serde_json::to_vec(&json!([
            origin,
            options.port,
            if options.token.is_some() {
                "token"
            } else {
                "code"
            },
            options.token.as_ref().or(options.code.as_ref())
        ]))
        .unwrap(),
    )
}
pub fn matches(saved: &Value, origin: &str, token: &str, port: u16) -> bool {
    let stored = saved["POCKYMOE_RELAY_SERVER_URL"]
        .as_str()
        .unwrap_or("")
        .replacen("wss:", "https:", 1)
        .replacen("ws:", "http:", 1);
    url::Url::parse(&stored).is_ok_and(|u| u.origin().ascii_serialization() == origin)
        && saved["POCKYMOE_RELAY_AGENT_TOKEN"] == token
        && saved["POCKYMOE_RELAY_SUPERVISOR_PORT"]
            .as_str()
            .unwrap_or("8787")
            .parse::<u16>()
            .ok()
            == Some(port)
}
fn local_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()?)
}
pub async fn device_api(port: u16, saved: &Value, route: &str, post: bool) -> Result<Value> {
    let base = format!("http://127.0.0.1:{port}");
    let client = local_client()?;
    let login = client.post(format!("{base}/api/auth/login")).json(&json!({"username":saved["POCKYMOE_ADMIN_USERNAME"],"password":saved["POCKYMOE_ADMIN_PASSWORD"]})).send().await?.error_for_status()?;
    let cookie = login
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.split(';').next())
        .map(str::to_string);
    let body: Value = login.json().await?;
    let mut request = if post {
        client.post(format!("{base}{route}")).json(&json!({}))
    } else {
        client.get(format!("{base}{route}"))
    };
    if let Some(cookie) = cookie {
        request = request.header(reqwest::header::COOKIE, cookie);
    } else if let Some(token) = body["token"].as_str() {
        request = request.bearer_auth(token);
    }
    Ok(request.send().await?.error_for_status()?.json().await?)
}
pub async fn health(port: u16, host: &str) -> Option<Value> {
    let host = match host {
        "0.0.0.0" => "127.0.0.1",
        "::" => "[::1]",
        h => h,
    };
    local_client()
        .ok()?
        .get(format!("http://{host}:{port}/healthz"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()
}
async fn online(port: u16, version: &str, config: &Path) -> Result<()> {
    for _ in 0..120 {
        if let Some(h) = health(port, "127.0.0.1").await {
            if h["status"] == "ok" && h["runningVersion"] == version && h["relayConnected"] == true
            {
                println!("Device is online and running Pockymoe {version}.");
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    bail!(
        "Device did not reconnect at the requested version. Logs: {}",
        service::log_path(config).display()
    )
}
fn bridge(path: &Path, binary: &Path) -> Result<()> {
    ensure!(
        path.file_name().and_then(|n| n.to_str()) == Some("remote-codex.mjs"),
        "Unrecognized legacy launcher path"
    );
    let package: Value = read(
        &path
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("package.json"),
    )?;
    ensure!(
        package["name"] == "remote-codex",
        "Unrecognized legacy launcher package"
    );
    let helper = path.with_file_name("supervisor-update.mjs");
    let source = std::fs::read(&helper)?;
    let backup = helper.with_extension("mjs.pre-native");
    if !backup.exists() {
        write_bytes(&backup, &source)?;
    }
    let js_path = serde_json::to_string(&binary.to_string_lossy())?;
    let code = format!("// One-time compatibility bridge: native updater owns all downloads.\nimport {{spawnSync}} from 'node:child_process';\nconst r=spawnSync({js_path},['runtime-maintenance',process.argv[2]],{{stdio:'inherit',env:process.env}});\nif(r.error){{console.error(r.error.message);process.exit(1);}}\nprocess.exit(r.status??1);\n");
    write_bytes(&helper, code.as_bytes())?;
    Ok(())
}
fn setup_database_path(saved: &Value) -> PathBuf {
    saved["POCKYMOE_DATABASE_PATH"]
        .as_str()
        .or_else(|| saved["DATABASE_URL"].as_str())
        .filter(|path| !path.is_empty() && !is_database_url(path))
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".remote-codex/relay-supervisor.sqlite"))
}

/// Ask a yes/no question on the terminal. `curl … | sh` hands the script to the
/// shell on stdin, so read the answer from the controlling terminal instead.
/// `None` means there is no terminal to ask.
fn ask_terminal(question: &str) -> Result<Option<bool>> {
    #[cfg(unix)]
    let (mut output, mut input): (Box<dyn Write>, Box<dyn BufRead>) =
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        {
            Ok(tty) => (
                Box::new(tty.try_clone()?),
                Box::new(std::io::BufReader::new(tty)),
            ),
            Err(_) => return Ok(None),
        };
    #[cfg(not(unix))]
    let (mut output, mut input): (Box<dyn Write>, Box<dyn BufRead>) = {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            return Ok(None);
        }
        (
            Box::new(std::io::stdout()),
            Box::new(std::io::BufReader::new(std::io::stdin())),
        )
    };
    write!(output, "{question} [y/N] ")?;
    output.flush()?;
    let mut answer = String::new();
    if input.read_line(&mut answer)? == 0 {
        return Ok(None);
    }
    Ok(Some(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    )))
}

fn database_busy(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<rusqlite::Error>()
            .and_then(rusqlite::Error::sqlite_error_code)
            .is_some_and(|code| {
                matches!(
                    code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                )
            })
    })
}

/// Keep an unusable database next to its original path, with its journals.
fn move_database_aside(path: &Path) -> Result<PathBuf> {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("supervisor");
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let target = path.with_file_name(format!("{stem}.unusable-{stamp}.sqlite"));
    ensure!(!target.exists(), "{} already exists", target.display());
    std::fs::rename(path, &target)?;
    for suffix in ["-wal", "-shm"] {
        let mut from = path.as_os_str().to_os_string();
        from.push(suffix);
        let from = PathBuf::from(from);
        if from.exists() {
            let mut to = target.as_os_str().to_os_string();
            to.push(suffix);
            std::fs::rename(&from, PathBuf::from(to))?;
        }
    }
    Ok(target)
}

/// Upgrade an existing device database before anything else is installed. Older
/// Remote Codex databases are migrated automatically; one that cannot be used is
/// moved aside only after the user agrees (or passed `--reset-database`).
fn prepare_device_database(
    path: &Path,
    reset: bool,
    ask: impl FnOnce(&str) -> Result<Option<bool>>,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let error = match pockymoe_runtime::db::Database::prepare_unowned(path) {
        Ok(_) => return Ok(()),
        Err(error) => error,
    };
    if database_busy(&error) {
        return Err(error.context(format!(
            "Another program is using {}. Stop the previous Pockymoe or Remote Codex installation, then run the setup command again",
            path.display()
        )));
    }
    // The path is already in the message; show only the underlying reason.
    let reason = error.root_cause().to_string();
    let accepted = if reset {
        true
    } else {
        let question = format!(
            "The existing device database at {} cannot be upgraded:\n  {reason}\nMove it aside and continue with an empty database? Threads from the old installation will not be shown; the old file is kept next to it.",
            path.display()
        );
        match ask(&question)? {
            Some(answer) => answer,
            None => bail!(
                "The existing device database at {} cannot be upgraded: {reason}\nRun the setup command again with --reset-database to move it aside (the old file is kept) and continue with an empty database.",
                path.display()
            ),
        }
    };
    ensure!(
        accepted,
        "Setup stopped; the existing database at {} was not changed",
        path.display()
    );
    let kept = move_database_aside(path)?;
    println!("Moved the old database to {}.", kept.display());
    Ok(())
}
fn validate_setup_binding(options: &SetupOptions) -> Result<()> {
    ensure!(
        options.account_key.is_some() == options.enroll.is_some(),
        "Provide --account-key and enrollment through POCKYMOE_ENROLL_SECRET or stdin"
    );
    if let (Some(account), Some(secret)) = (&options.account_key, &options.enroll) {
        crate::owner_trust::validate_binding(account, secret)?;
    }
    Ok(())
}
fn apply_setup_binding(saved: &Value, options: &SetupOptions) -> Result<()> {
    if let (Some(account), Some(secret)) = (&options.account_key, &options.enroll) {
        let database = saved["POCKYMOE_DATABASE_PATH"]
            .as_str()
            .or_else(|| saved["DATABASE_URL"].as_str())
            .context("Device database path is missing")?;
        crate::owner_trust::bind_account(
            Path::new(database),
            account,
            secret,
            options.web_origin.as_deref(),
        )?;
    }
    Ok(())
}
fn save_setup_web_origin(file: &Path, saved: &mut Value, origin: Option<&str>) -> Result<()> {
    let Some(origin) = origin else {
        return Ok(());
    };
    if saved["webOrigin"].as_str() == Some(origin) {
        return Ok(());
    }
    let receipt_file = file.with_extension("json.setup.json");
    let mut receipt = read::<Value>(&receipt_file).unwrap_or(Value::Null);
    let resume_receipt =
        receipt["identity"].is_string() && receipt["configHash"] == digest(&std::fs::read(file)?);
    saved["webOrigin"] = json!(origin);
    write_device_config(file, saved)?;
    // Preserve setup-code resumption after adding an origin to an older config.
    if resume_receipt {
        receipt["configHash"] = json!(digest(&std::fs::read(file)?));
        write(&receipt_file, &receipt)?;
    }
    Ok(())
}
pub struct LocalSetupOptions {
    /// None picks 8787, or the next free port.
    pub port: Option<u16>,
    /// Let other devices on the network connect; None asks on the terminal.
    pub lan: Option<bool>,
    pub reset_database: bool,
}

const LOCAL_DEFAULT_PORT: u16 = 8787;

fn port_free(port: u16) -> bool {
    TcpListener::bind(("0.0.0.0", port)).is_ok() && TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// The address other devices on this network would use, without sending traffic.
fn network_address() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip()).filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
}

async fn running_version(port: u16) -> Option<String> {
    health(port, "127.0.0.1")
        .await
        .and_then(|h| h["runningVersion"].as_str().map(str::to_owned))
}

/// Install Pockymoe for this computer (and optionally its network) without a
/// relay: one command with sensible defaults, rerunnable to repair or update.
pub async fn setup_local(options: LocalSetupOptions) -> Result<()> {
    let file = home()
        .join(".remote-codex")
        .join(service::LOCAL_CONFIG_FILE);
    setup_local_at(options, &file, ask_terminal).await
}

async fn setup_local_at(
    options: LocalSetupOptions,
    file: &Path,
    ask: impl Fn(&str) -> Result<Option<bool>> + Copy,
) -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let mut saved: Value = if file.exists() {
        read_device_config(file)?
    } else {
        json!({})
    };
    ensure!(
        saved.as_object().is_some_and(|object| object.is_empty())
            || saved["POCKYMOE_MODE"] == "local",
        "{} is not a local-mode configuration",
        file.display()
    );
    let saved_port = saved["POCKYMOE_PORT"]
        .as_str()
        .and_then(|port| port.parse::<u16>().ok());
    let running = match saved_port {
        Some(port) => running_version(port).await,
        None => None,
    };
    let port = match (options.port, saved_port) {
        (Some(port), _) => {
            ensure!(
                Some(port) == saved_port && running.is_some() || port_free(port),
                "Port {port} is already in use. Choose another one with --port"
            );
            port
        }
        (None, Some(port)) => port,
        (None, None) => (LOCAL_DEFAULT_PORT..LOCAL_DEFAULT_PORT + 100)
            .find(|port| port_free(*port))
            .context("No free port between 8787 and 8886. Choose one with --port")?,
    };
    let lan = match options.lan {
        Some(lan) => lan,
        None if saved.get("POCKYMOE_HOST").is_some() => saved["POCKYMOE_HOST"] == "0.0.0.0",
        None => {
            ask("Allow other devices on your network (phones, other computers) to open Pockymoe?")?
                .unwrap_or(false)
        }
    };
    let database = setup_database_path(&json!({
        "POCKYMOE_DATABASE_PATH": saved["POCKYMOE_DATABASE_PATH"].as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| home().join(".remote-codex/supervisor.sqlite").to_string_lossy().into_owned())
    }));
    if running.is_none() {
        // A running Supervisor owns its database; it has upgraded it already.
        prepare_device_database(&database, options.reset_database, ask)?;
    }
    let installed = releases::install(version, Some(&std::env::current_exe()?)).await?;
    saved["POCKYMOE_MODE"] = json!("local");
    saved["POCKYMOE_PORT"] = json!(port.to_string());
    saved["POCKYMOE_HOST"] = json!(if lan { "0.0.0.0" } else { "127.0.0.1" });
    saved["POCKYMOE_DATABASE_PATH"] = json!(database);
    saved["POCKYMOE_ENVIRONMENT"] = json!("production");
    let changed = !file.exists() || read_device_config(file)? != saved;
    write_device_config(file, &saved)?;
    releases::activate(&installed)?;
    let manager = service::detect();
    match (&running, manager.as_deref()) {
        (Some(current), _) if current == version && !changed => {}
        (Some(_), Some(manager)) => {
            service::install(manager, &installed, file)?;
            service::manage(manager, "restart", file)?;
        }
        (Some(_), None) => bail!(
            "Pockymoe {} is already running on port {}. Update it from Settings, then run this command again",
            running.as_deref().unwrap_or("?"),
            saved_port.unwrap_or(port)
        ),
        (None, manager) => service::start(&installed, file, manager)?,
    }
    for _ in 0..120 {
        if running_version(port).await.as_deref() == Some(version) {
            println!("\nPockymoe {version} is running: http://127.0.0.1:{port}");
            if lan {
                match network_address() {
                    Some(ip) => println!("Other devices on your network: http://{ip}:{port}"),
                    None => println!(
                        "Other devices on your network: http://<this computer's address>:{port}"
                    ),
                }
                println!("Anyone on your network can open it. To require a passcode, open Settings > Access passcode.");
            }
            if manager.is_none() {
                println!("Running detached. Automatic startup after reboot is unavailable without a user service manager.");
            }
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    bail!(
        "Pockymoe did not start on port {port}. Logs: {}",
        service::log_path(file).display()
    )
}

pub async fn setup(options: SetupOptions) -> Result<()> {
    setup_at(options, &config_path()).await
}
async fn setup_at(options: SetupOptions, file: &Path) -> Result<()> {
    std::env::remove_var("POCKYMOE_ENROLL_SECRET");
    std::env::remove_var("POCKYMOE_ENROLL");
    validate_setup_binding(&options)?;
    ensure!(
        options.port > 0 && options.token.is_some() != options.code.is_some(),
        "Provide one device token or setup code and a valid port"
    );
    let url = url::Url::parse(&options.relay)?;
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Invalid relay URL"
    );
    ensure!(
        url.scheme() == "https"
            || (url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))),
        "Relay must use HTTPS"
    );
    let origin = url.origin().ascii_serialization();
    let web_origin = options
        .web_origin
        .as_deref()
        .map(|origin| crate::owner_trust::validate_web_origin(origin))
        .transpose()?;
    let mut saved: Value = if file.exists() {
        read_device_config(&file)?
    } else {
        json!({})
    };
    ensure!(saved.is_object(), "Invalid device configuration");
    let receipt_file = file.with_extension("json.setup.json");
    let receipt = read::<Value>(&receipt_file).unwrap_or(Value::Null);
    let rebind = options.account_key.is_some()
        && saved["POCKYMOE_RELAY_AGENT_TOKEN"]
            .as_str()
            .is_some_and(|token| matches(&saved, &origin, token, options.port));
    let resume = rebind
        || options
            .token
            .as_deref()
            .is_some_and(|t| matches(&saved, &origin, t, options.port))
        || (options.code.is_some()
            && (receipt["identity"] == identity(&origin, &options)
                || receipt["identity"]
                    == digest(
                        &serde_json::to_vec(&json!([origin, options.port, options.code])).unwrap(),
                    ))
            && receipt["configHash"] == digest(&std::fs::read(&file).unwrap_or_default()));
    ensure!(
        web_origin.as_deref().is_none_or(|origin| !resume
            || saved["webOrigin"]
                .as_str()
                .or_else(|| saved["POCKYMOE_WEB_ORIGIN"].as_str())
                .is_none_or(|saved_origin| saved_origin == origin)),
        "The saved Web origin differs; update the local configuration before resuming setup"
    );
    let occupied = TcpListener::bind(("127.0.0.1", options.port)).is_err();
    ensure!(
        !occupied || resume,
        "Port is already in use by another device or service"
    );
    ensure!(
        !file.exists() || resume,
        "A different device configuration already exists; it will not be overwritten"
    );
    let version = env!("CARGO_PKG_VERSION");
    if occupied {
        // Authenticate the already-installed Supervisor before modifying its
        // binding. Fresh relay setup credentials never replace its registration.
        let status = device_api(options.port, &saved, "/api/management/supervisor", false).await?;
        apply_setup_binding(&saved, &options)?;
        save_setup_web_origin(file, &mut saved, web_origin.as_deref())?;
        if status["runningVersion"] == version {
            println!("Device is already running Pockymoe {version}.");
            return Ok(());
        }
        let installed = releases::install(version, Some(&std::env::current_exe()?)).await?;
        if status["manager"] != "github-release" {
            let legacy = status["path"].as_str().context("Existing installation cannot migrate automatically; no legacy launcher path was reported")?;
            bridge(Path::new(legacy), &installed.executable)
                .context("Install native updater compatibility bridge")?;
        }
        let result = device_api(
            options.port,
            &saved,
            "/api/management/supervisor/update",
            true,
        )
        .await?;
        ensure!(
            result["job"]["phase"] != "failed" && result["canUpdate"] != false,
            "Existing device could not start the native update"
        );
        return online(options.port, version, file).await;
    }
    // Before downloading or writing anything: a database left by an older
    // installation is upgraded here, or set aside with the user's consent.
    prepare_device_database(
        &setup_database_path(&saved),
        options.reset_database,
        ask_terminal,
    )?;
    let installed = releases::install(version, Some(&std::env::current_exe()?)).await?;
    if !resume {
        let token = if let Some(token) = &options.token {
            token.clone()
        } else {
            let response = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?
                .post(format!("{origin}/relay/setup/redeem"))
                .json(&json!({"code":options.code}))
                .send()
                .await?
                .error_for_status()?;
            let enrollment: Value = response.json().await?;
            enrollment["token"]
                .as_str()
                .filter(|s| !s.is_empty())
                .context("Invalid setup enrollment response")?
                .into()
        };
        saved["POCKYMOE_RELAY_SERVER_URL"] = json!(origin
            .replacen("https:", "wss:", 1)
            .replacen("http:", "ws:", 1));
        saved["POCKYMOE_RELAY_AGENT_TOKEN"] = json!(token);
        if let Some(origin) = &web_origin {
            saved["webOrigin"] = json!(origin);
        }
        saved["POCKYMOE_RELAY_SUPERVISOR_PORT"] = json!(options.port.to_string());
        saved["POCKYMOE_ADMIN_USERNAME"] = json!("admin");
        saved["POCKYMOE_ADMIN_PASSWORD"] = json!(format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ));
        saved["POCKYMOE_SESSION_SECRET"] = json!(format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ));
        saved["POCKYMOE_DATABASE_PATH"] =
            json!(home().join(".remote-codex/relay-supervisor.sqlite"));
        write_device_config(&file, &saved)?;
        write(
            &receipt_file,
            &json!({"identity":identity(&origin, &options),"configHash":digest(&std::fs::read(&file)?)}),
        )?;
    }
    apply_setup_binding(&saved, &options)?;
    save_setup_web_origin(file, &mut saved, web_origin.as_deref())?;
    releases::activate(&installed)?;
    let manager = service::detect();
    service::start(&installed, &file, manager.as_deref())?;
    if manager.is_none() {
        println!("Running detached. Automatic startup after reboot is unavailable without a user service manager.");
    }
    online(options.port, version, file).await
}
pub fn load_environment(config: &Path) -> Result<BTreeMap<String, String>> {
    load_environment_with_host_mode(config, None)
}

fn load_environment_with_host_mode(
    config: &Path,
    host_mode: Option<pockymoe_runtime::host_mode::HostMode>,
) -> Result<BTreeMap<String, String>> {
    let saved: Value = read_device_config(config)?;
    let mut env = BTreeMap::new();
    for (key, value) in saved.as_object().context("Invalid device configuration")? {
        if key.starts_with("POCKYMOE_") {
            if let Some(value) = value.as_str() {
                env.insert(key.clone(), value.into());
            }
        }
    }
    if let Some(origin) = saved["webOrigin"].as_str() {
        let origin = crate::owner_trust::validate_web_origin(origin)?;
        env.insert("POCKYMOE_WEB_ORIGIN".into(), origin);
    }
    // A local-mode install serves its own Web UI on POCKYMOE_HOST/POCKYMOE_PORT.
    let local = saved["POCKYMOE_MODE"].as_str() == Some("local");
    if local {
        env.insert("POCKYMOE_MODE".into(), "local".into());
        env.entry("POCKYMOE_ENVIRONMENT".into())
            .or_insert_with(|| "production".into());
    } else {
        env.insert("POCKYMOE_MODE".into(), "relay".into());
        env.insert("POCKYMOE_RELAY_SUPERVISOR_HOST".into(), "127.0.0.1".into());
    }
    env.insert(
        "POCKYMOE_RELAY_SUPERVISOR_CONFIG".into(),
        config.to_string_lossy().into_owned(),
    );
    if let Some(mode) = host_mode {
        env.insert("POCKYMOE_HOST_MODE".into(), mode.to_string());
    }
    let host_mode = env
        .get("POCKYMOE_HOST_MODE")
        .map(|value| value.parse::<pockymoe_runtime::host_mode::HostMode>())
        .transpose()?
        .unwrap_or_default();
    ensure!(
        local
            || !host_mode.capabilities().relay
            || env
                .get("POCKYMOE_RELAY_AGENT_TOKEN")
                .is_some_and(|t| !t.is_empty()),
        "Device token is missing from config"
    );
    let fallback = home().join(if local {
        ".remote-codex/supervisor.sqlite"
    } else {
        ".remote-codex/relay-supervisor.sqlite"
    });
    let configured = saved["POCKYMOE_DATABASE_PATH"]
        .as_str()
        .or_else(|| saved["DATABASE_URL"].as_str())
        .filter(|v| !v.is_empty());
    let database = match configured {
        Some(value) if is_database_url(value) => {
            ensure!(fallback.is_file() && fallback.with_extension("transport-identity").is_file(),
                "Saved configuration contains a database URL. Restore its original absolute SQLite path before starting");
            eprintln!("Restoring the original relay database path saved by an older launcher.");
            fallback
        }
        Some(value) => PathBuf::from(value),
        None => fallback,
    };
    ensure!(
        database.is_absolute(),
        "Device database path must be absolute"
    );
    env.insert(
        "POCKYMOE_DATABASE_PATH".into(),
        database.to_string_lossy().into_owned(),
    );
    Ok(env)
}
fn is_database_url(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() > 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
    {
        return false;
    }
    value.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    })
}
pub async fn run_device(config: PathBuf) -> Result<()> {
    run_device_with_host_mode(config, None).await
}

pub async fn run_device_with_host_mode(
    config: PathBuf,
    host_mode: Option<pockymoe_runtime::host_mode::HostMode>,
) -> Result<()> {
    let env = match host_mode {
        Some(mode) => load_environment_with_host_mode(&config, Some(mode))?,
        None => load_environment(&config)?,
    };
    for (key, _) in std::env::vars().filter(|(k, _)| k.starts_with("POCKYMOE_")) {
        if !matches!(
            key.as_str(),
            "POCKYMOE_MANAGED_SERVICE" | "POCKYMOE_SUPERVISOR_RESTART_STRATEGY"
        ) {
            std::env::remove_var(key);
        }
    }
    for (key, value) in env {
        std::env::set_var(key, value);
    }
    std::env::remove_var("TMUX");
    std::env::remove_var("TMUX_PANE");
    std::env::set_var("PATH", user_path());
    if let Ok(installed) = releases::current() {
        if std::env::var("POCKYMOE_SUPERVISOR_RESTART_STRATEGY")
            .unwrap_or_default()
            .parse::<pockymoe_runtime::RestartStrategy>()?
            != pockymoe_runtime::RestartStrategy::HostManaged
        {
            if let Ok(manager) = std::env::var("POCKYMOE_MANAGED_SERVICE") {
                if std::env::current_exe().ok().as_deref() == Some(installed.executable.as_path()) {
                    if let Err(error) = service::refresh(&manager, &installed, &config) {
                        tracing::warn!(%error, "could not refresh the service definition");
                    }
                }
            }
        }
        std::env::set_var("POCKYMOE_WEB_DIST_DIR", installed.web_dist);
    }
    let state = pockymoe_runtime::boot().await?;
    crate::serve(state).await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_mode_device_config_and_flag_select_startup_policy() {
        use pockymoe_runtime::host_mode::HostMode;
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("device.json");
        let mut saved = json!({"POCKYMOE_DATABASE_PATH":dir.path().join("test.sqlite"),"POCKYMOE_HOST_MODE":"hosted"});
        write_device_config(&config, &saved).unwrap();
        let env = load_environment(&config).unwrap();
        assert_eq!(env["POCKYMOE_HOST_MODE"], "hosted");
        assert_eq!(env["POCKYMOE_MODE"], "relay");
        assert!(load_environment_with_host_mode(&config, Some(HostMode::Standalone)).is_err());
        saved["POCKYMOE_HOST_MODE"] = json!("invalid");
        write_device_config(&config, &saved).unwrap();
        assert!(load_environment(&config).is_err());
        assert!(load_environment_with_host_mode(&config, Some(HostMode::Hosted)).is_ok());
        saved.as_object_mut().unwrap().remove("POCKYMOE_HOST_MODE");
        saved["POCKYMOE_RELAY_AGENT_TOKEN"] = json!("synthetic-token");
        write_device_config(&config, &saved).unwrap();
        let env = load_environment(&config).unwrap();
        assert!(!env.contains_key("POCKYMOE_HOST_MODE"));
        assert_eq!(env["POCKYMOE_MODE"], "relay");
    }

    #[tokio::test]
    async fn explicit_rebind_of_running_install_preserves_registration_without_installing_again() {
        assert_running_rebind_web_origin(None, None, false).await;
    }

    #[tokio::test]
    async fn existing_install_without_web_origin_saves_supplied_origin_when_rebinding() {
        assert_running_rebind_web_origin(None, Some("https://web.test"), false).await;
    }

    #[tokio::test]
    async fn existing_install_rebind_accepts_same_web_origin_and_refuses_different_origin() {
        assert_running_rebind_web_origin(Some("https://web.test"), Some("https://web.test"), false)
            .await;
        assert_running_rebind_web_origin(
            Some("https://original.test"),
            Some("https://web.test"),
            true,
        )
        .await;
    }

    async fn assert_running_rebind_web_origin(
        saved_origin: Option<&str>,
        supplied_origin: Option<&str>,
        expect_mismatch: bool,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("existing.sqlite");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let relay = format!("http://127.0.0.1:{port}");
        let config = dir.path().join("device.json");
        let mut saved = json!({"POCKYMOE_RELAY_SERVER_URL":relay,"POCKYMOE_RELAY_AGENT_TOKEN":"synthetic-existing-token","POCKYMOE_RELAY_SUPERVISOR_PORT":port.to_string(),"POCKYMOE_DATABASE_PATH":db,"POCKYMOE_ADMIN_USERNAME":"admin","POCKYMOE_ADMIN_PASSWORD":"synthetic-password"});
        if let Some(origin) = saved_origin {
            saved["webOrigin"] = json!(origin);
        }
        write_device_config(&config, &saved).unwrap();
        let before = std::fs::read(&config).unwrap();
        let receipt_file = config.with_extension("json.setup.json");
        let receipt = json!({"identity":"synthetic-setup-identity","configHash":digest(&before)});
        write(&receipt_file, &receipt).unwrap();
        async fn login() -> axum::Json<Value> {
            axum::Json(json!({"token":"synthetic-session"}))
        }
        async fn status(headers: axum::http::HeaderMap) -> axum::Json<Value> {
            assert_eq!(
                headers[axum::http::header::AUTHORIZATION],
                "Bearer synthetic-session"
            );
            axum::Json(json!({"runningVersion":env!("CARGO_PKG_VERSION")}))
        }
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new()
                    .route("/api/auth/login", axum::routing::post(login))
                    .route("/api/management/supervisor", axum::routing::get(status)),
            )
            .await
            .unwrap();
        });
        let v = crate::owner_trust::tests::vectors();
        crate::owner_trust::bind_account(
            &db,
            v["account"]["accountPublicKey"].as_str().unwrap(),
            v["enrollment"]["enrollSecret"].as_str().unwrap(),
            None,
        )
        .unwrap();
        crate::owner_trust::reset_account_trust(&db).unwrap();
        assert_eq!(
            crate::owner_trust::binding_status(&db).unwrap()["bound"],
            false
        );
        let options = SetupOptions {
            relay,
            web_origin: supplied_origin.map(str::to_owned),
            account_key: Some(v["account"]["accountPublicKey"].as_str().unwrap().into()),
            enroll: Some(base64::Engine::encode(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                [31; 32],
            )),
            token: None,
            code: Some("new-unused-code".into()),
            port,
            reset_database: false,
        };
        let result = setup_at(options, &config).await;
        server.abort();
        if expect_mismatch {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("saved Web origin differs"));
            assert_eq!(std::fs::read(&config).unwrap(), before);
            assert_eq!(read::<Value>(&receipt_file).unwrap(), receipt);
            assert_eq!(
                crate::owner_trust::binding_status(&db).unwrap()["bound"],
                false
            );
            return;
        }
        result.unwrap();
        if let Some(origin) = supplied_origin {
            saved["webOrigin"] = json!(origin);
            assert_eq!(
                load_environment(&config).unwrap()["POCKYMOE_WEB_ORIGIN"],
                origin
            );
        }
        assert_eq!(read_device_config(&config).unwrap(), saved);
        let after = std::fs::read(&config).unwrap();
        let updated_receipt: Value = read(&receipt_file).unwrap();
        assert_eq!(updated_receipt["identity"], receipt["identity"]);
        assert_eq!(updated_receipt["configHash"], digest(&after));
        if supplied_origin == saved_origin {
            assert_eq!(after, before);
        }
        assert_eq!(
            crate::owner_trust::binding_status(&db).unwrap()["bound"],
            true
        );
        // Fresh setup codes are not redeemed and no release downloader or service
        // launcher is used: the fake server exposes only local management routes.
    }

    /// The shape of the database that stopped setup on a device that had run an
    /// unrelated or pre-ledger Remote Codex: application tables, no Node ledger.
    fn unusable_database(path: &Path) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE hosts(id TEXT, hostname TEXT, platform TEXT, created_at TEXT, last_seen_at TEXT);
             CREATE TABLE workspaces(id TEXT, host_id TEXT, label TEXT, abs_path TEXT, is_favorite INTEGER, created_at TEXT, last_opened_at TEXT);
             CREATE TABLE threads(id TEXT, workspace_id TEXT, title TEXT);
             INSERT INTO threads VALUES('old', 'w', 'Old thread');",
        )
        .unwrap();
    }

    fn moved_aside(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".unusable-"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn unusable_database_needs_consent_and_is_kept_when_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("relay-supervisor.sqlite");
        unusable_database(&db);
        let before = std::fs::read(&db).unwrap();

        // No terminal to ask: explain the flag and change nothing.
        let error = prepare_device_database(&db, false, |_| Ok(None)).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("--reset-database"), "{message}");
        assert!(
            message.contains("unsupported `threads` schema"),
            "{message}"
        );
        assert_eq!(std::fs::read(&db).unwrap(), before);

        // The user declines.
        let mut asked = String::new();
        let error = prepare_device_database(&db, false, |question| {
            asked = question.to_owned();
            Ok(Some(false))
        })
        .unwrap_err();
        assert!(asked.contains("Move it aside"), "{asked}");
        assert!(format!("{error:#}").contains("was not changed"));
        assert_eq!(std::fs::read(&db).unwrap(), before);
        assert!(moved_aside(dir.path()).is_empty());

        // The user agrees: the old file is kept under a new name.
        prepare_device_database(&db, false, |_| Ok(Some(true))).unwrap();
        assert!(!db.exists());
        let kept = moved_aside(dir.path());
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(kept[0].starts_with("relay-supervisor.unusable-") && kept[0].ends_with(".sqlite"));
        let old = rusqlite::Connection::open(dir.path().join(&kept[0])).unwrap();
        let title: String = old
            .query_row("SELECT title FROM threads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(title, "Old thread");
        // Setup then starts from an empty database at the original path.
        assert!(pockymoe_runtime::db::Database::prepare_unowned(&db).unwrap());
    }

    #[test]
    fn reset_flag_moves_an_unusable_database_without_asking() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("relay-supervisor.sqlite");
        unusable_database(&db);
        prepare_device_database(&db, true, |_| panic!("must not ask")).unwrap();
        assert!(!db.exists());
        assert_eq!(moved_aside(dir.path()).len(), 1);
    }

    #[test]
    fn usable_or_missing_databases_are_prepared_without_asking() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.sqlite");
        prepare_device_database(&missing, true, |_| panic!("must not ask")).unwrap();
        assert!(!missing.exists(), "setup creates the database later");
        let current = dir.path().join("current.sqlite");
        drop(pockymoe_runtime::db::Database::open(&current).unwrap());
        prepare_device_database(&current, true, |_| panic!("must not ask")).unwrap();
        assert!(current.exists());
        assert!(moved_aside(dir.path()).is_empty());
    }

    #[test]
    fn local_device_config_runs_local_mode_without_a_relay_token() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(service::LOCAL_CONFIG_FILE);
        write_device_config(
            &config,
            &json!({"POCKYMOE_MODE":"local","POCKYMOE_PORT":"8790","POCKYMOE_HOST":"0.0.0.0","POCKYMOE_ENVIRONMENT":"production"}),
        )
        .unwrap();
        let env = load_environment(&config).unwrap();
        assert_eq!(env["POCKYMOE_MODE"], "local");
        assert_eq!(env["POCKYMOE_PORT"], "8790");
        assert_eq!(env["POCKYMOE_HOST"], "0.0.0.0");
        assert!(!env.contains_key("POCKYMOE_RELAY_SUPERVISOR_HOST"));
        assert!(env["POCKYMOE_DATABASE_PATH"].ends_with(".remote-codex/supervisor.sqlite"));
        // The update worker finds the service through this path.
        assert_eq!(
            env["POCKYMOE_RELAY_SUPERVISOR_CONFIG"],
            config.to_string_lossy()
        );
    }

    #[test]
    fn local_and_relay_installs_have_separate_services() {
        let installed = releases::Installed {
            version: "9.9.9".into(),
            executable: PathBuf::from("/opt/pockymoe/remote-codex"),
            web_dist: PathBuf::from("/opt/pockymoe/web"),
        };
        let local = Path::new("/home/u/.remote-codex").join(service::LOCAL_CONFIG_FILE);
        let relay = Path::new("/home/u/.remote-codex/relay-supervisor.json");
        let local_unit = service::definition("systemd-user", &installed, &local);
        let relay_plist = service::definition("launchd", &installed, relay);
        let local_plist = service::definition("launchd", &installed, &local);
        assert!(local_unit.contains("local-supervisor.json"));
        assert!(relay_plist.contains("<string>com.remote-codex.supervisor</string>"));
        assert!(local_plist.contains("<string>com.pockymoe.local</string>"));
        assert!(local_plist.contains("local-supervisor.log"));
        assert_ne!(service::log_path(&local), service::log_path(relay));
    }

    #[test]
    fn setup_database_path_follows_the_saved_configuration() {
        assert_eq!(
            setup_database_path(&json!({"POCKYMOE_DATABASE_PATH":"/data/device.sqlite"})),
            PathBuf::from("/data/device.sqlite")
        );
        assert_eq!(
            setup_database_path(&json!({})),
            home().join(".remote-codex/relay-supervisor.sqlite")
        );
    }

    #[test]
    fn setup_binds_fresh_database_before_runtime_and_keeps_enrollment_out_of_config() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("fresh.sqlite");
        let v = crate::owner_trust::tests::vectors();
        let mut options = SetupOptions {
            relay: "https://relay.test".into(),
            web_origin: Some("https://web.test".into()),
            account_key: Some(v["account"]["accountPublicKey"].as_str().unwrap().into()),
            enroll: Some(v["enrollment"]["enrollSecret"].as_str().unwrap().into()),
            token: Some("synthetic".into()),
            code: None,
            port: 8787,
            reset_database: false,
        };
        validate_setup_binding(&options).unwrap();
        let saved = json!({"POCKYMOE_DATABASE_PATH":db});
        apply_setup_binding(&saved, &options).unwrap();
        assert_eq!(
            crate::owner_trust::binding_status(&db).unwrap()["strict"],
            true
        );
        assert!(saved.get("POCKYMOE_ENROLL_SECRET").is_none());
        options.account_key = Some(
            v["otherAccount"]["accountPublicKey"]
                .as_str()
                .unwrap()
                .into(),
        );
        assert!(apply_setup_binding(&saved, &options).is_err());
        options.enroll = None;
        assert!(validate_setup_binding(&options).is_err());
        options.account_key = None;
        validate_setup_binding(&options).unwrap();
        apply_setup_binding(&json!({}), &options).unwrap();
    }

    #[test]
    fn local_web_origin_survives_config_persistence_and_environment_loading() {
        let root =
            std::env::temp_dir().join(format!("pockymoe-web-origin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("device.json");
        let saved = json!({"webOrigin":"https://web.example.test", "POCKYMOE_RELAY_SERVER_URL":"wss://relay.example.test", "POCKYMOE_RELAY_AGENT_TOKEN":"synthetic", "POCKYMOE_DATABASE_PATH":root.join("device.sqlite")});
        write_device_config(&file, &saved).unwrap();
        let loaded = read_device_config(&file).unwrap();
        assert_eq!(loaded["webOrigin"], "https://web.example.test");
        let env = load_environment(&file).unwrap();
        assert_eq!(env["POCKYMOE_WEB_ORIGIN"], "https://web.example.test");
        assert_eq!(env["POCKYMOE_RELAY_SERVER_URL"], "wss://relay.example.test");
        let mut invalid = loaded;
        invalid["webOrigin"] = json!("https://web.example.test/path");
        write_device_config(&file, &invalid).unwrap();
        assert!(load_environment(&file).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_setup_preserves_connection_identity_and_rejects_different_device() {
        let saved = json!({"POCKYMOE_RELAY_SERVER_URL":"wss://relay.example.test/","POCKYMOE_RELAY_AGENT_TOKEN":"synthetic","POCKYMOE_RELAY_SUPERVISOR_PORT":"8787"});
        assert!(matches(
            &saved,
            "https://relay.example.test",
            "synthetic",
            8787
        ));
        assert!(!matches(
            &saved,
            "https://other.example.test",
            "synthetic",
            8787
        ));
        assert!(!matches(
            &saved,
            "https://relay.example.test",
            "other",
            8787
        ));
        assert!(!matches(
            &saved,
            "https://relay.example.test",
            "synthetic",
            8788
        ));
    }
    #[tokio::test]
    async fn native_management_login_uses_session_cookie_and_does_not_send_password_as_bearer() {
        async fn login(
            axum::Json(input): axum::Json<Value>,
        ) -> (
            [(axum::http::HeaderName, &'static str); 1],
            axum::Json<Value>,
        ) {
            assert_eq!(input, json!({"username":"admin","password":"synthetic"}));
            (
                [(
                    axum::http::header::SET_COOKIE,
                    "session=synthetic-session; HttpOnly; Path=/",
                )],
                axum::Json(json!({"ok":true})),
            )
        }
        async fn status(headers: axum::http::HeaderMap) -> axum::Json<Value> {
            assert_eq!(
                headers[axum::http::header::COOKIE],
                "session=synthetic-session"
            );
            assert!(!headers.contains_key(axum::http::header::AUTHORIZATION));
            axum::Json(json!({"runningVersion":"9.1.0"}))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let router = axum::Router::new()
            .route("/api/auth/login", axum::routing::post(login))
            .route("/api/management/supervisor", axum::routing::get(status));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let saved =
            json!({"POCKYMOE_ADMIN_USERNAME":"admin","POCKYMOE_ADMIN_PASSWORD":"synthetic"});
        assert_eq!(
            device_api(port, &saved, "/api/management/supervisor", false)
                .await
                .unwrap()["runningVersion"],
            "9.1.0"
        );
        server.abort();
    }
}
