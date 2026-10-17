//! Best-effort recovery of this Treer Agent's hosted hub, never a generic service.
use super::*;
use std::io::{IsTerminal, Write};
use time::Instant;

#[derive(Default)]
pub(super) struct Recovery {
    unavailable_since: Option<Instant>,
    last_attempt: Option<Instant>,
}

impl Recovery {
    pub fn reachable(&mut self) {
        self.unavailable_since = None;
    }

    pub async fn unavailable(&mut self, session: &Session, now: Instant) {
        let since = *self.unavailable_since.get_or_insert(now);
        if now.duration_since(since) < Duration::from_secs(30)
            || self
                .last_attempt
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(120))
        {
            return;
        }
        // Throttle all attempts, including lookup failures and Policy denials.
        self.last_attempt = Some(now);
        if std::io::stdout().is_terminal() {
            print!("\r\x1b[2K");
            let _ = std::io::stdout().flush();
        }
        if start_hub(session).await.unwrap_or(false) {
            println!("Pockymoe attach: hub App start requested; reconnecting.");
        }
    }
}

async fn treer_json(session: &Session, args: &[&str]) -> Result<Value> {
    // Inherit this attach Agent's workload identity, never the hub's identity.
    let bytes = treer_command(&session.treer, &text_args(args)).await?;
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("Invalid Treer response"))
}

