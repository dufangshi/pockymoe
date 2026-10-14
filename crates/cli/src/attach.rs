//! Thin Treer workload: credentials travel only over the authenticated loopback socket.
use anyhow::{bail, ensure, Context, Result};
use clap::Args;
use futures_util::{SinkExt, StreamExt};
use pockymoe_protocol::treer::{
    AttachCreateThread, AttachHello, AttachReply, AttachTreerIdentity,
    TREER_AGENT_INTERFACE_PROTOCOL, TREER_ATTACH_PATH, TREER_ATTACH_PROTOCOL,
    TREER_ATTACH_TOKEN_FILE,
};
use serde_json::Value;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, time};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
};

#[derive(Args)]
pub struct Options {
    /// Rebind a previously created thread instead of creating one.
    #[arg(long, conflicts_with_all = ["provider", "agent", "model", "reasoning_effort", "title"])]
    thread: Option<String>,
    #[arg(long, required_unless_present = "thread", value_parser = ["codex", "claude", "opencode", "acp"])]
    provider: Option<String>,
    #[arg(long)]
    agent: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    reasoning_effort: Option<String>,
    #[arg(long)]
    title: Option<String>,
    /// Loopback HTTP hub URL; defaults to the normal local CLI discovery URL.
    #[arg(long)]
    hub: Option<String>,
    /// Private attach token file; defaults to the hub database directory.
    #[arg(long)]
    hub_token_file: Option<PathBuf>,
}

struct Contract {
    capabilities: Vec<String>,
    features: Vec<String>,
}
impl Contract {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("Treer interface contract is not valid JSON"))?;
        ensure!(
            value["protocol"] == TREER_AGENT_INTERFACE_PROTOCOL,
            "Treer AIS protocol is unknown or incompatible"
        );
        fn strings(value: &Value) -> Result<Vec<String>> {
            value
                .as_array()
                .context("Treer AIS contract field is unknown")?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .context("Treer AIS contract field is invalid")
                })
                .collect()
        }
        let capabilities = strings(&value["capabilities"])?;
        let features = strings(&value["features"])?;
        for required in ["agent.interface-provenance.v1", "agent.prompt-delivery.v1"] {
            ensure!(
                features.iter().any(|v| v == required),
                "Treer AIS provenance or prompt-delivery support is unknown or incompatible"
            );
        }
        Ok(Self {
            capabilities,
            features,
        })
    }
}

struct Session {
    hello: AttachHello,
    hub: String,
    token_file: PathBuf,
    cwd: PathBuf,
    treer: OsString,
    startup_pending: bool,
    startup_executable: Option<PathBuf>,
}

fn absolute(path: PathBuf, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

/// Deliberately ignore the ordinary thread CLI token: the attach token is separate.
fn connection(
    options: &Options,
    c: crate::threads::Connection,
    cwd: &Path,
) -> Result<(String, PathBuf)> {
    let config = pockymoe_runtime::RuntimeConfig::from_env()?;
    let discovery = c
        .cli_config
        .unwrap_or_else(|| config.database_url.with_extension("cli.json"));
    let saved: Value = std::fs::read(&discovery)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let hub = options
        .hub
        .clone()
        .or(c.url)
        .or_else(|| saved["url"].as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", config.port));
    let url = reqwest::Url::parse(&hub).map_err(|_| anyhow::anyhow!("Invalid hub URL"))?;
    ensure!(
        url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "Attach requires a loopback HTTP hub URL without credentials, query or path"
    );
    let token = options.hub_token_file.clone().unwrap_or_else(|| {
        discovery
            .parent()
            .unwrap_or(Path::new("."))
            .join(TREER_ATTACH_TOKEN_FILE)
    });
    Ok((hub.trim_end_matches('/').to_owned(), absolute(token, cwd)))
}

// Never forward subprocess stdout/stderr: Treer diagnostics can contain credentials.
// Bound output and lifetime, including a child that holds its stdout pipe open.
async fn treer_command(binary: &OsString, args: &[OsString]) -> Result<Vec<u8>> {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| anyhow::anyhow!("Cannot start treer; check TREER_BIN or PATH"))?;
    let mut stdout = child.stdout.take().context("Cannot read treer output")?;
    let result = time::timeout(Duration::from_secs(20), async {
        let mut bytes = Vec::new();
        (&mut stdout)
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
        ensure!(bytes.len() <= 1024 * 1024, "Treer output exceeded limit");
        ensure!(
            child.wait().await?.success(),
            "Treer command failed; check Controller configuration"
        );
        Ok(bytes)
    })
    .await;
    result.map_err(|_| anyhow::anyhow!("Treer command timed out"))?
}

fn versioned(path: &Path) -> bool {
    path.components().any(|p| p.as_os_str() == "releases")
}

/// Keep the lexical symlink/current path, not its versioned canonical target.
fn stable_executable() -> Result<PathBuf> {
    let running = std::env::current_exe().context("Cannot locate pockymoe executable")?;
    let target = std::fs::canonicalize(&running).context("Cannot resolve pockymoe executable")?;
    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        candidates.push(home.join(".local/bin/pockymoe"));
        candidates.push(home.join(".local/share/remote-codex/native/current/remote-codex"));
    }
    // Also support a native installation outside the default home directory.
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
    if let Some(arg0) = std::env::args_os().next() {
        let path = PathBuf::from(arg0);
        if path.components().count() > 1 {
            candidates.push(absolute(path, &std::env::current_dir()?));
        }
    }
    candidates.push(running);
    for path in candidates {
        if !versioned(&path) && std::fs::canonicalize(&path).ok().as_ref() == Some(&target) {
            return Ok(path);
        }
    }
    eprintln!("Pockymoe attach: only a versioned executable is available; install a stable pockymoe/current launcher.");
    bail!("Cannot save a versioned Treer startup path")
}

