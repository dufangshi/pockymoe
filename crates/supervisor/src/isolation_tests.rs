//! Agent isolation of a device and of workspaces (docs/agent-isolation.md).
use crate::auth::{PeerCaller, TrustedRelayForward};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Mode, Provider};
use pockymoe_runtime::{
    fake::FakeRuntime, local_sessions::LocalSessionHomes, Database, RuntimeConfig, Supervisor,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

struct Fixture {
    _dir: tempfile::TempDir,
    state: Arc<Supervisor>,
    router: Router,
    open: String,
    isolated: String,
}

async fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let config = RuntimeConfig {
        treer_enabled: false,
        host_mode: Default::default(),
        mode: Mode::Local,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root: dir.path().join("workspaces"),
        database_url: dir.path().join("test.sqlite"),
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
    let db = Database::open(&config.database_url).unwrap();
    let state = Arc::new(
        Supervisor::new(
            config,
            db,
            vec![Arc::new(FakeRuntime::new(Provider::Codex))],
        )
        .with_local_session_homes(LocalSessionHomes {
            codex_home: dir.path().join("codex"),
            grok_home: dir.path().join("grok"),
            claude_home: dir.path().join("claude"),
        }),
    );
    state.spawn_live_item_persister();
    state.configure_cli("http://127.0.0.1:0".into());
    let workspace = |name: &str| {
        let path = dir.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("notes.txt"), "secret").unwrap();
        state
            .create_workspace(CreateWorkspaceInput {
                abs_path: Some(path.to_string_lossy().into()),
                git_url: None,
                label: Some(name.into()),
            })
            .unwrap()
            .id
    };
    let open = workspace("open");
    let isolated = workspace("private");
    let router = crate::http::router(state.clone());
    Fixture {
        _dir: dir,
        state,
        router,
        open,
        isolated,
    }
}

async fn thread(state: &Supervisor, workspace: &str) -> String {
    state
        .create_thread(CreateThreadInput {
            workspace_id: workspace.into(),
            title: None,
            provider: Some(Provider::Codex),
            agent_id: None,
            model: "ios-e2e-stream".into(),
            reasoning_effort: None,
            approval_mode: "guarded".into(),
            parent_thread_id: None,
        })
        .await
        .unwrap()
        .id
}

async fn cli(f: &Fixture, caller: Option<&str>, input: Value) -> anyhow::Result<Value> {
    crate::interaction::run(&f.state, input, caller.map(str::to_owned)).await
}

fn forbidden(result: anyhow::Result<Value>) -> String {
    let error = result
        .expect_err("isolation should refuse this")
        .to_string();
    assert!(error.starts_with("forbidden: "), "{error}");
    error
}

fn peer(shared_control: Option<bool>) -> PeerCaller {
    PeerCaller {
        device_id: "device-b".into(),
        device_name: "Laptop".into(),
        user_id: "same-owner".into(),
        shared_control,
    }
}

async fn call(
    router: &Router,
    method: &str,
    uri: &str,
    body: Value,
    peer: bool,
) -> (StatusCode, Value) {
    call_as(router, method, uri, body, peer.then(|| self::peer(None))).await
}

