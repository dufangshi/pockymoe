mod attach;
mod automations;
mod legacy_env;
mod threads;
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::io::Read;

#[derive(Parser)]
#[command(name = "pockymoe", version, about = "Pockymoe supervisor and relay")]
struct Cli {
    /// Startup subsystem policy (independent of local/server/relay authentication).
    #[arg(long, global = true)]
    host_mode: Option<pockymoe_runtime::host_mode::HostMode>,
    #[command(flatten)]
    connection: threads::Connection,
    #[command(subcommand)]
    command: Commands,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_mode_flag_parses_for_all_startup_commands() {
        for args in [
            vec!["supervisor", "--host-mode", "hosted"],
            vec!["start", "--host-mode", "standalone"],
            vec!["relay-supervisor", "--host-mode", "hosted"],
            vec![
                "device-run",
                "--config",
                "device.json",
                "--host-mode",
                "hosted",
            ],
        ] {
            let cli = Cli::try_parse_from(std::iter::once("pockymoe").chain(args)).unwrap();
            assert!(cli.host_mode.is_some());
        }
        assert!(Cli::try_parse_from(["pockymoe", "start", "--host-mode", "relay"]).is_err());
        assert!(Cli::try_parse_from(["pockymoe", "start"])
            .unwrap()
            .host_mode
            .is_none());
    }

