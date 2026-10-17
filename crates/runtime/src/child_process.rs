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

/// Host-inherited variables a hub must not pass to a thread's processes. Only the
/// identity bound to that thread (`launch_env`) may follow. A host's workload
/// proxy signs requests as the hub itself, so it is removed with the identity;
/// any other configured proxy is left alone.
pub(crate) fn inherited_host_identity() -> Vec<&'static str> {
    let mut keys = vec![
        "TREER_AGENT_ID",
        "TREER_WORKLOAD_CREDENTIAL",
        "TREER_AGENT_SERVER_URL",
        "TREER_WORKSPACE_ID",
        "TREER_SERVER_ID",
    ];
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
            .filter(|key| std::env::var(key).is_ok_and(|v| v == own)),
        );
    }
    keys
}
