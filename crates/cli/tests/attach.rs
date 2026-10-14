//! Isolated CLI lifecycle tests: a fake hub and fake Treer, no harness/inference.
#![cfg(unix)]
use futures_util::{SinkExt, StreamExt};
use pockymoe_protocol::treer::{AttachHello, AttachReply, TREER_ATTACH_PATH};
use std::{os::unix::fs::PermissionsExt, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    process::{Child, Command},
    time,
};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{Request, Response},
        Message,
    },
    WebSocketStream,
};

struct Fixture {
    root: PathBuf,
    listener: TcpListener,
    child: Child,
}
impl Fixture {
    async fn new(args: &[&str], contract: Option<&str>) -> Self {
        let root = std::env::temp_dir().join(format!("pockymoe-attach-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("token"), "synthetic-attach-secret\n").unwrap();
        let fake = root.join("treer");
        std::fs::write(
            &fake,
            r#"#!/bin/sh
printf '%s\n' BEGIN "$@" END >> "$ATTACH_TEST_LOG"
if [ "$1 $2" = 'interface contract' ]; then
  printf '%s\n' "$ATTACH_TEST_CONTRACT"
fi
# A real Treer diagnostic may contain credentials; attach must discard it.
printf '%s\n' 'synthetic-workload-secret' >&2
"#,
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir_all(root.join(".local/bin")).unwrap();
        let launcher = root.join(".local/bin/pockymoe");
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_pockymoe"), &launcher).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let contract = contract.unwrap_or(r#"{"protocol":"treer.agent-interface/v1","features":["agent.interface-provenance.v1","agent.prompt-delivery.v1"],"capabilities":["prompt.submit","abort"]}"#);
        let child = Command::new(launcher)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &root)
            .env("TREER_BIN", &fake)
            .env("RUST_LOG", "trace")
            .env("POCKYMOE_E2E_FAKE_RUNTIME", "1")
            .env("TREER_AGENT_ID", "agent-test")
            .env("TREER_WORKLOAD_CREDENTIAL", "synthetic-workload-secret")
            .env("TREER_AGENT_SERVER_URL", "http://127.0.0.1:4321")
            .env("TREER_WORKSPACE_ID", "workspace-test")
            .env("TREER_SERVER_ID", "server-test")
            .env("ATTACH_TEST_LOG", root.join("calls"))
            .env("ATTACH_TEST_CONTRACT", contract)
            .current_dir(&root)
            .arg("attach")
            .args(args)
            .arg("--hub")
            .arg(format!("http://{}", listener.local_addr().unwrap()))
            .arg("--hub-token-file")
            .arg(root.join("token"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        Self {
            root,
            listener,
            child,
        }
    }
    async fn accept(&self) -> (WebSocketStream<TcpStream>, AttachHello) {
        let (tcp, _) = time::timeout(Duration::from_secs(10), self.listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut ws = accept_hdr_async(tcp, |request: &Request, response: Response| {
            assert_eq!(request.uri().path(), TREER_ATTACH_PATH);
            assert_eq!(
                request.headers()["authorization"],
                "Bearer synthetic-attach-secret"
            );
            Ok(response)
        })
        .await
        .unwrap();
        let frame = time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let hello: AttachHello = serde_json::from_str(frame.to_text().unwrap()).unwrap();
        (ws, hello)
    }
    async fn bound(ws: &mut WebSocketStream<TcpStream>, created: bool) {
        let reply = AttachReply::Bound {
            thread_id: "thread-test".into(),
            created,
            interface_port: 12345,
            ui_path: "/treer/agent".into(),
            capabilities: vec!["prompt.submit".into(), "abort".into()],
        };
        ws.send(Message::Text(serde_json::to_string(&reply).unwrap().into()))
            .await
            .unwrap();
    }
    async fn wait_calls(&self, registrations: usize) -> Vec<Vec<String>> {
        time::timeout(Duration::from_secs(10), async {
            loop {
                let text = std::fs::read_to_string(self.root.join("calls")).unwrap_or_default();
                assert!(!text.contains("synthetic-workload-secret"));
                assert!(!text.contains("synthetic-attach-secret"));
                let calls: Vec<Vec<String>> = text
                    .split("BEGIN\n")
                    .skip(1)
                    .filter_map(|s| {
                        s.strip_suffix("END\n")
                            .map(|s| s.lines().map(str::to_owned).collect())
                    })
                    .collect();
                if calls
                    .iter()
                    .filter(|c| {
                        c.first().map(String::as_str) == Some("interface")
                            && c.get(1).map(String::as_str) == Some("register")
                    })
                    .count()
                    >= registrations
                {
                    return calls;
                }
                time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn create_saves_startup_before_registration_and_reconnect_rebinds() {
    let f = Fixture::new(
        &[
            "--provider",
            "codex",
            "--agent",
            "custom",
            "--model",
            "test-model",
            "--reasoning-effort",
            "high",
            "--title",
            "Test",
        ],
        None,
    )
    .await;
    let (mut ws, hello) = f.accept().await;
    assert_eq!(hello.agent_id, "agent-test");
    assert_eq!(
        hello.treer.workload_credential.as_deref(),
        Some("synthetic-workload-secret")
    );
    assert_eq!(hello.treer.workspace_id.as_deref(), Some("workspace-test"));
    assert_eq!(hello.treer.server_id.as_deref(), Some("server-test"));
    assert!(hello.thread_id.is_none());
    let create = hello.create.unwrap();
    assert_eq!(create.cwd, f.root.to_str().unwrap());
    assert_eq!(create.model.as_deref(), Some("test-model"));
    assert_eq!(create.agent.as_deref(), Some("custom"));
    assert_eq!(create.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(create.title.as_deref(), Some("Test"));
    Fixture::bound(&mut ws, true).await;
    let calls = f.wait_calls(1).await;
    assert_eq!(&calls[0], &["interface", "contract"]);
    assert_eq!(&calls[1][..3], &["agent", "startup", "set"]);
    assert_eq!(
        calls[1][6],
        f.root.join(".local/bin/pockymoe").to_str().unwrap()
    );
    assert!(calls[1]
        .windows(3)
        .any(|w| w == ["attach", "--thread", "thread-test"]));
    assert_eq!(
        &calls[2][..4],
        &["interface", "register", "--port", "12345"]
    );
    assert!(calls[2]
        .windows(2)
        .any(|w| w == ["--instance-id", &hello.instance_id]));
    assert!(calls[2]
        .windows(2)
        .any(|w| w == ["--ui-path", "/treer/agent"]));
    ws.close(None).await.unwrap();
    drop(ws);
    let (mut ws, rebound) = f.accept().await;
    assert_eq!(rebound.instance_id, hello.instance_id);
    assert_eq!(rebound.thread_id.as_deref(), Some("thread-test"));
    assert!(rebound.create.is_none());
    Fixture::bound(&mut ws, false).await;
    let calls = f.wait_calls(2).await;
    assert_eq!(
        calls.len(),
        4,
        "contract/startup run once; register repeats"
    );
}

#[tokio::test]
async fn rebind_registers_without_rewriting_startup_and_new_process_has_fresh_instance() {
    let first = Fixture::new(&["--thread", "thread-test"], None).await;
    let (mut ws, hello) = first.accept().await;
    assert_eq!(hello.thread_id.as_deref(), Some("thread-test"));
    assert!(hello.create.is_none());
    Fixture::bound(&mut ws, false).await;
    assert_eq!(first.wait_calls(1).await.len(), 2);
    let second = Fixture::new(&["--thread", "thread-test"], None).await;
    let (_, hello2) = second.accept().await;
    assert_ne!(hello.instance_id, hello2.instance_id);
}

#[tokio::test]
async fn hub_error_reply_exits_nonzero_without_echoing_secrets() {
    let mut f = Fixture::new(&["--thread", "thread-test"], None).await;
    let (mut ws, _) = f.accept().await;
    let reply = AttachReply::Error {
        code: "bad-binding".into(),
        message: "synthetic-workload-secret".into(),
    };
    ws.send(Message::Text(serde_json::to_string(&reply).unwrap().into()))
        .await
        .unwrap();
    use tokio::io::AsyncReadExt;
    let mut stderr = f.child.stderr.take().unwrap();
    let status = time::timeout(Duration::from_secs(5), f.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(!status.success());
    let mut output = String::new();
    stderr.read_to_string(&mut output).await.unwrap();
    assert!(output.contains("Hub rejected attach binding"));
    assert!(!output.contains("synthetic-workload-secret"));
}

#[tokio::test]
async fn unknown_contract_fails_before_connecting() {
    let mut f = Fixture::new(
        &["--thread", "thread-test"],
        Some(r#"{"protocol":"treer.agent-interface/v1"}"#),
    )
    .await;
    let status = time::timeout(Duration::from_secs(5), f.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(!status.success());
    assert!(
        time::timeout(Duration::from_millis(100), f.listener.accept())
            .await
            .is_err()
    );
}
