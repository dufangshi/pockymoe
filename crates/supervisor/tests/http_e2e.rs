use std::sync::Arc;
use std::time::Duration;

use pockymoe_protocol::Provider;
use pockymoe_runtime::actor::SharedRuntime;
use pockymoe_runtime::config::RuntimeConfig;
use pockymoe_runtime::db::Database;
use pockymoe_runtime::fake::FakeRuntime;
use pockymoe_runtime::local_sessions::LocalSessionHomes;
use pockymoe_runtime::Supervisor;
use serde_json::{json, Value};
use tempfile::tempdir;
use tokio::net::TcpListener;

async fn spawn_supervisor(
    providers: Vec<Provider>,
) -> (tempfile::TempDir, u16, std::path::PathBuf) {
    spawn_supervisor_seeded(providers, |_| {}).await
}

#[tokio::test]
async fn explorer_create_file_never_overwrites_and_can_be_opened_as_document() {
    let (_dir, port, root) = spawn_supervisor(vec![Provider::Codex]).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let proj = root.join("new-files");
    std::fs::create_dir_all(proj.join("docs")).unwrap();
    let ws = json(
        &client,
        client
            .post(format!("{base}/api/workspaces"))
            .json(&json!({"absPath":proj,"label":"new-files"})),
    )
    .await;
    let id = ws["id"].as_str().unwrap();
    let endpoint = format!("{base}/api/workspaces/{id}/files");
    let response = client
        .post(&endpoint)
        .json(&json!({"path":"docs/new.md"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    assert_eq!(std::fs::read(proj.join("docs/new.md")).unwrap(), b"");
    std::fs::write(proj.join("docs/new.md"), "keep").unwrap();
    let response = client
        .post(&endpoint)
        .json(&json!({"path":"docs/new.md"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "fileAlreadyExists"
    );
    assert_eq!(
        std::fs::read_to_string(proj.join("docs/new.md")).unwrap(),
        "keep"
    );
    let response = client
        .post(&endpoint)
        .json(&json!({"path":"../outside.md"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(!root.join("outside.md").exists());
    let response = client
        .post(&endpoint)
        .json(&json!({"path":"docs/second.md"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let doc = json(
        &client,
        client.get(format!(
            "{base}/api/workspaces/{id}/files/document?path=docs/second.md"
        )),
    )
    .await;
    assert_eq!(doc["content"], "");
    assert_eq!(doc["encoding"], "utf-8");
    #[cfg(target_os = "linux")]
    assert_eq!(doc["readOnlyReason"], Value::Null);
}

async fn spawn_supervisor_seeded(
    providers: Vec<Provider>,
    seed: impl Fn(&FakeRuntime),
) -> (tempfile::TempDir, u16, std::path::PathBuf) {
    let (dir, port, root, _) = spawn_supervisor_state(providers, seed).await;
    (dir, port, root)
}

async fn spawn_supervisor_state(
    providers: Vec<Provider>,
    seed: impl Fn(&FakeRuntime),
) -> (tempfile::TempDir, u16, std::path::PathBuf, Arc<Supervisor>) {
    let dir = tempdir().unwrap();
    let ws_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&ws_root).unwrap();
    let config = RuntimeConfig {
        mode: pockymoe_protocol::Mode::Local,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root: ws_root.clone(),
        database_url: dir.path().join("test.sqlite"),
        app_name: "test".into(),
        app_version: "0.12.0".into(),
        environment: "test".into(),
        auth_required: false,
        admin_username: None,
        admin_password: None,
        session_secret: None,
        relay_server_url: None,
        web_origin: None,
        relay_agent_token: None,
        enabled_providers: providers.clone(),
        acp_command: None,
        acp_startup_timeout_ms: 1000,
        fake_runtime: true,
    };
    let db = Database::open(&config.database_url).unwrap();
    let fakes: Vec<Arc<FakeRuntime>> = providers
        .into_iter()
        .map(|provider| {
            let fake = Arc::new(FakeRuntime::new(provider));
            seed(&fake);
            fake
        })
        .collect();
    let runtimes: Vec<SharedRuntime> = fakes
        .iter()
        .map(|fake| fake.clone() as SharedRuntime)
        .collect();
    let state = Arc::new(
        Supervisor::new(config, db, runtimes).with_local_session_homes(LocalSessionHomes {
            codex_home: dir.path().join("codex-home"),
            grok_home: dir.path().join("grok-home"),
            claude_home: dir.path().join("claude-home"),
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let cli = state.configure_cli(format!("http://127.0.0.1:{port}"));
    std::fs::write(dir.path().join("cli-token"), cli.token).unwrap();
    let serving = state.clone();
    tokio::spawn(async move {
        axum::serve(listener, pockymoe_supervisor::router(serving))
            .await
            .unwrap();
    });
    for _ in 0..50 {
        if reqwest::get(format!("http://127.0.0.1:{port}/healthz"))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (dir, port, ws_root, state)
}

#[tokio::test]
async fn native_subagent_http_reads_scoped_transcripts_and_rejects_other_parent() {
    let (dir, port, root, state) = spawn_supervisor_state(vec![Provider::Codex], |_| {}).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let workspace = json(
        &client,
        client
            .post(format!("{base}/api/workspaces"))
            .json(&json!({"absPath":root,"label":"Native agents"})),
    )
    .await;
    let mut parents = vec![];
    for _ in 0..2 {
        let thread = json(&client, client.post(format!("{base}/api/threads/start")).json(&json!({"workspaceId":workspace["id"],"provider":"codex","model":"ios-e2e-stream","approvalMode":"yolo"}))).await;
        parents.push(thread["id"].as_str().unwrap().to_owned());
    }
    let session = state
        .get_thread(&parents[0])
        .unwrap()
        .provider_session_id
        .unwrap();
    let sessions = dir.path().join("codex-home/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let child = "22222222-2222-4222-8222-222222222222";
    let parent_records = [
        json!({"type":"response_item","timestamp":"2026-10-09T00:00:00Z","payload":{"type":"function_call","name":"spawn_agent","call_id":"spawn-1","arguments":"{\"message\":\"Review runtime\"}"}}),
        json!({"type":"response_item","timestamp":"2026-10-09T00:00:00Z","payload":{"type":"function_call_output","call_id":"spawn-1","output":format!("{{\"agent_id\":\"{child}\"}}")}}),
    ];
    let child_records = [
        json!({"type":"session_meta","payload":{"id":child}}),
        json!({"type":"response_item","timestamp":"2026-10-09T00:00:01Z","payload":{"type":"message","role":"assistant","content":[{"text":"Review finished\nPrivate record body"}]}}),
        json!({"type":"event_msg","timestamp":"2026-10-09T00:00:02Z","payload":{"type":"task_complete"}}),
    ];
    for (id, records) in [
        (&session, parent_records.as_slice()),
        (&child.to_owned(), child_records.as_slice()),
    ] {
        let text = records.iter().map(|v| format!("{v}\n")).collect::<String>();
        std::fs::write(sessions.join(format!("rollout-{id}.jsonl")), text).unwrap();
    }
    let list = json(
        &client,
        client.get(format!("{base}/api/threads/{}/subagents", parents[0])),
    )
    .await;
    assert_eq!(list["agents"][0]["id"], child);
    assert_eq!(list["agents"][0]["status"], "completed");
    let detail = json(
        &client,
        client.get(format!(
            "{base}/api/threads/{}/subagents/{child}",
            parents[0]
        )),
    )
    .await;
    assert_eq!(detail["items"][0]["text"], "Review finished");
    assert_eq!(detail["agent"]["prompt"], "Review runtime");
    assert_eq!(detail["historyMode"], "lazy-v1");
    let item_id = detail["items"][0]["id"].as_str().unwrap();
    let body = json(
        &client,
        client
            .get(format!(
                "{base}/api/threads/{}/subagents/{child}",
                parents[0]
            ))
            .query(&[("itemId", item_id)]),
    )
    .await;
    assert_eq!(body["text"], "Review finished\nPrivate record body");
    for parent in [&parents[1], "missing-parent"] {
        let response = client
            .get(format!("{base}/api/threads/{parent}/subagents/{child}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
    }
}

#[tokio::test]
async fn native_subagent_http_supports_direct_claude_without_acp_agent_id() {
    let (dir, port, root, state) = spawn_supervisor_state(vec![Provider::Claude], |_| {}).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let workspace = json(
        &client,
        client
            .post(format!("{base}/api/workspaces"))
            .json(&json!({"absPath":root,"label":"Claude native"})),
    )
    .await;
    let thread = json(&client, client.post(format!("{base}/api/threads/start")).json(&json!({"workspaceId":workspace["id"],"provider":"claude","model":"ios-e2e-stream","approvalMode":"yolo"}))).await;
    let id = thread["id"].as_str().unwrap();
    assert!(thread["agentId"].is_null());
    let session = state.get_thread(id).unwrap().provider_session_id.unwrap();
    let raw_session = session.rsplit("::").next().unwrap();
    let parent = dir
        .path()
        .join("claude-home/projects/demo")
        .join(format!("{raw_session}.jsonl"));
    let child = parent.with_extension("").join("subagents/agent-a123.jsonl");
    std::fs::create_dir_all(child.parent().unwrap()).unwrap();
    let parent_records = [
        json!({"type":"assistant","sessionId":raw_session,"timestamp":"2026-10-09T00:00:00Z","message":{"content":[{"type":"tool_use","id":"toolu-1","name":"Agent","input":{"description":"Native review","prompt":"Review code","run_in_background":true}}]}}),
        json!({"type":"user","sessionId":raw_session,"timestamp":"2026-10-09T00:00:00Z","toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"a123"},"message":{"content":[{"type":"tool_result","tool_use_id":"toolu-1"}]}}),
    ];
    std::fs::write(
        &parent,
        parent_records
            .iter()
            .map(|v| format!("{v}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let result = json!({"type":"assistant","agentId":"a123","timestamp":"2026-10-09T00:00:01Z","message":{"id":"msg-1","model":"claude-sonnet-4-5","stop_reason":"end_turn","usage":{"input_tokens":100,"output_tokens":10},"content":[{"type":"text","text":"Native review complete"}]}});
    std::fs::write(child, format!("{result}\n")).unwrap();
    let list = json(
        &client,
        client.get(format!("{base}/api/threads/{id}/subagents")),
    )
    .await;
    assert_eq!(list["agents"][0]["provider"], "claude");
    assert_eq!(list["agents"][0]["status"], "completed");
    assert_eq!(list["agents"][0]["tokenUsage"]["total"]["totalTokens"], 110);
    let detail = json(
        &client,
        client.get(format!("{base}/api/threads/{id}/subagents/toolu-1")),
    )
    .await;
    assert_eq!(detail["items"][0]["text"], "Native review complete");
}

#[tokio::test]
async fn device_metrics_api_returns_warmed_cached_native_samples() {
    let (_dir, port, _) = spawn_supervisor(vec![Provider::Codex]).await;
    let response = reqwest::get(format!("http://127.0.0.1:{port}/api/device/metrics"))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let first: pockymoe_protocol::DeviceMetricsDto = response.json().await.unwrap();
    assert!(first.sample_window_ms >= 900);
    assert_eq!(first.cpu.logical_core_count, first.cpu.cores.len());
    assert!(first.cpu.logical_core_count > 0);
    assert!(first.memory.total_bytes > 0);
    assert_eq!(
        first.memory.used_bytes + first.memory.available_bytes,
        first.memory.total_bytes
    );
    let second: pockymoe_protocol::DeviceMetricsDto =
        reqwest::get(format!("http://127.0.0.1:{port}/api/device/metrics"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    assert_eq!(first.sampled_at, second.sampled_at);
}

#[tokio::test]
async fn cli_delete_authenticates_the_parent_and_refuses_arbitrary_targets_and_spoofing() {
    let (dir, port, root, state) = spawn_supervisor_state(vec![Provider::Codex], |_| {}).await;
    let ws = state
        .create_workspace(pockymoe_protocol::CreateWorkspaceInput {
            abs_path: Some(root.to_string_lossy().into()),
            git_url: None,
            label: None,
        })
        .unwrap();
    let make = |parent| pockymoe_protocol::CreateThreadInput {
        workspace_id: ws.id.clone(),
        title: None,
        provider: Some(Provider::Codex),
        agent_id: None,
        model: "ios-e2e-stream".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: parent,
    };
    let parent = state.create_thread(make(None)).await.unwrap();
    let stranger = state.create_thread(make(None)).await.unwrap();
    let child = state
        .create_thread(make(Some(parent.id.clone())))
        .await
        .unwrap();
    let parent_token = state.cli_thread_token(&parent.id);
    let stranger_token = state.cli_thread_token(&stranger.id);
    let child_token = state.cli_thread_token(&child.id);
    let machine_token = std::fs::read_to_string(dir.path().join("cli-token")).unwrap();
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/api/cli");
    for (token, target, claimed) in [
        (&machine_token, &child.id, &parent.id),
        (&stranger_token, &child.id, &stranger.id),
        (&stranger_token, &child.id, &parent.id),
        (&parent_token, &stranger.id, &parent.id),
        (&child_token, &parent.id, &child.id),
    ] {
        let r = client
            .post(&url)
            .bearer_auth(token)
            .json(&json!({"operation":"delete","threadId":target,"fromThreadId":claimed}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            reqwest::StatusCode::FORBIDDEN,
            "{}",
            r.text().await.unwrap()
        );
    }
    // Managed tokens still perform ordinary CLI discovery and child creation.
    let listed = client
        .post(&url)
        .bearer_auth(&parent_token)
        .json(&json!({"operation":"list"}))
        .send()
        .await
        .unwrap();
    assert!(listed.status().is_success());
    let removed: Value = client
        .post(&url)
        .bearer_auth(&parent_token)
        .json(&json!({"operation":"delete","threadId":child.id,"fromThreadId":parent.id}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(removed["deleted"], true);
    assert!(state.get_thread(&child.id).is_err());
    let invalidated = client
        .post(&url)
        .bearer_auth(child_token)
        .json(&json!({"operation":"list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(invalidated.status(), reqwest::StatusCode::UNAUTHORIZED);
}

async fn spawn_authenticated_supervisor() -> (tempfile::TempDir, u16) {
    let dir = tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let config = RuntimeConfig {
        mode: pockymoe_protocol::Mode::Server,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root,
        database_url: dir.path().join("test.sqlite"),
        app_name: "test".into(),
        app_version: "0.12.0".into(),
        environment: "test".into(),
        auth_required: true,
        admin_username: Some("admin".into()),
        admin_password: Some("secret123".into()),
        session_secret: Some("0123456789abcdef".into()),
        relay_server_url: None,
        web_origin: None,
        relay_agent_token: None,
        enabled_providers: vec![Provider::Codex],
        acp_command: None,
        acp_startup_timeout_ms: 1000,
        fake_runtime: true,
    };
    let db = Database::open(&config.database_url).unwrap();
    let state = Arc::new(Supervisor::new(
        config,
        db,
        vec![Arc::new(FakeRuntime::new(Provider::Codex)) as SharedRuntime],
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, pockymoe_supervisor::router(state))
            .await
            .unwrap();
    });
    (dir, port)
}

async fn json(_client: &reqwest::Client, req: reqwest::RequestBuilder) -> Value {
    let response = req.send().await.unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    assert!(status.is_success(), "{status} {text}");
    if text.is_empty() {
        json!({})
    } else {
        serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }))
    }
}

#[tokio::test]
async fn server_auth_blocks_anonymous_requests_and_accepts_node_sessions() {
    let (_dir, port) = spawn_authenticated_supervisor().await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    let anonymous = client
        .get(format!("{base}/api/workspaces"))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), reqwest::StatusCode::UNAUTHORIZED);

    let login = client
        .post(format!("{base}/api/auth/login"))
        .json(&json!({ "username": "admin", "password": "secret123" }))
        .send()
        .await
        .unwrap();
    assert!(login.status().is_success());
    assert!(login.headers().contains_key(reqwest::header::SET_COOKIE));
    let login_body: Value = login.json().await.unwrap();
    let token = login_body["token"].as_str().unwrap();

    let authenticated = client
        .get(format!("{base}/api/workspaces"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert!(authenticated.status().is_success());

    let node_token = "eyJ1c2VybmFtZSI6ImFkbWluIiwiZXhwaXJlc0F0Ijo0MTAyNDQ0ODAwMDAwLCJub25jZSI6ImxlZ2FjeS1ub2RlIn0.bosISUS4ohy_K_Ygr6Oj9zpuOaRokEQkzosjQwrgAgI";
    let legacy = client
        .get(format!("{base}/api/auth/session"))
        .bearer_auth(node_token)
        .send()
        .await
        .unwrap();
    let legacy_body: Value = legacy.json().await.unwrap();
    assert_eq!(legacy_body["authenticated"], true);
    assert_eq!(legacy_body["username"], "admin");

    let websocket_without_auth = client.get(format!("{base}/ws")).send().await.unwrap();
    assert_eq!(
        websocket_without_auth.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
}

async fn wait_thread(client: &reqwest::Client, base: &str, id: &str) -> Value {
    for _ in 0..200 {
        let detail = json(client, client.get(format!("{base}/api/threads/{id}"))).await;
        let status = detail["thread"]["status"].as_str().unwrap_or("");
        if matches!(status, "idle" | "interrupted" | "failed")
            && detail["turns"]
                .as_array()
                .map(|turns| !turns.is_empty())
                .unwrap_or(false)
        {
            return detail;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    panic!("thread {id} did not settle");
}

async fn wait_for_pending_count(
    client: &reqwest::Client,
    base: &str,
    id: &str,
    expected: usize,
) -> Value {
    for _ in 0..100 {
        let detail = json(
            client,
            client.get(format!("{base}/api/threads/{id}?view=summary&limit=10")),
        )
        .await;
        if detail["pendingSteers"].as_array().map(Vec::len) == Some(expected) {
            return detail;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("thread {id} did not reach {expected} pending prompts");
}

#[tokio::test]
async fn pending_prompt_routes_match_the_frontend_contract() {
    let (_dir, port, ws_root) = spawn_supervisor(vec![Provider::Codex]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let workspace_path = ws_root.join("pending-prompts");
    std::fs::create_dir_all(&workspace_path).unwrap();
    let workspace = json(
        &client,
        client.post(format!("{base}/api/workspaces")).json(&json!({
            "absPath": workspace_path,
            "label": "Pending prompts"
        })),
    )
    .await;
    let thread = json(
        &client,
        client
            .post(format!("{base}/api/threads/start"))
            .json(&json!({
                "workspaceId": workspace["id"],
                "provider": "codex",
                "model": "ios-e2e-stream",
                "approvalMode": "yolo"
            })),
    )
    .await;
    let thread_id = thread["id"].as_str().unwrap();
    json(
        &client,
        client
            .post(format!("{base}/api/threads/{thread_id}/prompt"))
            .json(&json!({
                "prompt": "Inspect this repository in depth and keep the turn running while I add follow-ups."
            })),
    )
    .await;

    json(
        &client,
        client
            .post(format!("{base}/api/threads/{thread_id}/prompt"))
            .json(&json!({
                "prompt": "Cancel this queued follow-up.",
                "clientRequestId": "queued-cancel"
            })),
    )
    .await;
    let queued = wait_for_pending_count(&client, &base, thread_id, 1).await;
    let pending = &queued["pendingSteers"][0];
    assert_eq!(pending["clientRequestId"], "queued-cancel");
    assert_eq!(pending["prompt"], "Cancel this queued follow-up.");
    assert_eq!(pending["delivery"], "continuation");
    assert!(pending.get("displayPrompt").is_none());
    assert!(pending.get("submittedPrompt").is_none());
    assert!(pending.get("threadId").is_none());
    let cancelled_id = pending["id"].as_str().unwrap();
    let cancelled = json(
        &client,
        client.delete(format!(
            "{base}/api/threads/{thread_id}/pending-steers/{cancelled_id}"
        )),
    )
    .await;
    assert_eq!(cancelled["thread"]["id"], thread_id);
    assert_eq!(cancelled["pendingSteers"].as_array().map(Vec::len), Some(0));

    json(
        &client,
        client
            .post(format!("{base}/api/threads/{thread_id}/prompt"))
            .json(&json!({
                "prompt": "Steer this queued follow-up now.",
                "clientRequestId": "queued-steer"
            })),
    )
    .await;
    let queued = wait_for_pending_count(&client, &base, thread_id, 1).await;
    let steer_id = queued["pendingSteers"][0]["id"].as_str().unwrap();
    let steered = json(
        &client,
        client.post(format!(
            "{base}/api/threads/{thread_id}/pending-steers/{steer_id}/steer"
        )),
    )
    .await;
    assert_eq!(steered["thread"]["id"], thread_id);
    assert_eq!(
        steered["pendingSteers"][0]["clientRequestId"],
        "queued-steer"
    );
    assert_eq!(steered["pendingSteers"][0]["delivery"], "steer");
    let duplicate_steer = client
        .post(format!(
            "{base}/api/threads/{thread_id}/pending-steers/{steer_id}/steer"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate_steer.status(), reqwest::StatusCode::OK);
    let receipt = json(
        &client,
        client.get(format!("{base}/api/threads/{thread_id}?view=delivery")),
    )
    .await;
    assert_eq!(receipt["turns"], json!([]));
    assert_eq!(receipt["acceptedSteerIds"], json!([steer_id]));

    json(
        &client,
        client.post(format!("{base}/api/threads/{thread_id}/interrupt")),
    )
    .await;
    let settled = wait_thread(&client, &base, thread_id).await;
    assert_eq!(settled["pendingSteers"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn http_files_prompt_interrupt_export_and_capabilities() {
    let (_dir, port, ws_root) = spawn_supervisor(vec![
        Provider::Codex,
        Provider::Claude,
        Provider::Opencode,
        Provider::Acp,
    ])
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let proj = ws_root.join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(proj.join("README.md"), "# files\n").unwrap();
    std::fs::write(proj.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(
        proj.join("dot.png"),
        [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ],
    )
    .unwrap();

    let workspace = json(
        &client,
        client.post(format!("{base}/api/workspaces")).json(&json!({
            "absPath": proj.to_string_lossy(),
            "label": "proj"
        })),
    )
    .await;
    let workspace_id = workspace["id"].as_str().unwrap();

    let tree = json(
        &client,
        client.get(format!(
            "{base}/api/workspaces/{workspace_id}/files/tree?path=."
        )),
    )
    .await;
    assert_eq!(tree["kind"], "directory");
    let names: Vec<_> = tree["children"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["name"].as_str())
        .collect();
    assert!(names.contains(&"README.md"));
    assert!(names.contains(&"src"));
    let subtree = json(
        &client,
        client.get(format!(
            "{base}/api/workspaces/{workspace_id}/files/tree?path=./src"
        )),
    )
    .await;
    assert_eq!(subtree["name"], "src");
    assert_eq!(subtree["path"], "./src");
    assert_eq!(subtree["children"][0]["path"], "./src/main.rs");
    assert_eq!(subtree["childrenLoaded"], true);

    let preview = json(
        &client,
        client.get(format!(
            "{base}/api/workspaces/{workspace_id}/files/preview?path=README.md"
        )),
    )
    .await;
    assert!(preview["content"].as_str().unwrap().contains("# files"));

    json(
        &client,
        client
            .put(format!("{base}/api/workspaces/{workspace_id}/files"))
            .json(&json!({
                "path": "notes.txt",
                "content": "hello-files"
            })),
    )
    .await;
    json(
        &client,
        client
            .patch(format!("{base}/api/workspaces/{workspace_id}/files/move"))
            .json(&json!({ "fromPath": "notes.txt", "toPath": "docs/notes.txt" })),
    )
    .await;
    let moved = json(
        &client,
        client.get(format!(
            "{base}/api/workspaces/{workspace_id}/files/preview?path=docs/notes.txt"
        )),
    )
    .await;
    assert_eq!(moved["content"], "hello-files");

    let escaped = client
        .get(format!(
            "{base}/api/workspaces/{workspace_id}/files/preview?path=../secret"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(escaped.status(), 400);

    let backends = json(&client, client.get(format!("{base}/api/agent-runtimes"))).await;
    let list = backends.as_array().cloned().unwrap_or_default();
    assert!(list.iter().any(|backend| backend["provider"] == "codex"));
    assert!(list.iter().any(|backend| backend["provider"] == "claude"));
    let codex = list
        .iter()
        .find(|backend| backend["provider"] == "codex")
        .unwrap();
    assert_eq!(codex["capabilities"]["turns"]["compact"], true);
    assert_eq!(codex["capabilities"]["branching"]["fork"], false);
    let claude = list
        .iter()
        .find(|backend| backend["provider"] == "claude")
        .unwrap();
    assert_eq!(claude["capabilities"]["branching"]["fork"], true);

    for provider in ["codex", "claude", "opencode", "acp"] {
        let thread = json(
            &client,
            client
                .post(format!("{base}/api/threads/start"))
                .json(&json!({
                    "workspaceId": workspace_id,
                    "title": format!("{provider} hello"),
                    "provider": provider,
                    "model": "ios-e2e-stream",
                    "approvalMode": "yolo"
                })),
        )
        .await;
        let thread_id = thread["id"].as_str().unwrap().to_string();
        json(
            &client,
            client
                .post(format!("{base}/api/threads/{thread_id}/prompt"))
                .json(&json!({ "prompt": "hello, reply me with hello" })),
        )
        .await;
        let detail = wait_thread(&client, &base, &thread_id).await;
        let texts = detail["turns"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|turn| turn["items"].as_array().cloned().unwrap_or_default())
            .filter_map(|item| item["text"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert!(
            texts.iter().any(|text| text == "hello"),
            "{provider} {texts:?}"
        );

        if provider == "codex" {
            let turn_id = detail["turns"][0]["id"].as_str().unwrap();
            let summary = json(
                &client,
                client.get(format!("{base}/api/threads/{thread_id}?view=summary")),
            )
            .await;
            assert!(summary["turns"][0]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["kind"] == "userMessage"));
            let turn_detail = json(
                &client,
                client.get(format!(
                    "{base}/api/threads/{thread_id}/turns/{turn_id}/detail"
                )),
            )
            .await;
            assert_eq!(turn_detail["id"], turn_id);
            assert_eq!(turn_detail["hasDeferredItems"], false);
            let attachment_dir = proj.join(".temp/threads").join(&thread_id);
            std::fs::create_dir_all(&attachment_dir).unwrap();
            std::fs::copy(proj.join("dot.png"), attachment_dir.join("dot.png")).unwrap();
            let image = client
                .get(format!(
                    "{base}/api/threads/{thread_id}/assets/image?path=./.temp/threads/{thread_id}/dot.png"
                ))
                .send()
                .await
                .unwrap();
            assert_eq!(image.status(), 200);
            assert_eq!(image.headers().get("content-type").unwrap(), "image/png");
            let escaped_image = client
                .get(format!(
                    "{base}/api/threads/{thread_id}/assets/image?path=../secret.png"
                ))
                .send()
                .await
                .unwrap();
            assert_eq!(escaped_image.status(), 400);
        }

        let html = client
            .get(format!(
                "{base}/api/threads/{thread_id}/exports/html?format=html&mode=latest&limit=10"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(html.status(), 200);
        assert_eq!(
            html.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
        assert!(html
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(".html\""));
        let html_body = html.text().await.unwrap();
        assert!(html_body.starts_with("<!doctype html>"));
        assert!(html_body.contains("hello"));
        let pdf = client
            .get(format!(
                "{base}/api/threads/{thread_id}/exports/pdf?format=pdf&mode=latest&limit=10"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(pdf.status(), 404);
    }

    let long = json(
        &client,
        client
            .post(format!("{base}/api/threads/start"))
            .json(&json!({
                "workspaceId": workspace_id,
                "title": "long",
                "provider": "codex",
                "model": "ios-e2e-stream",
                "approvalMode": "yolo"
            })),
    )
    .await;
    let long_id = long["id"].as_str().unwrap().to_string();
    json(
        &client,
        client.post(format!("{base}/api/threads/{long_id}/prompt")).json(&json!({
            "prompt": "Inspect this repository in depth and write a detailed multi-section report."
        })),
    )
    .await;
    for _ in 0..50 {
        let thread = json(&client, client.get(format!("{base}/api/threads/{long_id}"))).await;
        if thread["thread"]["status"] == "running" || thread["status"] == "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    json(
        &client,
        client.post(format!("{base}/api/threads/{long_id}/interrupt")),
    )
    .await;
    let interrupted = wait_thread(&client, &base, &long_id).await;
    assert_ne!(interrupted["thread"]["status"], "running");
    assert!(interrupted["turns"]
        .as_array()
        .and_then(|turns| turns.last())
        .and_then(|turn| turn["completedAt"].as_str())
        .is_some());
}

#[tokio::test]
async fn local_cli_requires_credentials_and_exposes_existing_threads() {
    let (dir, port, root) = spawn_supervisor(vec![Provider::Codex, Provider::Acp]).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let denied = client
        .post(format!("{base}/api/cli"))
        .json(&json!({"operation":"list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::UNAUTHORIZED);
    let token = std::fs::read_to_string(dir.path().join("cli-token")).unwrap();
    let path = root.join("cli-project");
    std::fs::create_dir_all(&path).unwrap();
    let ws: Value = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"absPath":path,"label":"cli-project"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let created:Value=client.post(format!("{base}/api/cli")).bearer_auth(&token).json(&json!({"operation":"create","workspaceId":ws["id"],"provider":"acp","agentId":"grok","model":"ios-e2e-stream","approvalMode":"yolo"})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    let id = created["threadId"].as_str().unwrap();
    let receipt: Value = client
        .post(format!("{base}/api/cli"))
        .bearer_auth(&token)
        .json(&json!({"operation":"send","delivery":"direct","threadId":id,"text":"hello"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(receipt["delivery"], "queued");
    assert!(receipt["pendingSteerId"].is_string());
    for _ in 0..80 {
        let status: Value = client
            .post(format!("{base}/api/cli"))
            .bearer_auth(&token)
            .json(&json!({"operation":"status","threadId":id}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(status.get("turns").is_none());
        if status["status"] == "idle" && status["queuedCount"] == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let transcript: Value = client
        .post(format!("{base}/api/cli"))
        .bearer_auth(&token)
        .json(&json!({"operation":"transcript","threadId":id}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(transcript["turns"][0]["items"][1]["text"], "hello");
}

#[tokio::test]
async fn managed_peer_delivery_cannot_bypass_policy_by_omitting_or_overriding_identity() {
    let (_dir, port, root, state) = spawn_supervisor_state(vec![Provider::Codex], |_| {}).await;
    let ws = state
        .create_workspace(pockymoe_protocol::CreateWorkspaceInput {
            abs_path: Some(root.to_string_lossy().into()),
            git_url: None,
            label: None,
        })
        .unwrap();
    let input = || pockymoe_protocol::CreateThreadInput {
        workspace_id: ws.id.clone(),
        title: None,
        provider: Some(Provider::Codex),
        agent_id: None,
        model: "default".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: None,
    };
    let sender = state.create_thread(input()).await.unwrap().id;
    let receiver = state.create_thread(input()).await.unwrap().id;
    let other = state.create_thread(input()).await.unwrap().id;
    let token = state.cli_thread_token(&sender);
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/api/cli");
    for body in [
        json!({"operation":"send","threadId":receiver,"delivery":"direct","text":"result ready"}),
        json!({"operation":"send","threadId":receiver,"delivery":"queue","kind":"result","text":"result ready"}),
        json!({"operation":"send","threadId":receiver,"delivery":"direct","kind":"task","text":"correct the calculation"}),
        json!({"operation":"send","threadId":receiver,"fromThreadId":other,"delivery":"queue","kind":"task","text":"spoofed assignment"}),
    ] {
        let response = client
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_client_error(),
            "{body}: {:?}",
            response.status()
        );
        assert_eq!(
            state.interaction_status(&receiver).await.unwrap()["queuedCount"],
            0
        );
        assert_eq!(state.inbox_unread_count(&receiver).unwrap(), 0);
    }
    let accepted: Value = client.post(&url).bearer_auth(&token)
        .json(&json!({"operation":"send","threadId":receiver,"delivery":"inbox","kind":"result","text":"artifact is ready"}))
        .send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    let mail = state
        .inbox_read(&receiver, &json!({"messageId":accepted["messageId"]}))
        .unwrap();
    assert_eq!(mail["fromThreadId"], sender);
    assert_eq!(accepted["delivery"], "inbox");
    let assigned: Value = client.post(&url).bearer_auth(&token)
        .json(&json!({"operation":"send","threadId":receiver,"delivery":"queue","kind":"task","text":"perform the assigned calculation"}))
        .send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(assigned["delivery"], "queued");
    let guide: Value = client
        .post(&url)
        .bearer_auth(&token)
        .json(&json!({"operation":"skill"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(guide["version"], pockymoe_protocol::APP_VERSION);
    assert_eq!(guide["text"], pockymoe_protocol::THREAD_INTERACTION_SKILL);
}

/// Exercises the real path an agent takes: POST /api/cli with operation=create and
/// fromThreadId, then the workspace listing the web client actually calls.
#[tokio::test]
async fn agent_created_threads_group_under_their_root_over_http() {
    let (dir, port, ws_root) = spawn_supervisor(vec![Provider::Codex]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    // /api/cli is gated on local CLI credentials, as a real agent would present them.
    let cli_token = std::fs::read_to_string(dir.path().join("cli-token")).unwrap();
    let workspace_path = ws_root.join("lineage");
    std::fs::create_dir_all(&workspace_path).unwrap();
    let workspace = json(
        &client,
        client.post(format!("{base}/api/workspaces")).json(&json!({
            "absPath": workspace_path, "label": "Lineage"
        })),
    )
    .await;
    let root = json(
        &client,
        client
            .post(format!("{base}/api/threads/start"))
            .json(&json!({
                "workspaceId": workspace["id"], "model": "default", "provider": "codex"
            })),
    )
    .await;
    let root_id = root["thread"]["id"]
        .as_str()
        .or_else(|| root["id"].as_str())
        .expect("thread id")
        .to_string();

    for _ in 0..3 {
        let spawned = json(
            &client,
            client
                .post(format!("{base}/api/cli"))
                .bearer_auth(&cli_token)
                .json(&json!({
                    "operation": "create",
                    "fromThreadId": root_id,
                    "model": "default",
                    "provider": "codex"
                })),
        )
        .await;
        assert!(
            spawned["threadId"].is_string(),
            "agent create failed: {spawned}"
        );
    }

    let ws_id = workspace["id"].as_str().unwrap();
    let visible = json(
        &client,
        client.get(format!("{base}/api/threads?workspaceId={ws_id}")),
    )
    .await;
    let rows = visible.as_array().expect("thread list array");
    assert_eq!(
        rows.len(),
        1,
        "the workspace listing must show only the thread the person started, got {rows:?}"
    );
    assert_eq!(rows[0]["id"].as_str(), Some(root_id.as_str()));
    assert_eq!(
        rows[0]["descendantCount"].as_i64(),
        Some(3),
        "the root should report how many agent threads it owns"
    );

    let everything = json(
        &client,
        client.get(format!(
            "{base}/api/threads?workspaceId={ws_id}&includeAgentThreads=true"
        )),
    )
    .await;
    assert_eq!(
        everything.as_array().unwrap().len(),
        4,
        "opting in returns the whole tree"
    );

    // And the CLI listing honours the same grouping.
    let grouped = json(
        &client,
        client
            .post(format!("{base}/api/cli"))
            .bearer_auth(&cli_token)
            .json(&json!({
                "operation": "list", "workspaceId": ws_id, "groupId": root_id
            })),
    )
    .await;
    assert_eq!(
        grouped["threads"].as_array().unwrap().len(),
        3,
        "--group lists exactly that root's descendants"
    );
}

#[tokio::test]
async fn coordination_operations_work_over_the_cli_api() {
    let (dir, port, ws_root) = spawn_supervisor(vec![Provider::Codex]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let cli_token = std::fs::read_to_string(dir.path().join("cli-token")).unwrap();
    let workspace_path = ws_root.join("coord");
    std::fs::create_dir_all(workspace_path.join(".remote-codex/agents")).unwrap();
    std::fs::write(
        workspace_path.join(".remote-codex/agents/tester.md"),
        "---\ndescription: Runs tests\n---\nRun the narrowest relevant tests.\n",
    )
    .unwrap();
    let workspace = json(
        &client,
        client.post(format!("{base}/api/workspaces")).json(&json!({
            "absPath": workspace_path, "label": "Coord"
        })),
    )
    .await;
    let root = json(
        &client,
        client
            .post(format!("{base}/api/threads/start"))
            .json(&json!({
                "workspaceId": workspace["id"], "model": "default", "provider": "codex"
            })),
    )
    .await;
    let root_id = root["thread"]["id"]
        .as_str()
        .or_else(|| root["id"].as_str())
        .unwrap()
        .to_string();
    let cli = |body: serde_json::Value| {
        client
            .post(format!("{base}/api/cli"))
            .bearer_auth(&cli_token)
            .json(&body)
    };

    let roles = json(
        &client,
        cli(json!({"operation":"roles","fromThreadId":root_id})),
    )
    .await;
    assert_eq!(roles["roles"][0]["name"], "tester", "{roles}");
    let made = json(
        &client,
        cli(json!({"operation":"create","fromThreadId":root_id,"model":"default","provider":"codex","name":"tester","role":"tester"})),
    )
    .await;
    assert_eq!(made["name"], "tester", "{made}");
    assert_eq!(made["role"], "tester");
    let resolved = json(
        &client,
        cli(json!({"operation":"resolve","name":"tester","fromThreadId":root_id})),
    )
    .await;
    assert_eq!(resolved["threadId"], made["threadId"]);

    let task = json(
        &client,
        cli(json!({"operation":"taskAdd","fromThreadId":root_id,"title":"run tests","assignThreadId":made["threadId"]})),
    )
    .await;
    assert_eq!(task["number"], 1, "{task}");
    let claimed = json(
        &client,
        cli(json!({"operation":"taskClaim","fromThreadId":made["threadId"]})),
    )
    .await;
    assert_eq!(claimed["claimed"]["number"], 1, "{claimed}");

    let waited = json(
        &client,
        cli(json!({"operation":"wait","threadIds":[made["threadId"]],"timeoutSeconds":5})),
    )
    .await;
    assert_eq!(waited["done"], true, "{waited}");
    let tree = json(
        &client,
        cli(json!({"operation":"tree","fromThreadId":made["threadId"]})),
    )
    .await;
    let rows = tree["threads"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{tree}");
    assert_eq!(rows[1]["currentTask"]["number"], 1);
    assert_eq!(rows[0]["unreadMessages"], 0);

    let inbox = json(
        &client,
        cli(json!({"operation":"inboxWait","threadId":made["threadId"],"kinds":["task"],"timeoutSeconds":5})),
    )
    .await;
    assert!(
        inbox["messages"][0]["subject"]
            .as_str()
            .unwrap()
            .contains("Task #1 assigned"),
        "{inbox}"
    );
    let closed = json(
        &client,
        cli(json!({"operation":"close","threadId":made["threadId"],"fromThreadId":root_id})),
    )
    .await;
    assert!(closed["closedAt"].is_string(), "{closed}");
}

#[tokio::test]
async fn conversation_search_reads_bounded_messages_without_hydrating_history() {
    let (_dir, port, root, state) = spawn_supervisor_state(vec![Provider::Codex], |_| {}).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let workspace = json(
        &client,
        client
            .post(format!("{base}/api/workspaces"))
            .json(&json!({"absPath":root,"label":"Search"})),
    )
    .await;
    let mut ids = vec![];
    for title in ["Search here", "Other thread"] {
        let thread = json(&client, client.post(format!("{base}/api/threads/start"))
            .json(&json!({"workspaceId":workspace["id"],"title":title,"provider":"codex","model":"ios-e2e-stream","approvalMode":"yolo"}))).await;
        ids.push(thread["id"].as_str().unwrap().to_owned());
    }
    state.db.with(|conn| {
        for (owner, id) in ids.iter().enumerate() {
            for n in 0..12 {
                let turn = format!("search-{owner}-{n}");
                conn.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES(?1,?2,'completed',?3)",(&turn,id,n))?;
                for (kind, text) in [("agentMessage",format!("{} ÄÖ 中文 100% _ {}", "before ".repeat(200), "after ".repeat(200))), ("commandExecution", "only tool secret ÄÖ 中文 100% _".into())] {
                    let item = format!("{turn}-{kind}");
                    conn.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,?2,?3,?1,?4,'2030-01-01T00:00:00Z','2030-01-01T00:00:00Z')",(&item,id,&turn,json!({"id":item,"kind":kind,"text":text}).to_string()))?;
                }
            }
        }
        Ok(())
    }).unwrap();
    let search = json(
        &client,
        client
            .get(format!("{base}/api/threads/{}/search", ids[0]))
            .query(&[("q", "äö 中文 100% _"), ("limit", "2")]),
    )
    .await;
    let matches = search["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 2);
    assert_eq!(search["hasMore"], true);
    assert_eq!(matches[0]["turnId"], "search-0-11");
    for item in matches {
        assert_eq!(item["role"], "Assistant");
        assert!(item["text"].as_str().unwrap().contains("ÄÖ 中文 100% _"));
        assert!(item["text"].as_str().unwrap().chars().count() < 400);
    }
    let global = json(
        &client,
        client.get(format!("{base}/api/search")).query(&[
            ("q", "äö 中文 100% _"),
            ("limit", "2"),
            ("offset", "0"),
        ]),
    )
    .await;
    assert_eq!(global["matches"].as_array().unwrap().len(), 2);
    assert_eq!(global["hasMore"], true);
    assert_eq!(global["nextOffset"], 2);
    assert!(global["matches"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["workspaceId"] == workspace["id"]));
    let titles = json(
        &client,
        client.get(format!(
            "{base}/api/workspaces/{}/search?q=Other",
            workspace["id"].as_str().unwrap()
        )),
    )
    .await;
    assert_eq!(titles["matches"][0]["threadId"], ids[1]);
    assert_eq!(titles["matches"][0]["kind"], "title");
    assert!(titles["matches"][0]["turnId"].is_null());
    for suffix in ["q=hello&limit=0", "q=hello&offset=10001", "q="] {
        assert_eq!(
            client
                .get(format!("{base}/api/search?{suffix}"))
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    assert_eq!(
        client
            .get(format!("{base}/api/workspaces/not-found/search?q=hello"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let tools = json(
        &client,
        client.get(format!(
            "{base}/api/threads/{}/search?q=only%20tool%20secret",
            ids[0]
        )),
    )
    .await;
    assert_eq!(tools["matches"], json!([]));
    assert_eq!(state.get_thread(&ids[0]).unwrap().status, "idle");
    // Imported Codex context and duplicate messages must match the visible
    // normalized conversation, including the item id used for navigation.
    state.db.with(|conn| {
        conn.execute("UPDATE threads SET source='local_codex_import' WHERE id=?1", [&ids[0]])?;
        for (item, text) in [
            ("context", "# AGENTS.md instructions\nhidden injection"),
            ("request-first", "# AGENTS.md instructions\nhidden injection\n## My request:\nvisible search"),
            ("request-duplicate", "visible search"),
        ] {
            conn.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,?2,'search-0-11',?1,?3,'2030-01-01T00:00:01Z','2030-01-01T00:00:01Z')", (item, &ids[0], json!({"id":item,"kind":"userMessage","text":text}).to_string()))?;
        }
        Ok(())
    }).unwrap();
    let imported = json(
        &client,
        client.get(format!(
            "{base}/api/threads/{}/search?q=visible%20search",
            ids[0]
        )),
    )
    .await;
    assert_eq!(imported["matches"].as_array().unwrap().len(), 1);
    assert_eq!(imported["matches"][0]["itemId"], "request-first");
    let hidden = json(
        &client,
        client.get(format!(
            "{base}/api/threads/{}/search?q=hidden%20injection",
            ids[0]
        )),
    )
    .await;
    assert_eq!(hidden["matches"], json!([]));
    for (q, limit) in [("", "2"), ("hello", "0"), ("hello", "101")] {
        assert_eq!(
            client
                .get(format!("{base}/api/threads/{}/search", ids[0]))
                .query(&[("q", q), ("limit", limit)])
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    assert_eq!(
        client
            .get(format!("{base}/api/threads/not-found/search?q=hello"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
}

#[tokio::test]
async fn file_document_http_conditional_save_conflict_and_receipt_use_real_disk() {
    let (_dir, port, root) = spawn_supervisor(vec![Provider::Codex]).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let ws: Value = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"absPath":root,"label":"safe files"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = ws["id"].as_str().unwrap();
    std::fs::write(root.join("notes.txt"), "base").unwrap();
    let caps: Value = client
        .get(format!("{base}/api/workspaces/{id}/files/capabilities"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(caps["documentRead"], true);
    let doc: Value = client
        .get(format!(
            "{base}/api/workspaces/{id}/files/document?path=notes.txt"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let input = json!({"path":"notes.txt","workspaceRevision":doc["workspaceRevision"],"fileIdentity":doc["fileIdentity"],"expectedHash":doc["contentHash"],"content":"my draft","draftRevision":7,"operationId":operation,"operationCreatedAt":chrono::Utc::now().timestamp_millis()});
    std::fs::write(root.join("notes.txt"), "agent change").unwrap();
    let response = client
        .post(format!("{base}/api/workspaces/{id}/files/save"))
        .json(&input)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let conflict: Value = response.json().await.unwrap();
    assert_eq!(conflict["code"], "fileConflict");
    assert_eq!(conflict["details"]["snapshot"]["content"], "agent change");
    std::fs::write(root.join("notes.txt"), "later agent change").unwrap();
    let receipt: Value = client
        .get(format!(
            "{base}/api/workspaces/{id}/files/operations/{operation}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(receipt["snapshot"]["content"], "agent change");
    assert_eq!(
        std::fs::read_to_string(root.join("notes.txt")).unwrap(),
        "later agent change"
    );
    let latest: Value = client
        .get(format!(
            "{base}/api/workspaces/{id}/files/document?path=notes.txt"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut safe = input;
    safe["operationId"] = json!(uuid::Uuid::new_v4().to_string());
    safe["fileIdentity"] = latest["fileIdentity"].clone();
    safe["expectedHash"] = latest["contentHash"].clone();
    let saved: Value = client
        .post(format!("{base}/api/workspaces/{id}/files/save"))
        .json(&safe)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved["status"], "saved");
    assert_eq!(
        std::fs::read_to_string(root.join("notes.txt")).unwrap(),
        "my draft"
    );
}

fn noisy_jpeg(width: u32, height: u32) -> Vec<u8> {
    use image::ImageEncoder;
    let pixels = image::ImageBuffer::from_fn(width, height, |x, y| {
        image::Rgb([(x * 7 ^ y * 13) as u8, (x * y) as u8, (x + y * 3) as u8])
    });
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
        .write_image(
            pixels.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
    bytes
}

#[tokio::test]
async fn thread_image_thumbnails_are_cached_validated_and_revalidated() {
    let (dir, port, ws_root) = spawn_supervisor(vec![Provider::Codex]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let proj = ws_root.join("photos");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(ws_root.join("secret.jpg"), noisy_jpeg(900, 900)).unwrap();
    let workspace = json(
        &client,
        client
            .post(format!("{base}/api/workspaces"))
            .json(&json!({"absPath": proj, "label": "photos"})),
    )
    .await;
    let start = json!({"workspaceId": workspace["id"], "provider": "codex", "model": "ios-e2e-stream", "approvalMode": "yolo"});
    let thread = json(
        &client,
        client
            .post(format!("{base}/api/threads/start"))
            .json(&start),
    )
    .await;
    let other = json(
        &client,
        client
            .post(format!("{base}/api/threads/start"))
            .json(&start),
    )
    .await;
    let (tid, other) = (
        thread["id"].as_str().unwrap(),
        other["id"].as_str().unwrap(),
    );
    let attachments = proj.join(".temp/threads").join(tid);
    std::fs::create_dir_all(&attachments).unwrap();
    std::fs::create_dir_all(proj.join(".temp/threads").join(other)).unwrap();
    let photo = noisy_jpeg(1264, 2780);
    std::fs::write(attachments.join("shot-0a1b2c3d.jpg"), &photo).unwrap();
    std::fs::write(attachments.join("anim.gif"), b"GIF89a\x01\x00\x01\x00").unwrap();
    std::fs::write(
        proj.join(".temp/threads")
            .join(other)
            .join("x-0a1b2c3d.jpg"),
        &photo,
    )
    .unwrap();
    let url = |path: &str, variant: &str| {
        format!(
            "{base}/api/threads/{tid}/assets/image?path={}{variant}",
            url::form_urlencoded::byte_serialize(path.as_bytes()).collect::<String>()
        )
    };
    let shot = format!("./.temp/threads/{tid}/shot-0a1b2c3d.jpg");
    let get = |url: String, etag: Option<String>| {
        let mut request = client.get(url);
        if let Some(etag) = etag {
            request = request.header("if-none-match", etag);
        }
        request.send()
    };

    let original = get(url(&shot, ""), None).await.unwrap();
    assert_eq!(original.status(), 200);
    assert_eq!(original.headers()["content-type"], "image/jpeg");
    assert_eq!(
        original.headers()["cache-control"],
        "private, max-age=31536000, immutable"
    );
    let original_etag = original.headers()["etag"].to_str().unwrap().to_owned();
    assert_eq!(original.bytes().await.unwrap(), photo);

    let thumb = get(url(&shot, "&variant=thumbnail"), None).await.unwrap();
    assert_eq!(thumb.status(), 200);
    assert_eq!(thumb.headers()["content-type"], "image/jpeg");
    assert_eq!(
        thumb.headers()["cache-control"],
        "private, max-age=31536000, immutable"
    );
    assert_eq!(
        thumb.headers()["content-security-policy"],
        "sandbox; default-src 'none'"
    );
    let thumb_etag = thumb.headers()["etag"].to_str().unwrap().to_owned();
    assert_ne!(thumb_etag, original_etag);
    let thumb = thumb.bytes().await.unwrap();
    assert!(
        thumb.len() * 5 < photo.len(),
        "{} vs {}",
        thumb.len(),
        photo.len()
    );
    let decoded = image::load_from_memory(&thumb).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (218, 480));
    // Cached in the data directory, not in the workspace.
    let cache = dir.path().join("cache/thread-thumbnails");
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(&attachments).unwrap().count(), 2);
    let again = get(url(&shot, "&variant=thumbnail"), None).await.unwrap();
    assert_eq!(again.bytes().await.unwrap(), thumb);

    for (variant, etag) in [("", &original_etag), ("&variant=thumbnail", &thumb_etag)] {
        let response = get(url(&shot, variant), Some(format!("\"other\", {etag}")))
            .await
            .unwrap();
        assert_eq!(response.status(), 304, "{variant}");
        assert_eq!(response.headers()["etag"], etag.as_str());
        assert!(response.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable"));
        assert!(response.bytes().await.unwrap().is_empty());
    }
    let stale = get(
        url(&shot, "&variant=thumbnail"),
        Some(original_etag.clone()),
    )
    .await
    .unwrap();
    assert_eq!(stale.status(), 200);

    // GIFs, unknown variants and names that may be rewritten.
    let gif = get(
        url(
            &format!(".temp/threads/{tid}/anim.gif"),
            "&variant=thumbnail",
        ),
        None,
    )
    .await
    .unwrap();
    assert_eq!(gif.headers()["content-type"], "image/gif");
    assert_eq!(gif.headers()["cache-control"], "private, no-cache");
    assert_eq!(
        gif.bytes().await.unwrap().as_ref(),
        b"GIF89a\x01\x00\x01\x00"
    );
    let unknown = get(url(&shot, "&variant=huge"), None).await.unwrap();
    assert_eq!(unknown.bytes().await.unwrap(), photo);

    // The thumbnail variant validates paths exactly like the original.
    for path in [
        "../secret.jpg".to_owned(),
        format!(".temp/threads/{tid}/../../../../secret.jpg"),
        format!(".temp/threads/{other}/x-0a1b2c3d.jpg"),
        format!(".temp/threads/{tid}"),
        "/etc/passwd".to_owned(),
    ] {
        for variant in ["", "&variant=thumbnail"] {
            let response = get(url(&path, variant), Some("*".into())).await.unwrap();
            assert!(
                response.status().is_client_error(),
                "{path}{variant}: {}",
                response.status()
            );
        }
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            ws_root.join("secret.jpg"),
            attachments.join("link-0a1b2c3d.jpg"),
        )
        .unwrap();
        let link = format!(".temp/threads/{tid}/link-0a1b2c3d.jpg");
        for variant in ["", "&variant=thumbnail"] {
            assert_eq!(get(url(&link, variant), None).await.unwrap().status(), 400);
        }
    }

    // Workspace files revalidate instead of being cached as immutable.
    let workspace_id = workspace["id"].as_str().unwrap();
    std::fs::write(proj.join("diagram.png"), b"\x89PNG\r\n\x1a\nfirst").unwrap();
    let raw_url = format!("{base}/api/workspaces/{workspace_id}/files/raw?path=diagram.png");
    let raw = get(raw_url.clone(), None).await.unwrap();
    assert_eq!(raw.status(), 200);
    assert_eq!(raw.headers()["cache-control"], "private, no-cache");
    let raw_etag = raw.headers()["etag"].to_str().unwrap().to_owned();
    let cached = get(raw_url.clone(), Some(raw_etag.clone())).await.unwrap();
    assert_eq!(cached.status(), 304);
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(proj.join("diagram.png"), b"\x89PNG\r\n\x1a\nsecond!").unwrap();
    let changed = get(raw_url, Some(raw_etag.clone())).await.unwrap();
    assert_eq!(changed.status(), 200);
    assert_ne!(changed.headers()["etag"].to_str().unwrap(), raw_etag);
    assert_eq!(
        changed.bytes().await.unwrap().as_ref(),
        b"\x89PNG\r\n\x1a\nsecond!"
    );
}

async fn seed_tool_history(
    state: &Supervisor,
    client: &reqwest::Client,
    base: &str,
    root: &std::path::Path,
) -> String {
    let workspace = json(
        client,
        client
            .post(format!("{base}/api/workspaces"))
            .json(&json!({"absPath":root,"label":"Compact"})),
    )
    .await;
    let thread = json(client, client.post(format!("{base}/api/threads/start"))
        .json(&json!({"workspaceId":workspace["id"],"title":"Compact","provider":"codex","model":"ios-e2e-stream","approvalMode":"yolo"}))).await;
    let id = thread["id"].as_str().unwrap().to_owned();
    state.db.with(|conn| {
        conn.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('compact-turn',?1,'completed',0)", [&id])?;
        let mut items = vec![
            json!({"id":"user","kind":"userMessage","text":"inspect","sourceTurnId":"compact-turn"}),
            json!({"id":"reply","kind":"agentMessage","text":"done","sourceTurnId":"compact-turn"}),
            json!({"id":"note","kind":"reasoning","text":"thinking","detailText":"reasoning detail","sourceTurnId":"compact-turn"}),
        ];
        for kind in pockymoe_runtime::history::TOOL_ITEM_KINDS {
            items.push(json!({"id":format!("{kind}-1"),"kind":kind,"text":format!("{kind} title"),"previewText":format!("{kind} title"),"detailText":format!("{kind} full detail {}", "line\n".repeat(400)),"status":"completed","sourceTurnId":"compact-turn"}));
        }
        for (n, item) in items.iter().enumerate() {
            conn.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,?2,'compact-turn',?3,?4,?5,?5)",
                (format!("row-{n}"), &id, item["id"].as_str().unwrap(), item.to_string(), format!("2030-01-01T00:00:{n:02}Z")))?;
        }
        Ok(())
    }).unwrap();
    id
}

#[tokio::test]
async fn compact_history_defers_every_tool_kind_only_when_requested() {
    let (_dir, port, root, state) = spawn_supervisor_state(vec![Provider::Codex], |_| {}).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let id = seed_tool_history(&state, &client, &base, &root).await;
    let item = |turn: &Value, id: &str| {
        turn["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .cloned()
            .unwrap()
    };
    // Old Web, CLI and native clients: unchanged legacy encoding.
    let legacy = json(
        &client,
        client.get(format!("{base}/api/threads/{id}/turns/compact-turn/detail")),
    )
    .await;
    assert_eq!(
        item(&legacy, "commandExecution-1")["hasDeferredDetail"],
        true
    );
    assert!(item(&legacy, "commandExecution-1")
        .get("detailText")
        .is_none());
    assert!(item(&legacy, "fileRead-1")["detailText"]
        .as_str()
        .unwrap()
        .starts_with("fileRead full detail"));
    assert_eq!(item(&legacy, "toolCall-1")["previewText"], "toolCall title");
    assert_eq!(item(&legacy, "user")["sourceTurnId"], "compact-turn");

    let compact = json(
        &client,
        client.get(format!(
            "{base}/api/threads/{id}/turns/compact-turn/detail?compact=1"
        )),
    )
    .await;
    for kind in pockymoe_runtime::history::TOOL_ITEM_KINDS {
        let value = item(&compact, &format!("{kind}-1"));
        assert!(value.get("detailText").is_none(), "{value}");
        assert_eq!(value["hasDeferredDetail"], true, "{value}");
        assert!(value.get("sourceTurnId").is_none(), "{value}");
        assert_eq!(
            value.get("previewText").is_some(),
            *kind == "fileChange",
            "{value}"
        );
        // Expanding the item loads the full detail through the existing endpoint.
        let detail = json(
            &client,
            client.get(format!("{base}/api/threads/{id}/items/{kind}-1/detail")),
        )
        .await;
        assert!(
            detail["text"]
                .as_str()
                .unwrap()
                .starts_with(&format!("{kind} full detail")),
            "{detail}"
        );
        assert_eq!(detail["title"], format!("{kind} title"));
    }
    assert_eq!(item(&compact, "note")["detailText"], "reasoning detail");
    assert!(item(&compact, "user").get("sourceTurnId").is_none());

    let summary = json(
        &client,
        client.get(format!(
            "{base}/api/threads/{id}?view=summary&limit=3&compact=1"
        )),
    )
    .await;
    let turn = &summary["turns"][0];
    assert_eq!(turn["hasDeferredItems"], true);
    assert!(item(turn, "user").get("sourceTurnId").is_none());
    let full = json(
        &client,
        client.get(format!("{base}/api/threads/{id}?view=full&compact=1")),
    )
    .await;
    assert!(item(&full["turns"][0], "webSearch-1")
        .get("detailText")
        .is_none());
    let full = json(
        &client,
        client.get(format!("{base}/api/threads/{id}?view=full")),
    )
    .await;
    assert!(item(&full["turns"][0], "webSearch-1")["detailText"].is_string());
}

#[tokio::test]
async fn direct_http_compresses_large_text_for_clients_that_accept_it() {
    use std::io::Read;
    let (_dir, port, root, state) = spawn_supervisor_state(vec![Provider::Codex], |_| {}).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let id = seed_tool_history(&state, &client, &base, &root).await;
    let url = format!("{base}/api/threads/{id}/turns/compact-turn/detail");
    let plain = client.get(&url).send().await.unwrap();
    assert!(plain.headers().get("content-encoding").is_none());
    let plain = plain.bytes().await.unwrap();
    assert!(plain.len() > 8 * 1024);

    let gzip = client
        .get(&url)
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert_eq!(gzip.headers()["content-encoding"], "gzip");
    assert!(gzip.headers()["vary"]
        .to_str()
        .unwrap()
        .contains("accept-encoding"));
    let body = gzip.bytes().await.unwrap();
    assert!(
        body.len() * 5 < plain.len(),
        "{} vs {}",
        body.len(),
        plain.len()
    );
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(&body[..])
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, plain);

    let br = client
        .get(&url)
        .header("accept-encoding", "br")
        .send()
        .await
        .unwrap();
    assert_eq!(br.headers()["content-encoding"], "br");
    assert!(br.bytes().await.unwrap().len() * 5 < plain.len());

    // Small bodies stay identity even when the client accepts gzip.
    let health = client
        .get(format!("{base}/healthz"))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert!(health.headers().get("content-encoding").is_none());
}

#[tokio::test]
async fn local_web_ui_is_served_runnable_with_real_content_types() {
    let dist = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dist.path().join("assets")).unwrap();
    std::fs::write(
        dist.path().join("index.html"),
        "<!doctype html><script type=\"module\" src=\"/assets/app.js\"></script>",
    )
    .unwrap();
    std::fs::write(dist.path().join("assets/app.js"), "export {};").unwrap();
    std::fs::write(dist.path().join("assets/art.webp"), b"RIFF0000WEBP").unwrap();
    // Only Web UI paths read this variable; API tests are unaffected.
    std::env::set_var("POCKYMOE_WEB_DIST_DIR", dist.path());
    let (_dir, port, _root) = spawn_supervisor(vec![Provider::Codex]).await;
    let client = reqwest::Client::new();
    let get = |path: &str| client.get(format!("http://127.0.0.1:{port}{path}")).send();
    for (path, content_type) in [
        ("/", "text/html; charset=utf-8"),
        ("/threads/some-thread", "text/html; charset=utf-8"),
        ("/assets/app.js", "text/javascript; charset=utf-8"),
        ("/assets/art.webp", "image/webp"),
    ] {
        let response = get(path).await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert_eq!(response.headers()["content-type"], content_type, "{path}");
        // A sandbox CSP or text/plain scripts would stop the app from running.
        assert!(
            response.headers().get("content-security-policy").is_none(),
            "{path}"
        );
    }
    std::env::remove_var("POCKYMOE_WEB_DIST_DIR");
}
