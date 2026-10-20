//! Resolve the installation actually selected by the harness, never a second PATH copy.
use crate::{acp::builtin_agents, Supervisor};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

pub fn harness_prefix() -> PathBuf {
    crate::config::home_dir().join(".local/share/remote-codex/harnesses")
}
pub fn harness_bin() -> PathBuf {
    if cfg!(windows) {
        harness_prefix()
    } else {
        harness_prefix().join("bin")
    }
}
fn npm_package(id: &str) -> Option<&'static str> {
    match id {
        "codex" => Some("@openai/codex"),
        "claude" => Some("@anthropic-ai/claude-code"),
        "gemini" => Some("@google/gemini-cli"),
        "copilot" => Some("@github/copilot"),
        "opencode" => Some("opencode-ai"),
        // DSH ships ACP and its Web console bundle in one package; `latest`
        // is its current release candidate while it publishes no stable line.
        "deepseek" => Some("@deepseek-ai/dsh"),
        _ => None,
    }
}
pub fn can_install(id: &str) -> bool {
    npm_package(id).is_some() || (!cfg!(windows) && matches!(id, "grok" | "cursor"))
}
/// Why a harness has no Install action.
fn no_installer(id: &str) -> &'static str {
    if matches!(id, "grok" | "cursor") {
        "This harness has no managed installer on Windows. Install it with its own installer."
    } else {
        "This harness has no managed installer. Install it with its own installer."
    }
}
pub async fn ensure_adapter(def: &crate::acp::catalog::AcpAgentDef) -> Result<()> {
    crate::acp::dependencies::ensure(def, false).await
}
pub async fn install_harness(state: &Supervisor, id: &str) -> Result<()> {
    let _install = state.harness_install_gate.lock().await;
    let def = builtin_agents(state.config.acp_command.as_deref())
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| anyhow!("Unknown harness"))?;
    if let Ok(found) = inspect(&def.base_command, id).await {
        if !found.path.starts_with(harness_bin()) {
            bail!(
                "A harness is already installed. Use Update to preserve the selected installation."
            );
        }
    }
    if let Some(package) = npm_package(id) {
        crate::node_runtime::ensure().await?;
        let command = shell_words::join([
            "npm",
            "install",
            "--global",
            "--prefix",
            &harness_prefix().to_string_lossy(),
            &format!("{package}@latest"),
            "--no-audit",
            "--no-fund",
        ]);
        let parsed = crate::acp::rpc::parse_spawn_command(&command)?;
        output(Path::new(&parsed.program), &parsed.args, 600).await?;
    } else if matches!(id, "grok" | "cursor") && !cfg!(windows) {
        let url = if id == "grok" {
            "https://x.ai/cli/install.sh"
        } else {
            "https://cursor.com/install"
        };
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?
            .get(url)
            .send()
            .await?
            .error_for_status()?;
        let script = response.bytes().await?;
        if script.len() > 1024 * 1024 {
            bail!("Installer is unexpectedly large");
        }
        let tmp = tempfile::tempdir()?;
        let file = tmp.path().join("install.sh");
        std::fs::write(&file, script)?;
        // Official installers only; templates can never supply a URL or command.
        output(
            &resolve("bash")?,
            &[file.to_string_lossy().into_owned()],
            600,
        )
        .await?;
    } else {
        bail!(no_installer(id));
    }
    let installed = inspect(&def.base_command, id)
        .await
        .context("Installer finished, but harness could not be found")?;
    verify_executable(&installed).await?;
    crate::acp::dependencies::ensure(&def, false).await?;
    state.restart_harness(id).await?;
    Ok(())
}

