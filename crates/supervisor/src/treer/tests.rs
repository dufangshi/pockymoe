use super::*;
use futures_util::SinkExt;
use pockymoe_protocol::Provider;
use pockymoe_runtime::{fake::FakeRuntime, Database, RuntimeConfig};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, Message as WireMessage},
    MaybeTlsStream, WebSocketStream,
};
type ClientSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;
struct Fixture {
    _dir: tempfile::TempDir,
    hub: Arc<Hub>,
    port: u16,
    server: tokio::task::JoinHandle<()>,
    fake: Arc<FakeRuntime>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn fixture() -> Fixture {
    fixture_in(tempfile::tempdir().unwrap()).await
}
async fn fixture_in(dir: tempfile::TempDir) -> Fixture {
    fixture_with_auth(dir, false).await
}
async fn fixture_with_auth(dir: tempfile::TempDir, authenticated_mode: bool) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = RuntimeConfig {
        treer_enabled: true,
        host_mode: pockymoe_runtime::host_mode::HostMode::Hosted,
        mode: if authenticated_mode {
            pockymoe_protocol::Mode::Server
        } else {
            pockymoe_protocol::Mode::Local
        },
        host: "127.0.0.1".into(),
        port,
        workspace_root: dir.path().into(),
        database_url: dir.path().join("db.sqlite"),
        app_name: "test".into(),
        app_version: "test".into(),
        environment: "test".into(),
        auth_required: authenticated_mode,
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
    let db = Database::open(&config.database_url).unwrap();
    let fake = Arc::new(FakeRuntime::new(Provider::Codex));
    let state = Arc::new(Supervisor::new(config, db, vec![fake.clone()]));
    state.configure_cli(format!("http://127.0.0.1:{port}"));
    state.spawn_live_item_persister();
    let hub = Hub::new(state.clone(), port).unwrap().unwrap();
    let app = crate::http::router_with_treer(state, Some(hub.clone()));
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Fixture {
        _dir: dir,
        hub,
        port,
        server,
        fake,
    }
}
fn hello(f: &Fixture, agent: &str) -> AttachHello {
    AttachHello {
        protocol_version: 1,
        agent_id: agent.into(),
        instance_id: format!("instance-{agent}"),
        thread_id: None,
        create: Some(AttachCreateThread {
            cwd: f.hub.state.config.workspace_root.to_string_lossy().into(),
            provider: "codex".into(),
            agent: None,
            model: Some("ios-e2e-stream".into()),
            reasoning_effort: Some("high".into()),
            title: Some("Treer test".into()),
        }),
        treer: AttachTreerIdentity {
            agent_server_url: Some("http://127.0.0.1:9".into()),
            workload_credential: Some("fixture-credential".into()),
            workspace_id: Some("workspace".into()),
            server_id: Some("server".into()),
        },
    }
}
async fn attach_client(f: &Fixture, h: &AttachHello) -> (ClientSocket, Value) {
    let mut req = format!("ws://127.0.0.1:{}/api/treer/attach", f.port)
        .into_client_request()
        .unwrap();
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {}", f.hub.token).parse().unwrap(),
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    ws.send(WireMessage::Text(serde_json::to_string(h).unwrap().into()))
        .await
        .unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    let reply = serde_json::from_str(reply.to_text().unwrap()).unwrap();
    (ws, reply)
}
fn request(
    f: &Fixture,
    h: &AttachHello,
    method: reqwest::Method,
    path: &str,
    ui: bool,
) -> reqwest::RequestBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .request(method, format!("http://127.0.0.1:{}{path}", f.port))
        .header(
            "host",
            if ui {
                format!("127.0.0.1:{}", f.port)
            } else {
                "agent-interface".into()
            },
        )
        .header(
            "x-treer-interface-source",
            if ui { "proxy-ui" } else { "controller-api" },
        )
        .header("x-treer-agent-id", &h.agent_id)
        .header("x-treer-interface-instance", &h.instance_id)
}
async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        while !predicate() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn attach_create_rebind_refuse_version_token_and_identity() {
    let f = fixture().await;
    let h = hello(&f, "ag-one");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(f._dir.path().join(TREER_ATTACH_TOKEN_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(!f.hub.token_matches("invalid"));
    assert!(f.hub.token_matches(&f.hub.token));
    let mut bad = format!("ws://127.0.0.1:{}/api/treer/attach", f.port)
        .into_client_request()
        .unwrap();
    bad.headers_mut()
        .insert("authorization", "Bearer wrong".parse().unwrap());
    assert!(
        matches!(tokio_tungstenite::connect_async(bad).await,Err(tokio_tungstenite::tungstenite::Error::Http(r)) if r.status()==401)
    );
    let (mut ws, reply) = attach_client(&f, &h).await;
    assert_eq!(reply["type"], "bound");
    assert_eq!(reply["created"], true);
    assert_eq!(reply["interfacePort"], f.port);
    let thread = reply["threadId"].as_str().unwrap().to_owned();
    let env = f
        .hub
        .state
        .with_cli_context(&thread, async {
            pockymoe_runtime::interaction::launch_env()
        })
        .await;
    assert_eq!(
        env.iter().find(|(k, _)| k == "TREER_AGENT_ID").unwrap().1,
        "ag-one"
    );
    assert_eq!(
        env.iter()
            .find(|(k, _)| k == "TREER_WORKLOAD_CREDENTIAL")
            .unwrap()
            .1,
        "fixture-credential"
    );
    let child = f
        .hub
        .state
        .create_thread(CreateThreadInput {
            workspace_id: f.hub.state.get_thread(&thread).unwrap().workspace_id,
            title: None,
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: Some(thread.clone()),
        })
        .await
        .unwrap();
    assert!(f
        .hub
        .state
        .set_thread_cli_identity(&child.id, env.clone())
        .is_err());
    let child_env = f
        .hub
        .state
        .with_cli_context(&child.id, async {
            pockymoe_runtime::interaction::launch_env()
        })
        .await;
    assert!(!child_env.iter().any(|(k, _)| k.starts_with("TREER_")));
    let mut collision = hello(&f, "ag-two");
    collision.thread_id = Some(thread.clone());
    collision.create = None;
    let (_, refused) = attach_client(&f, &collision).await;
    assert_eq!(refused["type"], "error");
    let mut child_hello = collision.clone();
    child_hello.thread_id = Some(child.id);
    let (_, refused) = attach_client(&f, &child_hello).await;
    assert_eq!(refused["type"], "error");
    let mut version = hello(&f, "ag-v");
    version.protocol_version = 999;
    let (_, refused) = attach_client(&f, &version).await;
    assert_eq!(refused["code"], "unsupported_protocol");
    ws.close(None).await.unwrap();
    wait_until(|| f.hub.live.lock().unwrap().is_empty()).await;
    wait_until(|| f.hub.state.integration_thread_paused(&thread)).await;
    let env = f
        .hub
        .state
        .with_cli_context(&thread, async {
            pockymoe_runtime::interaction::launch_env()
        })
        .await;
    assert!(!env.iter().any(|(k, _)| k.starts_with("TREER_")));
    let mut rebind = h.clone();
    rebind.instance_id = "new-instance".into();
    rebind.thread_id = Some(thread.clone());
    rebind.create = None;
    let (mut ws, reply) = attach_client(&f, &rebind).await;
    assert_eq!(reply["threadId"], thread);
    assert_eq!(reply["created"], false);
    ws.close(None).await.unwrap();
    let persisted = f
        .hub
        .state
        .db
        .with(|c| {
            Ok(c.query_row(
                "SELECT agent_id FROM treer_bindings WHERE thread_id=?1",
                [&thread],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap();
    assert_eq!(persisted, h.agent_id);
    // Agent restart without --thread reuses its durable binding instead of making another thread.
    wait_until(|| f.hub.live.lock().unwrap().is_empty()).await;
    let (mut ws, reply) = attach_client(&f, &h).await;
    assert_eq!(reply["threadId"], thread);
    assert_eq!(reply["created"], false);
    ws.close(None).await.unwrap();
}
#[tokio::test]
async fn ais_prompt_idempotency_queue_cancel_steer_transcript_and_detach_release() {
    use pockymoe_runtime::actor::AgentRuntime;
    let f = fixture().await;
    let h = hello(&f, "ag-one");
    let (mut ws, bound) = attach_client(&f, &h).await;
    let thread = bound["threadId"].as_str().unwrap();
    let manifest: Value = request(&f, &h, reqwest::Method::GET, "/v1/manifest", false)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(manifest["protocol"], TREER_AGENT_INTERFACE_PROTOCOL);
    assert_eq!(manifest["instance_id"], h.instance_id);
    for capability in ["prompt.queue", "prompt.queue.read", "prompt.notify"] {
        assert!(manifest["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!(capability)));
        assert!(bound["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!(capability)));
    }
    assert_eq!(
        request(&f, &h, reqwest::Method::GET, "/v1/health", false)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let prompt = json!({"operation_id":"op-one","text":"inspect this repository","mode":"prompt","delivery":"queue","sender":{"agent_id":"sender","path":"/sender","name":"Sender"},"notify":{"on":[],"to":"sender"}});
    let receipt: Value = request(&f, &h, reqwest::Method::POST, "/v1/prompts", false)
        .json(&prompt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = receipt["submission"]["id"].as_str().unwrap();
    assert_eq!(receipt["submission"]["disposition"], "queued");
    let duplicate: Value = request(&f, &h, reqwest::Method::POST, "/v1/prompts", false)
        .json(&prompt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(duplicate["submission"]["id"], id);
    assert_eq!(duplicate["duplicate"], true);
    let mut conflict = prompt.clone();
    conflict["text"] = json!("changed");
    assert_eq!(
        request(&f, &h, reqwest::Method::POST, "/v1/prompts", false)
            .json(&conflict)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    wait_until(|| {
        f.hub
            .state
            .get_thread(thread)
            .unwrap()
            .active_turn_id
            .is_some()
    })
    .await;
    let status: Value = request(&f, &h, reqwest::Method::GET, "/v1/status", false)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["status"], "working");
    assert_eq!(status["busy"], true);
    let queued: Value = request(&f, &h, reqwest::Method::POST, "/v1/prompts", false)
        .json(&json!({"operation_id":"op-two","text":"second","mode":"prompt","delivery":"queue"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second = queued["submission"]["id"].as_str().unwrap();
    let queue: Value = request(&f, &h, reqwest::Method::GET, "/v1/queue", false)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(queue["submissions"].as_array().unwrap().len(), 1);
    assert_eq!(queue["submissions"][0]["id"], second);
    let cancelled: Value = request(
        &f,
        &h,
        reqwest::Method::DELETE,
        &format!("/v1/queue/{second}"),
        false,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(cancelled["submission"]["state"], "interrupted");
    assert_eq!(
        request(
            &f,
            &h,
            reqwest::Method::DELETE,
            &format!("/v1/queue/{id}"),
            false
        )
        .send()
        .await
        .unwrap()
        .status(),
        409
    );
    let steer:Value=request(&f,&h,reqwest::Method::POST,"/v1/prompts",false).json(&json!({"operation_id":"op-steer","text":"correction","mode":"prompt","delivery":"steer"})).send().await.unwrap().json().await.unwrap();
    assert_eq!(steer["submission"]["disposition"], "steered");
    let transcript: Value = request(
        &f,
        &h,
        reqwest::Method::GET,
        "/v1/transcript?page=0&cursor=0&limit=1",
        false,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(transcript["page_count"], 1);
    assert_eq!(transcript["cursor"], "0");
    assert_eq!(transcript["entries"][0]["role"], "user");
    assert!(!f.fake.loaded_sessions().await.is_empty());
    ws.close(None).await.unwrap();
    wait_until(|| {
        f.hub
            .state
            .get_thread(thread)
            .unwrap()
            .active_turn_id
            .is_none()
    })
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        while !f.fake.loaded_sessions().await.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(f.hub.state.get_thread(thread).is_ok());
    assert_eq!(
        f.hub
            .state
            .get_thread_detail(thread, None)
            .await
            .unwrap()
            .turns
            .len(),
        1
    );
    assert!(f.hub.state.integration_thread_paused(thread));
    assert_eq!(
        request(&f, &h, reqwest::Method::GET, "/v1/status", false)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let mut resume = h.clone();
    resume.instance_id = "resume".into();
    resume.thread_id = Some(thread.into());
    resume.create = None;
    let (mut ws, _) = attach_client(&f, &resume).await;
    let saved: Value = request(
        &f,
        &resume,
        reqwest::Method::GET,
        &format!("/v1/submissions/{id}"),
        false,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(saved["submission"]["state"], "interrupted");
    assert_eq!(saved["submission"]["sender"]["agent_id"], "sender");
    let duplicate: Value = request(&f, &resume, reqwest::Method::POST, "/v1/prompts", false)
        .json(&prompt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(duplicate["submission"]["id"], id);
    ws.close(None).await.unwrap();
}
#[tokio::test]
async fn gate_provenance_matrix_http_and_websocket() {
    let f = fixture().await;
    let h = hello(&f, "ag-one");
    let (mut attach, bound) = attach_client(&f, &h).await;
    let root = bound["threadId"].as_str().unwrap();
    let other = hello(&f, "ag-other");
    let (mut other_socket, other_bound) = attach_client(&f, &other).await;
    let other_id = other_bound["threadId"].as_str().unwrap();
    let child = f
        .hub
        .state
        .create_thread(CreateThreadInput {
            workspace_id: f.hub.state.get_thread(root).unwrap().workspace_id,
            title: None,
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: Some(root.into()),
        })
        .await
        .unwrap();
    let hidden_dir = f._dir.path().join("hidden-workspace");
    std::fs::create_dir(&hidden_dir).unwrap();
    let hidden_workspace = f
        .hub
        .state
        .create_workspace(CreateWorkspaceInput {
            abs_path: Some(hidden_dir.to_string_lossy().into()),
            label: None,
            git_url: None,
        })
        .unwrap();
    let unbound = f
        .hub
        .state
        .create_thread(CreateThreadInput {
            workspace_id: hidden_workspace.id.clone(),
            title: Some("Unbound".into()),
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            parent_thread_id: None,
        })
        .await
        .unwrap();
    for path in [
        format!("/api/threads/{root}"),
        format!("/api/threads/{other_id}"),
        format!("/api/threads/{}", child.id),
        "/api/auth/session".into(),
        "/api/config/runtime".into(),
    ] {
        assert_eq!(
            request(&f, &h, reqwest::Method::GET, &path, true)
                .send()
                .await
                .unwrap()
                .status(),
            200,
            "{path}"
        );
    }
    for (method, path) in [
        (reqwest::Method::GET, format!("/api/threads/{}", unbound.id)),
        (reqwest::Method::GET, "/api/management/supervisor".into()),
        (
            reqwest::Method::PATCH,
            "/api/config/workspace-settings".into(),
        ),
        (reqwest::Method::GET, "/api/device/metrics".into()),
        (reqwest::Method::POST, "/api/cli".into()),
        (reqwest::Method::GET, "/v1/manifest".into()),
    ] {
        assert_eq!(
            request(&f, &h, method, &path, true)
                .send()
                .await
                .unwrap()
                .status(),
            403,
            "{path}"
        );
    }
    let pending = |id: &str| pockymoe_protocol::ThreadActionRequestDto {
        id: id.into(),
        kind: "approval".into(),
        title: "Approval".into(),
        description: None,
        turn_id: None,
        item_id: None,
        created_at: now_rfc3339(),
        questions: vec![],
    };
    f.fake
        .seed_pending_requests(other_id, vec![pending("other-request")]);
    f.fake
        .seed_pending_requests(root, vec![pending("root-request")]);
    let status: Value = request(&f, &h, reqwest::Method::GET, "/v1/status", false)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["status"], "blocked");
    assert_eq!(
        request(
            &f,
            &h,
            reqwest::Method::POST,
            &format!("/api/threads/{root}/requests/other-request/respond"),
            true
        )
        .json(&json!({"allow":true}))
        .send()
        .await
        .unwrap()
        .status(),
        404
    );
    f.fake.seed_pending_requests(root, vec![]);
    let listing: Value = request(&f, &h, reqwest::Method::GET, "/api/threads", true)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listing.as_array().unwrap().len(), 3);
    assert!(listing
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"] == other_id));
    assert!(!listing
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"] == unbound.id));
    let workspaces: Value = request(&f, &h, reqwest::Method::GET, "/api/workspaces", true)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(workspaces.as_array().unwrap().len(), 1);
    assert!(!workspaces
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["id"] == hidden_workspace.id));
    let redirect = request(&f, &h, reqwest::Method::GET, "/", true)
        .send()
        .await
        .unwrap();
    assert_eq!(redirect.status(), 307);
    assert_eq!(redirect.headers()["location"], format!("threads/{root}"));
    for headers in [
        [("origin", "https://bad.test")],
        [("sec-fetch-site", "cross-site")],
        [("x-treer-interface-instance", "wrong")],
        [("host", "bad.test")],
    ] {
        let (key, value) = headers[0];
        assert_eq!(
            request(&f, &h, reqwest::Method::GET, "/v1/status", false)
                .header(key, value)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    assert_eq!(
        reqwest::get(format!("http://127.0.0.1:{}/v1/status", f.port))
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        reqwest::get(format!(
            "http://127.0.0.1:{}/api/threads/{other_id}",
            f.port
        ))
        .await
        .unwrap()
        .status(),
        200
    );
    let mut req = format!("ws://127.0.0.1:{}/ws", f.port)
        .into_client_request()
        .unwrap();
    for (key, value) in [
        ("x-treer-interface-source", "proxy-ui"),
        ("x-treer-agent-id", h.agent_id.as_str()),
        ("x-treer-interface-instance", h.instance_id.as_str()),
    ] {
        req.headers_mut().insert(key, value.parse().unwrap());
    }
    let (mut socket, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let event = socket.next().await.unwrap().unwrap();
    assert!(event.to_text().unwrap().contains("supervisor.connected"));
    for id in [unbound.id.as_str(), other_id, root, child.id.as_str()] {
        f.hub
            .state
            .bus
            .emit(pockymoe_protocol::ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
                thread_id: id.into(),
                timestamp: now_rfc3339(),
                payload: json!({"marker":id}),
            });
    }
    for expected in [other_id, root, child.id.as_str()] {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let event: Value = serde_json::from_str(event.to_text().unwrap()).unwrap();
        assert_eq!(event["threadId"], expected);
    }
    // Unscoped shell commands cannot subscribe to another thread's process output.
    socket
        .send(WireMessage::Text(
            json!({"type":"shell.attach","shellId":"other"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    socket
        .send(WireMessage::Text(
            json!({"type":"supervisor.ping"}).to_string().into(),
        ))
        .await
        .unwrap();
    let event = socket.next().await.unwrap().unwrap();
    assert!(event.to_text().unwrap().contains("supervisor.pong"));
    attach.close(None).await.unwrap();
    other_socket.close(None).await.unwrap();
    socket.close(None).await.unwrap();
}
#[test]
fn byte_bounded_previews() {
    assert_eq!(bounded("aé字z", 5), "aé");
}

#[tokio::test]
async fn queued_work_and_receipts_survive_hub_restart_and_wait_for_attach() {
    use pockymoe_runtime::actor::AgentRuntime;
    let mut f = fixture().await;
    let h = hello(&f, "ag-restart");
    let (mut ws, reply) = attach_client(&f, &h).await;
    let root = reply["threadId"].as_str().unwrap().to_owned();
    let first: Value=request(&f,&h,reqwest::Method::POST,"/v1/prompts",false).json(&json!({"operation_id":"long","text":"inspect this repository","mode":"prompt","delivery":"queue"})).send().await.unwrap().json().await.unwrap();
    wait_until(|| {
        f.hub
            .state
            .get_thread(&root)
            .unwrap()
            .active_turn_id
            .is_some()
    })
    .await;
    let prompt = json!({"operation_id":"queued","text":"hello","mode":"prompt","delivery":"queue"});
    let queued: Value = request(&f, &h, reqwest::Method::POST, "/v1/prompts", false)
        .json(&prompt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = queued["submission"]["id"].as_str().unwrap().to_owned();
    let old_token = f.hub.token.clone();
    ws.close(None).await.unwrap();
    wait_until(|| f.hub.live.lock().unwrap().is_empty()).await;
    assert_eq!(
        f.hub.submission(&root, &id).unwrap().unwrap()["state"],
        "queued"
    );
    assert_eq!(
        f.hub
            .submission(&root, first["submission"]["id"].as_str().unwrap())
            .unwrap()
            .unwrap()["state"],
        "interrupted"
    );
    let dir = std::mem::replace(&mut f._dir, tempfile::tempdir().unwrap());
    let old_state = Arc::downgrade(&f.hub.state);
    let server = std::mem::replace(&mut f.server, tokio::spawn(async {}));
    server.abort();
    let _ = server.await;
    drop(f);
    // Wait for socket/worker teardown, not just listener cancellation: SQLite
    // ownership must be released before simulating a new hub process.
    wait_until(|| old_state.upgrade().is_none()).await;
    let f = fixture_in(dir).await;
    assert!(f.hub.token == old_token);
    assert!(f.hub.state.integration_thread_paused(&root));
    assert_eq!(
        f.hub.submission(&root, &id).unwrap().unwrap()["state"],
        "queued"
    );
    assert!(f.fake.loaded_sessions().await.is_empty());
    let mut resumed = h;
    resumed.thread_id = Some(root.clone());
    resumed.create = None;
    resumed.instance_id = "after-restart".into();
    let (mut ws, reply) = attach_client(&f, &resumed).await;
    assert_eq!(reply["threadId"], root);
    let receipt: Value = request(&f, &resumed, reqwest::Method::POST, "/v1/prompts", false)
        .json(&prompt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(receipt["submission"]["id"], id);
    wait_until(|| f.hub.submission(&root, &id).unwrap().unwrap()["state"] == "done").await;
    assert_eq!(
        f.hub
            .state
            .get_thread_detail(&root, None)
            .await
            .unwrap()
            .turns
            .len(),
        2
    );
    let page: Value = request(
        &f,
        &resumed,
        reqwest::Method::GET,
        "/v1/transcript?cursor=1&limit=1",
        false,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(page["page_count"], 2);
    assert_eq!(page["page"], 1);
    assert_eq!(page["next_page"], Value::Null);
    assert_eq!(page["entries"][0]["content"], "hello");
    ws.close(None).await.unwrap();
}

#[tokio::test]
async fn forwarding_headers_without_loopback_transport_never_grant_scope() {
    use tower::ServiceExt;
    let f = fixture().await;
    let h = hello(&f, "ag-local");
    let (mut ws, _) = attach_client(&f, &h).await;
    let app = crate::http::router_with_treer(f.hub.state.clone(), Some(f.hub.clone()));
    for peer in [None, Some("192.0.2.1:12345".parse::<SocketAddr>().unwrap())] {
        let mut req = axum::http::Request::builder()
            .uri("/v1/status")
            .header("host", "agent-interface")
            .header("x-treer-interface-source", "controller-api")
            .header("x-treer-agent-id", &h.agent_id)
            .header("x-treer-interface-instance", &h.instance_id)
            .body(axum::body::Body::empty())
            .unwrap();
        if let Some(peer) = peer {
            req.extensions_mut().insert(ConnectInfo(peer));
        }
        assert_eq!(app.clone().oneshot(req).await.unwrap().status(), 403);
    }
    ws.close(None).await.unwrap();
}

#[tokio::test]
async fn failed_attach_persistence_releases_the_new_identity_bearing_harness() {
    use pockymoe_runtime::actor::AgentRuntime;
    for table in ["threads", "treer_bindings"] {
        let f = fixture().await;
        f.hub.state.db.with(|c| {c.execute_batch(&format!("CREATE TRIGGER fail_attach BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'fixture persistence failure'); END;"))?;Ok(())}).unwrap();
        let (_ws, reply) = attach_client(&f, &hello(&f, "ag-fail")).await;
        assert_eq!(reply["type"], "error");
        assert!(f.hub.live.lock().unwrap().is_empty());
        assert!(f.fake.loaded_sessions().await.is_empty());
        for thread in f.hub.state.list_threads(None, true).unwrap() {
            let env = f
                .hub
                .state
                .with_cli_context(&thread.id, async {
                    pockymoe_runtime::interaction::launch_env()
                })
                .await;
            assert!(!env.iter().any(|(key, _)| key.starts_with("TREER_")));
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn notifications_retry_durably_using_only_the_live_bound_identity() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = fixture().await;
    let h = hello(&f, "ag-notify");
    let (mut ws, reply) = attach_client(&f, &h).await;
    let root = reply["threadId"].as_str().unwrap().to_owned();
    let prompt = json!({"operation_id":"notify-op","text":"hello","mode":"prompt","delivery":"queue","sender":{"agent_id":"sender","path":"/sender","name":"Sender"},"notify":{"on":["done"],"to":null}});
    let receipt: Value = request(&f, &h, reqwest::Method::POST, "/v1/prompts", false)
        .json(&prompt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = receipt["submission"]["id"].as_str().unwrap().to_owned();
    wait_until(|| f.hub.submission(&root, &id).unwrap().unwrap()["state"] == "done").await;
    let executable = f._dir.path().join("fake-treer");
    let marker = f._dir.path().join("attempt");
    let payload = f._dir.path().join("message.json");
    // Synthetic fixture credential is checked in the process; it never enters the recorded message.
    std::fs::write(&executable,format!("#!/bin/sh\n[ \"$TREER_AGENT_ID\" = ag-notify ] || exit 2\n[ \"$TREER_WORKLOAD_CREDENTIAL\" = fixture-credential ] || exit 3\n[ \"$1 $2 $3 $4 $5 $6\" = 'message send --to sender --idempotency-key prompt-submission/{id}/terminal' ] || exit 4\nif [ ! -f '{}' ]; then touch '{}'; exit 1; fi\ncat > '{}'\n",marker.display(),marker.display(),payload.display())).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    f.hub
        .deliver_notifications_with(executable.as_os_str())
        .await;
    fn delivery_state(hub: &Hub, id: &str) -> (bool, u32, i64) {
        hub.state.db.with(|c| {
            Ok(c.query_row(
                "SELECT notified,notification_attempts,notification_next_attempt FROM treer_submissions WHERE id=?1",
                [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        }).unwrap()
    }
    let scheduled = delivery_state(&f.hub, &id);
    assert!(!scheduled.0);
    assert_eq!(scheduled.1, 1);
    assert!(scheduled.2 > chrono::Utc::now().timestamp_millis());
    f.hub
        .deliver_notifications_with(executable.as_os_str())
        .await;
    assert_eq!(delivery_state(&f.hub, &id), scheduled);
    assert!(!payload.exists());

    // A new hub retains the retry schedule and terminal result, and never reruns
    // the completed task. A changed socket instance supplies the current identity.
    ws.close(None).await.unwrap();
    wait_until(|| f.hub.live.lock().unwrap().is_empty()).await;
    let dir = std::mem::replace(&mut f._dir, tempfile::tempdir().unwrap());
    let old_state = Arc::downgrade(&f.hub.state);
    let server = std::mem::replace(&mut f.server, tokio::spawn(async {}));
    server.abort();
    let _ = server.await;
    drop(f);
    wait_until(|| old_state.upgrade().is_none()).await;
    let f = fixture_in(dir).await;
    assert_eq!(delivery_state(&f.hub, &id), scheduled);
    assert_eq!(
        f.hub.submission(&root, &id).unwrap().unwrap()["state"],
        "done"
    );
    let mut h = hello(&f, "ag-notify");
    h.instance_id = "replacement-instance".into();
    let (mut ws, bound) = attach_client(&f, &h).await;
    assert_eq!(bound["threadId"], root);
    assert_eq!(
        f.hub
            .state
            .get_thread_detail(&root, None)
            .await
            .unwrap()
            .turns
            .len(),
        1
    );
    // Advance only the persisted deadline instead of sleeping through backoff.
    f.hub
        .state
        .db
        .with(|c| {
            c.execute(
                "UPDATE treer_submissions SET notification_next_attempt=0 WHERE id=?1",
                [&id],
            )?;
            Ok(())
        })
        .unwrap();
    f.hub
        .deliver_notifications_with(executable.as_os_str())
        .await;
    assert!(delivery_state(&f.hub, &id).0);
    let message: Value = serde_json::from_slice(&std::fs::read(&payload).unwrap()).unwrap();
    assert_eq!(message["submission_id"], id);
    assert_eq!(message["status"], "done");
    std::fs::remove_file(&payload).unwrap();
    f.hub
        .deliver_notifications_with(executable.as_os_str())
        .await;
    assert!(!payload.exists());
    ws.close(None).await.unwrap();
}

#[tokio::test]
async fn embedded_scope_supplies_server_session_without_granting_unmarked_access() {
    let f = fixture_with_auth(tempfile::tempdir().unwrap(), true).await;
    let h = hello(&f, "ag-auth");
    let (mut ws, reply) = attach_client(&f, &h).await;
    let root = reply["threadId"].as_str().unwrap();
    assert_eq!(
        reqwest::get(format!("http://127.0.0.1:{}/api/threads/{root}", f.port))
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        request(
            &f,
            &h,
            reqwest::Method::GET,
            &format!("/api/threads/{root}"),
            true
        )
        .send()
        .await
        .unwrap()
        .status(),
        200
    );
    let session: Value = request(&f, &h, reqwest::Method::GET, "/api/auth/session", true)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["authenticated"], true);
    assert_eq!(session["authRequired"], false);
    assert_eq!(
        request(
            &f,
            &h,
            reqwest::Method::GET,
            "/api/management/supervisor",
            true
        )
        .send()
        .await
        .unwrap()
        .status(),
        403
    );
    assert_eq!(
        request(&f, &h, reqwest::Method::GET, "/v1/status", false)
            .header("x-treer-agent-id", "unknown")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    ws.close(None).await.unwrap();
}

#[tokio::test]
async fn detach_interrupts_even_when_the_harness_is_already_unloaded() {
    use pockymoe_runtime::actor::AgentRuntime;
    let f = fixture().await;
    let h = hello(&f, "ag-unloaded");
    let (mut ws, reply) = attach_client(&f, &h).await;
    let root = reply["threadId"].as_str().unwrap();
    let response = request(&f, &h, reqwest::Method::POST, "/v1/prompts", false)
        .json(&json!({
            "operation_id": "op-unloaded", "text": "inspect this repository",
            "mode": "prompt", "delivery": "queue"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    wait_until(|| {
        f.hub
            .state
            .get_thread(root)
            .unwrap()
            .active_turn_id
            .is_some()
    })
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !f.fake.loaded_sessions().await.iter().any(|session| {
            session
                .busy
                .contains(&pockymoe_protocol::HarnessBusyReason::Turn)
        }) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.fake.simulate_process_exit();
    assert!(f.fake.loaded_sessions().await.is_empty());
    ws.close(None).await.unwrap();
    wait_until(|| f.hub.live.lock().unwrap().is_empty()).await;
    wait_until(|| {
        f.hub
            .state
            .get_thread(root)
            .unwrap()
            .active_turn_id
            .is_none()
    })
    .await;
    assert_eq!(
        f.hub
            .state
            .get_thread_detail(root, None)
            .await
            .unwrap()
            .turns[0]
            .status,
        "interrupted"
    );
}

#[tokio::test]
async fn notification_retry_schema_upgrades_existing_hubs() {
    let f = fixture().await;
    f.hub
        .state
        .db
        .with(|c| {
            c.execute_batch(
                "ALTER TABLE treer_submissions DROP COLUMN notification_attempts;
             ALTER TABLE treer_submissions DROP COLUMN notification_next_attempt;",
            )?;
            Ok(())
        })
        .unwrap();
    for _ in 0..2 {
        let upgraded = Hub::new(f.hub.state.clone(), f.port).unwrap().unwrap();
        upgraded
            .state
            .db
            .with(|c| {
                c.prepare(
                    "SELECT notification_attempts,notification_next_attempt FROM treer_submissions",
                )?;
                Ok(())
            })
            .unwrap();
    }
}

#[tokio::test]
async fn runtime_config_exposes_scope_only_to_bound_proxy_ui() {
    let f = fixture().await;
    let h = hello(&f, "ag-scope-config");
    let (_ws, reply) = attach_client(&f, &h).await;
    let scoped: Value = request(&f, &h, reqwest::Method::GET, "/api/config/runtime", true)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        scoped["hostScope"],
        json!({
            "kind": "treerAgent", "boundThreadId": reply["threadId"],
            "hubId": f.hub.hub_id,
            "allowed": {"boundThreads":true,"childThreads":true,"topLevelThreads":true,
                "workspaceCreation":false,"management":false,"workspaceFiles":"read"}
        })
    );
    let unscoped: Value = reqwest::get(format!("http://127.0.0.1:{}/api/config/runtime", f.port))
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(unscoped.get("hostScope").is_none());
    for (method, path) in [
        (reqwest::Method::POST, "/api/threads"),
        (reqwest::Method::POST, "/api/workspaces"),
        (reqwest::Method::GET, "/api/management/supervisor"),
    ] {
        let response = request(&f, &h, method, path, true).send().await.unwrap();
        assert_eq!(response.status(), 403);
        assert_eq!(
            response.json::<Value>().await.unwrap()["code"],
            "thread_scope_forbidden"
        );
    }
    assert_eq!(
        request(&f, &h, reqwest::Method::GET, "/api/config/runtime", false)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        request(&f, &h, reqwest::Method::GET, "/api/config/runtime", true)
            .header("x-treer-agent-id", "unknown")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
}

#[cfg(unix)]
fn fake_browser_treer(f: &Fixture, fail: bool) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = f._dir.path().join("fake-treer");
    let log = f._dir.path().join("treer-argv.jsonl");
    // The fake checks credentials internally and records only non-secret argv.
    std::fs::write(
        &script,
        format!(
            r##"#!/usr/bin/env python3
import json, os, sys, sqlite3
assert os.environ['TREER_AGENT_ID'] == 'ag-browser'
assert os.environ['TREER_WORKLOAD_CREDENTIAL'] == 'fixture-credential'
assert 'TREER_OPERATOR_CREDENTIAL' not in os.environ
args=sys.argv[1:]
if args == ['whoami']:
    print(json.dumps({{'machine': {{'server_id':'server', 'root': {root}}}}}))
else:
    db=sqlite3.connect({db})
    ids=[r[0] for r in db.execute('SELECT id FROM threads')]
    with open({log}, 'a') as out: out.write(json.dumps({{'args':args, 'existing':ids}})+'\n')
    if {fail}: sys.exit(1)
    print(json.dumps({{'agent_id':'ag-created'}}))
"##,
            root = serde_json::to_string(&f._dir.path().to_string_lossy()).unwrap(),
            db = serde_json::to_string(&f.hub.state.config.database_url.to_string_lossy()).unwrap(),
            log = serde_json::to_string(&log.to_string_lossy()).unwrap(),
            fail = if fail { "True" } else { "False" }
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    *f.hub.browser_commands.lock().unwrap() = browser::Commands {
        treer: script,
        pockymoe: Some("/stable/pockymoe".into()),
    };
    log
}

#[cfg(unix)]
#[tokio::test]
async fn browser_create_rename_delete_sync_agents_with_live_identity() {
    let f = fixture().await;
    let h = hello(&f, "ag-browser");
    let (_ws, reply) = attach_client(&f, &h).await;
    let root = reply["threadId"].as_str().unwrap();
    let workspace = f.hub.state.get_thread(root).unwrap().workspace_id;
    let log = fake_browser_treer(&f, false);
    let response = request(&f, &h, reqwest::Method::POST, "/api/threads/start", true)
        .json(&json!({"workspaceId":workspace,"provider":"codex","model":"ios-e2e-stream","title":"reply / 1"}))
        .send().await.unwrap();
    assert_eq!(response.status(), 200);
    let created: Value = response.json().await.unwrap();
    let id = created["id"].as_str().unwrap();
    let records = || {
        std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>()
    };
    let args = records()[0]["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        &args[..7],
        [
            "agent",
            "admin",
            "create",
            "--machine",
            "server",
            "--kind",
            "command"
        ]
    );
    assert_eq!(args[9], "--cwd");
    assert_eq!(args[10], ".");
    assert_eq!(
        &args[11..],
        [
            "--",
            "/stable/pockymoe",
            "attach",
            "--thread",
            id,
            "--hub",
            &format!("http://127.0.0.1:{}", f.port),
            "--hub-token-file",
            f._dir
                .path()
                .join(TREER_ATTACH_TOKEN_FILE)
                .to_str()
                .unwrap()
        ]
    );
    assert!(!args.iter().any(|a| a == "--folder"));
    let env = f
        .hub
        .state
        .with_cli_context(id, async { pockymoe_runtime::interaction::launch_env() })
        .await;
    assert!(!env.iter().any(|(key, _)| key.starts_with("TREER_")));
    // Its detached binding is already visible, and its later attach reuses it.
    let list: Value = request(&f, &h, reqwest::Method::GET, "/api/threads", true)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list.as_array().unwrap().iter().any(|t| t["id"] == id));
    let mut new_hello = hello(&f, "ag-created");
    new_hello.create = None;
    new_hello.thread_id = Some(id.into());
    let (_created_socket, bound) = attach_client(&f, &new_hello).await;
    assert_eq!(bound["threadId"], id);
    assert_eq!(bound["created"], false);
    assert_eq!(
        request(
            &f,
            &h,
            reqwest::Method::PATCH,
            &format!("/api/threads/{id}"),
            true
        )
        .json(&json!({"title":"renamed"}))
        .send()
        .await
        .unwrap()
        .status(),
        200
    );
    assert_eq!(f.hub.state.get_thread(id).unwrap().title, "renamed");
    assert_eq!(
        records()[1]["args"],
        json!([
            "agent",
            "admin",
            "rename",
            "ag-created",
            pockymoe_treer::agent_label("renamed", id)
        ])
    );
    // A failed Treer delete/rename must preserve the Pockymoe thread and title.
    fake_browser_treer(&f, true);
    assert_eq!(
        request(
            &f,
            &h,
            reqwest::Method::PATCH,
            &format!("/api/threads/{id}"),
            true
        )
        .json(&json!({"title":"failed rename"}))
        .send()
        .await
        .unwrap()
        .status(),
        502
    );
    assert_eq!(f.hub.state.get_thread(id).unwrap().title, "renamed");
    assert_eq!(
        request(
            &f,
            &h,
            reqwest::Method::DELETE,
            &format!("/api/threads/{id}"),
            true
        )
        .send()
        .await
        .unwrap()
        .status(),
        502
    );
    assert!(f.hub.state.get_thread(id).is_ok());
    fake_browser_treer(&f, false);
    assert_eq!(
        request(
            &f,
            &h,
            reqwest::Method::DELETE,
            &format!("/api/threads/{id}"),
            true
        )
        .send()
        .await
        .unwrap()
        .status(),
        204
    );
    let records = records();
    let deletion = records.last().unwrap();
    assert_eq!(
        deletion["args"],
        json!(["agent", "admin", "delete", "ag-created"])
    );
    assert!(deletion["existing"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == id));
    assert!(f.hub.state.get_thread(id).is_err());
    // The requesting Agent's own top-level thread can also be deleted.
    assert_eq!(
        request(
            &f,
            &h,
            reqwest::Method::DELETE,
            &format!("/api/threads/{root}"),
            true
        )
        .send()
        .await
        .unwrap()
        .status(),
        204
    );
    assert!(f.hub.state.get_thread(root).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn browser_create_rolls_back_when_treer_fails_and_rejects_unbound_workspace() {
    let f = fixture().await;
    let h = hello(&f, "ag-browser");
    let (_ws, reply) = attach_client(&f, &h).await;
    let workspace = f
        .hub
        .state
        .get_thread(reply["threadId"].as_str().unwrap())
        .unwrap()
        .workspace_id;
    fake_browser_treer(&f, true);
    let response = request(&f, &h, reqwest::Method::POST, "/api/threads/start", true)
        .json(&json!({"workspaceId":workspace,"model":"ios-e2e-stream"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    let body = response.text().await.unwrap();
    assert!(body.contains("treer_agent_operation_failed"));
    assert!(body.contains("Treer could not update"));
    assert!(!body.contains("fixture-credential"));
    assert_eq!(f.hub.state.list_threads(None, true).unwrap().len(), 1);
    assert!(f.hub.creating.lock().unwrap().is_empty());
    assert_eq!(
        request(&f, &h, reqwest::Method::POST, "/api/threads/start", true)
            .json(&json!({"workspaceId":"unbound","model":"ios-e2e-stream"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
}

#[cfg(unix)]
#[tokio::test]
async fn browser_self_delete_finishes_when_treer_closes_the_callers_attach() {
    let f = fixture().await;
    let h = hello(&f, "ag-browser");
    let (mut socket, reply) = attach_client(&f, &h).await;
    fake_browser_treer(&f, false);
    let script = f._dir.path().join("fake-treer");
    let signal = f._dir.path().join("delete-started");
    let finish = f._dir.path().join("delete-finish");
    let text=std::fs::read_to_string(&script).unwrap().replace("    print(json.dumps({'agent_id':'ag-created'}))", &format!("    import time\n    open({}, 'w').close()\n    while not os.path.exists({}): time.sleep(0.01)\n    print(json.dumps({{'agent_id':'ag-created'}}))", serde_json::to_string(&signal.to_string_lossy()).unwrap(), serde_json::to_string(&finish.to_string_lossy()).unwrap()));
    std::fs::write(&script, text).unwrap();
    let id = reply["threadId"].as_str().unwrap();
    let deletion = tokio::spawn(
        request(
            &f,
            &h,
            reqwest::Method::DELETE,
            &format!("/api/threads/{id}"),
            true,
        )
        .send(),
    );
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !signal.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    socket.close(None).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if f.hub
                .live
                .lock()
                .unwrap()
                .get(&h.agent_id)
                .is_some_and(|b| b.cancelled.is_cancelled())
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    std::fs::write(finish, "").unwrap();
    assert_eq!(deletion.await.unwrap().unwrap().status(), 204);
    assert!(f.hub.state.get_thread(id).is_err());
}
