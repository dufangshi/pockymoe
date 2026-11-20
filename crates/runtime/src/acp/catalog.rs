use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct AcpAgentDef {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub transport: String,
    pub base_command: String,
    pub server_command: String,
    pub install_command: Option<String>,
    pub model_list_command: Option<String>,
}

pub fn builtin_agents(custom: Option<&str>) -> Vec<AcpAgentDef> {
    builtin_agents_with_commands(
        custom,
        std::env::var("CODEX_COMMAND").ok().as_deref(),
        std::env::var("CLAUDE_COMMAND").ok().as_deref(),
        std::env::var("OPENCODE_COMMAND").ok().as_deref(),
    )
}

fn builtin_agents_with_commands(
    custom: Option<&str>,
    codex_command: Option<&str>,
    claude_command: Option<&str>,
    opencode_command: Option<&str>,
) -> Vec<AcpAgentDef> {
    let codex_command = nonempty_command(codex_command).unwrap_or("codex");
    let claude_command = nonempty_command(claude_command).unwrap_or("claude");
    let opencode_command = nonempty_command(opencode_command).unwrap_or("opencode");
    let opencode_server = command_with_subcommand(opencode_command, "acp");
    let opencode_models = command_with_subcommand(opencode_command, "models");
    let mut agents = vec![
        def(
            "grok",
            "Grok Build",
            "native",
            "grok",
            "grok agent stdio",
            None,
            Some("grok models"),
        ),
        def(
            "cursor",
            "Cursor Agent",
            "native",
            "cursor-agent",
            "cursor-agent acp",
            None,
            None,
        ),
        def(
            "codex",
            "OpenAI Codex",
            "adapter",
            codex_command,
            "codex-acp",
            Some("npm install -g @agentclientprotocol/codex-acp@latest"),
            None,
        ),
        def(
            "claude",
            "Claude Agent",
            "adapter",
            claude_command,
            "claude-agent-acp",
            Some("npm install -g @agentclientprotocol/claude-agent-acp@latest"),
            None,
        ),
        def(
            "gemini",
            "Gemini CLI",
            "native",
            "gemini",
            "gemini --acp",
            None,
            None,
        ),
        def(
            "copilot",
            "GitHub Copilot CLI",
            "native",
            "copilot",
            "copilot --acp",
            None,
            None,
        ),
        def(
            "opencode",
            "OpenCode",
            "native",
            opencode_command,
            &opencode_server,
            None,
            Some(&opencode_models),
        ),
        def(
            "deepseek",
            "DeepSeek Harness",
            "native",
            "dsh",
            "dsh --profile acp",
            None,
            None,
        ),
    ];
    if let Some(custom) = custom
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "grok agent stdio")
    {
        let parsed = shell_words::split(custom).unwrap_or_default();
        let exe = parsed.first().map(String::as_str).unwrap_or("acp");
        agents.push(def(
            "custom",
            "Custom ACP Agent",
            "custom",
            exe,
            custom,
            None,
            None,
        ));
    }
    agents
}

fn nonempty_command(command: Option<&str>) -> Option<&str> {
    command.map(str::trim).filter(|command| !command.is_empty())
}

fn command_with_subcommand(command: &str, subcommand: &str) -> String {
    let Ok(mut parts) = shell_words::split(command) else {
        return format!("{command} {subcommand}");
    };
    if parts.is_empty() {
        return subcommand.to_string();
    }
    parts.insert(1, subcommand.to_string());
    shell_words::join(parts)
}

fn def(
    id: &str,
    name: &str,
    transport: &str,
    base: &str,
    server: &str,
    install: Option<&str>,
    model_list: Option<&str>,
) -> AcpAgentDef {
    AcpAgentDef {
        id: id.into(),
        display_name: name.into(),
        description: format!("{name} over ACP."),
        transport: transport.into(),
        base_command: base.into(),
        server_command: server.into(),
        install_command: super::dependencies::install_command(id)
            .or_else(|| install.map(str::to_string)),
        model_list_command: model_list.map(str::to_string),
    }
}

pub fn extra_bin_dirs() -> Vec<PathBuf> {
    let mut dirs = super::dependencies::managed_bin_dirs();
    dirs.extend([
        super::dependencies::bin_dir(),
        crate::management::harness_bin(),
        crate::node_runtime::bin_dir(),
    ]);
    let home = crate::config::home_dir();
    dirs.extend([
        home.join(".local/bin"),
        home.join(".grok/bin"),
        home.join(".cargo/bin"),
        home.join(".local/share/fnm/aliases/default/bin"),
        home.join(".fnm/aliases/default/bin"),
        home.join(".npm-global/bin"),
        home.join(".volta/bin"),
        home.join(".bun/bin"),
    ]);
    dirs.extend(nvm_default_bin(
        &std::env::var_os("NVM_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".nvm")),
    ));
    if let Ok(app_data) = std::env::var("APPDATA") {
        dirs.push(PathBuf::from(app_data).join("npm"));
    }
    dirs.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    dirs
}

/// nvm only adds its default Node to PATH from shell startup files. Resolve
/// `alias/default` (`24`, `v24.20.0`, `node`) to the newest installed match.
fn nvm_default_bin(nvm: &std::path::Path) -> Option<PathBuf> {
    let alias = std::fs::read_to_string(nvm.join("alias/default")).ok()?;
    let wanted = alias.trim().trim_start_matches('v');
    let any = matches!(wanted, "node" | "stable");
    let versions = nvm.join("versions/node");
    std::fs::read_dir(&versions)
        .ok()?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| {
            let version = name.trim_start_matches('v');
            any || version == wanted || version.starts_with(&format!("{wanted}."))
        })
        .max_by_key(|name| {
            name.trim_start_matches('v')
                .split('.')
                .map(|part| part.parse::<u64>().unwrap_or(0))
                .collect::<Vec<_>>()
        })
        .map(|name| versions.join(name).join("bin"))
}

