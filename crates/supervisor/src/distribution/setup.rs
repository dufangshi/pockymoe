use super::{releases, service, *};
use anyhow::{bail, ensure};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, net::TcpListener, time::Duration};

pub struct SetupOptions {
    pub relay: String,
    pub web_origin: Option<String>,
    pub account_key: Option<String>,
    pub enroll: Option<String>,
    pub token: Option<String>,
    pub code: Option<String>,
    pub port: u16,
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
async fn online(port: u16, version: &str) -> Result<()> {
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
        service::log_path().display()
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
fn validate_setup_binding(options: &SetupOptions) -> Result<()> {
    ensure!(
        options.account_key.is_some() == options.enroll.is_some(),
        "Provide both --account-key and --enroll"
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
pub async fn setup(options: SetupOptions) -> Result<()> {
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
    let file = config_path();
    let mut saved: Value = if file.exists() {
        read_device_config(&file)?
    } else {
        json!({})
    };
    ensure!(saved.is_object(), "Invalid device configuration");
    let receipt_file = file.with_extension("json.setup.json");
    let receipt = read::<Value>(&receipt_file).unwrap_or(Value::Null);
    let resume = options
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
                == Some(origin)),
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
    let installed = releases::install(version, Some(&std::env::current_exe()?)).await?;
    if occupied {
        apply_setup_binding(&saved, &options)?;
        let status = device_api(options.port, &saved, "/api/management/supervisor", false).await?;
        if status["runningVersion"] == version {
            println!("Device is already running Pockymoe {version}.");
            return Ok(());
        }
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
        return online(options.port, version).await;
    }
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
    releases::activate(&installed)?;
    let manager = service::detect();
    service::start(&installed, &file, manager.as_deref())?;
    if manager.is_none() {
        println!("Running detached. Automatic startup after reboot is unavailable without a user service manager.");
    }
    online(options.port, version).await
}
pub fn load_environment(config: &Path) -> Result<BTreeMap<String, String>> {
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
    env.insert("POCKYMOE_MODE".into(), "relay".into());
    env.insert("POCKYMOE_RELAY_SUPERVISOR_HOST".into(), "127.0.0.1".into());
    env.insert(
        "POCKYMOE_RELAY_SUPERVISOR_CONFIG".into(),
        config.to_string_lossy().into_owned(),
    );
    ensure!(
        env.get("POCKYMOE_RELAY_AGENT_TOKEN")
            .is_some_and(|t| !t.is_empty()),
        "Device token is missing from config"
    );
    let fallback = home().join(".remote-codex/relay-supervisor.sqlite");
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
    let env = load_environment(&config)?;
    for (key, _) in std::env::vars().filter(|(k, _)| k.starts_with("POCKYMOE_")) {
        if key != "POCKYMOE_MANAGED_SERVICE" {
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
        if let Ok(manager) = std::env::var("POCKYMOE_MANAGED_SERVICE") {
            if std::env::current_exe().ok().as_deref() == Some(installed.executable.as_path()) {
                if let Err(error) = service::refresh(&manager, &installed, &config) {
                    tracing::warn!(%error, "could not refresh the service definition");
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
        };
        validate_setup_binding(&options).unwrap();
        let saved = json!({"POCKYMOE_DATABASE_PATH":db});
        apply_setup_binding(&saved, &options).unwrap();
        assert_eq!(
            crate::owner_trust::binding_status(&db).unwrap()["strict"],
            true
        );
        assert!(saved.get("POCKYMOE_ENROLL").is_none());
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
