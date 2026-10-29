#![cfg(unix)]
use std::{path::Path, process::Command, sync::Arc};

use pockymoe_protocol::Provider;
use pockymoe_runtime::{
    fake::FakeRuntime, plugins::PluginSettings, Database, RuntimeConfig, Supervisor,
};
use serde_json::{json, Value};

fn setup(dir: &Path) -> Arc<Supervisor> {
    let config = RuntimeConfig {
        treer_enabled: false,
        host_mode: Default::default(),
        mode: pockymoe_protocol::Mode::Local,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root: dir.into(),
        database_url: dir.join("test.sqlite"),
        app_name: "test".into(),
        app_version: "test".into(),
        environment: "test".into(),
        auth_required: false,
        admin_username: None,
        admin_password: None,
        session_secret: None,
        relay_server_url: None,
        web_origin: None,
        relay_agent_token: None,
        enabled_providers: vec![Provider::Codex],
        acp_command: None,
        acp_startup_timeout_ms: 1000,
        fake_runtime: true,
        supervisor_restart_strategy: Default::default(),
    };
    let supervisor = Arc::new(Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![Arc::new(FakeRuntime::new(Provider::Codex))],
    ));
    supervisor.configure_cli("http://127.0.0.1:9".into());
    supervisor
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// A git repository whose plugin echoes its request and a few environment facts.
fn plugin_repo(root: &Path, manifest: Value) -> std::path::PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join("pockymoe-plugin.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(
        repo.join("plugin.sh"),
        r#"#!/bin/sh
request=$(cat)
case "$request" in *'"fail"'*) echo "boom" >&2; exit 3;; esac
printf '{"request":%s,"token":"%s","data":"%s","home":"%s","setup":"%s"}' \
  "$request" "${POCKYMOE_TOKEN:+set}" "$POCKYMOE_PLUGIN_DATA_DIR" "${HOME:+set}" \
  "$(cat setup-ran 2>/dev/null)"