pub fn augment_path() {
    let mut parts: Vec<PathBuf> = extra_bin_dirs()
        .into_iter()
        .filter(|dir| dir.is_dir())
        .collect();
    if let Ok(path) = std::env::var("PATH") {
        for part in std::env::split_paths(&path) {
            if !part.as_os_str().is_empty() && !parts.iter().any(|existing| existing == &part) {
                parts.push(part);
            }
        }
    }
    if let Ok(path) = std::env::join_paths(parts) {
        std::env::set_var("PATH", path);
    }
}

/// PATH for every child that may run a harness, its adapter or their Node:
/// ACP processes and the thread environment their tools inherit, terminals,
/// automation commands and plugins. Computed at each spawn, because the
/// process PATH (`augment_path`) only holds the managed directories that
/// existed at startup, and a first harness install adds its Node later.
pub fn child_path() -> std::ffi::OsString {
    child_path_with(&[])
}

/// `child_path` behind `front`, without repeated directories.
pub fn child_path_with(front: &[PathBuf]) -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = front.to_vec();
    dirs.extend(super::dependencies::managed_bin_dirs());
    dirs.extend([
        super::dependencies::bin_dir(),
        crate::management::harness_bin(),
        crate::node_runtime::bin_dir(),
    ]);
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.extend(extra_bin_dirs());
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|dir| !dir.as_os_str().is_empty() && seen.insert(dir.clone()));
    std::env::join_paths(dirs).unwrap_or_default()
}

pub fn resolve_executable(command: &str) -> Option<PathBuf> {
    let exe = command_program(command)?;
    which::which_in(&exe, Some(child_path()), std::env::current_dir().ok()?).ok()
}

pub(crate) fn command_program(command: &str) -> Option<String> {
    let literal = command.trim();
    let literal = literal
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            literal
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(literal);
    let literal_path = PathBuf::from(literal);
    if literal_path.is_file() {
        return Some(literal.to_string());
    }

    let parsed = shell_words::split(command).ok()?;
    parsed.first().cloned()
}

pub fn command_available(command: &str) -> bool {
    resolve_executable(command).is_some()
}

pub fn classify_availability(def: &AcpAgentDef) -> &'static str {
    let base = command_available(&def.base_command);
    let server = command_available(&def.server_command);
    if !base {
        "base_missing"
    } else if def.transport == "adapter" && !server {
        "adapter_missing"
    } else if !server {
        "server_unavailable"
    } else {
        "ready"
    }
}

pub fn parse_command_models(output: &str) -> Vec<(String, bool)> {
    let default_model = output
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("Default model:")
                .map(|value| value.trim().to_string())
        })
        .filter(|value| !value.is_empty());
    let mut models = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        let (name, marked_default) = if let Some(rest) = trimmed.strip_prefix("* ") {
            (rest.trim(), true)
        } else if let Some(rest) = trimmed.strip_prefix("- ") {
            (rest.trim(), false)
        } else {
            continue;
        };
        let name = name
            .split_whitespace()
            .next()
            .unwrap_or(name)
            .trim_end_matches("(default)")
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let is_default = marked_default
            || default_model.as_deref() == Some(name.as_str())
            || trimmed.contains("(default)");
        if !models.iter().any(|(existing, _)| existing == &name) {
            models.push((name, is_default));
        }
    }
    if !models.iter().any(|(_, is_default)| *is_default) {
        if let Some(first) = models.first_mut() {
            first.1 = true;
        }
    }
    models
}

#[cfg(test)]
mod tests {
    #[test]
    fn child_path_keeps_the_front_and_the_managed_node_once() {
        let front = std::path::PathBuf::from("/tmp/pockymoe-cli-bin");
        let path = super::child_path_with(&[front.clone(), front.clone()]);
        let dirs: Vec<_> = std::env::split_paths(&path).collect();
        assert_eq!(dirs.first(), Some(&front));
        assert_eq!(dirs.iter().filter(|dir| **dir == front).count(), 1);
        let node = crate::node_runtime::bin_dir();
        assert_eq!(dirs.iter().filter(|dir| **dir == node).count(), 1);
        assert!(dirs.contains(&crate::management::harness_bin()));
    }

    use super::*;

    #[test]
    fn nvm_default_alias_resolves_to_the_newest_matching_version() {
        let nvm = tempfile::tempdir().unwrap();
        for version in ["v22.9.0", "v24.2.0", "v24.20.0"] {
            std::fs::create_dir_all(nvm.path().join("versions/node").join(version)).unwrap();
        }
        std::fs::create_dir_all(nvm.path().join("alias")).unwrap();
        let bin = |version: &str| Some(nvm.path().join("versions/node").join(version).join("bin"));
        for (alias, expected) in [
            ("24\n", "v24.20.0"),
            ("v22.9.0", "v22.9.0"),
            ("node", "v24.20.0"),
        ] {
            std::fs::write(nvm.path().join("alias/default"), alias).unwrap();
            assert_eq!(nvm_default_bin(nvm.path()), bin(expected), "{alias}");
        }
        std::fs::write(nvm.path().join("alias/default"), "lts/*").unwrap();
        assert_eq!(nvm_default_bin(nvm.path()), None);
    }
}
