use std::env;
use std::path::PathBuf;

use pockymoe_protocol::{Mode, Provider};

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub mode: Mode,
    pub host: String,
    pub port: u16,
    pub workspace_root: PathBuf,
    pub database_url: PathBuf,
    pub app_name: String,
    pub app_version: String,
    pub environment: String,
    pub auth_required: bool,
    pub admin_username: Option<String>,
    pub admin_password: Option<String>,
    pub session_secret: Option<String>,
    pub relay_server_url: Option<String>,
    pub web_origin: Option<String>,
    pub relay_agent_token: Option<String>,
    pub enabled_providers: Vec<Provider>,
    pub acp_command: Option<String>,
    pub acp_startup_timeout_ms: u64,
    pub fake_runtime: bool,
}

impl RuntimeConfig {
    pub fn from_env() -> Self {
        let mode = match env::var("POCKYMOE_MODE").unwrap_or_default().as_str() {
            "server" => Mode::Server,
            "relay" => Mode::Relay,
            _ => Mode::Local,
        };
        let port_names = if mode == Mode::Relay {
            &["POCKYMOE_RELAY_SUPERVISOR_PORT"][..]
        } else {
            &["PORT"][..]
        };
        let port = first_env(port_names)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8787);
        let host_names = if mode == Mode::Relay {
            &["POCKYMOE_RELAY_SUPERVISOR_HOST"][..]
        } else {
            &["HOST"][..]
        };
        let host = first_env(host_names).unwrap_or_else(|_| "127.0.0.1".into());
        let setting = |name, legacy| {
            nonempty_env(name).or_else(|| {
                (mode != Mode::Relay)
                    .then(|| nonempty_env(legacy))
                    .flatten()
            })
        };
        let environment =
            setting("POCKYMOE_ENVIRONMENT", "NODE_ENV").unwrap_or_else(|| "development".into());
        let workspace_root = PathBuf::from(
            setting("POCKYMOE_WORKSPACE_ROOT", "WORKSPACE_ROOT")
                .unwrap_or_else(|| default_workspace_root()),
        );
        let database_url = PathBuf::from(
            setting("POCKYMOE_DATABASE_PATH", "DATABASE_URL")
                .unwrap_or_else(|| default_database_path(&environment, mode == Mode::Relay)),
        );
        let enabled_providers =
            parse_providers(env::var("POCKYMOE_ENABLED_AGENT_PROVIDERS").ok().as_deref());
        let fake_runtime = env_flag("POCKYMOE_E2E_FAKE_RUNTIME");
        Self {
            mode,
            host,
            port,
            workspace_root,
            database_url,
            app_name: setting("POCKYMOE_APP_NAME", "APP_NAME").unwrap_or_else(|| "Pockymoe".into()),
            app_version: pockymoe_protocol::APP_VERSION.to_string(),
            environment,
            auth_required: mode != Mode::Local,
            admin_username: env::var("POCKYMOE_ADMIN_USERNAME").ok(),
            admin_password: env::var("POCKYMOE_ADMIN_PASSWORD").ok(),
            session_secret: env::var("POCKYMOE_SESSION_SECRET").ok(),
            relay_server_url: env::var("POCKYMOE_RELAY_SERVER_URL").ok(),
            web_origin: nonempty_env("POCKYMOE_WEB_ORIGIN"),
            relay_agent_token: env::var("POCKYMOE_RELAY_AGENT_TOKEN").ok(),
            enabled_providers,
            acp_command: setting("POCKYMOE_ACP_COMMAND", "ACP_COMMAND")
                .filter(|s| !s.trim().is_empty()),
            acp_startup_timeout_ms: setting(
                "POCKYMOE_ACP_STARTUP_TIMEOUT_MS",
                "ACP_STARTUP_TIMEOUT_MS",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(10_000),
            fake_runtime,
        }
    }
}

fn env_flag(name: &str) -> bool {
    matches!(
        env::var(name).unwrap_or_default().to_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn nonempty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn first_env(names: &[&str]) -> Result<String, env::VarError> {
    names
        .iter()
        .find_map(|name| nonempty_env(name))
        .ok_or(env::VarError::NotPresent)
}

fn parse_providers(raw: Option<&str>) -> Vec<Provider> {
    if let Some(raw) = raw.filter(|s| !s.trim().is_empty()) {
        return raw
            .split(',')
            .filter_map(|part| match part.trim() {
                "codex" => Some(Provider::Codex),
                "claude" => Some(Provider::Claude),
                "opencode" => Some(Provider::Opencode),
                "acp" => Some(Provider::Acp),
                _ => None,
            })
            .collect();
    }
    if cfg!(windows) {
        return vec![Provider::Codex, Provider::Acp];
    }
    vec![
        Provider::Codex,
        Provider::Claude,
        Provider::Opencode,
        Provider::Acp,
    ]
}

fn default_workspace_root() -> String {
    home_dir().to_string_lossy().into()
}

fn default_database_path(environment: &str, relay: bool) -> String {
    if relay {
        return home_dir()
            .join(".remote-codex")
            .join("relay-supervisor.sqlite")
            .to_string_lossy()
            .into();
    }
    if environment == "production" {
        return home_dir()
            .join(".remote-codex")
            .join("supervisor.sqlite")
            .to_string_lossy()
            .into();
    }
    PathBuf::from(".local")
        .join("supervisor-dev.sqlite")
        .to_string_lossy()
        .into()
}

pub(crate) fn home_dir() -> PathBuf {
    select_home_dir(
        nonempty_env("HOME"),
        nonempty_env("USERPROFILE"),
        nonempty_env("HOMEDRIVE"),
        nonempty_env("HOMEPATH"),
    )
}

fn select_home_dir(
    home: Option<String>,
    user_profile: Option<String>,
    home_drive: Option<String>,
    home_path: Option<String>,
) -> PathBuf {
    home.or(user_profile)
        .map(PathBuf::from)
        .or_else(|| {
            let drive = home_drive?;
            let path = home_path?;
            Some(PathBuf::from(format!("{drive}{path}")))
        })
        .unwrap_or_else(|| PathBuf::from("."))
}