fn text_args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// Treer startup directories are relative to the enrolled machine root.
fn startup_cwd(cwd: &Path, identity: &[u8]) -> Result<PathBuf> {
    let identity: Value = serde_json::from_slice(identity)
        .map_err(|_| anyhow::anyhow!("Treer identity is not valid JSON"))?;
    let root = identity["machine"]["root"]
        .as_str()
        .filter(|root| Path::new(root).is_absolute())
        .context("Treer machine root is unavailable")?;
    let root = std::fs::canonicalize(root).context("Cannot resolve Treer machine root")?;
    let cwd = std::fs::canonicalize(cwd).context("Cannot resolve attach working directory")?;
    let relative = cwd
        .strip_prefix(root)
        .context("Attach working directory is outside the Treer machine root")?;
    Ok(if relative.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative.to_owned()
    })
}

impl Session {
    fn startup_args(&self, executable: PathBuf) -> Vec<OsString> {
        let mut args = text_args(&["agent", "startup", "set", "--cwd"]);
        args.push(self.cwd.clone().into_os_string());
        args.push("--".into());
        args.push(executable.into_os_string());
        args.extend(text_args(&[
            "attach",
            "--thread",
            self.hello.thread_id.as_deref().unwrap(),
            "--hub",
            &self.hub,
            "--hub-token-file",
        ]));
        args.push(self.token_file.clone().into_os_string());
        args
    }