async fn verify_executable(installation: &Installation) -> Result<()> {
    let version = output(&installation.path, &["--version".into()], 30)
        .await
        .context("Installed executable could not run --version")?;
    if !version.chars().any(|c| c.is_ascii_digit()) {
        bail!("Installed executable did not report a version");
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Installation {
    pub path: PathBuf,
    pub real_path: PathBuf,
    pub manager: String,
    pub version: Option<String>,
    pub update: Vec<String>,
    pub reason: Option<String>,
}

pub async fn output(program: &Path, args: &[String], timeout: u64) -> Result<String> {
    let mut command = tokio::process::Command::new(program);
    crate::child_process::hide_tokio(&mut command);
    command
        .env("PATH", crate::acp::child_path())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let result = tokio::time::timeout(Duration::from_secs(timeout), command.output()).await??;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let detail: String = stderr
            .chars()
            .rev()
            .take(4000)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        bail!("Command failed ({}): {}", result.status, detail.trim());
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
}

pub fn resolve(command: &str) -> Result<PathBuf> {
    crate::acp::resolve_executable(command)
        .ok_or_else(|| anyhow!("Executable not found: {command}"))
}

fn text_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn npm_shim_target(path: &Path) -> Option<PathBuf> {
    if !matches!(
        path.extension().and_then(|v| v.to_str()),
        Some("cmd" | "ps1")
    ) {
        return None;
    }
    let source = std::fs::read_to_string(path).ok()?.replace('\\', "/");
    for quoted in source.split('"') {
        let Some(start) = quoted.find("node_modules/") else {
            continue;
        };
        let relative = &quoted[start..];
        if ![".js", ".mjs", ".cjs"]
            .iter()
            .any(|suffix| relative.ends_with(suffix))
            || relative.split('/').any(|part| part == "..")
        {
            continue;
        }
        let target = path.parent()?.join(relative);
        if target.is_file() {
            return target.canonicalize().ok();
        }
    }
    None
}

pub fn installation(path: PathBuf, agent: &str) -> Installation {
    let real = npm_shim_target(&path)
        .unwrap_or_else(|| path.canonicalize().unwrap_or_else(|_| path.clone()));
    let mut found = Installation {
        path,
        real_path: real.clone(),
        manager: "manual".into(),
        version: None,
        update: vec![],
        reason: Some("Update using the installer that owns this executable.".into()),
    };
    // App bundles are owned by the desktop application's updater, not npm.
    if real
        .components()
        .any(|c| c.as_os_str().to_string_lossy().ends_with(".app"))
    {
        found.manager = "app".into();
        found.reason =
            Some("Bundled with a desktop app. Update that app to update this harness.".into());
        return found;
    }
    for ancestor in real.ancestors().skip(1) {
        if let Some(parent) = ancestor.parent() {
            if matches!(
                parent.file_name().and_then(|v| v.to_str()),
                Some("Cellar" | "Caskroom")
            ) {
                if let (Some(prefix), Some(name)) = (parent.parent(), ancestor.file_name()) {
                    let brew = prefix.join("bin/brew");
                    if brew.is_file() {
                        found.manager = "homebrew".into();
                        found.reason = None;
                        found.update = vec![
                            text_path(&brew),
                            "upgrade".into(),
                            if parent.ends_with("Caskroom") {
                                "--cask"
                            } else {
                                "--formula"
                            }
                            .into(),
                            name.to_string_lossy().into_owned(),
                        ];
                        return found;
                    }
                }
            }
        }
        let Ok(raw) = std::fs::read(ancestor.join("package.json")) else {
            continue;
        };
        let Ok(package) = serde_json::from_slice::<Value>(&raw) else {
            continue;
        };
        let Some(name) = package["name"].as_str() else {
            continue;
        };
        // Only global npm layouts; workspace, pnpm stores, and arbitrary package names
        // must not be reinterpreted as a global npm install.
        let allowed = matches!(
            name,
            "@openai/codex"
                | "@anthropic-ai/claude-code"
                | "@google/gemini-cli"
                | "@github/copilot"
                | "opencode-ai"
                | "@deepseek-ai/dsh"
                | "@agentclientprotocol/codex-acp"
                | "@agentclientprotocol/claude-agent-acp"
                | "pockymoe"
                // Global npm installs from before the native runtime and the rename.
                | "remote-codex"
        );
        if !allowed {
            continue;
        }
        let Some(modules) = ancestor
            .ancestors()
            .find(|p| p.file_name().is_some_and(|v| v == "node_modules"))
        else {
            continue;
        };
        let Some(parent) = modules.parent() else {
            continue;
        };
        let prefix = if parent.ends_with("lib") {
            parent.parent().unwrap_or(parent)
        } else {
            parent
        };
        found.version = package["version"].as_str().map(str::to_owned);
        let mut npm = modules.join("npm/bin/npm-cli.js");
        let node = if cfg!(windows) {
            let local = prefix.join("node.exe");
            if local.is_file() {
                local
            } else {
                which::which("node").unwrap_or(local)
            }
        } else {
            prefix.join("bin/node")
        };
        // Windows npm often stores global packages in AppData, while npm itself
        // lives next to node.exe. --prefix still pins the installation being updated.
        if cfg!(windows) && !npm.is_file() {
            if let Some(parent) = node.parent() {
                npm = parent.join("node_modules/npm/bin/npm-cli.js");
            }
        }
        if npm.is_file() && node.is_file() {
            found.manager = "npm".into();
            found.version = package["version"].as_str().map(str::to_owned);
            found.reason = None;
            found.update = vec![
                text_path(&node),
                text_path(&npm),
                "install".into(),
                "--global".into(),
                "--prefix".into(),
                text_path(prefix),
                format!("{name}@latest"),
                "--no-audit".into(),
                "--no-fund".into(),
            ];
            return found;
        }
    }
    if agent == "claude"
        && real
            .to_string_lossy()
            .replace('\\', "/")
            .contains("/.local/share/claude/versions/")
    {
        found.manager = "native".into();
        found.update = vec![text_path(&found.path), "update".into()];
        found.reason = None;
    }
    found
}

pub async fn inspect(command: &str, agent: &str) -> Result<Installation> {
    let mut found = installation(resolve(command)?, agent);
    if found.path.starts_with(harness_bin()) && npm_package(agent).is_some() {
        let parsed = crate::acp::rpc::parse_spawn_command(&shell_words::join([
            "npm",
            "install",
            "--global",
            "--prefix",
            &harness_prefix().to_string_lossy(),
            &format!("{}@latest", npm_package(agent).unwrap()),
            "--no-audit",
            "--no-fund",
        ]))?;
        found.manager = "pockymoe".into();
        found.reason = None;
        found.update = vec![parsed.program];
        found.update.extend(parsed.args);
    } else if agent == "grok"
        && found
            .real_path
            .starts_with(crate::config::home_dir().join(".grok"))
    {
        found.manager = "native".into();
        found.reason = None;
        found.update = vec![text_path(&found.path), "update".into()];
    } else if agent == "cursor"
        && found
            .real_path
            .starts_with(crate::config::home_dir().join(".local"))
    {
        found.manager = "native".into();
        found.reason = None;
        found.update = vec![text_path(&found.path), "update".into()];
    }
    if found.version.is_some() {
        return Ok(found);
    }
    if let Ok(version) = output(&found.path, &["--version".into()], 5).await {
        let line = version.lines().next().unwrap_or_default();
        if line.chars().any(|c| c.is_ascii_digit()) {
            found.version = Some(line.chars().take(100).collect());
        }
    }
    Ok(found)
}

fn dto(found: Installation) -> Value {
    json!({"path":found.path,"resolvedPath":found.real_path,"version":found.version,"manager":found.manager,"canUpdate":!found.update.is_empty(),"updateCommand":shell_words::join(&found.update),"reason":found.reason})
}

pub async fn inventory(state: &Supervisor) -> Value {
    let rows = futures_util::future::join_all(builtin_agents(state.config.acp_command.as_deref()).into_iter().map(|def| async move {
        let (base, adapter) = tokio::join!(inspect(&def.base_command, &def.id), async {
            if def.transport == "adapter" { Some(adapter_inventory(&def).await) } else { None }
        });
        let job = state.management_jobs.lock().unwrap().get(&def.id).cloned();
        let base=base.map(dto).unwrap_or_else(|_|json!({"installed":false,"canInstall":can_install(&def.id),"canUpdate":false,"path":"","resolvedPath":"","manager":"pockymoe","reason":if can_install(&def.id){""}else{no_installer(&def.id)}}));
        let adapter_command = (def.transport == "adapter").then_some(&def.server_command);
        json!({"id":def.id,"name":def.display_name,"transport":def.transport,"baseCommand":def.base_command,"adapterCommand":adapter_command,"base":base,"adapter":adapter,"job":job})
    })).await;
    json!(rows)
}

pub async fn update_harness(state: &Supervisor, id: &str, component: &str) -> Result<()> {
    let _install = state.harness_install_gate.lock().await;
    let def = builtin_agents(state.config.acp_command.as_deref())
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| anyhow!("Unknown harness"))?;
    let command = match component {
        "base" => &def.base_command,
        "adapter" if def.transport == "adapter" => &def.server_command,
        _ => bail!("Unknown component"),
    };
    if component == "adapter" {
        // Windows cannot replace an executable still owned by a live harness.
        // restart_harness rejects active turns and retires idle processes; the
        // next use resumes their persisted sessions after installation.
        state.restart_harness(id).await?;
        crate::acp::dependencies::ensure(&def, true).await?;
        state.restart_harness(id).await?;
        let provider = if state
            .config
            .enabled_providers
            .contains(&pockymoe_protocol::Provider::Acp)
        {
            pockymoe_protocol::Provider::Acp
        } else {
            serde_json::from_value(json!(id))?
        };
        state
            .list_models(
                provider,
                Some(id),
                Some(&state.config.workspace_root.to_string_lossy()),
            )
            .await
            .map_err(|error| {
                anyhow!("Adapter installed, but ACP verification failed: {error:#}")
            })?;
        return Ok(());
    }
    let found = inspect(command, id).await?;
    let Some((program, args)) = found.update.split_first() else {
        bail!("{}", found.reason.unwrap_or_default());
    };
    state.restart_harness(id).await?;
    output(Path::new(program), args, 300).await?;
    let updated = inspect(command, id).await?;
    verify_executable(&updated).await?;
    state.restart_harness(id).await?;
    Ok(())
}

async fn adapter_inventory(def: &crate::acp::catalog::AcpAgentDef) -> Value {
    let mut value = match inspect(&def.server_command, "adapter").await {
        Ok(found) => {
            let found_managed = found.path.starts_with(crate::acp::dependencies::prefix());
            let mut value = dto(found);
            value["installed"] = json!(true);
            value["reason"] = Value::Null;
            if found_managed {
                value["manager"] = json!("pockymoe");
            }
            value
        }
        Err(error) => {
            json!({"installed":false,"version":null,"path":null,"reason":error.to_string()})
        }
    };
    value["canInstall"] = json!(crate::acp::dependencies::package(&def.id).is_some());
    value["canUpdate"] = value["installed"].clone();
    value["updateCommand"] = json!(crate::acp::dependencies::install_command(&def.id));
    value
}
