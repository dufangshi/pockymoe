use super::*;
use crate::owner_trust::{
    tests::{decoded, encrypted, encrypted_with_key, live_owner},
    BrowserCertificate, BrowserPrincipal,
};
use p256::ecdsa::SigningKey;
use rand::rngs::OsRng;
fn grantee_material() -> (PinnedAccount, SigningKey, BrowserCertificate, SigningKey) {
    let account = SigningKey::random(&mut OsRng);
    let browser = SigningKey::random(&mut OsRng);
    let pin = crate::owner_trust::tests::account(&account);
    let cert = crate::owner_trust::tests::browser_certificate(&account, &browser);
    (pin, account, cert, browser)
}
fn grant(record: PinnedAccount) -> Grant {
    Grant {
        version: 1,
        grant_id: B64.encode([1; 16]),
        relay_ref: RelayRef {
            kind: "share".into(),
            id: "relay-row".into(),
        },
        grantee: Grantee {
            user_id: "recipient".into(),
            label: "Recipient".into(),
            account_key_id: record.account_key_id,
            account_public_key: record.account_public_key,
        },
        scope: "thread".into(),
        thread_id: Some("root".into()),
        workspace_id: Some("workspace".into()),
        workspace_scope: "selected".into(),
        workspace_ids: vec![],
        thread_access: "read".into(),
        workspace_access: "none".into(),
        can_create_threads: false,
        expires_at: None,
    }
}
pub(crate) async fn request(
    state: &Arc<Supervisor>,
    p: Option<&BrowserPrincipal>,
    signing: &SigningKey,
    method: &str,
    path: &str,
    body: Value,
) -> (u64, Value) {
    let (r, key, aad) = encrypted_with_key(state, method, path, body, p, signing);
    decoded(
        crate::tunnel::forward_local(state, r, None).await,
        &key,
        &aad,
    )
}
async fn register(state: &Arc<Supervisor>, r: &BrowserCertificate) -> (u64, Value) {
    let (request, key, aad) = encrypted(
        state,
        "POST",
        "/api/transport/browser",
        serde_json::to_value(r).unwrap(),
        None,
    );
    decoded(
        crate::tunnel::forward_local(state, request, None).await,
        &key,
        &aad,
    )
}
pub(crate) async fn fixture() -> (
    tempfile::TempDir,
    Arc<Supervisor>,
    BrowserPrincipal,
    Grant,
    BrowserCertificate,
    SigningKey,
    BrowserPrincipal,
) {
    let (dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let owner = live_owner(&state);
    let (record, _, r, key) = grantee_material();
    let g = grant(record);
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    let (status, body) = register(&state, &r).await;
    assert_eq!(status, 200);
    assert_eq!(body["role"], "grantee");
    let p = BrowserPrincipal {
        account_key_id: g.grantee.account_key_id.clone(),
        browser_id: body["browserId"].as_str().unwrap().into(),
        grantee_user_id: Some("recipient".into()),
    };
    (dir, state, owner, g, r, key, p)
}
fn error_status(result: Result<Value>) -> u16 {
    owner_trust::response(result).status().as_u16()
}
#[tokio::test]
async fn grant_crud_validation_limits_and_owner_enforcement() {
    let (_dir, state, owner, mut g, _r, key, p) = fixture().await;
    let owner_key = crate::owner_trust::tests::session_key();
    let (status, body) = request(
        &state,
        Some(&owner),
        &owner_key,
        "GET",
        "/api/transport/grants",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["grants"][0]["createdBy"], owner.actor());
    assert!(body["grants"][0]["createdAt"].is_i64());
    let path = format!("/api/transport/grants/{}", g.grant_id);
    let (status, body) = request(
        &state,
        Some(&owner),
        &owner_key,
        "PUT",
        &path,
        serde_json::to_value(&g).unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["grant"]["grantId"], g.grant_id);
    for (method, path) in [
        ("GET", "/api/transport/grants"),
        ("PUT", path.as_str()),
        ("DELETE", path.as_str()),
        ("GET", "/api/transport/trust"),
        ("PUT", "/api/transport/device-certificate"),
        ("POST", "/api/port-mappings"),
        ("PATCH", "/api/config/workspace-settings"),
    ] {
        let (status, body) = request(&state, Some(&p), &key, method, path, json!({})).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (403, Some("owner_required")),
            "{path}"
        );
    }
    for field in [
        "version",
        "scope",
        "workspaceScope",
        "threadAccess",
        "workspaceAccess",
        "grantId",
    ] {
        let mut body = serde_json::to_value(&g).unwrap();
        body[field] = json!("invalid");
        let (status, _) = request(&state, Some(&owner), &owner_key, "PUT", &path, body).await;
        assert_eq!(status, 400, "{field}");
    }
    for field in ["accountKeyId", "accountPublicKey"] {
        let mut bad = serde_json::to_value(&g).unwrap();
        bad["grantee"][field] = json!("invalid");
        let (status, body) = request(&state, Some(&owner), &owner_key, "PUT", &path, bad).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (400, Some("invalid_grant"))
        );
    }
    let mut bad = g.clone();
    let pin = owner_trust::pinned_account(&state).unwrap().unwrap();
    bad.grantee.account_key_id = pin.account_key_id;
    bad.grantee.account_public_key = pin.account_public_key;
    let (status, body) = request(
        &state,
        Some(&owner),
        &owner_key,
        "PUT",
        &path,
        serde_json::to_value(bad).unwrap(),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (400, Some("grantee_is_owner"))
    );
    let mut bad = g.clone();
    bad.grant_id = B64.encode([8; 16]);
    bad.grantee.user_id = "conflicting-user".into();
    assert_eq!(
        error_status(put(
            &state.db,
            &bad.grant_id,
            &bad,
            &owner,
            owner_trust::now()
        )),
        400
    );
    for i in 2u16..=256 {
        g.grant_id = B64.encode([i.to_be_bytes().as_slice(), &[0; 14]].concat());
        put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    }
    g.grant_id = B64.encode([9; 16]);
    assert_eq!(
        error_status(put(&state.db, &g.grant_id, &g, &owner, owner_trust::now())),
        409
    );
}
#[tokio::test]
async fn grantee_registration_live_revocation_expiry_and_reset() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let owner = live_owner(&state);
    let (record, _, r, key) = grantee_material();
    let mut g = grant(record);
    assert_eq!(register(&state, &r).await.0, 403);
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    let (status, body) = register(&state, &r).await;
    assert_eq!(status, 200);
    let p = BrowserPrincipal {
        browser_id: body["browserId"].as_str().unwrap().into(),
        account_key_id: g.grantee.account_key_id.clone(),
        grantee_user_id: Some("recipient".into()),
    };
    assert!(p.valid(&state));
    assert_eq!(p.actor(), "grantee:recipient");
    let first = g.grant_id.clone();
    g.grant_id = B64.encode([2; 16]);
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    delete(&state.db, &first, owner_trust::now()).unwrap();
    assert!(p.valid(&state));
    delete(&state.db, &g.grant_id, owner_trust::now()).unwrap();
    assert!(!p.valid(&state));
    assert_eq!(
        request(
            &state,
            Some(&p),
            &key,
            "GET",
            "/api/threads/root",
            Value::Null
        )
        .await
        .0,
        401
    );
    // An explicit new grant permits fresh registration with the account's certificate.
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    assert_eq!(register(&state, &r).await.0, 200);
    // Expiry immediately removes authority.
    let (_other, other, other_owner, mut expiring, _r, _key, principal) = fixture().await;
    expiring.expires_at = Some(owner_trust::now() - 1);
    put(
        &other.db,
        &expiring.grant_id,
        &expiring,
        &other_owner,
        owner_trust::now(),
    )
    .unwrap();
    assert!(!principal.valid(&other));
    owner_trust::reset_account_trust(&state.config.database_url).unwrap();
    state
        .db
        .with(|c| {
            assert!(rows(c)?.is_empty());
            assert_eq!(
                c.query_row("SELECT count(*) FROM browser_registrations", [], |r| r
                    .get::<_, u64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
    assert!(!owner_trust::strict(&state));
}
#[tokio::test]
async fn thread_lineage_and_route_access_are_local_read_and_control() {
    let (_dir, state, owner, mut g, _r, key, p) = fixture().await;
    // Minimal real lineage rows; no relay-provided parent IDs are consulted.
    let ws = state
        .create_workspace(pockymoe_protocol::CreateWorkspaceInput {
            abs_path: Some(state.config.workspace_root.to_string_lossy().into()),
            git_url: None,
            label: Some("ACL tests".into()),
        })
        .unwrap();
    let mut parent = None;
    let mut ids = Vec::new();
    for depth in 0..=4 {
        let id = format!("lineage-{depth}");
        state.db.with(|c| { c.execute("INSERT INTO threads(id,workspace_id,provider,title,source,status,created_at,updated_at,parent_thread_id,lineage_depth) VALUES(?1,?2,'codex','ACL','web','idle','now','now',?3,?4)",params![id,ws.id,parent,depth])?;Ok(())}).unwrap();
        parent = Some(id.clone());
        ids.push(id);
    }
    g.thread_id = Some(ids[0].clone());
    g.workspace_id = Some(ws.id.clone());
    g.workspace_access = "read".into();
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    for id in &ids[..4] {
        assert!(authorize_path(&state, &p, "GET", &format!("/api/threads/{id}/search")).is_ok());
        assert!(authorize_path(&state, &p, "POST", &format!("/api/threads/{id}/prompt")).is_err());
    }
    assert!(authorize_path(
        &state,
        &p,
        "GET",
        &format!("/api/threads/{}/search", ids[4])
    )
    .is_err());
    for (method, path, allowed) in [
        ("GET", format!("/api/workspaces/{}/files/raw", ws.id), true),
        (
            "POST",
            format!("/api/workspaces/{}/files/save", ws.id),
            false,
        ),
        ("GET", "/api/search".into(), false),
        ("GET", "/api/device/metrics".into(), false),
        ("GET", "/api/workspaces/other/files/raw".into(), false),
        ("GET", "/api/threads/other".into(), false),
    ] {
        assert_eq!(
            authorize_path(&state, &p, method, &path).is_ok(),
            allowed,
            "{path}"
        );
    }
    g.thread_access = "control".into();
    g.workspace_access = "write".into();
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    assert!(authorize_path(
        &state,
        &p,
        "POST",
        &format!("/api/threads/{}/prompt", ids[3])
    )
    .is_ok());
    assert!(authorize_path(
        &state,
        &p,
        "POST",
        &format!("/api/workspaces/{}/files/save", ws.id)
    )
    .is_ok());
    // Real HPKE dispatch refuses out-of-scope reads before routing, and permits scoped reads.
    let (status, body) = request(
        &state,
        Some(&p),
        &key,
        "GET",
        "/api/threads/other",
        Value::Null,
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (403, Some("grant_denied")));
    assert_eq!(
        request(
            &state,
            Some(&p),
            &key,
            "GET",
            &format!("/api/threads/{}/search?q=test", ids[1]),
            Value::Null
        )
        .await
        .0,
        200
    );
    g.relay_ref.kind = "grant".into();
    g.scope = "device".into();
    g.thread_id = None;
    g.can_create_threads = true;
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    for (method, path) in [
        ("GET", "/api/search"),
        ("GET", "/api/workspaces/other/search"),
        ("GET", "/api/device/metrics"),
        ("POST", "/api/threads/start"),
    ] {
        assert!(authorize_path(&state, &p, method, path).is_ok(), "{path}");
    }
    g.scope = "workspace".into();
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    assert!(authorize_path(
        &state,
        &p,
        "GET",
        &format!("/api/workspaces/{}/files/preview", ws.id)
    )
    .is_ok());
    assert!(authorize_path(&state, &p, "GET", &format!("/api/threads/{}", ids[0])).is_err());
    assert!(authorize_path(&state, &p, "GET", "/api/search").is_err());
}

#[tokio::test]
async fn websocket_filters_threads_unknown_messages_and_live_changes() {
    let (_dir, state, owner, mut g, _r, _key, p) = fixture().await;
    assert!(websocket_message_allowed(
        &state,
        &p,
        Some("root"),
        &json!({"type":"supervisor.ping"})
    ));
    for kind in [
        "thread.prompt",
        "shell.attach",
        "shell.input",
        "shell.resize",
        "shell.clear",
    ] {
        assert!(
            !websocket_message_allowed(
                &state,
                &p,
                Some("root"),
                &json!({"type":kind,"shellId":"nonexistent"})
            ),
            "{kind}"
        );
    }
    assert!(websocket_event_allowed(
        &state,
        &p,
        Some("root"),
        &json!({"type":"thread.updated","threadId":"root"})
    ));
    assert!(!websocket_event_allowed(
        &state,
        &p,
        Some("root"),
        &json!({"type":"thread.updated","threadId":"other"})
    ));
    assert!(!websocket_event_allowed(
        &state,
        &p,
        None,
        &json!({"type":"private.event"})
    ));
    g.thread_id = Some("other".into());
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    assert!(!websocket_event_allowed(
        &state,
        &p,
        Some("root"),
        &json!({"type":"thread.updated","threadId":"root"})
    ));
    assert!(!websocket_message_allowed(
        &state,
        &p,
        Some("root"),
        &json!({"type":"supervisor.ping"})
    ));
}
#[cfg(unix)]
#[tokio::test]
async fn websocket_shell_control_uses_local_shell_thread_and_rechecks_downgrades() {
    let (_dir, state, owner, mut g, _r, _key, p) = fixture().await;
    let (id, _) = crate::shells::hub()
        .create(
            "root",
            "workspace",
            state.config.workspace_root.to_str().unwrap(),
            80,
            24,
            None,
        )
        .unwrap();
    let (other, _) = crate::shells::hub()
        .create(
            "other",
            "workspace",
            state.config.workspace_root.to_str().unwrap(),
            80,
            24,
            None,
        )
        .unwrap();
    let attach = json!({"type":"shell.attach","shellId":id,"cols":80,"rows":24});
    assert!(!websocket_message_allowed(
        &state,
        &p,
        Some("root"),
        &attach
    ));
    g.thread_access = "control".into();
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    for kind in [
        "shell.attach",
        "shell.detach",
        "shell.input",
        "shell.resize",
        "shell.clear",
    ] {
        assert!(
            websocket_message_allowed(&state, &p, Some("root"), &json!({"type":kind,"shellId":id})),
            "{kind}"
        );
        assert!(
            !websocket_message_allowed(
                &state,
                &p,
                Some("root"),
                &json!({"type":kind,"shellId":other,"threadId":"root"})
            ),
            "{kind}"
        );
    }
    assert!(websocket_event_allowed(
        &state,
        &p,
        Some("root"),
        &json!({"type":"shell.output","shellId":id,"payload":{"data":"scoped output"}})
    ));
    assert!(!websocket_event_allowed(
        &state,
        &p,
        Some("root"),
        &json!({"type":"shell.output","shellId":other,"threadId":"root","payload":{"data":"private output"}})
    ));
    g.thread_access = "read".into();
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    assert!(!websocket_event_allowed(
        &state,
        &p,
        Some("root"),
        &json!({"type":"shell.output","shellId":id})
    ));
    assert!(!websocket_message_allowed(
        &state,
        &p,
        Some("root"),
        &attach
    ));
    crate::shells::hub().terminate(&id).unwrap();
    crate::shells::hub().terminate(&other).unwrap();
}
#[cfg(target_os = "linux")]
#[tokio::test]
async fn file_save_actor_comes_from_grantee_not_relay_headers() {
    let (_dir, state, owner, mut g, _r, key, p) = fixture().await;
    let workspace = state
        .create_workspace(pockymoe_protocol::CreateWorkspaceInput {
            abs_path: Some(state.config.workspace_root.to_string_lossy().into()),
            git_url: None,
            label: None,
        })
        .unwrap();
    g.workspace_id = Some(workspace.id.clone());
    g.workspace_access = "write".into();
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    std::fs::write(state.config.workspace_root.join("actor.txt"), "base").unwrap();
    let doc = state.file_document(&workspace.id, "actor.txt").unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let save = pockymoe_runtime::file_documents::SaveDocument {
        path: doc.path,
        workspace_revision: doc.workspace_revision,
        file_identity: doc.file_identity,
        expected_hash: doc.content_hash.unwrap(),
        content: "recipient".into(),
        draft_revision: 1,
        operation_id: operation.clone(),
        operation_created_at: owner_trust::now() as u64,
    };
    let (mut r, response_key, aad) = encrypted_with_key(
        &state,
        "POST",
        &format!("/api/workspaces/{}/files/save", workspace.id),
        serde_json::to_value(save).unwrap(),
        Some(&p),
        &key,
    );
    r["headers"]["x-rcd-file-actor"] = json!("owner:forged");
    let (status, body) = decoded(
        crate::tunnel::forward_local(&state, r, None).await,
        &response_key,
        &aad,
    );
    assert_eq!(status, 200);
    assert_eq!(body["status"], "saved");
    assert!(state
        .file_operation("grantee:recipient", &workspace.id, &operation)
        .is_ok());
    assert!(state
        .file_operation("relay:owner:forged", &workspace.id, &operation)
        .is_err());
}

#[tokio::test]
async fn grantee_registration_accepts_key_fetched_from_thread_scoped_handshake() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let owner = live_owner(&state);
    let (record, _, registration, signing_key) = grantee_material();
    let g = grant(record);
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    let response=crate::tunnel::forward_local(&state,json!({"method":"GET","path":"/api/threads/root/transport/key?challenge=recipient-handshake"}),None).await;
    assert_eq!(response["statusCode"], 200);
    let descriptor: Value = serde_json::from_str(response["body"].as_str().unwrap()).unwrap();
    assert_eq!(descriptor["challenge"], "recipient-handshake");
    let (request, key, aad) = crate::owner_trust::tests::encrypted_with_descriptor(
        "POST",
        "/api/transport/browser",
        serde_json::to_value(&registration).unwrap(),
        None,
        &signing_key,
        &descriptor,
    );
    assert_eq!(request["headers"]["x-rcd-key"], descriptor["keyId"]);
    let (status, body) = decoded(
        crate::tunnel::forward_local(&state, request, None).await,
        &key,
        &aad,
    );
    assert_eq!(status, 200);
    assert_eq!(body["role"], "grantee");
    let principal = BrowserPrincipal {
        browser_id: body["browserId"].as_str().unwrap().into(),
        account_key_id: registration.account_key_id,
        grantee_user_id: Some(g.grantee.user_id),
    };
    assert!(principal.valid(&state));
    // The fetched key is also accepted for the signed thread channel request.
    let (request, key, aad) = crate::owner_trust::tests::encrypted_with_descriptor(
        "POST",
        "/api/threads/root/transport/session",
        Value::Null,
        Some(&principal),
        &signing_key,
        &descriptor,
    );
    let (status, body) = decoded(
        crate::tunnel::forward_local(&state, request, None).await,
        &key,
        &aad,
    );
    assert_eq!(status, 200);
    assert_eq!(
        crate::secure_transport::transport(&state)
            .unwrap()
            .session(body["channelId"].as_str().unwrap())
            .unwrap()
            .principal
            .as_ref()
            .unwrap()
            .actor(),
        "grantee:recipient"
    );
}