    #[test]
    fn hosted_database_binding_command_returns_stable_error() {
        let root = std::env::temp_dir().join(format!("host-mode-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let database = root.join("test.sqlite");
        assert!(require_database_account_binding(&database).is_ok());
        std::fs::write(
            database.with_extension("cli.json"),
            r#"{"hostCapabilities":{"accountBinding":false}}"#,
        )
        .unwrap();
        assert!(require_database_account_binding(&database)
            .unwrap_err()
            .to_string()
            .starts_with("feature_disabled_by_host_mode:"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bind_and_setup_enrollment_never_accept_secret_arguments() {
        for prefix in [
            vec!["pockymoe", "bind"],
            vec![
                "pockymoe",
                "setup",
                "--relay",
                "https://relay.test",
                "--code",
                "synthetic",
            ],
        ] {
            assert!(
                Cli::try_parse_from(prefix.iter().copied().chain(["--account-key", "public"]))
                    .is_ok()
            );
            assert!(Cli::try_parse_from(prefix.iter().copied().chain([
                "--account-key",
                "public",
                "--enroll-stdin"
            ]))
            .is_ok());
            assert!(Cli::try_parse_from(prefix.iter().copied().chain([
                "--account-key",
                "public",
                "--enroll",
                "synthetic-secret"
            ]))
            .is_err());
            assert!(Cli::try_parse_from(prefix.iter().copied().chain(["--enroll-stdin"])).is_err());
        }
        for args in [vec!["bind", "--status"], vec!["bind", "--reset", "--yes"]] {
            assert!(Cli::try_parse_from(std::iter::once("pockymoe").chain(args)).is_ok());
        }
        for args in [
            vec!["pair"],
            vec!["bind"],
            vec!["bind", "--yes"],
            vec!["bind", "--status", "--reset"],
        ] {
            assert!(Cli::try_parse_from(std::iter::once("pockymoe").chain(args)).is_err());
        }
    }
    #[test]
    fn enrollment_sources_require_a_key_and_do_not_echo_invalid_input() {
        let mut stdin = &b"synthetic-secret\n"[..];
        assert_eq!(
            enrollment_secret(true, true, None, &mut stdin)
                .unwrap()
                .as_deref(),
            Some("synthetic-secret")
        );
        assert_eq!(
            enrollment_secret(true, false, Some("synthetic-secret".into()), &mut &b""[..])
                .unwrap()
                .as_deref(),
            Some("synthetic-secret")
        );
        for (account, stdin, env) in [
            (true, false, None),
            (false, false, Some("synthetic-secret".into())),
            (true, true, Some("synthetic-secret".into())),
        ] {
            let error = enrollment_secret(account, stdin, env, &mut &b""[..])
                .unwrap_err()
                .to_string();
            assert!(!error.contains("synthetic-secret"));
        }
        assert!(enrollment_secret(true, true, None, &mut &vec![b'a'; 1025][..]).is_err());
    }

    #[test]
    fn preview_cli_reserves_before_startup_and_rejects_invalid_ports() {
        for port in ["0", "65536", "-1", "abc"] {
            assert!(
                Cli::try_parse_from(["pockymoe", "preview", "create", "--port", port]).is_err()
            );
        }
        for args in [
            vec!["preview", "create", "--port", "4013", "--path", "/app?q=1"],
            vec!["preview", "list"],
            vec!["preview", "check", "4013", "--websocket-path", "/ws"],
            vec!["preview", "stop", "4013"],
        ] {
            assert!(Cli::try_parse_from(std::iter::once("pockymoe").chain(args)).is_ok());
        }
    }

    #[test]
    fn peer_send_accepts_explicit_interrupt_reason_and_status_topic() {
        let cli = Cli::try_parse_from([
            "pockymoe",
            "thread",
            "send",
            "worker",
            "--delivery",
            "direct",
            "--kind",
            "task",
            "--interrupt-reason",
            "Invalid inputs would waste the active calculation",
            "--text",
            "stop",
        ])
        .unwrap();
        let Commands::Thread {
            command: threads::ThreadCommand::Send { body, .. },
        } = cli.command
        else {
            panic!("expected send")
        };
        assert_eq!(
            body.interrupt_reason.as_deref(),
            Some("Invalid inputs would waste the active calculation")
        );
        let cli = Cli::try_parse_from([
            "pockymoe",
            "thread",
            "send",
            "worker",
            "--kind",
            "status",
            "--topic-key",
            "simulation",
            "--text",
            "20 of 100",
        ])
        .unwrap();
        let Commands::Thread {
            command: threads::ThreadCommand::Send { body, .. },
        } = cli.command
        else {
            panic!("expected send")
        };
        assert_eq!(body.topic_key.as_deref(), Some("simulation"));
    }

    #[test]
    fn inbox_listing_supports_repeatable_sender_and_kind_filters() {
        let cli = Cli::try_parse_from([
            "pockymoe",
            "inbox",
            "list",
            "--from-thread",
            "producer",
            "--from-thread",
            "reviewer",
            "--kind",
            "result",
            "--kind",
            "question",
        ])
        .unwrap();
        let Commands::Inbox(args) = cli.command else {
            panic!("expected inbox")
        };
        let Some(threads::InboxCommand::List {
            from_threads, kind, ..
        }) = args.command
        else {
            panic!("expected list")
        };
        assert_eq!(from_threads, ["producer", "reviewer"]);
        assert_eq!(kind, ["result", "question"]);
    }

    #[test]
    fn child_delete_is_a_single_target_command_without_force_or_recursive_flags() {
        let cli = Cli::try_parse_from([
            "pockymoe",
            "thread",
            "delete",
            "00000000-0000-0000-0000-000000000001",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Commands::Thread {
                command: threads::ThreadCommand::Delete { .. }
            }
        ));
        for extra in [
            "--force",
            "--recursive",
            "00000000-0000-0000-0000-000000000002",
        ] {
            assert!(Cli::try_parse_from([
                "pockymoe",
                "thread",
                "delete",
                "00000000-0000-0000-0000-000000000001",
                extra
            ])
            .is_err());
        }
    }

    #[test]
    fn peer_cli_parses_device_file_outbox_and_remote_thread_commands() {
        for args in [
            vec!["device", "list"],
            vec!["device", "access"],
            vec!["device", "access", "on"],
            vec!["device", "access", "off"],
            vec!["device", "trust", "Treer", "--reset"],
            vec!["device", "workspaces", "Treer"],
            vec!["thread", "list", "--device", "Treer", "--workspace", "ws"],
            vec!["thread", "backends", "--device", "Treer"],
            vec!["thread", "models", "--device", "Treer", "--workspace", "ws"],
            vec!["thread", "create", "--device", "Treer", "--workspace", "ws"],
            vec![
                "thread",
                "show",
                "Treer/00000000-0000-0000-0000-000000000001",
            ],
            vec![
                "thread",
                "status",
                "00000000-0000-0000-0000-000000000001",
                "--device",
                "Treer",
            ],
            vec![
                "transcript",
                "Treer/00000000-0000-0000-0000-000000000001",
                "--limit",
                "1",
            ],
            vec!["fs", "ls", "Treer", "--workspace", "ws"],
            vec!["fs", "ls", "Treer", "--workspace", "ws", "src"],
            vec![
                "fs",
                "get",
                "Treer",
                "--workspace",
                "ws",
                "src/file",
                "--out",
                "copy",
            ],
            vec!["outbox"],
        ] {
            let argv = std::iter::once("pockymoe").chain(args.iter().copied());
            assert!(Cli::try_parse_from(argv).is_ok(), "{args:?}");
        }
        let cli = Cli::try_parse_from([
            "pockymoe",
            "thread",
            "send",
            "Treer/00000000-0000-0000-0000-000000000001",
            "--attach",
            "file",
            "--attach",
            "dir",
            "--text",
            "review",
        ])
        .unwrap();
        let Commands::Thread {
            command: threads::ThreadCommand::Send { attach, .. },
        } = cli.command
        else {
            panic!("expected send")
        };
        assert_eq!(
            attach,
            vec![
                std::path::PathBuf::from("file"),
                std::path::PathBuf::from("dir")
            ]
        );
    }

    #[test]
    fn peer_cli_rejects_invalid_access_trust_and_missing_file_arguments() {
        for args in [
            vec!["device", "access", "yes"],
            vec!["device", "trust", "Treer"],
            vec!["fs", "ls", "Treer"],
            vec!["fs", "get", "Treer", "--workspace", "ws"],
            vec!["thread", "wait", "thread", "--device", "Treer"],
            vec!["inbox", "--device", "Treer"],
        ] {
            assert!(
                Cli::try_parse_from(std::iter::once("pockymoe").chain(args.iter().copied()))
                    .is_err(),
                "{args:?}"
            );
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Bind one Treer Agent to a hub thread and keep its interface registered.
    Attach(attach::Options),
    /// Reserve, inspect and diagnose private device web previews.
    #[command(after_long_help = r#"More: pockymoe guide preview"#)]
    Preview {
        #[command(subcommand)]
        command: threads::PreviewCommand,
    },
    /// Durable device hooks: trigger -> typed condition -> prompt, inbox or script.
    #[command(after_long_help = r#"More: pockymoe guide automation"#)]
    #[command(alias = "hooks", alias = "hook")]
    Automation {
        #[command(subcommand)]
        command: automations::AutomationCommand,
    },
    /// Controlled command execution with persistent exit/output and completion hooks.
    Command {
        #[command(subcommand)]
        command: automations::CommandCommand,
    },
    /// Create, contact, and inspect local threads or same-owner device peers.
    #[command(
        after_long_help = r#"Rules for delivery and collaboration: pockymoe skill
Details: pockymoe guide delegate | messaging"#
    )]
    Thread {
        #[command(subcommand)]
        command: threads::ThreadCommand,
    },
    /// Read recent conversation text, then expand one turn or item.
    #[command(after_long_help = r#"More: pockymoe guide transcript"#)]
    Transcript(threads::Transcript),
    /// Discover same-owner devices and manage this device's peer access/trust.
    #[command(after_long_help = r#"More: pockymoe guide devices"#)]
    Device {
        #[command(subcommand)]
        command: threads::DeviceCommand,
    },
    /// Read workspace files on another device.
    Fs {
        #[command(subcommand)]
        command: threads::FsCommand,
    },
    /// List cross-device messages waiting for delivery from this device.
    Outbox,
    /// Read and acknowledge persistent peer messages without starting turns.
    #[command(after_long_help = r#"Rules:
  - Read at natural checkpoints: when the turn notice names something, before work that
    depends on a peer, after a build or batch. Not after every tool call.
  - To wait, use `inbox wait --kind result --kind question` instead of polling.
  - Ack only what you handled. Do not reply "received"; answer questions before acking.

More: pockymoe guide inbox"#)]
    Inbox(threads::Inbox),
    /// Shared task board for the threads of one lineage.
    #[command(
        after_long_help = r#"Delegates loop: `task claim --wait`, do it, `task done --result`, until `finished: true`.

More: pockymoe guide tasks"#
    )]
    Task {
        #[command(subcommand)]
        command: threads::TaskCommand,
    },
    /// Print the running Supervisor's interaction skill, or the bundled guide offline.
    Skill,
    /// Print a detailed interaction guide; without a topic, list the topics.
    Guide {
        /// Topic name, for example `delegate`, `messaging` or `preview`.
        topic: Option<String>,
    },

    /// Run the local supervisor HTTP API.
    Supervisor,
    /// Alias for supervisor (local mode).
    Start,
    /// Print a simple status payload.
    Status,
    /// Run the public relay server.
    Relay,
    /// Inspect or apply a non-destructive relay database migration.
    RelayMigrate {
        /// Relay data directory containing relay-store.sqlite or relay.sqlite.
        #[arg(
            long,
            env = "POCKYMOE_RELAY_DATA_DIR",
            default_value = ".local/relay-server"
        )]
        data_dir: std::path::PathBuf,
        /// Print the migration plan and row counts without writing any files.
        #[arg(long)]
        dry_run: bool,
        /// Proceed while preserving data for relay features not yet implemented in Rust.
        #[arg(long, conflicts_with = "dry_run")]
        allow_unsupported_data: bool,
    },
    /// Run a supervisor that connects out to a relay.
    RelaySupervisor,
    /// Show the device encryption fingerprint for independent browser verification.
    RelayFingerprint {
        /// Supervisor database path; defaults to relay-mode configuration.
        #[arg(long, env = "POCKYMOE_DATABASE_PATH")]
        database: Option<std::path::PathBuf>,
    },
    /// Bind this device to an account key using the local database.
    Bind {
        #[arg(long, env = "POCKYMOE_DATABASE_PATH")]
        database: Option<std::path::PathBuf>,
        #[arg(long, required_unless_present_any = ["reset", "status"], conflicts_with = "status")]
        account_key: Option<String>,
        /// Read the enrollment secret from stdin instead of POCKYMOE_ENROLL_SECRET.
        #[arg(long, requires = "account_key", conflicts_with = "status")]
        enroll_stdin: bool,
        #[arg(long, requires = "account_key")]
        web_origin: Option<String>,
        /// Delete all account trust and owner previews, and restore relay trust.
        #[arg(long, conflicts_with = "status")]
        reset: bool,
        #[arg(long, requires = "reset")]
        yes: bool,
        /// Print binding state and the account fingerprint.
        #[arg(long)]
        status: bool,
    },
    /// Install this device from the official GitHub runtime release.
    Setup {
        #[arg(long)]
        relay: String,
        /// Trusted Web origin saved locally for account enrollment.
        #[arg(long, env = "POCKYMOE_WEB_ORIGIN")]
        web_origin: Option<String>,
        #[arg(long, env = "POCKYMOE_ACCOUNT_KEY")]
        account_key: Option<String>,
        /// Read the enrollment secret from stdin instead of POCKYMOE_ENROLL_SECRET.
        #[arg(long, requires = "account_key")]
        enroll_stdin: bool,
        #[arg(long, conflicts_with = "code", required_unless_present = "code")]
        token: Option<String>,
        #[arg(long, conflicts_with = "token")]
        code: Option<String>,
        #[arg(long, default_value_t = 8787, value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
        /// If the existing device database cannot be upgraded, move it aside
        /// (the old file is kept) instead of asking.
        #[arg(long)]
        reset_database: bool,
    },
    /// Run a native device using its saved private configuration.
    DeviceRun {
        #[arg(long)]
        config: std::path::PathBuf,
    },
    #[command(hide = true)]
    RuntimeMaintenance { action: String },
    #[command(hide = true)]
    InternalUpdate {
        #[arg(long)]
        plan: std::path::PathBuf,
    },
    /// Print version.
    Version,
}

fn main() -> Result<()> {
    legacy_env::adopt();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

fn require_database_account_binding(database: &std::path::Path) -> Result<()> {
    let discovery = std::fs::read(database.with_extension("cli.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let enabled = discovery
        .as_ref()
        .and_then(|value| value["hostCapabilities"]["accountBinding"].as_bool())
        .unwrap_or(true);
    pockymoe_runtime::host_mode::require_feature(enabled, "accountBinding")
}

fn device_identity_fingerprint(database: &std::path::Path) -> Result<String> {
    Ok(format!(
        "SHA-256 {}",
        pockymoe_supervisor::relay_device_fingerprint(database)?
    ))
}

fn enrollment_secret(
    account: bool,
    stdin: bool,
    environment: Option<String>,
    reader: &mut impl std::io::Read,
) -> Result<Option<String>> {
    anyhow::ensure!(
        !(stdin && environment.is_some()),
        "Choose stdin or POCKYMOE_ENROLL_SECRET for enrollment"
    );
    let secret = if stdin {
        let mut bytes = Vec::new();
        reader.take(1025).read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() <= 1024, "Invalid enrollment input");
        Some(
            String::from_utf8(bytes)
                .map_err(|_| anyhow::anyhow!("Invalid enrollment input"))?
                .trim()
                .to_owned(),
        )
    } else {
        environment
    };
    anyhow::ensure!(
        account == secret.is_some(),
        "Provide --account-key with POCKYMOE_ENROLL_SECRET or --enroll-stdin"
    );
    Ok(secret)
}

async fn run() -> Result<()> {
    // Remove the secret before any child or Supervisor can inherit the environment.
    let enrollment_env = std::env::var_os("POCKYMOE_ENROLL_SECRET");
    std::env::remove_var("POCKYMOE_ENROLL_SECRET");
    std::env::remove_var("POCKYMOE_ENROLL");
    let enrollment_env = enrollment_env
        .map(|v| {
            v.into_string()
                .map_err(|_| anyhow::anyhow!("Invalid enrollment input"))
        })
        .transpose()?;
    if std::env::args().nth(1).as_deref() == Some("app-server")
        && std::env::var_os("POCKYMOE_APP_SERVER_BRIDGE").is_some()
    {
        return pockymoe_runtime::acp::run_codex_app_server_bridge().await;
    }
    let cli = Cli::parse();
    // WebSocket trace diagnostics may contain the attach identity payload.
    let filter = if matches!(cli.command, Commands::Attach(_)) {
        tracing_subscriber::EnvFilter::new("off")
    } else {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info,rusqlite=warn,hyper=warn".into())
    };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();
    if let Some(mode) = cli.host_mode {
        std::env::set_var("POCKYMOE_HOST_MODE", mode.to_string());
    }
    if matches!(&cli.command, Commands::Bind { .. } | Commands::Setup { .. }) {
        let caps = pockymoe_runtime::host_mode::HostMode::from_env()?.capabilities();
        pockymoe_runtime::host_mode::require_feature(caps.account_binding, "accountBinding")?;
    }
    match cli.command {
        Commands::Attach(options) => attach::run(options, cli.connection).await?,
        Commands::Preview { command } => {
            let value = threads::Client::new(cli.connection)?
                .preview(command)
                .await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        Commands::Automation { command } => println!(
            "{}",
            serde_json::to_string_pretty(
                &threads::Client::new(cli.connection)?
                    .automation(command)
                    .await?
            )?
        ),
        Commands::Command { command } => {
            let value = threads::Client::new(cli.connection)?
                .command_execution(command)
                .await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
            if matches!(
                value["state"].as_str(),
                Some("failed" | "timedOut" | "uncertain")
            ) {
                std::process::exit(
                    value["exitCode"]
                        .as_i64()
                        .filter(|c| (1..=255).contains(c))
                        .unwrap_or(1) as i32,
                );
            }
        }
        Commands::Guide { topic } => {
            let guides = pockymoe_protocol::THREAD_INTERACTION_GUIDES;
            match topic.as_deref() {
                None => {
                    println!("Usage: pockymoe guide TOPIC\n\nTopics:");
                    for (name, summary, _) in guides {
                        println!("  {name:<11} {summary}");
                    }
                }
                Some(name) => match guides.iter().find(|(topic, _, _)| *topic == name) {
                    Some((_, _, text)) => print!("{text}"),
                    None => {
                        let names: Vec<_> = guides.iter().map(|(name, _, _)| *name).collect();
                        eprintln!("Unknown guide {name:?}. Topics: {}", names.join(", "));
                        std::process::exit(2);
                    }
                },
            }
        }
        Commands::Skill => {
            let text = match threads::Client::new(cli.connection) {
                Ok(client) => match client.skill().await {
                    Ok(text) => text,
                    Err(_) => {
                        eprintln!("Supervisor skill unavailable; using the CLI's bundled guide.");
                        pockymoe_protocol::THREAD_INTERACTION_SKILL.to_owned()
                    }
                },
                Err(_) => pockymoe_protocol::THREAD_INTERACTION_SKILL.to_owned(),
            };
            println!("{text}");
        }
        Commands::Thread { command } => {
            let value = threads::Client::new(cli.connection)?
                .thread(command)
                .await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
            if value["failed"].as_array().is_some_and(|f| !f.is_empty()) {
                std::process::exit(1);
            }
        }
        Commands::Task { command } => {
            let value = threads::Client::new(cli.connection)?.task(command).await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        Commands::Inbox(args) => {
            let value = threads::Client::new(cli.connection)?.inbox(args).await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        Commands::Transcript(query) => {
            let value = threads::Client::new(cli.connection)?
                .transcript(query)
                .await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        Commands::Device { command } => {
            let value = threads::Client::new(cli.connection)?
                .device(command)
                .await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        Commands::Fs { command } => {
            let value = threads::Client::new(cli.connection)?.fs(command).await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        Commands::Outbox => {
            let value = threads::Client::new(cli.connection)?.outbox().await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }

        Commands::Supervisor | Commands::Start => {
            let state = pockymoe_runtime::boot().await?;
            pockymoe_supervisor::serve(state).await?;
        }
        Commands::Status => {
            let port = std::env::var("PORT").unwrap_or_else(|_| "8787".into());
            match reqwest_status(&port).await {
                Ok(body) => println!("{body}"),
                Err(_) => println!("{{\"status\":\"stopped\",\"port\":{port}}}"),
            }
        }
        Commands::Relay => {
            pockymoe_relay::serve().await?;
        }
        Commands::RelayMigrate {
            data_dir,
            dry_run,
            allow_unsupported_data,
        } => {
            let report = if dry_run {
                pockymoe_relay::inspect_relay_migration(&data_dir)?
            } else {
                pockymoe_relay::migrate_relay_data_dir_with_options(
                    &data_dir,
                    pockymoe_relay::RelayMigrationOptions {
                        allow_unsupported_data,
                    },
                )?
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Commands::RelaySupervisor => {
            std::env::set_var("POCKYMOE_MODE", "relay");
            let state = pockymoe_runtime::boot().await?;
            pockymoe_supervisor::serve(state).await?;
        }
        Commands::RelayFingerprint { database } => {
            std::env::set_var("POCKYMOE_MODE", "relay");
            let database = match database {
                Some(database) => database,
                None => pockymoe_runtime::RuntimeConfig::from_env()?.database_url,
            };
            println!("{}", device_identity_fingerprint(&database)?);
        }
        Commands::Bind {
            database,
            account_key,
            enroll_stdin,
            web_origin,
            reset,
            yes,
            status,
        } => {
            std::env::set_var("POCKYMOE_MODE", "relay");
            let config = pockymoe_runtime::RuntimeConfig::from_env()?;
            let database = database.unwrap_or(config.database_url);
            require_database_account_binding(&database)?;
            let enroll = enrollment_secret(
                account_key.is_some(),
                enroll_stdin,
                enrollment_env,
                &mut std::io::stdin().lock(),
            )?;
            let web_origin = web_origin
                .as_deref()
                .or(config.web_origin.as_deref())
                .map(pockymoe_supervisor::validate_web_origin)
                .transpose()?;
            if let (Some(account), Some(secret)) = (&account_key, &enroll) {
                pockymoe_supervisor::validate_binding(account, secret)?;
            }
            if reset {
                if !yes {
                    use std::io::Write;
                    eprint!("Delete account trust, browser registrations, certificates, grants and owner previews, and restore relay trust? [y/N] ");
                    std::io::stderr().flush()?;
                    let mut answer = String::new();
                    std::io::stdin().read_line(&mut answer)?;
                    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                        anyhow::bail!("Binding reset cancelled");
                    }
                }
                pockymoe_supervisor::reset_account_trust(&database)?;
                println!("Account trust reset. The device trusts the relay again.");
            }
            if let (Some(account), Some(secret)) = (account_key, enroll) {
                let result = pockymoe_supervisor::bind_account(
                    &database,
                    &account,
                    &secret,
                    web_origin.as_deref(),
                )?;
                let id = result["accountKeyId"].as_str().unwrap();
                println!("Bound to account {}. Strict mode is on.", &id[..12]);
            } else if status {
                let result = pockymoe_supervisor::binding_status(&database)?;
                println!("{}", device_identity_fingerprint(&database)?);
                if let Some(origin) = result["webOrigin"].as_str() {
                    println!("Web origin (installation hint): {origin}");
                }
                if result["relayStatus"]["code"] == "device_id_mismatch" {
                    println!("Relay assigned a different device id. Reconnecting with backoff; restore the original relay registration or explicitly reset binding.");
                }
                if let Some(id) = result["accountKeyId"].as_str() {
                    println!(
                        "Bound to account {}. Strict mode: {}.",
                        &id[..12],
                        if result["strict"] == true {
                            "on"
                        } else {
                            "off"
                        }
                    );
                } else {
                    println!("Not bound. Strict mode: off.");
                }
            }
        }
        Commands::Setup {
            relay,
            web_origin,
            account_key,
            enroll_stdin,
            token,
            code,
            port,
            reset_database,
        } => {
            let enroll = enrollment_secret(
                account_key.is_some(),
                enroll_stdin,
                enrollment_env,
                &mut std::io::stdin().lock(),
            )?;
            pockymoe_supervisor::distribution::setup(
                pockymoe_supervisor::distribution::SetupOptions {
                    relay,
                    web_origin,
                    account_key,
                    enroll,
                    token,
                    code,
                    port,
                    reset_database,
                },
            )
            .await?;
        }
        Commands::DeviceRun { config } => {
            pockymoe_supervisor::distribution::run_device_with_host_mode(config, cli.host_mode)
                .await?
        }
        Commands::RuntimeMaintenance { action } => println!(
            "{}",
            serde_json::to_string(
                &pockymoe_supervisor::distribution::maintenance_cli(&action).await?
            )?
        ),
        Commands::InternalUpdate { plan } => {
            pockymoe_supervisor::distribution::run_worker(plan).await?
        }
        Commands::Version => {
            println!("{}", env!("CARGO_PKG_VERSION"));
        }
    }
    Ok(())
}

async fn reqwest_status(port: &str) -> Result<String> {
    let response = reqwest::get(format!("http://127.0.0.1:{port}/healthz")).await?;
    if !response.status().is_success() {
        anyhow::bail!("unhealthy: HTTP {}", response.status());
    }
    Ok(response.text().await?)
}