async fn start_hub(session: &Session) -> Result<bool> {
    let own = treer_json(session, &["agent", "show", &session.hello.agent_id]).await?;
    let server = own["server_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("No machine")?;
    let port = reqwest::Url::parse(&session.hub)?
        .port_or_known_default()
        .context("No hub port")?;
    let apps = treer_json(session, &["app", "list"]).await?;
    let apps = apps["apps"].as_array().context("No App inventory")?;
    let mut candidates = apps.iter().filter(|app| {
        app["server_id"] == server && app["port"] == port
            && match session.hub_app.as_deref() {
                Some(target) => app["app_id"] == target || app["name"] == target,
                None => app["name"] == "pockymoe",
            }
            // Start only a Treer-hosted Pockymoe hub; standalone is untouched.
            && app["args"].as_array().is_some_and(|args| {
                ["POCKYMOE_HOST_MODE=hosted", "POCKYMOE_HOSTED_BY=treer"]
                    .iter().all(|required| args.iter().any(|arg| arg == required))
            })
    });
    let Some(app) = candidates.next() else {
        return Ok(false);
    };
    if candidates.next().is_some() || !matches!(app["status"].as_str(), Some("stopped" | "exited"))
    {
        return Ok(false);
    }
    let target = app["app_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("No App ID")?;
    treer_command(&session.treer, &text_args(&["agent", "start", target])).await?;
    Ok(true)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    // Avoid overlapping writes/forks of freshly created executable fixtures.
    static FIXTURES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct Fixture {
        root: PathBuf,
        session: Session,
        _lock: tokio::sync::MutexGuard<'static, ()>,
    }
    impl Fixture {
        async fn new(status: &str, target: Option<&str>, denied: bool) -> Self {
            let lock = FIXTURES.lock().await;
            let root =
                std::env::temp_dir().join(format!("attach-recovery-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let binary = root.join("treer");
            std::fs::write(
                &binary,
                format!(
                    r#"#!/bin/sh
cd '{}'
printf '%s\n' "$*" >> calls
case "$1 $2" in
  'agent show') printf '%s' '{{"server_id":"machine-test"}}';;
  'app list') cat apps;;
  'agent start')
    printf '%s' 'synthetic-secret-must-not-escape' >&2
    exit {};;
esac
"#,
                    root.display(),
                    if denied { 1 } else { 0 }
                ),
            )
            .unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
            let fixture = Self {
                session: Session {
                    hello: AttachHello {
                        protocol_version: 1,
                        agent_id: "own-agent".into(),
                        instance_id: "instance-test".into(),
                        thread_id: Some("thread-test".into()),
                        create: None,
                        treer: AttachTreerIdentity {
                            agent_server_url: None,
                            workload_credential: None,
                            workspace_id: None,
                            server_id: None,
                            http_proxy: None,
                        },
                    },
                    hub: "http://127.0.0.1:8787".into(),
                    hub_app: target.map(str::to_owned),
                    token_file: root.join("token"),
                    cwd: root.clone(),
                    treer: binary.into_os_string(),
                    startup_pending: false,
                    startup_executable: None,
                },
                root,
                _lock: lock,
            };
            fixture.apps(serde_json::json!([{
                "app_id":"app-test", "name":"pockymoe", "port":8787,
                "server_id":"machine-test", "status":status,
                "args":["POCKYMOE_HOST_MODE=hosted","POCKYMOE_HOSTED_BY=treer"]
            }]));
            fixture
        }
        fn apps(&self, apps: Value) {
            std::fs::write(
                self.root.join("apps"),
                serde_json::json!({"apps":apps}).to_string(),
            )
            .unwrap();
        }
        fn calls(&self) -> String {
            std::fs::read_to_string(self.root.join("calls")).unwrap_or_default()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn stopped_hub_starts_once_after_30_seconds_and_throttles_for_two_minutes() {
        let f = Fixture::new("stopped", Some("app-test"), false).await;
        let mut recovery = Recovery::default();
        let now = Instant::now();
        for second in [0, 29] {
            recovery
                .unavailable(&f.session, now + Duration::from_secs(second))
                .await;
        }
        assert!(f.calls().is_empty());
        for second in [30, 31, 120, 149] {
            recovery
                .unavailable(&f.session, now + Duration::from_secs(second))
                .await;
        }
        assert_eq!(
            f.calls(),
            "agent show own-agent\napp list\nagent start app-test\n"
        );
        recovery
            .unavailable(&f.session, now + Duration::from_secs(150))
            .await;
        assert_eq!(f.calls().matches("agent start").count(), 2);
    }

    #[tokio::test]
    async fn legacy_attach_discovers_exited_hub_by_loopback_port() {
        let f = Fixture::new("exited", None, false).await;
        assert!(start_hub(&f.session).await.unwrap());
        assert!(f.calls().ends_with("agent start app-test\n"));
        assert!(!f.calls().contains("stop"));
    }

    #[tokio::test]
    async fn running_or_unknown_hub_never_starts() {
        for status in [
            "running",
            "pending",
            "unavailable",
            "idle",
            "failed",
            "unknown",
        ] {
            let f = Fixture::new(status, Some("pockymoe"), false).await;
            assert!(!start_hub(&f.session).await.unwrap());
            assert!(!f.calls().contains("agent start"));
        }
    }

    #[tokio::test]
    async fn policy_denial_is_quiet_and_throttled() {
        let f = Fixture::new("stopped", Some("pockymoe"), true).await;
        let mut recovery = Recovery::default();
        let now = Instant::now();
        for second in [0, 30, 31, 149] {
            // Public recovery swallows denied commands and their diagnostics.
            recovery
                .unavailable(&f.session, now + Duration::from_secs(second))
                .await;
        }
        assert_eq!(f.calls().matches("agent start").count(), 1);
    }

    #[tokio::test]
    async fn standalone_wrong_machine_port_target_and_ambiguous_apps_are_untouched() {
        let f = Fixture::new("stopped", None, false).await;
        let valid: Value =
            serde_json::from_slice::<Value>(&std::fs::read(f.root.join("apps")).unwrap()).unwrap()
                ["apps"][0]
                .clone();
        for (field, value) in [
            ("args", serde_json::json!([])),
            ("server_id", serde_json::json!("other-machine")),
            ("port", serde_json::json!(8788)),
            ("name", serde_json::json!("other-app")),
        ] {
            let mut app = valid.clone();
            app[field] = value;
            f.apps(serde_json::json!([app]));
            assert!(!start_hub(&f.session).await.unwrap());
        }
        f.apps(serde_json::json!([valid, valid]));
        assert!(!start_hub(&f.session).await.unwrap());
        assert!(!f.calls().contains("agent start"));
    }

    #[tokio::test]
    async fn reachable_hub_resets_outage_but_keeps_attempt_throttle() {
        let f = Fixture::new("stopped", None, false).await;
        let mut recovery = Recovery::default();
        let now = Instant::now();
        recovery.unavailable(&f.session, now).await;
        recovery.reachable();
        recovery
            .unavailable(&f.session, now + Duration::from_secs(30))
            .await;
        assert!(f.calls().is_empty());
        recovery
            .unavailable(&f.session, now + Duration::from_secs(60))
            .await;
        recovery.reachable();
        recovery
            .unavailable(&f.session, now + Duration::from_secs(61))
            .await;
        recovery
            .unavailable(&f.session, now + Duration::from_secs(91))
            .await;
        assert_eq!(f.calls().matches("agent start").count(), 1);
    }
}