"#,
    )
    .unwrap();
    std::fs::write(repo.join("setup.sh"), "#!/bin/sh\necho yes > setup-ran\n").unwrap();
    for script in ["plugin.sh", "setup.sh"] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(repo.join(script), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

fn manifest(permissions: Value) -> Value {
    json!({
        "schemaVersion": 1,
        "id": "tester.echo",
        "name": "Echo",
        "version": "0.1.0",
        "entry": ["./plugin.sh"],
        "setup": ["./setup.sh"],
        "actions": [{"name": "tick", "timeoutSeconds": 10}, {"name": "status"}],
        "schedules": [{"action": "tick", "everySeconds": 10}],
        "permissions": permissions,
        "defaultSettings": {"greeting": "hi"},
    })
}

#[tokio::test]
async fn install_enable_run_schedule_update_and_remove_a_git_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let repo = plugin_repo(dir.path(), manifest(json!(["pockymoeCli"])));
    let s = setup(dir.path());

    let dto = s
        .install_external_plugin(repo.to_str().unwrap(), None)
        .await
        .unwrap();
    assert_eq!(dto.id, "tester.echo");
    assert_eq!(dto.source.as_deref(), Some("external"));
    assert!(!dto.enabled, "installing never enables");
    assert_eq!(dto.capabilities["permissions"], json!(["pockymoeCli"]));
    assert_eq!(
        dto.capabilities["install"]["commit"]
            .as_str()
            .unwrap()
            .len(),
        40
    );
    assert!(s
        .install_external_plugin(repo.to_str().unwrap(), None)
        .await
        .is_err());

    let disabled = s
        .run_external_plugin_action("tester.echo", "status", json!({}), "manual")
        .await;
    assert!(disabled.unwrap_err().to_string().contains("not enabled"));

    s.set_plugin_enabled("tester.echo", true).unwrap();
    s.save_plugin_settings(
        "tester.echo",
        PluginSettings {
            revision: 0,
            values: json!({"greeting": "hello"}),
        },
    )
    .unwrap();
    let out = s
        .run_external_plugin_action("tester.echo", "status", json!({"x": 1}), "manual")
        .await
        .unwrap();
    assert_eq!(out["request"]["action"], "status");
    assert_eq!(out["request"]["scope"], "device");
    assert_eq!(out["request"]["input"], json!({"x": 1}));
    assert_eq!(out["request"]["settings"]["greeting"], "hello");
    assert_eq!(out["token"], "set");
    assert_eq!(out["home"], "set");
    assert_eq!(out["setup"], "yes");
    assert!(Path::new(out["data"].as_str().unwrap()).is_dir());

    let failed = s
        .run_external_plugin_action("tester.echo", "status", json!("fail"), "manual")
        .await
        .unwrap_err()
        .to_string();
    assert!(failed.contains("boom"), "{failed}");
    let runs = s.external_plugin_runs("tester.echo").unwrap();
    assert_eq!(runs[0]["state"], "failed");
    assert_eq!(runs[1]["state"], "completed");

    s.external_plugin_tick().unwrap();
    for _ in 0..100 {
        if s.external_plugin_runs("tester.echo").unwrap()[0]["trigger"] == "schedule" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let runs = s.external_plugin_runs("tester.echo").unwrap();
    assert_eq!(runs[0]["trigger"], "schedule");
    assert_eq!(runs[0]["action"], "tick");

    let mut next = manifest(json!(["pockymoeCli"]));
    next["version"] = json!("0.2.0");
    std::fs::write(
        repo.join("pockymoe-plugin.json"),
        serde_json::to_string(&next).unwrap(),
    )
    .unwrap();
    git(&repo, &["commit", "-q", "-am", "bump"]);
    assert_eq!(
        s.update_external_plugin("tester.echo")
            .await
            .unwrap()
            .version,
        "0.2.0"
    );

    // A restarted Supervisor finds the installed plugin again.
    drop(s);
    let s = setup(dir.path());
    assert_eq!(s.get_plugin("tester.echo").unwrap().version, "0.2.0");
    assert!(s.plugin_enabled("tester.echo", false));

    let data = dir.path().join("test.plugin-data/tester.echo");
    s.remove_external_plugin("tester.echo", false).unwrap();
    assert!(s.get_plugin("tester.echo").is_err());
    assert!(!dir.path().join("test.plugins/tester.echo").exists());
    assert!(data.is_dir(), "data stays without purge");
    assert!(s
        .db
        .get_kv("plugin:tester.echo:settings")
        .unwrap()
        .is_none());
    assert!(s
        .remove_external_plugin("remote-codex.terminal", false)
        .is_err());
}

#[tokio::test]
async fn plugins_without_cli_permission_get_no_credential_and_bad_manifests_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let repo = plugin_repo(dir.path(), manifest(json!([])));
    let s = setup(dir.path());
    s.install_external_plugin(repo.to_str().unwrap(), None)
        .await
        .unwrap();
    s.set_plugin_enabled("tester.echo", true).unwrap();
    let out = s
        .run_external_plugin_action("tester.echo", "status", json!({}), "manual")
        .await
        .unwrap();
    assert_eq!(out["token"], "");

    for (field, value) in [
        ("id", json!("pockymoe.fake")),
        ("id", json!("nodots")),
        ("entry", json!([])),
        ("schedules", json!([{"action": "tick", "everySeconds": 1}])),
        (
            "schedules",
            json!([{"action": "missing", "everySeconds": 60}]),
        ),
    ] {
        let other = tempfile::tempdir().unwrap();
        let mut m = manifest(json!([]));
        m[field] = value;
        let repo = plugin_repo(other.path(), m);
        assert!(
            s.install_external_plugin(repo.to_str().unwrap(), None)
                .await
                .is_err(),
            "{field}"
        );
    }
    let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("test.plugins"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(leftovers, vec![std::ffi::OsString::from("tester.echo")]);
}
