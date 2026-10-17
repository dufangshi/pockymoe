//! Target side of device-to-device messaging: `/api/peer/cli` for other devices of
//! the same owner, and this device's opt-in. Contract: docs/cross-device-peer.zh.md.
use crate::{
    auth::PeerCaller,
    http::{err, map_err, ApiErr},
};
use axum::{extract::State, http::StatusCode, Extension, Json};
use pockymoe_runtime::{
    interaction::{AgentRequester, RemoteSender, SendInput},
    Supervisor,
};
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) async fn cli(
    State(state): State<Arc<Supervisor>>,
    caller: Option<Extension<PeerCaller>>,
    Json(mut input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    let Extension(caller) = caller.ok_or_else(|| {
        err(
            StatusCode::FORBIDDEN,
            "peer_forbidden",
            "Peer device identity is required.",
        )
    })?;
    if !state.peer_access_enabled() {
        return Err(err(
            StatusCode::FORBIDDEN,
            "peer_access_disabled",
            "Peer access is disabled on this device.",
        ));
    }
    let operation = input["operation"].as_str().unwrap_or("").to_owned();
    if ![
        "info",
        "workspaces",
        "list",
        "status",
        "show",
        "transcript",
        "backends",
        "models",
        "create",
        "send",
    ]
    .contains(&operation.as_str())
    {
        return Err(err(
            StatusCode::FORBIDDEN,
            "peer_operation_forbidden",
            "This operation is not available across devices.",
        ));
    }
    // An isolated device still answers `info`, so callers can tell why.
    if operation != "info" {
        state.ensure_peer_reaches_device().map_err(map_err)?;
        caller.ensure_source_private()?;
    }
    let sender = RemoteSender {
        device_id: caller.device_id,
        device_name: caller.device_name,
        thread_id: input["fromThreadId"].as_str().map(str::to_owned),
    };
    match operation.as_str() {
        "info" => {
            let identity = crate::peer_link::relay_identity(&state);
            Ok(Json(json!({
                "deviceId": state.db.host_id,
                "relayDeviceId": identity.as_ref().map(|i| &i.device_id),
                "deviceName": identity.as_ref().map(|i| &i.device_name),
                "peerAccess": true,
                "isolated": state.agent_isolation().device,
            })))
        }
        "workspaces" => Ok(Json(json!(state
            .list_workspaces()
            .map_err(map_err)?
            .iter()
            .filter(|ws| !ws.agent_isolated)
            .map(|ws| { json!({"id": ws.id, "name": ws.label, "absPath": ws.abs_path}) })
            .collect::<Vec<_>>()))),
        "send" => {
            let id = input["threadId"]
                .as_str()
                .ok_or_else(|| map_err(anyhow::anyhow!("threadId required")))?;
            state
                .ensure_agent_reaches_thread(AgentRequester::Peer, id)
                .map_err(map_err)?;
            let body = serde_json::from_value::<SendInput>(input.clone())
                .map_err(|e| map_err(e.into()))?;
            let mut receipt = state
                .send_to_thread_from_peer(id, body, sender)
                .map_err(map_err)?;
            if receipt["delivery"] == "steer" {
                let pending = receipt["pendingSteerId"].as_str().unwrap();
                match state.steer_pending_prompt(id, pending).await {
                    Ok(_) => receipt["delivery"] = json!("steered"),
                    Err(error) => {
                        receipt["delivery"] = json!("held");
                        receipt["error"] = json!(error.to_string());
                    }
                }
            }
            Ok(Json(receipt))
        }
        _ => {
            // A remote thread ID has no meaning in this device's lineage or defaults.
            let fields = input.as_object_mut().unwrap();
            fields.remove("fromThreadId");
            if operation == "create" {
                if !fields
                    .get("workspaceId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty())
                {
                    return Err(map_err(anyhow::anyhow!(
                        "workspaceId required for remote create"
                    )));
                }
                fields.remove("parentThreadId");
                fields.remove("name");
            }
            let result = crate::interaction::run_from_peer(&state, input)
                .await
                .map_err(map_err)?;
            if operation == "create" {
                if let Some(id) = result["threadId"].as_str() {
                    state
                        .db
                        .set_kv(
                            &format!("peer:origin:{id}"),
                            &serde_json::to_string(&sender).map_err(|e| map_err(e.into()))?,
                        )
                        .map_err(map_err)?;
                }
            }
            Ok(Json(result))
        }
    }
}

pub(crate) async fn access(State(state): State<Arc<Supervisor>>) -> Json<Value> {
    Json(json!({"enabled": state.peer_access_enabled()}))
}

pub(crate) async fn set_access(
    State(state): State<Arc<Supervisor>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    let enabled = input["enabled"]
        .as_bool()
        .ok_or_else(|| map_err(anyhow::anyhow!("enabled must be true or false")))?;
    Ok(Json(state.set_peer_access(enabled).map_err(map_err)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::TrustedRelayForward;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
        Router,
    };
    use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Mode, Provider};
    use pockymoe_runtime::{
        fake::FakeRuntime, local_sessions::LocalSessionHomes, Database, RuntimeConfig,
    };
    use std::{process::Command, time::Duration};
    use tower::ServiceExt;

    fn setup() -> (tempfile::TempDir, Arc<Supervisor>, Router, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let config = RuntimeConfig {
            treer_enabled: false,
            host_mode: Default::default(),
            mode: Mode::Local,
            host: "127.0.0.1".into(),
            port: 0,
            workspace_root: root.clone(),
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
        let workspace = state
            .create_workspace(CreateWorkspaceInput {
                abs_path: Some(root.to_string_lossy().into()),
                git_url: None,
                label: Some("Peer workspace".into()),
            })
            .unwrap();
        let router = crate::http::router(state.clone());
        (dir, state, router, workspace.id)
    }

    fn caller(device: &str) -> PeerCaller {
        PeerCaller {
            device_id: device.into(),
            device_name: "Peer laptop".into(),
            user_id: "same-owner".into(),
            shared_control: None,
        }
    }

    async fn request(
        router: &Router,
        input: Value,
        peer: Option<PeerCaller>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/peer/cli")
            .header("content-type", "application/json")
            .extension(TrustedRelayForward);
        if let Some(peer) = peer {
            request = request.extension(peer);
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::from(input.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        (status, body)
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

    #[tokio::test]
    async fn peer_cli_requires_identity_opt_in_and_allowlisted_operation() {
        let (_dir, state, router, _) = setup();
        let (status, _) = request(&router, json!({"operation":"info"}), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, body) = request(
            &router,
            json!({"operation":"delete"}),
            Some(caller("device-a")),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "peer_access_disabled");
        state.set_peer_access(true).unwrap();
        let (status, body) = request(
            &router,
            json!({"operation":"info"}),
            Some(caller("device-a")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["deviceId"], state.db.host_id);
        assert_eq!(body["peerAccess"], true);
        assert!(body.get("relayDeviceId").is_some());
        assert!(body.get("deviceName").is_some());
        for operation in [
            "wait",
            "wake",
            "tree",
            "taskAdd",
            "close",
            "delete",
            "inbox",
            "inboxRead",
            "inboxAck",
            "inboxWait",
            "peerAccess",
            "resolve",
            "unknown",
            "",
        ] {
            let (status, body) = request(
                &router,
                json!({"operation":operation,"enabled":false}),
                Some(caller("device-a")),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{operation}: {body}");
            assert_eq!(body["code"], "peer_operation_forbidden");
        }
        assert!(state.peer_access_enabled());
        let (status, _) = request(
            &router,
            json!({"operation":"info","peerCaller":{"deviceId":"forged"}}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn peer_cli_reads_ignore_remote_from_and_models_select_workspace() {
        let (_dir, state, router, workspace) = setup();
        state.set_peer_access(true).unwrap();
        let target = thread(&state, &workspace).await;
        let (status, workspaces) = request(
            &router,
            json!({"operation":"workspaces"}),
            Some(caller("device-a")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            workspaces,
            json!([{"id": workspace, "name":"Peer workspace", "absPath":state.config.workspace_root.to_string_lossy()}])
        );
        for operation in ["list", "show", "status", "transcript", "backends", "models"] {
            let (status, body) = request(
                &router,
                json!({
                    "operation":operation,"threadId":target,"fromThreadId":"not-a-local-thread",
                    "provider":"codex","workspaceId":workspace,
                }),
                Some(caller("device-a")),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{operation}: {body}");
        }
        let (status, _) = request(
            &router,
            json!({"operation":"models","provider":"codex","workspaceId":"missing"}),
            Some(caller("device-a")),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn peer_cli_create_drops_lineage_and_inheritance_and_records_origin() {
        let (_dir, state, router, workspace) = setup();
        state.set_peer_access(true).unwrap();
        let local_parent = thread(&state, &workspace).await;
        let (status, _) = request(&router, json!({"operation":"create","provider":"codex","model":"default","fromThreadId":local_parent}), Some(caller("device-a"))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(state.list_threads(None, true).unwrap().len(), 1);
        let (status, created) = request(
            &router,
            json!({
                "operation":"create","workspaceId":workspace,"provider":"codex","model":"default",
                "fromThreadId":local_parent,"parentThreadId":local_parent,"name":"foreign-name",
                "deviceId":"forged", "deviceName":"forged",
            }),
            Some(caller("device-a")),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let id = created["threadId"].as_str().unwrap();
        let saved = state.get_thread(id).unwrap();
        assert!(saved.parent_thread_id.is_none());
        assert!(saved.root_thread_id.is_none());
        assert!(saved.agent_name.is_none());
        assert_eq!(saved.approval_mode, "yolo");
        let origin: Value = serde_json::from_str(
            &state
                .db
                .get_kv(&format!("peer:origin:{id}"))
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            origin,
            json!({"deviceId":"device-a","deviceName":"Peer laptop","threadId":local_parent})
        );
    }

    #[tokio::test]
    async fn peer_cli_create_supports_workspace_role_worktree_and_explicit_approval() {
        let (_dir, state, router, workspace) = setup();
        state.set_peer_access(true).unwrap();
        let root = &state.config.workspace_root;
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Peer test",
                "-c",
                "user.email=peer@example.test",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "test root",
            ],
        ] {
            let output = Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let roles = root.join(".remote-codex/agents");
        std::fs::create_dir_all(&roles).unwrap();
        std::fs::write(
            roles.join("reviewer.md"),
            "---\nmodel: ios-e2e-stream\neffort: medium\n---\nReview the changes.",
        )
        .unwrap();
        let (status, created) = request(&router, json!({
            "operation":"create","workspaceId":workspace,"provider":"codex","model":"default",
            "fromThreadId":"foreign-thread","role":"reviewer","worktree":true,"approvalMode":"guarded",
        }), Some(caller("device-a"))).await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let saved = state
            .get_thread(created["threadId"].as_str().unwrap())
            .unwrap();
        assert_eq!(saved.agent_role.as_deref(), Some("reviewer"));
        assert_eq!(saved.model.as_deref(), Some("ios-e2e-stream"));
        assert_eq!(saved.reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(saved.approval_mode, "guarded");
        let worktree = std::path::Path::new(saved.worktree_path.as_deref().unwrap());
        assert!(worktree.starts_with(_dir.path()));
        assert!(worktree.join(".git").is_file());
        assert!(saved.parent_thread_id.is_none());
    }

    #[tokio::test]
    async fn peer_cli_remote_mail_and_retry_use_server_identity() {
        let (_dir, state, router, workspace) = setup();
        state.set_peer_access(true).unwrap();
        let target = thread(&state, &workspace).await;
        let input = json!({
            "operation":"send","threadId":target,"fromThreadId":"foreign-thread","text":"hello",
            "kind":"question","subject":"Review","clientRequestId":"stable","inReplyTo":"prior",
            "fromDeviceId":"forged","fromDeviceName":"forged","replyTo":"forged/thread",
            "remoteSender":{"deviceId":"forged","deviceName":"forged","threadId":"forged"},
        });
        let (status, receipt) = request(&router, input.clone(), Some(caller("device-a"))).await;
        assert_eq!(status, StatusCode::OK);
        let (status, retry) = request(&router, input.clone(), Some(caller("device-a"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(retry, receipt);
        let (status, another) = request(&router, input.clone(), Some(caller("device-b"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_ne!(another["messageId"], receipt["messageId"]);
        let mut conflict = input;
        conflict["text"] = json!("changed");
        let (status, _) = request(&router, conflict, Some(caller("device-a"))).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(state.inbox_unread_count(&target).unwrap(), 2);
        let message = state
            .inbox_read(&target, &json!({"messageId":receipt["messageId"]}))
            .unwrap();
        assert_eq!(message["fromDeviceId"], "device-a");
        assert_eq!(message["fromDeviceName"], "Peer laptop");
        assert_eq!(message["fromThreadId"], "foreign-thread");
        assert_eq!(message["replyTo"], "device-a/foreign-thread");
        assert_eq!(message["inReplyTo"], "prior");
        assert_eq!(message["kind"], "question");
        assert_eq!(message["subject"], "Review");

        let token = state
            .interaction
            .context
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .token
            .clone();
        for from in [Some("foreign-thread"), None] {
            let input = json!({
                "operation":"send","threadId":target,"text":"local JSON","fromThreadId":from,
                "fromDeviceId":"forged","fromDeviceName":"forged",
                "remoteSender":{"deviceId":"forged","deviceName":"forged","threadId":"foreign-thread"},
            });
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/cli")
                        .header("content-type", "application/json")
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::from(input.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let receipt: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            if from.is_some() {
                assert_eq!(status, StatusCode::NOT_FOUND);
            } else {
                assert_eq!(status, StatusCode::OK);
                let message = state
                    .inbox_read(&target, &json!({"messageId":receipt["messageId"]}))
                    .unwrap();
                assert!(message.get("fromDeviceId").is_none());
                assert!(message.get("replyTo").is_none());
            }
        }
    }

    #[tokio::test]
    async fn peer_cli_direct_and_steer_acknowledge_remote_input_on_active_turn() {
        let (_dir, state, router, workspace) = setup();
        state.set_peer_access(true).unwrap();
        for delivery in ["direct", "steer"] {
            let target = thread(&state, &workspace).await;
            let turn = uuid::Uuid::new_v4().to_string();
            state.db.with(|conn| {
                conn.execute("UPDATE threads SET status='running' WHERE id=?1", [&target])?;
                conn.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES (?1,?2,'inProgress',1)", [&turn, &target])?;
                Ok(())
            }).unwrap();
            let input = json!({
                "operation":"send","threadId":target,"fromThreadId":"foreign-thread","text":"correction",
                "delivery":delivery,"clientRequestId":"steer","subject":"Review","kind":"task",
                "interruptReason":"the active turn is working from stale input",
            });
            let (status, receipt) = request(&router, input.clone(), Some(caller("device-a"))).await;
            assert_eq!(status, StatusCode::OK, "{receipt}");
            assert_eq!(receipt["delivery"], "steered");
            assert_eq!(receipt["requestedDelivery"], delivery);
            let (_, retry) = request(&router, input, Some(caller("device-a"))).await;
            assert_eq!(retry, receipt);
            let transcript = state
                .transcript(
                    &target,
                    &pockymoe_runtime::interaction::TranscriptQuery {
                        turn_id: Some(turn),
                        view: Some("overview".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            let items = transcript["turns"][0]["items"].as_array().unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0]["text"], "[Pockymoe task from device-a/foreign-thread (device \"Peer laptop\") | Review]\nImmediate handling needed: the active turn is working from stale input\ncorrection");
        }
    }

    #[tokio::test]
    async fn peer_cli_completion_is_persisted_in_outbox_by_runtime() {
        let (_dir, state, router, workspace) = setup();
        state.set_peer_access(true).unwrap();
        let target = thread(&state, &workspace).await;
        let input = json!({
            "operation":"send","threadId":target,"fromThreadId":"foreign-thread","text":"hello",
            "delivery":"queue","notifyOnComplete":true,"clientRequestId":"notify","subject":"Review","kind":"task",
        });
        let (status, receipt) = request(&router, input.clone(), Some(caller("device-a"))).await;
        assert_eq!(status, StatusCode::OK, "{receipt}");
        let records = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let records: Vec<Value> = state
                    .db
                    .with(|conn| {
                        let mut stmt =
                            conn.prepare("SELECT value FROM kv WHERE key GLOB 'peer:outbox:*'")?;
                        let raw = stmt
                            .query_map([], |r| r.get::<_, String>(0))?
                            .collect::<std::result::Result<Vec<_>, _>>()?;
                        Ok(raw
                            .iter()
                            .map(|s| serde_json::from_str(s).unwrap())
                            .collect())
                    })
                    .unwrap();
                if !records.is_empty() {
                    break records;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record["targetDeviceId"], "device-a");
        assert_eq!(record["request"]["threadId"], "foreign-thread");
        assert_eq!(record["request"]["fromThreadId"], target);
        assert_eq!(record["request"]["delivery"], "inbox");
        assert_eq!(record["request"]["kind"], "result");
        let text = record["request"]["text"].as_str().unwrap();
        assert!(text.contains("ended with status completed"));
        assert!(text.contains("Its closing message:\nhello"));
        assert_eq!(state.inbox_unread_count(&target).unwrap(), 0);
        let (status, retry) = request(&router, input, Some(caller("device-a"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(retry, receipt);
        assert!(state
            .db
            .get_kv(&format!("peer:outbox:{}", record["id"].as_str().unwrap()))
            .unwrap()
            .is_some());
    }
}
