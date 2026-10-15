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
    fixture_with_provider(dir, authenticated_mode, Provider::Codex).await
}
async fn fixture_with_provider(
    dir: tempfile::TempDir,
    authenticated_mode: bool,
    provider: Provider,
) -> Fixture {
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
        enabled_providers: vec![provider],
        acp_command: None,
        acp_startup_timeout_ms: 1000,
        fake_runtime: true,
        supervisor_restart_strategy: Default::default(),
    };
    // Strong-count zero precedes the end of field destruction; wait for the
    // old Supervisor's OS ownership lock as well when simulating a restart.
    let db = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            match Database::open(&config.database_url) {
                Ok(db) => break db,
                Err(e) if e.to_string().contains("already owned") => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await
                }
                Err(e) => panic!("{e}"),
            }
        }
    })
    .await
    .unwrap();
    let fake = Arc::new(FakeRuntime::new(provider));
    let state = Arc::new(Supervisor::new(config, db, vec![fake.clone()]));
    state.configure_cli(format!("http://127.0.0.1:{port}"));
    state.spawn_live_item_persister();
    let hub = Hub::new_with_identity(state.clone(), port, None)
        .unwrap()
        .unwrap();
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
            provider: f.hub.state.config.enabled_providers[0].as_str().into(),
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
async fn attach_reports_known_setup_failures_without_exposing_internal_errors() {
    let f = fixture().await;
    let mut h = hello(&f, "ag-disabled-provider");
    h.create.as_mut().unwrap().provider = "claude".into();
    let (_, reply) = attach_client(&f, &h).await;
    assert_eq!(reply["code"], "provider_unavailable");
    assert_eq!(reply["message"], "claude is not enabled on this hub.");
    assert!(f.hub.state.list_threads(None, true).unwrap().is_empty());

    h.agent_id = "ag-missing-thread".into();
    h.create = None;
    h.thread_id = Some("missing-thread".into());
    let (_, reply) = attach_client(&f, &h).await;
    assert_eq!(reply["code"], "thread_not_found");

    let known = serde_json::to_value(attach_failure(&anyhow::anyhow!(
        "Install the base agent first: grok"
    )))
    .unwrap();
    assert_eq!(known["code"], "base_agent_missing");
    assert_eq!(known["message"], "Install the base agent first: grok");
    for detail in [
        "subprocess failed: fixture-private-credential",
        "Install the base agent first: fixture-private-credential",
        "claude is not enabled: fixture-private-credential",
    ] {
        let reply = serde_json::to_value(attach_failure(&anyhow::anyhow!(detail))).unwrap();
        assert_eq!(reply["code"], "binding_refused");
        assert_eq!(reply["message"], "Attach binding refused");
        assert!(!reply.to_string().contains("fixture-private-credential"));
    }
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
        .create_attached_thread(
            CreateThreadInput {
                workspace_id: f.hub.state.get_thread(&thread).unwrap().workspace_id,
                title: None,
                provider: Some(Provider::Codex),
                agent_id: None,
                model: "ios-e2e-stream".into(),
                reasoning_effort: None,
                approval_mode: "yolo".into(),
                parent_thread_id: Some(thread.clone()),
            },
            vec![],
            "ag-child".into(),
        )
        .await
        .unwrap();
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
    child_hello.agent_id = "ag-child".into();
    child_hello.instance_id = "child-instance".into();
    child_hello.thread_id = Some(child.id.clone());
    let (mut child_socket, bound) = attach_client(&f, &child_hello).await;
    assert_eq!(bound["type"], "bound");
    let child_env = f
        .hub
        .state
        .with_cli_context(&child.id, async {
            pockymoe_runtime::interaction::launch_env()
        })
        .await;
    assert_eq!(
        child_env
            .iter()
            .find(|(k, _)| k == "TREER_AGENT_ID")
            .unwrap()
            .1,
        "ag-child"
    );
    child_socket.close(None).await.unwrap();
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
        .create_attached_thread(
            CreateThreadInput {
                workspace_id: f.hub.state.get_thread(root).unwrap().workspace_id,
                title: None,
                provider: Some(Provider::Codex),
                agent_id: None,
                model: "ios-e2e-stream".into(),
                reasoning_effort: None,
                approval_mode: "yolo".into(),
                parent_thread_id: Some(root.into()),
            },
            vec![],
            "ag-child".into(),
        )
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
        .create_thread_with_origin(
            CreateThreadInput {
                workspace_id: hidden_workspace.id.clone(),
                title: Some("Unbound".into()),
                provider: Some(Provider::Codex),
                agent_id: None,
                model: "ios-e2e-stream".into(),
                reasoning_effort: None,
                approval_mode: "yolo".into(),
                parent_thread_id: None,
            },
            Default::default(),
            pockymoe_runtime::host_hooks::ThreadOrigin::Hidden,
        )
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
        registry::ensure_schema(&f.hub.state).unwrap();
        f.hub
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
elif args[:2] == ['agent', 'show']:
    print(json.dumps({{'name':'Treer test'}}))
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
    *f.hub.hub_identity.lock().unwrap() = f.hub.live.lock().unwrap().get("ag-browser").cloned();
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
    let response = tokio::spawn(request(&f, &h, reqwest::Method::POST, "/api/threads/start", true)
        .json(&json!({"workspaceId":workspace,"provider":"codex","model":"ios-e2e-stream","title":"reply / 1"}))
        .send());
    wait_until(|| {
        f.hub
            .state
            .list_threads(None, true)
            .unwrap()
            .iter()
            .any(|t| t.title == "reply / 1")
    })
    .await;
    let id = f
        .hub
        .state
        .list_threads(None, true)
        .unwrap()
        .into_iter()
        .find(|t| t.title == "reply / 1")
        .unwrap()
        .id;
    let mut new_hello = hello(&f, "ag-created");
    new_hello.create = None;
    new_hello.thread_id = Some(id.clone());
    let (_created_socket, bound) = attach_client(&f, &new_hello).await;
    assert_eq!(bound["threadId"], id);
    assert_eq!(bound["created"], false);
    let response = response.await.unwrap().unwrap();
    assert_eq!(response.status(), 200);
    let created: Value = response.json().await.unwrap();
    assert_eq!(created["id"], id);
    let id = id.as_str();
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
    assert_eq!(
        env.iter()
            .find(|(key, _)| key == "TREER_AGENT_ID")
            .unwrap()
            .1,
        "ag-created"
    );
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
    assert!(body.contains("Treer could not register"));
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
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
                .is_none_or(|b| b.cancelled.is_cancelled())
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

#[cfg(unix)]
fn registry_cli(f: &Fixture) {
    use std::os::unix::fs::PermissionsExt;
    let script = f._dir.path().join("registry-cli");
    let script_source = r#"#!/usr/bin/env python3
import sys, os, json, pathlib, sqlite3, fcntl
base = pathlib.Path(__file__).parent
args = sys.argv[1:]
with open(base/'registry.lock','w') as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    path = base/'registry-agents.json'
    agents = json.loads(path.read_text()) if path.exists() else {}
    def fail(code):
        print(json.dumps({'error':{'code':code,'message':'suppressed fixture diagnostic'}}),file=sys.stderr)
        sys.exit(1)
    if args == ['whoami']:
        print(json.dumps({'machine':{'server_id':'server','root':str(base)}})); sys.exit(0)
    if (base/'registry-fail').exists(): fail((base/'registry-fail').read_text())
    if args[:2] == ['agent','ls']:
        print(json.dumps({'agents':list(agents.values())})); sys.exit(0)
    if args[:4] == ['agent','admin','startup','show']:
        if args[4] not in agents: fail('agent_not_found')
        startup = agents[args[4]].get('startup')
        if not startup: fail('startup_not_found')
        print(json.dumps({'command':startup[0],'args':startup[1:]})); sys.exit(0)
    if args[:2] == ['agent','show']:
        if args[2] not in agents: fail('agent_not_found')
        print(json.dumps(agents[args[2]])); sys.exit(0)
    db = sqlite3.connect(base/'db.sqlite')
    existing = [r[0] for r in db.execute('SELECT id FROM threads')]
    with open(base/'registry-calls.jsonl','a') as log:
        log.write(json.dumps({'args':args,'caller':os.environ.get('TREER_AGENT_ID'),'existing':existing})+'\n')
    assert 'TREER_OPERATOR_CREDENTIAL' not in os.environ
    if args[:3] == ['agent','admin','create']:
        name = args[args.index('--name')+1]
        if any(a['name']==name for a in agents.values()): fail('agent_name_conflict')
        counter = base/'registry-counter'
        n = int(counter.read_text())+1 if counter.exists() else 1
        counter.write_text(str(n))
        aid = 'ag_created_'+str(n)
        agents[aid] = {'agent_id':aid,'name':name,'status':'running','thread':args[args.index('--thread')+1], 'startup':args[args.index('--')+1:]}
        path.write_text(json.dumps(agents))
        print(json.dumps({'agent_id':aid})); sys.exit(0)
    if args[:3] == ['agent','admin','rename']:
        aid, name = args[3:5]
        if aid not in agents: fail('agent_not_found')
        if any(k!=aid and a['name']==name for k,a in agents.items()): fail('agent_name_conflict')
        agents[aid]['name']=name
    elif args[:3] == ['agent','admin','delete']:
        agents.pop(args[3],None)
    elif args[:2] in (['agent','start'],['agent','stop']):
        if args[2] not in agents: fail('agent_not_found')
        agents[args[2]]['status']='running' if args[1]=='start' else 'stopped'
    else: fail('unsupported')
    path.write_text(json.dumps(agents))
    print('{}')
"#;
    std::fs::write(&script, script_source).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    *f.hub.browser_commands.lock().unwrap() = browser::Commands {
        treer: script,
        pockymoe: Some("/stable/pockymoe".into()),
    };
    let mut app = hello(f, "app-hub");
    app.create = None;
    *f.hub.hub_identity.lock().unwrap() = Some(Binding {
        thread: String::new(),
        agent: app.agent_id,
        instance: "app-instance".into(),
        identity: app.treer,
        cancelled: Default::default(),
    });
}
#[cfg(unix)]
fn agents(f: &Fixture) -> Value {
    std::fs::read(f._dir.path().join("registry-agents.json"))
        .ok()
        .map(|raw| serde_json::from_slice(&raw).unwrap())
        .unwrap_or_else(|| json!({}))
}
#[cfg(unix)]
fn set_agents(f: &Fixture, value: Value) {
    std::fs::write(
        f._dir.path().join("registry-agents.json"),
        value.to_string(),
    )
    .unwrap();
}
#[cfg(unix)]
fn calls(f: &Fixture) -> Vec<Value> {
    std::fs::read_to_string(f._dir.path().join("registry-calls.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
#[cfg(unix)]
fn create_input(f: &Fixture, title: &str, parent: Option<&str>) -> CreateThreadInput {
    let ws = f
        .hub
        .state
        .list_workspaces()
        .unwrap()
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            f.hub
                .state
                .create_workspace(CreateWorkspaceInput {
                    abs_path: Some(f._dir.path().to_string_lossy().into()),
                    label: None,
                    git_url: None,
                })
                .unwrap()
        });
    CreateThreadInput {
        workspace_id: ws.id,
        title: Some(title.into()),
        provider: Some(f.hub.state.config.enabled_providers[0]),
        agent_id: None,
        model: "ios-e2e-stream".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: parent.map(str::to_owned),
    }
}
#[cfg(unix)]
async fn bind_created(f: &Fixture, title: &str) -> Binding {
    let mut found = None;
    wait_until(|| {
        found = agents(f)
            .as_object()
            .unwrap()
            .iter()
            .find(|(agent, a)| {
                !f.hub.live.lock().unwrap().contains_key(*agent)
                    && f.hub
                        .state
                        .list_threads_unfiltered(None, true)
                        .unwrap()
                        .iter()
                        .any(|t| t.id == a["thread"] && t.title == title)
            })
            .map(|(agent, a)| (agent.clone(), a["thread"].as_str().unwrap().to_owned()));
        found.is_some()
    })
    .await;
    let (agent, thread) = found.unwrap();
    let mut h = hello(f, &agent);
    h.create = None;
    h.thread_id = Some(thread);
    h.instance_id = format!("instance-{agent}");
    f.hub.bind(h).await.unwrap().0
}
#[cfg(unix)]
async fn registered(
    f: &Fixture,
    title: &str,
    parent: Option<&str>,
    name: Option<&str>,
) -> (pockymoe_protocol::ThreadDto, Binding) {
    let state = f.hub.state.clone();
    let input = create_input(f, title, parent);
    let options = pockymoe_runtime::interaction::AgentOptions {
        name: name.map(str::to_owned),
        ..Default::default()
    };
    let task = tokio::spawn(async move { state.create_thread_with(input, options).await });
    let binding = bind_created(f, title).await;
    let thread = task.await.unwrap().unwrap();
    (thread, binding)
}

#[cfg(unix)]
#[tokio::test]
async fn registry_creator_identities_names_dedupe_cascade_and_delete_order() {
    let f = fixture().await;
    registry_cli(&f);
    let (root, _) = registered(&f, "Lead / title", None, Some("top-alias")).await;
    let root_agent = agents(&f)
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, a)| a["thread"] == root.id)
        .unwrap()
        .0
        .clone();
    assert_eq!(agents(&f)[&root_agent]["name"], "Lead  title");
    let (child, _) = registered(&f, "Worker title", Some(&root.id), Some("worker")).await;
    let child_agent = agents(&f)
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, a)| a["thread"] == child.id)
        .unwrap()
        .0
        .clone();
    assert_eq!(agents(&f)[&child_agent]["name"], "Lead  title.worker");
    let creates = calls(&f)
        .into_iter()
        .filter(|c| c["args"][2] == "create")
        .collect::<Vec<_>>();
    assert_eq!(creates[0]["caller"], "app-hub");
    assert_eq!(creates[1]["caller"], root_agent);
    assert!(!creates[1]["args"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a == "--folder"));
    let child_env = f
        .hub
        .state
        .with_cli_context(&child.id, async {
            pockymoe_runtime::interaction::launch_env()
        })
        .await;
    assert_eq!(
        child_env
            .iter()
            .find(|(key, _)| key == "TREER_AGENT_ID")
            .unwrap()
            .1,
        child_agent
    );
    assert!(!child_env
        .iter()
        .any(|(key, value)| key == "TREER_AGENT_ID" && value == &root_agent));
    let (other, _) = registered(&f, "Lead / title", None, None).await;
    let all = agents(&f);
    let name = all
        .as_object()
        .unwrap()
        .values()
        .find(|a| a["thread"] == other.id)
        .unwrap()["name"]
        .as_str()
        .unwrap();
    assert_eq!(name, "Lead  title-2");
    let (dotted, _) = registered(&f, "Lead  title.unrelated", None, None).await;
    let (grandchild, _) = registered(&f, "Grandchild", Some(&child.id), Some("nested")).await;
    f.hub.state.rename_thread(&root.id, "Renamed").unwrap();
    assert_eq!(agents(&f)[&root_agent]["name"], "Renamed");
    assert_eq!(agents(&f)[&child_agent]["name"], "Renamed.worker");
    assert_eq!(
        f.hub.state.get_thread(&child.id).unwrap().title,
        "Worker title"
    );
    let all = agents(&f);
    assert_eq!(
        all.as_object()
            .unwrap()
            .values()
            .find(|a| a["thread"] == dotted.id)
            .unwrap()["name"],
        "Lead  title.unrelated"
    );
    assert_eq!(
        all.as_object()
            .unwrap()
            .values()
            .find(|a| a["thread"] == grandchild.id)
            .unwrap()["name"],
        "Renamed.worker.nested"
    );
    assert!(f
        .hub
        .state
        .delete_child_thread(&root.id, &child.id)
        .await
        .is_err());
    f.hub
        .state
        .delete_child_thread(&child.id, &grandchild.id)
        .await
        .unwrap();
    f.hub
        .state
        .delete_child_thread(&root.id, &child.id)
        .await
        .unwrap();
    let deletion = calls(&f)
        .into_iter()
        .find(|c| c["args"] == json!(["agent", "admin", "delete", child_agent]))
        .unwrap();
    assert!(deletion["existing"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == &child.id));
    assert!(f.hub.state.get_thread(&child.id).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn registry_failure_timeout_hidden_and_stable_binding_rejection() {
    let f = fixture().await;
    registry_cli(&f);
    std::fs::write(f._dir.path().join("registry-fail"), "policy_denied").unwrap();
    let error = f
        .hub
        .state
        .create_thread(create_input(&f, "Denied", None))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "treer_command_failed");
    assert!(f.hub.state.list_threads(None, true).unwrap().is_empty());
    std::fs::remove_file(f._dir.path().join("registry-fail")).unwrap();
    let error = f
        .hub
        .state
        .create_thread(create_input(&f, "No attach", None))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("attach did not bind"));
    assert!(agents(&f).as_object().unwrap().is_empty());
    assert!(f.hub.state.list_threads(None, true).unwrap().is_empty());
    assert!(f.hub.creating.lock().unwrap().is_empty());
    let hidden = f
        .hub
        .state
        .create_thread_with_origin(
            create_input(&f, "Internal", None),
            Default::default(),
            pockymoe_runtime::host_hooks::ThreadOrigin::Hidden,
        )
        .await
        .unwrap();
    assert!(f.hub.state.list_threads(None, true).unwrap().is_empty());
    assert!(f.hub.state.get_thread(&hidden.id).is_err());
    f.hub.reconcile_registry().await;
    assert!(agents(&f).as_object().unwrap().is_empty());
    let (root, binding) = registered(&f, "Bound", None, None).await;
    f.hub.detach(&binding).await;
    let mut h = hello(&f, "ag-imposter");
    h.create = None;
    h.thread_id = Some(root.id);
    assert_eq!(
        f.hub.bind(h).await.err().unwrap().to_string(),
        "agent_thread_mismatch"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn registry_close_reserves_name_reopens_before_delivery_and_delete_failure_preserves_history()
{
    let f = fixture().await;
    registry_cli(&f);
    let (root, _) = registered(&f, "Root", None, None).await;
    let (child, binding) = registered(&f, "Child", Some(&root.id), Some("worker")).await;
    std::fs::write(f._dir.path().join("registry-fail"), "policy_denied").unwrap();
    assert!(f
        .hub
        .state
        .close_agent_thread_releasing(Some(&root.id), &child.id, false)
        .await
        .is_err());
    assert!(f
        .hub
        .state
        .get_thread(&child.id)
        .unwrap()
        .closed_at
        .is_none());
    assert!(!f.hub.state.integration_thread_paused(&child.id));
    std::fs::remove_file(f._dir.path().join("registry-fail")).unwrap();
    f.hub
        .state
        .close_agent_thread_releasing(Some(&root.id), &child.id, false)
        .await
        .unwrap();
    assert_eq!(agents(&f)[&binding.agent]["status"], "stopped");
    assert_eq!(
        f.hub.state.resolve_agent(Some(&root.id), "worker").unwrap(),
        child.id
    );
    assert!(f
        .hub
        .state
        .get_thread(&child.id)
        .unwrap()
        .closed_at
        .is_some());
    assert!(f
        .hub
        .state
        .create_thread_with(
            create_input(&f, "Collision", Some(&root.id)),
            pockymoe_runtime::interaction::AgentOptions {
                name: Some("worker".into()),
                ..Default::default()
            }
        )
        .await
        .is_err());
    f.hub.detach(&binding).await;
    let state = f.hub.state.clone();
    let child_id = child.id.clone();
    let prompt = tokio::spawn(async move {
        state
            .prompt(
                &child_id,
                serde_json::from_value(json!({"prompt":"hello"})).unwrap(),
            )
            .await
    });
    wait_until(|| {
        calls(&f)
            .iter()
            .any(|c| c["args"] == json!(["agent", "start", binding.agent]))
    })
    .await;
    assert!(f
        .hub
        .state
        .get_thread(&child.id)
        .unwrap()
        .active_turn_id
        .is_none());
    assert!(f
        .hub
        .state
        .get_thread_detail(&child.id, None)
        .await
        .unwrap()
        .turns
        .is_empty());
    let mut h = hello(&f, &binding.agent);
    h.create = None;
    h.thread_id = Some(child.id.clone());
    h.instance_id = "restarted".into();
    f.hub.bind(h).await.unwrap();
    prompt.await.unwrap().unwrap();
    assert!(f
        .hub
        .state
        .get_thread(&child.id)
        .unwrap()
        .closed_at
        .is_none());
    let count = f
        .hub
        .state
        .get_thread_detail(&child.id, None)
        .await
        .unwrap()
        .turns
        .len();
    assert_eq!(count, 1);
    std::fs::write(f._dir.path().join("registry-fail"), "policy_denied").unwrap();
    assert!(f
        .hub
        .state
        .delete_thread_releasing(&child.id)
        .await
        .is_err());
    assert_eq!(
        f.hub
            .state
            .get_thread_detail(&child.id, None)
            .await
            .unwrap()
            .turns
            .len(),
        count
    );
}

#[cfg(unix)]
#[tokio::test]
async fn registry_reconcile_external_rename_delete_outage_and_legacy_unbound_children() {
    let f = fixture().await;
    registry_cli(&f);
    let (root, root_binding) = registered(&f, "Root", None, None).await;
    let (child, child_binding) = registered(&f, "Child", Some(&root.id), Some("worker")).await;
    let mut data = agents(&f);
    data[&child_binding.agent]["name"] = json!("Root.external");
    set_agents(&f, data.clone());
    f.hub.reconcile_registry().await;
    assert_eq!(f.hub.state.get_thread(&child.id).unwrap().title, "external");
    data = agents(&f);
    data[&root_binding.agent]["name"] = json!("Moved");
    set_agents(&f, data);
    f.hub.reconcile_registry().await;
    assert_eq!(f.hub.state.get_thread(&root.id).unwrap().title, "Moved");
    assert_eq!(agents(&f)[&child_binding.agent]["name"], "Moved.external");
    assert_eq!(f.hub.state.get_thread(&child.id).unwrap().title, "external");
    let mut data = agents(&f);
    std::fs::write(f._dir.path().join("registry-fail"), "policy_denied").unwrap();
    f.hub.reconcile_registry().await;
    assert!(f.hub.state.get_thread(&child.id).is_ok());
    std::fs::remove_file(f._dir.path().join("registry-fail")).unwrap();
    // Retained history and search projection survive external Agent deletion.
    f.hub
        .state
        .prompt(
            &child.id,
            serde_json::from_value(json!({"prompt":"hello"})).unwrap(),
        )
        .await
        .unwrap();
    data.as_object_mut().unwrap().remove(&child_binding.agent);
    set_agents(&f, data);
    f.hub.detach(&child_binding).await;
    f.hub.reconcile_registry().await;
    assert!(f.hub.state.get_thread(&child.id).is_err());
    assert!(!f
        .hub
        .state
        .list_threads(None, true)
        .unwrap()
        .iter()
        .any(|t| t.id == child.id));
    assert!(f
        .hub
        .state
        .search_conversations("external", None, 100, 0)
        .unwrap()["matches"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(
        !f.hub.state.agent_tree(&root.id, true).await.unwrap()["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["threadId"] == child.id)
    );
    let retained=f.hub.state.db.with(|c|Ok(c.query_row("SELECT (SELECT count(*) FROM threads WHERE id=?1),(SELECT count(*) FROM thread_turns WHERE thread_id=?1)",[&child.id],|r|Ok((r.get::<_,usize>(0)?,r.get::<_,usize>(1)?)))?)).unwrap();
    assert_eq!(retained, (1, 1));
    let legacy = uuid::Uuid::new_v4().to_string();
    f.hub.state.db.with(|c| {c.execute("INSERT INTO threads(id,workspace_id,provider,title,source,status,created_at,updated_at,parent_thread_id,root_thread_id,lineage_depth,agent_name) VALUES(?1,?2,'codex','Legacy child','supervisor','idle',?3,?3,?4,?4,1,'legacy')",params![legacy,root.workspace_id,now_rfc3339(),root.id])?;Ok(())}).unwrap();
    let scope = Scope {
        hub: f.hub.clone(),
        thread: root.id.clone(),
        agent: root_binding.agent.clone(),
        instance: root_binding.instance.clone(),
        controller: false,
    };
    assert!(!scope.allows(&legacy));
    let hub = f.hub.clone();
    let reconcile = tokio::spawn(async move { hub.reconcile_registry().await });
    wait_until(|| {
        agents(&f)
            .as_object()
            .unwrap()
            .values()
            .any(|a| a["thread"] == legacy)
    })
    .await;
    assert!(scope.allows(&legacy));
    let binding = bind_created(&f, "Legacy child").await;
    reconcile.await.unwrap();
    assert!(f.hub.state.get_thread(&legacy).is_ok());
    assert_eq!(agents(&f)[&binding.agent]["name"], "Moved.legacy");
    assert_eq!(calls(&f).last().unwrap()["caller"], "app-hub");
    f.hub.reconcile_registry().await;
    assert!(!agents(&f)
        .as_object()
        .unwrap()
        .contains_key(&child_binding.agent));
}

#[cfg(unix)]
#[tokio::test]
async fn registry_fork_uses_source_agent_identity_and_reconciliation_preserves_external_stop() {
    let f = fixture_with_provider(tempfile::tempdir().unwrap(), false, Provider::Claude).await;
    registry_cli(&f);
    let (root, root_binding) = registered(&f, "Source", None, None).await;
    let state = f.hub.state.clone();
    let id = root.id.clone();
    let fork = tokio::spawn(async move { state.fork_thread(&id).await });
    let binding = bind_created(&f, "Source / fork").await;
    let fork = fork.await.unwrap().unwrap();
    assert_eq!(calls(&f).last().unwrap()["caller"], root_binding.agent);
    assert_eq!(agents(&f)[&binding.agent]["name"], "Source  fork");
    assert!(fork.parent_thread_id.is_none());
    f.hub.detach(&binding).await;
    let mut data = agents(&f);
    data[&binding.agent]["status"] = json!("exited");
    set_agents(&f, data);
    f.hub.reconcile_registry().await;
    assert!(f
        .hub
        .state
        .get_thread(&fork.id)
        .unwrap()
        .closed_at
        .is_some());
    assert!(f
        .hub
        .state
        .list_threads(None, true)
        .unwrap()
        .iter()
        .any(|t| t.id == fork.id));
    assert!(agents(&f).get(&binding.agent).is_some());
}

#[cfg(unix)]
#[tokio::test]
async fn registry_failed_cleanup_is_durable_and_retried_after_thread_rollback() {
    let f = fixture().await;
    registry_cli(&f);
    let state = f.hub.state.clone();
    let input = create_input(&f, "Timeout cleanup", None);
    let create = tokio::spawn(async move { state.create_thread(input).await });
    wait_until(|| !agents(&f).as_object().unwrap().is_empty()).await;
    std::fs::write(f._dir.path().join("registry-fail"), "policy_denied").unwrap();
    assert!(create.await.unwrap().is_err());
    assert!(f.hub.state.list_threads(None, true).unwrap().is_empty());
    let pending = f
        .hub
        .state
        .db
        .with(|c| {
            Ok(
                c.query_row("SELECT count(*) FROM treer_registry_cleanup", [], |r| {
                    r.get::<_, usize>(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(pending, 1);
    std::fs::remove_file(f._dir.path().join("registry-fail")).unwrap();
    f.hub.reconcile_registry().await;
    assert!(agents(&f).as_object().unwrap().is_empty());
    let pending = f
        .hub
        .state
        .db
        .with(|c| {
            Ok(
                c.query_row("SELECT count(*) FROM treer_registry_cleanup", [], |r| {
                    r.get::<_, usize>(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(pending, 0);
}

#[cfg(unix)]
#[tokio::test]
async fn registry_recovers_lost_create_response_by_stable_startup_thread_id() {
    let f = fixture().await;
    registry_cli(&f);
    let (root, binding) = registered(&f, "Lost reply", None, None).await;
    f.hub.detach(&binding).await;
    // The Agent exists, but the hub crashed before persisting the create reply.
    f.hub
        .state
        .db
        .with(|c| {
            c.execute("DELETE FROM treer_bindings WHERE thread_id=?1", [&root.id])?;
            c.execute(
                "UPDATE treer_registry SET state='registering' WHERE thread_id=?1",
                [&root.id],
            )?;
            Ok(())
        })
        .unwrap();
    let count = calls(&f)
        .iter()
        .filter(|c| c["args"][2] == "create")
        .count();
    let hub = f.hub.clone();
    let reconcile = tokio::spawn(async move { hub.reconcile_registry().await });
    wait_until(|| {
        calls(&f)
            .iter()
            .any(|c| c["args"] == json!(["agent", "start", binding.agent]))
    })
    .await;
    let rebound = bind_created(&f, "Lost reply").await;
    reconcile.await.unwrap();
    assert_eq!(rebound.agent, binding.agent);
    assert_eq!(
        calls(&f)
            .iter()
            .filter(|c| c["args"][2] == "create")
            .count(),
        count
    );
    assert_eq!(agents(&f).as_object().unwrap().len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn registry_migrates_a_closed_legacy_child_then_stops_its_agent() {
    let f = fixture().await;
    registry_cli(&f);
    let (root, _) = registered(&f, "Root", None, None).await;
    let legacy = uuid::Uuid::new_v4().to_string();
    f.hub.state.db.with(|c| {c.execute("INSERT INTO threads(id,workspace_id,provider,title,source,status,created_at,updated_at,parent_thread_id,root_thread_id,lineage_depth,agent_name,closed_at) VALUES(?1,?2,'codex','Closed legacy','supervisor','idle',?3,?3,?4,?4,1,'legacy',?3)",params![legacy,root.workspace_id,now_rfc3339(),root.id])?;Ok(())}).unwrap();
    let hub = f.hub.clone();
    let reconcile = tokio::spawn(async move { hub.reconcile_registry().await });
    let binding = bind_created(&f, "Closed legacy").await;
    reconcile.await.unwrap();
    assert_eq!(agents(&f)[&binding.agent]["status"], "stopped");
    assert!(f.hub.state.get_thread(&legacy).unwrap().closed_at.is_some());
    assert!(f.hub.state.integration_thread_paused(&legacy));
    assert_eq!(
        f.hub.state.resolve_agent(Some(&root.id), "legacy").unwrap(),
        legacy
    );
}

#[cfg(unix)]
#[tokio::test]
async fn registry_startup_pauses_legacy_unbound_threads_before_inbox_recovery() {
    let mut f = fixture().await;
    registry_cli(&f);
    let (root, _) = registered(&f, "Root", None, None).await;
    let legacy = uuid::Uuid::new_v4().to_string();
    f.hub.state.db.with(|c| {c.execute("INSERT INTO threads(id,workspace_id,provider,title,source,status,created_at,updated_at,parent_thread_id,root_thread_id,lineage_depth) VALUES(?1,?2,'codex','Legacy child','supervisor','idle',?3,?3,?4,?4,1)",params![legacy,root.workspace_id,now_rfc3339(),root.id])?;Ok(())}).unwrap();
    let dir = std::mem::replace(&mut f._dir, tempfile::tempdir().unwrap());
    drop(f);
    let f = fixture_in(dir).await;
    assert!(f.hub.state.integration_thread_paused(&root.id));
    assert!(f.hub.state.integration_thread_paused(&legacy));
    assert!(f.hub.state.get_thread(&legacy).is_ok());
}
