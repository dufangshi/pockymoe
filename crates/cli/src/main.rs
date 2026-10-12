mod automations;
mod legacy_env;
mod threads;
use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pockymoe", version, about = "Pockymoe supervisor and relay")]
struct Cli {
    #[command(flatten)]
    connection: threads::Connection,
    #[command(subcommand)]
    command: Commands,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_command_accepts_offline_options_and_requires_explicit_reset_for_yes() {
        let cli = Cli::try_parse_from([
            "pockymoe",
            "pair",
            "--database",
            "/tmp/test.sqlite",
            "--web-origin",
            "https://pockymoe.test",
        ])
        .unwrap();
        let Commands::Pair {
            database,
            web_origin,
            reset,
            yes,
        } = cli.command
        else {
            panic!("expected pair");
        };
        assert_eq!(
            database.unwrap(),
            std::path::PathBuf::from("/tmp/test.sqlite")
        );
        assert_eq!(web_origin.as_deref(), Some("https://pockymoe.test"));
        assert!(!reset && !yes);
        assert!(matches!(
            Cli::try_parse_from(["pockymoe", "pair", "--reset", "--yes"])
                .unwrap()
                .command,
            Commands::Pair {
                reset: true,
                yes: true,
                ..
            }
        ));
        assert!(Cli::try_parse_from(["pockymoe", "pair", "--yes"]).is_err());
        assert!(Cli::try_parse_from([
            "pockymoe",
            "pair",
            "--reset",
            "--web-origin",
            "https://pockymoe.test"
        ])
        .is_err());
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
    /// Pair the device with an owner passkey using a local terminal QR code.
    Pair {
        #[arg(long, env = "POCKYMOE_DATABASE_PATH")]
        database: Option<std::path::PathBuf>,
        #[arg(long, conflicts_with = "reset")]
        web_origin: Option<String>,
        /// Remove owner trust and restore relay trust using local database authority.
        #[arg(long)]
        reset: bool,
        /// Confirm the reset without an interactive prompt.
        #[arg(long, requires = "reset")]
        yes: bool,
    },
    /// Install this device from the official GitHub runtime release.
    Setup {
        #[arg(long)]
        relay: String,
        #[arg(long, conflicts_with = "code", required_unless_present = "code")]
        token: Option<String>,
        #[arg(long, conflicts_with = "token")]
        code: Option<String>,
        #[arg(long, default_value_t = 8787, value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
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

async fn run() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("app-server")
        && std::env::var_os("POCKYMOE_APP_SERVER_BRIDGE").is_some()
    {
        return pockymoe_runtime::acp::run_codex_app_server_bridge().await;
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,rusqlite=warn,hyper=warn".into()),
        )
        .init();
    let cli = Cli::parse();
    match cli.command {
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
            let database = database
                .unwrap_or_else(|| pockymoe_runtime::RuntimeConfig::from_env().database_url);
            println!(
                "SHA-256 {}",
                pockymoe_supervisor::relay_device_fingerprint(&database)?
            );
        }
        Commands::Pair {
            database,
            web_origin,
            reset,
            yes,
        } => {
            std::env::set_var("POCKYMOE_MODE", "relay");
            let config = pockymoe_runtime::RuntimeConfig::from_env();
            let database = database.unwrap_or(config.database_url);
            if reset {
                if !yes {
                    use std::io::Write;
                    eprint!("Remove all owner credentials, sessions, certificates and pairing offers, and restore relay trust? [y/N] ");
                    std::io::stderr().flush()?;
                    let mut answer = String::new();
                    std::io::stdin().read_line(&mut answer)?;
                    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                        anyhow::bail!("Pairing reset cancelled");
                    }
                }
                pockymoe_supervisor::reset_owner_trust(&database)?;
                println!("Owner trust reset. The device trusts the relay again.");
            } else {
                let url = pockymoe_supervisor::create_pairing_offer(
                    &database,
                    web_origin.as_deref(),
                    config.relay_server_url.as_deref(),
                )?;
                let qr = qrcode::QrCode::new(url.as_bytes())?;
                println!(
                    "{}",
                    qr.render::<qrcode::render::unicode::Dense1x2>().build()
                );
                println!("{url}");
            }
        }
        Commands::Setup {
            relay,
            token,
            code,
            port,
        } => {
            pockymoe_supervisor::distribution::setup(
                pockymoe_supervisor::distribution::SetupOptions {
                    relay,
                    token,
                    code,
                    port,
                },
            )
            .await?;
        }
        Commands::DeviceRun { config } => {
            pockymoe_supervisor::distribution::run_device(config).await?
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
