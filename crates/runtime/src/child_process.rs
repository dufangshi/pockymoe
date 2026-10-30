#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(crate) fn hide_tokio(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    #[cfg(not(windows))]
    let _ = command;
}

pub(crate) fn hide_std(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// The Supervisor's own credentials, as read from its environment. A thread's
/// agent, its terminals and the owner's shells never need them: CLI access is
/// re-added per thread by `launch_env`, and harness API keys use other names.
const SUPERVISOR_SECRETS: &[&str] = &[
    "POCKYMOE_SESSION_SECRET",
    "POCKYMOE_ADMIN_PASSWORD",
    "POCKYMOE_RELAY_AGENT_TOKEN",
    "POCKYMOE_RELAY_SESSION_SECRET",
    "POCKYMOE_RELAY_SUPERVISOR_TOKEN",
    "POCKYMOE_RELAY_REGISTRATION_PASSWORD",
    "POCKYMOE_ENROLL_SECRET",
    "POCKYMOE_ACCOUNT_KEY",
    "POCKYMOE_GITHUB_OAUTH_CLIENT_SECRET",
    "POCKYMOE_GOOGLE_OAUTH_CLIENT_SECRET",
    "POCKYMOE_APNS_KEY_ID",
    "POCKYMOE_APNS_KEY_PATH",
    "POCKYMOE_TOKEN",
    "REMOTE_CODEX_TOKEN",
    "REMOTE_CODEX_RELAY_AGENT_TOKEN",
];

fn supervisor_secret(name: &str) -> bool {
    if SUPERVISOR_SECRETS.contains(&name) {
        return true;
    }
    // Catch credentials added later under the same prefixes. `*_API_KEY` names are
    // harness credential references (for example the DSH key) and stay.
    let upper = name.to_ascii_uppercase();
    (upper.starts_with("POCKYMOE_") || upper.starts_with("REMOTE_CODEX_"))
        && !upper.ends_with("_API_KEY")
        && ["SECRET", "PASSWORD", "_TOKEN", "PRIVATE_KEY"]
            .iter()
            .any(|word| upper.contains(word))
}

/// Supervisor credentials present in this process's environment, to remove from
/// every child that runs a thread's or the owner's commands.
pub fn supervisor_secret_env() -> Vec<String> {
    std::env::vars_os()
        .filter_map(|(key, _)| key.into_string().ok())
        .filter(|key| supervisor_secret(key))
        .collect()
}

/// Host-inherited variables a hub must not pass to a thread's processes. Only the
/// identity bound to that thread (`launch_env`) may follow. A host's workload
/// proxy signs requests as the hub itself, so it is removed with the identity;
/// any other configured proxy is left alone.
pub(crate) fn inherited_host_identity() -> Vec<String> {
    let mut keys: Vec<String> = supervisor_secret_env();
    keys.extend(
        [
            "TREER_AGENT_ID",
            "TREER_WORKLOAD_CREDENTIAL",
            "TREER_AGENT_SERVER_URL",
            "TREER_WORKSPACE_ID",
            "TREER_SERVER_ID",
        ]
        .map(String::from),
    );
    if let Some(own) = std::env::var("TREER_HTTP_PROXY")
        .ok()
        .filter(|v| !v.is_empty())
    {
        keys.extend(
            [
                "http_proxy",
                "https_proxy",
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "TREER_HTTP_PROXY",
            ]
            .into_iter()
            .filter(|key| std::env::var(key).is_ok_and(|v| v == own))
            .map(String::from),
        );
    }
    keys
}

#[cfg(test)]
mod secret_tests {
    use super::supervisor_secret;

    #[test]
    fn supervisor_credentials_are_scrubbed_but_harness_keys_and_settings_stay() {
        for name in [
            "POCKYMOE_SESSION_SECRET",
            "POCKYMOE_ADMIN_PASSWORD",
            "POCKYMOE_RELAY_AGENT_TOKEN",
            "POCKYMOE_TOKEN",
            "REMOTE_CODEX_TOKEN",
            "POCKYMOE_FUTURE_WEBHOOK_SECRET",
            "remote_codex_relay_agent_token",
        ] {
            assert!(supervisor_secret(name), "{name}");
        }
        for name in [
            "REMOTE_CODEX_DSH_API_KEY",
            "POCKYMOE_URL",
            "POCKYMOE_THREAD_ID",
            "POCKYMOE_MODE",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GITHUB_TOKEN",
        ] {
            assert!(!supervisor_secret(name), "{name}");
        }
    }
}