    async fn serve(&mut self, contract: &Contract) -> Result<()> {
        let mut delay = Duration::from_secs(1);
        loop {
            // Re-read after restart in case the hub rotated the file. A not-yet-created
            // token is normal when Treer starts the App and Agent concurrently.
            let token = match tokio::fs::read_to_string(&self.token_file).await {
                Ok(token) => token,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    println!("Pockymoe attach: waiting for hub token.");
                    retry(&mut delay).await;
                    continue;
                }
                Err(_) => bail!("Cannot read hub token file"),
            };
            let token = token.trim();
            ensure!(
                !token.is_empty() && token.len() <= 4096,
                "Invalid hub token file"
            );
            let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| anyhow::anyhow!("Invalid hub token file"))?;
            auth.set_sensitive(true);
            let mut request = format!(
                "ws://{}{}",
                self.hub.strip_prefix("http://").unwrap(),
                TREER_ATTACH_PATH
            )
            .into_client_request()
            .map_err(|_| anyhow::anyhow!("Invalid attach URL"))?;
            request.headers_mut().insert("authorization", auth);
            let connected = time::timeout(Duration::from_secs(10), connect_async(request)).await;
            let mut socket = match connected {
                Ok(Ok((socket, _))) => socket,
                Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response)))
                    if matches!(response.status().as_u16(), 401 | 403 | 404) =>
                {
                    bail!("Hub refused attach authentication or does not support attach")
                }
                _ => {
                    println!("Pockymoe attach: hub unavailable; reconnecting.");
                    retry(&mut delay).await;
                    continue;
                }
            };
            let hello = serde_json::to_string(&self.hello).context("Cannot encode attach hello")?;
            if socket.send(Message::Text(hello.into())).await.is_err() {
                retry(&mut delay).await;
                continue;
            }
            println!(
                "Pockymoe attach: waiting for hub reply (thread setup may take a few minutes)."
            );
            let mut progress = time::interval_at(
                time::Instant::now() + Duration::from_secs(30),
                Duration::from_secs(30),
            );
            progress.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
            let reply = async {
                loop {
                    let frame = tokio::select! {
                        frame = socket.next() => frame,
                        _ = progress.tick() => {
                            println!("Pockymoe attach: still waiting for hub reply.");
                            continue;
                        }
                    };
                    let Some(frame) = frame else {
                        return Ok(None);
                    };
                    match frame {
                        Ok(Message::Text(text)) => return Ok(Some(text)),
                        Ok(Message::Ping(payload)) => {
                            socket.send(Message::Pong(payload)).await?;
                        }
                        Ok(Message::Close(_)) => return Ok(None),
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }
            }
            .await;
            let text = match reply {
                Ok(Some(text)) => text,
                _ => {
                    retry(&mut delay).await;
                    continue;
                }
            };
            let reply: AttachReply = serde_json::from_str(&text)
                .map_err(|_| anyhow::anyhow!("Hub returned an invalid attach reply"))?;
            let (thread_id, created, port, ui_path, capabilities) = match reply {
                AttachReply::Error { code, message } => {
                    let secrets = [
                        Some(token),
                        self.hello.treer.workload_credential.as_deref(),
                        self.hello.treer.agent_server_url.as_deref(),
                        self.hello.treer.workspace_id.as_deref(),
                        self.hello.treer.server_id.as_deref(),
                    ];
                    bail!(
                        "Hub rejected attach binding [{}]: {}",
                        diagnostic(&code, &secrets, 96),
                        diagnostic(&message, &secrets, 512)
                    );
                }
                AttachReply::Bound {
                    thread_id,
                    created,
                    interface_port,
                    ui_path,
                    capabilities,
                } => (thread_id, created, interface_port, ui_path, capabilities),
            };
            ensure!(
                !thread_id.is_empty()
                    && port != 0
                    && ui_path.starts_with('/')
                    && !ui_path.starts_with("//"),
                "Hub returned invalid interface configuration"
            );
            ensure!(
                self.hello
                    .thread_id
                    .as_ref()
                    .is_none_or(|id| id == &thread_id),
                "Hub returned a different thread binding"
            );
            ensure!(
                capabilities
                    .iter()
                    .all(|c| contract.capabilities.contains(c)),
                "Hub AIS capabilities are unknown to this Treer Controller"
            );
            self.hello.thread_id = Some(thread_id);
            self.hello.create = None;
            self.startup_pending |= created;
            if self.startup_pending {
                let executable = match &self.startup_executable {
                    Some(path) => path.clone(),
                    None => stable_executable()?,
                };
                let args = self.startup_args(executable);
                if treer_command(&self.treer, &args).await.is_err() {
                    println!("Pockymoe attach: startup save unavailable; retrying.");
                    retry(&mut delay).await;
                    continue;
                }
                self.startup_pending = false;
            }
            let mut args = text_args(&[
                "interface",
                "register",
                "--port",
                &port.to_string(),
                "--instance-id",
                &self.hello.instance_id,
            ]);
            for capability in &capabilities {
                args.extend(text_args(&["--capability", capability]));
            }
            args.extend(text_args(&["--ui-path", &ui_path]));
            if treer_command(&self.treer, &args).await.is_err() {
                println!("Pockymoe attach: interface registration unavailable; retrying.");
                retry(&mut delay).await;
                continue;
            }
            println!("Pockymoe attach: thread connected; Treer interface ready.");
            delay = Duration::from_secs(1);
            let mut heartbeat = time::interval(Duration::from_secs(15));
            let mut last_seen = time::Instant::now();
            loop {
                tokio::select! {
                    frame = socket.next() => match frame {
                        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                        Some(Ok(Message::Ping(payload))) => {
                            last_seen = time::Instant::now();
                            if socket.send(Message::Pong(payload)).await.is_err() { break; }
                        }
                        Some(Ok(_)) => last_seen = time::Instant::now(),
                    },
                    _ = heartbeat.tick() => {
                        if last_seen.elapsed() >= Duration::from_secs(45)
                            || socket.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
                    }
                }
            }
            println!("Pockymoe attach: hub disconnected; reconnecting.");
            retry(&mut delay).await;
        }
    }
}

