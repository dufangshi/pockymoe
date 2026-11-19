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

/// One child pipe drained on its own thread, keeping at most `limit` bytes and
/// discarding the rest so the writer never blocks on a full pipe. A descendant
/// that inherits the pipe can hold it open after the direct child exits, so
/// callers wait only until their own deadline and then abandon the reader; its
/// thread ends when the last writer finally closes the pipe.
pub(crate) struct PipeReader(std::sync::mpsc::Receiver<std::io::Result<(Vec<u8>, bool)>>);

impl PipeReader {
    pub(crate) fn spawn(mut pipe: impl std::io::Read + Send + 'static, limit: usize) -> Self {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut truncated = false;
            let mut buf = [0; 8192];
            let result = loop {
                match pipe.read(&mut buf) {
                    Ok(0) => break Ok((kept, truncated)),
                    Ok(n) => {
                        let take = n.min(limit.saturating_sub(kept.len()));
                        kept.extend_from_slice(&buf[..take]);
                        truncated |= take < n;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => break Err(e),
                }
            };
            let _ = sender.send(result);
        });
        Self(receiver)
    }

    /// The kept bytes and whether more were discarded, or `None` when the pipe
    /// is still open at `deadline`.
    pub(crate) fn finish(
        &self,
        deadline: std::time::Instant,
    ) -> Option<std::io::Result<(Vec<u8>, bool)>> {
        use std::sync::mpsc::RecvTimeoutError;
        match self
            .0
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        {
            Ok(result) => Some(result),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                Some(Err(std::io::Error::other("pipe reader stopped")))
            }
        }
    }
}

/// Start a short-lived helper in its own process group, so a timeout can stop
/// the descendants that would otherwise keep its output pipes open.
pub(crate) fn own_process_group(command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = command;
}

/// Kill the group created by `own_process_group`. The group ID stays reserved
/// while any member lives, so this cannot reach an unrelated process.
pub(crate) fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// Run a helper whose stdout is the only pipe, bounded as a whole by
/// `deadline`: the process, its output and the closing of that pipe. Returns
/// the exit status, the kept bytes and whether more were discarded.
pub(crate) fn run_bounded(
    command: &mut std::process::Command,
    limit: usize,
    deadline: std::time::Instant,
) -> std::io::Result<(std::process::ExitStatus, Vec<u8>, bool)> {
    use std::io::{Error, ErrorKind};
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    own_process_group(command);
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::other("no stdout"))?;
    let reader = PipeReader::spawn(stdout, limit);
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                kill_process_group(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::new(ErrorKind::TimedOut, "timed out"));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    };
    let Some(output) = reader.finish(deadline) else {
        // The direct child exited, but a descendant still holds stdout.
        kill_process_group(child.id());
        return Err(Error::new(
            ErrorKind::TimedOut,
            "timed out waiting for output to close",
        ));
    };
    let (bytes, truncated) = output?;
    Ok((status, bytes, truncated))
}

#[cfg(test)]
mod pipe_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn pipe_reader_caps_output_and_reports_truncation() {
        let reader = PipeReader::spawn(std::io::Cursor::new(vec![7u8; 20_000]), 1000);
        let (kept, truncated) = reader
            .finish(Instant::now() + Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(kept.len(), 1000);
        assert!(truncated);
    }

    #[cfg(unix)]
    #[test]
    fn a_descendant_holding_the_pipe_cannot_outlast_the_deadline() {
        let mut command = std::process::Command::new("sh");
        command
            .args(["-c", "sleep 30 & echo done"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        own_process_group(&mut command);
        let mut child = command.spawn().unwrap();
        let reader = PipeReader::spawn(child.stdout.take().unwrap(), 1000);
        assert!(child.wait().unwrap().success());
        let started = Instant::now();
        assert!(reader
            .finish(Instant::now() + Duration::from_millis(300))
            .is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
        kill_process_group(child.id());
        let (kept, truncated) = reader
            .finish(Instant::now() + Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(kept, b"done\n");
        assert!(!truncated);
    }
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