async fn call_as(
    router: &Router,
    method: &str,
    uri: &str,
    body: Value,
    peer: Option<PeerCaller>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(peer) = peer {
        request = request.extension(TrustedRelayForward).extension(peer);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn an_isolated_workspace_keeps_out_threads_from_other_workspaces_but_not_its_own_or_the_owner(
) {
    let f = setup().await;
    let outside = thread(&f.state, &f.open).await;
    let inside = thread(&f.state, &f.isolated).await;
    let sibling = thread(&f.state, &f.isolated).await;
    let (status, workspace) = call(
        &f.router,
        "PUT",
        &format!("/api/config/agent-isolation/workspaces/{}", f.isolated),
        json!({"isolated": true}),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{workspace}");
    assert_eq!(workspace["agentIsolated"], true);

    let listed = cli(&f, Some(&outside), json!({"operation":"list"}))
        .await
        .unwrap();
    let ids: Vec<_> = listed["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, vec![outside.clone()]);
    assert_eq!(listed["totalCount"], 1);

    for input in [
        json!({"operation":"status","threadId":inside}),
        json!({"operation":"transcript","threadId":inside}),
        json!({"operation":"send","threadId":inside,"text":"hi"}),
        json!({"operation":"inbox","threadId":inside}),
        json!({"operation":"inboxRead","threadId":inside}),
        json!({"operation":"inboxWait","threadId":inside,"timeoutSeconds":0}),
        json!({"operation":"wait","threadIds":[inside],"timeoutSeconds":0}),
        json!({"operation":"wake","fromThreadId":outside,"threadIds":[inside]}),
        json!({"operation":"tree","rootThreadId":inside}),
        json!({"operation":"close","threadId":inside}),
        json!({"operation":"taskAdd","fromThreadId":outside,"title":"x","assignThreadId":inside}),
        json!({"operation":"models","workspaceId":f.isolated}),
        json!({"operation":"create","workspaceId":f.isolated,"title":"helper"}),
    ] {
        let error = forbidden(cli(&f, Some(&outside), input.clone()).await);
        assert!(error.contains("private"), "{input}: {error}");
    }

    // A thread in the workspace and the computer's owner are not restricted.
    let seen = cli(&f, Some(&inside), json!({"operation":"list"}))
        .await
        .unwrap();
    assert_eq!(seen["totalCount"], 3);
    cli(
        &f,
        Some(&inside),
        json!({"operation":"status","threadId":sibling}),
    )
    .await
    .unwrap();
    cli(&f, None, json!({"operation":"status","threadId":inside}))
        .await
        .unwrap();
    assert_eq!(
        cli(&f, None, json!({"operation":"list"})).await.unwrap()["totalCount"],
        3
    );
    // Isolation is inbound only: the workspace's threads still reach others.
    cli(
        &f,
        Some(&inside),
        json!({"operation":"status","threadId":outside}),
    )
    .await
    .unwrap();

    // The Web UI of the owner is unaffected.
    let (status, _) = call(
        &f.router,
        "GET",
        &format!("/api/threads/{inside}"),
        Value::Null,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = call(
        &f.router,
        "PUT",
        &format!("/api/config/agent-isolation/workspaces/{}", f.isolated),
        json!({"isolated": false}),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    cli(
        &f,
        Some(&outside),
        json!({"operation":"status","threadId":inside}),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn other_devices_cannot_reach_an_isolated_device_or_workspace() {
    let f = setup().await;
    let open = thread(&f.state, &f.open).await;
    let private = thread(&f.state, &f.isolated).await;
    // A device without peer access counts as isolated; ending isolation opens it.
    let (_, view) = call(
        &f.router,
        "GET",
        "/api/config/agent-isolation",
        Value::Null,
        false,
    )
    .await;
    assert_eq!(view["device"], true);
    let (status, view) = call(
        &f.router,
        "PATCH",
        "/api/config/agent-isolation",
        json!({"device": false}),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["device"], false);
    assert!(f.state.peer_access_enabled());

    f.state.set_workspace_isolation(&f.isolated, true).unwrap();
    let (status, workspaces) = call(
        &f.router,
        "POST",
        "/api/peer/cli",
        json!({"operation":"workspaces"}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(workspaces.as_array().unwrap().len(), 1);
    assert_eq!(workspaces[0]["id"], f.open);
    let (_, listed) = call(
        &f.router,
        "POST",
        "/api/peer/cli",
        json!({"operation":"list"}),
        true,
    )
    .await;
    assert_eq!(listed["totalCount"], 1);
    for input in [
        json!({"operation":"status","threadId":private}),
        json!({"operation":"transcript","threadId":private}),
        json!({"operation":"send","threadId":private,"text":"hi"}),
        json!({"operation":"create","workspaceId":f.isolated,"title":"x"}),
    ] {
        let (status, body) = call(&f.router, "POST", "/api/peer/cli", input.clone(), true).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{input}: {body}");
    }
    let (status, _) = call(
        &f.router,
        "POST",
        "/api/peer/files/list",
        json!({"workspaceId":f.isolated}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &f.router,
        "POST",
        "/api/peer/files/list",
        json!({"workspaceId":f.open}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &f.router,
        "POST",
        "/api/peer/cli",
        json!({"operation":"status","threadId":open}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Device isolation closes everything but `info`, and keeps outbound peer access.
    let (_, view) = call(
        &f.router,
        "PATCH",
        "/api/config/agent-isolation",
        json!({"device": true}),
        false,
    )
    .await;
    assert_eq!(view["device"], true);
    assert!(f.state.peer_access_enabled());
    let (status, info) = call(
        &f.router,
        "POST",
        "/api/peer/cli",
        json!({"operation":"info"}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["isolated"], true);
    for (uri, input) in [
        ("/api/peer/cli", json!({"operation":"workspaces"})),
        ("/api/peer/cli", json!({"operation":"list"})),
        (
            "/api/peer/cli",
            json!({"operation":"status","threadId":open}),
        ),
        (
            "/api/peer/cli",
            json!({"operation":"send","threadId":open,"text":"hi"}),
        ),
        ("/api/peer/files/list", json!({"workspaceId":f.open})),
    ] {
        let (status, body) = call(&f.router, "POST", uri, input.clone(), true).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{input}: {body}");
        assert!(
            body["message"].as_str().unwrap_or("").contains("isolated"),
            "{body}"
        );
    }
    // Same-device threads are not affected by device isolation.
    cli(
        &f,
        Some(&open),
        json!({"operation":"status","threadId":open}),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_device_shared_for_control_cannot_reach_this_device_but_others_still_can() {
    let f = setup().await;
    f.state.set_peer_access(true).unwrap();
    let open = thread(&f.state, &f.open).await;
    let requests = [
        (
            "POST",
            "/api/peer/cli".to_string(),
            json!({"operation":"workspaces"}),
        ),
        (
            "POST",
            "/api/peer/cli".to_string(),
            json!({"operation":"list"}),
        ),
        (
            "POST",
            "/api/peer/cli".to_string(),
            json!({"operation":"transcript","threadId":open}),
        ),
        (
            "POST",
            "/api/peer/cli".to_string(),
            json!({"operation":"send","threadId":open,"text":"hi"}),
        ),
        (
            "POST",
            "/api/peer/cli".to_string(),
            json!({"operation":"create","workspaceId":f.open,"provider":"codex","model":"default"}),
        ),
        (
            "POST",
            "/api/peer/files/list".to_string(),
            json!({"workspaceId":f.open}),
        ),
        (
            "GET",
            format!("/api/peer/files/read?workspaceId={}&path=notes.txt", f.open),
            Value::Null,
        ),
        (
            "POST",
            "/api/peer/files/uploads".to_string(),
            json!({"threadId":open,"name":"a.txt","size":1,"sha256":"0".repeat(64)}),
        ),
    ];
    for (method, uri, input) in &requests {
        let (status, body) = call_as(
            &f.router,
            method,
            uri,
            input.clone(),
            Some(peer(Some(true))),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} {input}: {body}");
        assert_eq!(body["code"], "peer_source_shared", "{uri} {input}");
        assert!(body["message"]
            .as_str()
            .unwrap()
            .contains("can control its agents"));
    }
    // `info` still answers, like an isolated device, so callers can tell why.
    let (status, info) = call_as(
        &f.router,
        "POST",
        "/api/peer/cli",
        json!({"operation":"info"}),
        Some(peer(Some(true))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{info}");
    assert!(f.state.list_threads(None, true).unwrap().len() == 1);
    // A private source, or a relay that predates the rule, keeps today's behavior.
    for flag in [Some(false), None] {
        let (status, body) = call_as(
            &f.router,
            "POST",
            "/api/peer/cli",
            json!({"operation":"workspaces"}),
            Some(peer(flag)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{flag:?}: {body}");
        let (status, body) = call_as(
            &f.router,
            "POST",
            "/api/peer/files/list",
            json!({"workspaceId":f.open}),
            Some(peer(flag)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{flag:?}: {body}");
    }
}

#[tokio::test]
async fn other_people_and_peers_cannot_change_isolation() {
    let f = setup().await;
    let (status, _) = call(
        &f.router,
        "PATCH",
        "/api/config/agent-isolation",
        json!({"device": false}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!f.state.peer_access_enabled());
    assert!(crate::grants::owner_only(&format!(
        "/api/config/agent-isolation/workspaces/{}",
        f.isolated
    )));
}

#[tokio::test]
async fn automations_and_commands_cannot_act_in_or_watch_an_isolated_workspace() {
    let f = setup().await;
    let outside = thread(&f.state, &f.open).await;
    let inside = thread(&f.state, &f.isolated).await;
    f.state.set_workspace_isolation(&f.isolated, true).unwrap();
    let watch = json!({"name":"watch","trigger":{"kind":"threadEnded","sourceThreadId":inside},"action":{"kind":"notifyInbox","subject":"done","text":"done","includeClosingMessage":true}});
    for input in [
        json!({"operation":"automationCreate","threadId":outside,"definition":watch}),
        json!({"operation":"automationPreview","threadId":outside,"definition":watch}),
        json!({"operation":"automationList","threadId":inside}),
        json!({"operation":"commandRun","threadId":inside,"input":{"shell":"cat notes.txt","cwd":"."}}),
        json!({"operation":"commandShow","threadId":inside,"commandId":"x"}),
    ] {
        forbidden(cli(&f, Some(&outside), input).await);
    }
    // The owner, and threads in the workspace, still can.
    cli(
        &f,
        None,
        json!({"operation":"automationList","threadId":inside}),
    )
    .await
    .unwrap();
    cli(
        &f,
        Some(&inside),
        json!({"operation":"automationList","threadId":inside}),
    )
    .await
    .unwrap();
}