// Remove terminal controls before redacting, so controls cannot split a known secret.
fn diagnostic(value: &str, secrets: &[Option<&str>], limit: usize) -> String {
    fn clean(value: &str) -> String {
        value
            .chars()
            .filter(|c| {
                !c.is_control()
                    && !matches!(*c,
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            })
            .collect()
    }
    let mut value = clean(value);
    let mut secrets: Vec<_> = secrets
        .iter()
        .flatten()
        .map(|s| clean(s))
        .filter(|s| !s.is_empty())
        .collect();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for secret in secrets {
        value = value.replace(&secret, "[redacted]");
    }
    let truncated = value.chars().count() > limit;
    let mut value: String = value.chars().take(limit).collect();
    if truncated {
        value.push('…');
    }
    value
}

async fn retry(delay: &mut Duration) {
    time::sleep(*delay).await;
    *delay = (*delay * 2).min(Duration::from_secs(30));
}

pub async fn run(options: Options, connection_options: crate::threads::Connection) -> Result<()> {
    let agent_id = std::env::var("TREER_AGENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .context("pockymoe attach requires TREER_AGENT_ID (launch it as a Treer Agent)")?;
    let cwd = std::env::current_dir().context("Cannot read attach working directory")?;
    let (hub, token_file) = connection(&options, connection_options, &cwd)?;
    // Fail before creating a thread if its restore command cannot be made stable.
    let startup_executable = if options.thread.is_none() {
        Some(stable_executable()?)
    } else {
        None
    };
    let treer = std::env::var_os("TREER_BIN").unwrap_or_else(|| "treer".into());
    let bytes = treer_command(&treer, &text_args(&["interface", "contract"])).await?;
    let contract = Contract::parse(&bytes)?;
    let restore_cwd = if options.thread.is_none() {
        // Host can start the process before Proxy discovery publishes its Agent.
        // Keep waiting: Treer does not restart an ordinary Agent that exits.
        let started = time::Instant::now();
        let mut delay = Duration::from_millis(100);
        let mut reported = false;
        let identity = loop {
            if let Ok(identity) = treer_command(&treer, &text_args(&["whoami"])).await {
                break identity;
            }
            if !reported && started.elapsed() >= Duration::from_secs(10) {
                println!("Pockymoe attach: waiting for Treer to publish this Agent's identity.");
                reported = true;
            }
            time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(30));
        };
        startup_cwd(&cwd, &identity)?
    } else {
        cwd.clone()
    };
    println!(
        "Pockymoe attach: AIS v1 contract verified ({} capabilities, {} features).",
        contract.capabilities.len(),
        contract.features.len()
    );
    let create = options.provider.map(|provider| AttachCreateThread {
        cwd: cwd.to_string_lossy().into_owned(),
        provider,
        agent: options.agent,
        model: options.model,
        reasoning_effort: options.reasoning_effort,
        title: options.title,
    });
    let mut session = Session {
        hello: AttachHello {
            protocol_version: TREER_ATTACH_PROTOCOL,
            agent_id,
            instance_id: uuid::Uuid::new_v4().to_string(),
            thread_id: options.thread,
            create,
            treer: AttachTreerIdentity {
                agent_server_url: std::env::var("TREER_AGENT_SERVER_URL").ok(),
                workload_credential: std::env::var("TREER_WORKLOAD_CREDENTIAL").ok(),
                workspace_id: std::env::var("TREER_WORKSPACE_ID").ok(),
                server_id: std::env::var("TREER_SERVER_ID").ok(),
            },
        },
        hub,
        token_file,
        cwd: restore_cwd,
        treer,
        startup_pending: false,
        startup_executable,
    };
    session.serve(&contract).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(hub: Option<String>) -> Options {
        Options {
            thread: Some("t".into()),
            provider: None,
            agent: None,
            model: None,
            reasoning_effort: None,
            title: None,
            hub,
            hub_token_file: None,
        }
    }

    fn discovery(path: PathBuf) -> crate::threads::Connection {
        crate::threads::Connection {
            url: None,
            token: Some("ordinary-cli-token-must-not-be-used".into()),
            from: None,
            cli_config: Some(path),
        }
    }

    #[test]
    fn diagnostics_strip_controls_redact_before_truncating_and_cap_length() {
        assert_eq!(
            diagnostic(
                "bad\n\x1b\u{202e}synthetic-secret",
                &[Some("synthetic-secret")],
                96
            ),
            "bad[redacted]"
        );
        assert_eq!(
            diagnostic("synthetic-secret", &[Some("synthetic-secret")], 5),
            "[reda…"
        );
        let text = diagnostic(&"é".repeat(1000), &[], 512);
        assert_eq!(text.chars().count(), 513);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn token_default_follows_cli_discovery_directory() {
        let cwd = std::env::temp_dir().join(format!("attach-discovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).unwrap();
        let path = cwd.join("hub.cli.json");
        std::fs::write(
            &path,
            r#"{"url":"http://127.0.0.1:1234","token":"ordinary-token"}"#,
        )
        .unwrap();
        let (hub, token) = connection(&options(None), discovery(path), &cwd).unwrap();
        assert_eq!(hub, "http://127.0.0.1:1234");
        assert_eq!(token, cwd.join(TREER_ATTACH_TOKEN_FILE));
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn remote_or_credential_bearing_hub_urls_fail_without_echo() {
        for hub in [
            "http://example.com",
            "https://127.0.0.1",
            "http://secret@127.0.0.1",
            "http://127.0.0.1?token=secret",
            "http://127.0.0.1/api",
        ] {
            let error = connection(
                &options(Some(hub.into())),
                discovery("missing.cli.json".into()),
                Path::new("/tmp"),
            )
            .unwrap_err()
            .to_string();
            assert!(!error.contains("secret"));
        }
    }

    #[test]
    fn missing_contract_fields_are_unknown() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({"protocol": TREER_AGENT_INTERFACE_PROTOCOL}),
            serde_json::json!({"protocol": TREER_AGENT_INTERFACE_PROTOCOL, "features": [], "capabilities": []}),
        ] {
            assert!(Contract::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }

    #[test]
    fn startup_uses_bound_id_and_explicit_nonsecret_connection() {
        let session = Session {
            hello: AttachHello {
                protocol_version: 1,
                agent_id: "a".into(),
                instance_id: "i".into(),
                thread_id: Some("bound".into()),
                create: None,
                treer: AttachTreerIdentity {
                    agent_server_url: None,
                    workload_credential: Some("never-save-me".into()),
                    workspace_id: None,
                    server_id: None,
                },
            },
            hub: "http://127.0.0.1:8787".into(),
            token_file: "/data/token".into(),
            cwd: "project".into(),
            treer: "treer".into(),
            startup_pending: true,
            startup_executable: None,
        };
        let args = session.startup_args("/bin/pockymoe".into());
        let args: Vec<_> = args.iter().map(|v| v.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "agent",
                "startup",
                "set",
                "--cwd",
                "project",
                "--",
                "/bin/pockymoe",
                "attach",
                "--thread",
                "bound",
                "--hub",
                "http://127.0.0.1:8787",
                "--hub-token-file",
                "/data/token"
            ]
        );
    }

    #[test]
    fn startup_directory_is_machine_relative_and_rejects_outside_root() {
        let root = std::env::temp_dir().join(format!("attach-cwd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("project")).unwrap();
        let identity = serde_json::to_vec(&serde_json::json!({"machine":{"root":root}})).unwrap();
        assert_eq!(
            startup_cwd(&root.join("project"), &identity).unwrap(),
            PathBuf::from("project")
        );
        assert_eq!(startup_cwd(&root, &identity).unwrap(), PathBuf::from("."));
        assert!(startup_cwd(root.parent().unwrap(), &identity).is_err());
        assert!(startup_cwd(&root, b"{}").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
