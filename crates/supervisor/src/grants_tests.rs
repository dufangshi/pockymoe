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
    let (dir, state, owner, g, r, key, p, _) = fixture_with_account().await;
    (dir, state, owner, g, r, key, p)
}
/// Same as `fixture`, also returning the grantee's account key so a test can
/// certify additional browsers for the same recipient.
async fn fixture_with_account() -> (
    tempfile::TempDir,
    Arc<Supervisor>,
    BrowserPrincipal,
    Grant,
    BrowserCertificate,
    SigningKey,
    BrowserPrincipal,
    SigningKey,
) {
    let (dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let owner = live_owner(&state);
    let (record, account, r, key) = grantee_material();
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
    (dir, state, owner, g, r, key, p, account)
}
fn error_status(result: Result<Value>) -> u16 {
    owner_trust::response(result).status().as_u16()
}
#[tokio::test]
async fn second_review_grantee_tombstones_are_capped_and_idempotent() {
    use p256::ecdsa::{signature::Signer, Signature};
    let (_dir, state, owner, _grant, _cert, _key, principal, account) =
        fixture_with_account().await;
    state
        .db
        .with(|c| {
            for n in 0..1023 {
                c.execute(
                    "INSERT INTO account_revocations VALUES(?1,'browser',?2,'{}')",
                    rusqlite::params![principal.account_key_id, format!("seed-{n}")],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let mut r = crate::owner_trust::tests::browser_revocation(&principal);
    r.subject_id = "last-grantee-tombstone".into();
    let signature: Signature = account.sign(owner_trust::revocation_signing_input(&r).as_bytes());
    r.signature = B64.encode(signature.to_bytes());
    let body = json!({"revocations":[r]});
    for _ in 0..2 {
        assert_eq!(
            request(
                &state,
                None,
                &crate::owner_trust::tests::session_key(),
                "PUT",
                "/api/transport/revocations",
                body.clone()
            )
            .await
            .0,
            200
        );
    }
    r.subject_id = "overflow-grantee-tombstone".into();
    let signature: Signature = account.sign(owner_trust::revocation_signing_input(&r).as_bytes());
    r.signature = B64.encode(signature.to_bytes());
    assert_eq!(
        request(
            &state,
            None,
            &crate::owner_trust::tests::session_key(),
            "PUT",
            "/api/transport/revocations",
            json!({"revocations":[r]})
        )
        .await,
        (429, json!({"code":"revocation_limit"}))
    );
    state
        .db
        .with(|c| {
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM account_revocations WHERE account_key_id=?1",
                    [&principal.account_key_id],
                    |r| r.get::<_, u64>(0)
                )?,
                1024
            );
            Ok(())
        })
        .unwrap();
    assert!(principal.valid(&state));
    let (second_pin, second_account, _certificate, _browser) = grantee_material();
    let mut second_grant = grant(second_pin);
    second_grant.grant_id = B64.encode([2; 16]);
    second_grant.grantee.user_id = "second-recipient".into();
    put(
        &state.db,
        &second_grant.grant_id,
        &second_grant,
        &owner,
        owner_trust::now(),
    )
    .unwrap();
    r.account_key_id = second_grant.grantee.account_key_id;
    let signature: Signature =
        second_account.sign(owner_trust::revocation_signing_input(&r).as_bytes());
    r.signature = B64.encode(signature.to_bytes());
    owner_trust::apply_revocations(&state, &[r]).unwrap();
    // The recipient's cap does not block owner-signed tombstones.
    owner_trust::apply_revocations(
        &state,
        &[crate::owner_trust::tests::browser_revocation(&owner)],
    )
    .unwrap();
}
#[tokio::test]
async fn second_review_missing_registration_only_owner_tombstone_is_revoked() {
    use p256::ecdsa::{signature::Signer, Signature};
    let (_dir, state, owner, _grant, _cert, _key, principal, account) =
        fixture_with_account().await;
    let mut r = crate::owner_trust::tests::browser_revocation(&principal);
    r.subject_id = owner.browser_id.clone();
    let signature: Signature = account.sign(owner_trust::revocation_signing_input(&r).as_bytes());
    r.signature = B64.encode(signature.to_bytes());
    owner_trust::apply_revocations(&state, &[r]).unwrap();
    state
        .db
        .with(|c| {
            c.execute(
                "DELETE FROM browser_registrations WHERE browser_id=?1",
                [&owner.browser_id],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        request(
            &state,
            Some(&owner),
            &crate::owner_trust::tests::session_key(),
            "GET",
            "/api/transport/trust",
            Value::Null
        )
        .await,
        (401, json!({"code":"browser_registration_required"}))
    );
    owner_trust::apply_revocations(
        &state,
        &[crate::owner_trust::tests::browser_revocation(&owner)],
    )
    .unwrap();
    assert_eq!(
        request(
            &state,
            Some(&owner),
            &crate::owner_trust::tests::session_key(),
            "GET",
            "/api/transport/trust",
            Value::Null
        )
        .await,
        (401, json!({"code":"browser_revoked"}))
    );
}
#[tokio::test]
async fn second_review_verified_owner_replaces_grantee_registration_and_channels() {
    let (_dir, state, owner, _grant, _cert, _key, _principal, account) =
        fixture_with_account().await;
    state
        .db
        .with(|c| {
            c.execute(
                "DELETE FROM browser_registrations WHERE browser_id=?1",
                [&owner.browser_id],
            )?;
            Ok(())
        })
        .unwrap();
    let key = crate::owner_trust::tests::session_key();
    let grantee_cert = crate::owner_trust::tests::browser_certificate(&account, &key);
    assert_eq!(register(&state, &grantee_cert).await.1["role"], "grantee");
    let old = BrowserPrincipal {
        browser_id: owner.browser_id.clone(),
        account_key_id: grantee_cert.account_key_id.clone(),
        grantee_user_id: Some("recipient".into()),
    };
    let (status, body) = request(
        &state,
        Some(&old),
        &key,
        "POST",
        "/api/threads/root/transport/session",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    let channel = crate::secure_transport::transport(&state)
        .unwrap()
        .session(body["channelId"].as_str().unwrap())
        .unwrap();
    let owner_cert = crate::owner_trust::tests::browser_certificate(
        &crate::owner_trust::tests::account_key(),
        &key,
    );
    let mut invalid = owner_cert.clone();
    invalid.signature = B64.encode([0; 64]);
    assert_eq!(register(&state, &invalid).await.0, 400);
    assert!(old.valid(&state));
    assert_eq!(register(&state, &owner_cert).await.1["role"], "owner");
    assert!(owner.valid(&state));
    assert!(!old.valid(&state));
    assert!(!channel.alive());
    // Recipients cannot displace the verified owner on subsequent registration.
    assert_eq!(
        register(&state, &grantee_cert).await,
        (403, json!({"code":"account_mismatch"}))
    );
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
    g.workspace_scope = "all".into();
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
async fn harness_processes_settings_and_thread_view_follow_grant_scope() {
    let (_dir, state, owner, mut g, _r, key, p) = fixture().await;
    let owner_key = crate::owner_trust::tests::session_key();
    let settings = "/api/harness-resources/settings";
    let (status, body) = request(
        &state,
        Some(&owner),
        &owner_key,
        "GET",
        "/api/device/processes",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["supervisorPid"], std::process::id());
    assert!(body["threads"].is_array() && body["metric"].is_string());
    let (status, body) = request(
        &state,
        Some(&owner),
        &owner_key,
        "GET",
        settings,
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, body),
        (
            200,
            json!({"idleReleaseMinutes":10,"maxLoadedThreads":null,"maxHarnessMemoryBytes":null})
        )
    );
    let invalid =
        json!({"idleReleaseMinutes":0,"maxLoadedThreads":null,"maxHarnessMemoryBytes":null});
    let (status, body) = request(&state, Some(&owner), &owner_key, "PUT", settings, invalid).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (400, Some("invalid_settings"))
    );
    let valid = json!({"idleReleaseMinutes":30,"maxLoadedThreads":5,"maxHarnessMemoryBytes":null});
    let (status, body) = request(
        &state,
        Some(&owner),
        &owner_key,
        "PUT",
        settings,
        valid.clone(),
    )
    .await;
    assert_eq!((status, body), (200, valid));

    // A thread share must not see other threads' titles or change the policy.
    for (method, path) in [
        ("GET", "/api/device/processes"),
        ("GET", settings),
        ("PUT", settings),
    ] {
        let (status, body) = request(&state, Some(&p), &key, method, path, json!({})).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (403, Some("grant_denied")),
            "{method} {path}"
        );
    }
    // Its relay connection displays only the shared thread.
    let (output, _outgoing) = crate::bounded_channel::channel();
    let socket = crate::socket::SocketSession::spawn_authorized(
        state.clone(),
        output,
        Some(p.clone()),
        Some("root".into()),
    );
    assert!(socket.send(json!({"type":"thread.view","threadIds":["root","other"]})));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !state.thread_displayed("root") {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!state.thread_displayed("other"));
    drop(socket);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while state.thread_displayed("root") {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    g.relay_ref.kind = "grant".into();
    g.scope = "device".into();
    g.workspace_scope = "all".into();
    g.thread_id = None;
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    assert!(authorize_path(&state, &p, "GET", "/api/device/processes").is_ok());
    assert!(authorize_path(&state, &p, "GET", settings).is_ok());
    assert!(
        authorize_path(&state, &p, "PUT", settings).is_err(),
        "read-only device access cannot change the policy"
    );
    g.thread_access = "control".into();
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    assert!(authorize_path(&state, &p, "PUT", settings).is_ok());
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

#[tokio::test]
async fn device_selected_workspaces_are_rejected_and_legacy_rows_fail_closed() {
    let (_dir, state, owner, g, _r, key, p) = fixture().await;
    let owner_key = crate::owner_trust::tests::session_key();
    let mut unsupported = g.clone();
    unsupported.relay_ref.kind = "grant".into();
    unsupported.scope = "device".into();
    unsupported.workspace_ids = vec!["selected-workspace".into()];
    unsupported.workspace_access = "write".into();
    unsupported.can_create_threads = true;
    for level in ["read", "control"] {
        unsupported.thread_access = level.into();
        assert_eq!(
            error_status(
                state
                    .db
                    .with(|c| validate(c, &g.grant_id, &unsupported))
                    .map(|_| json!({}))
            ),
            400
        );
        let (status, body) = request(
            &state,
            Some(&owner),
            &owner_key,
            "PUT",
            &format!("/api/transport/grants/{}", g.grant_id),
            serde_json::to_value(&unsupported).unwrap(),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (400, Some("invalid_grant"))
        );
    }
    // Keep a valid thread share so this remains an authenticated grantee. Simulate
    // an unsupported device grant persisted by an older Supervisor version.
    unsupported.grant_id = B64.encode([9; 16]);
    state.db.with(|c| {
        c.execute("INSERT INTO device_grants(grant_id,grant_json,created_at,created_by) VALUES(?1,?2,?3,?4)", params![unsupported.grant_id, serde_json::to_string(&unsupported)?, owner_trust::now(), owner.actor()])?;
        assert_eq!(live_rows(c, owner_trust::now())?.len(), 1);
        Ok(())
    }).unwrap();
    for (method, path) in [
        ("GET", "/api/workspaces/selected-workspace/files/raw"),
        ("GET", "/api/workspaces/other/files/raw"),
        ("GET", "/api/threads/other"),
        ("GET", "/api/workspaces/other/search"),
        ("GET", "/api/search"),
        ("GET", "/api/threads"),
        ("GET", "/api/workspaces"),
        ("POST", "/api/threads/start"),
    ] {
        assert!(authorize_path(&state, &p, method, path).is_err());
        let (status, body) = request(
            &state,
            Some(&p),
            &key,
            method,
            path,
            json!({"workspaceId":"other"}),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (403, Some("grant_denied")),
            "{path}"
        );
    }
    delete(&state.db, &g.grant_id, owner_trust::now()).unwrap();
    assert!(!p.valid(&state));
    assert!(state
        .db
        .with(|c| live_account(c, &p.account_key_id, owner_trust::now()))
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn device_grantees_cannot_read_host_local_import_candidates() {
    let (_dir, state, owner, mut g, _r, key, p) = fixture().await;
    g.relay_ref.kind = "grant".into();
    g.scope = "device".into();
    g.workspace_scope = "all".into();
    for level in ["read", "control"] {
        g.thread_access = level.into();
        put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
        for literal in ["start", "import", "import-candidates"] {
            let path = format!("/api/threads/{literal}?view=all");
            assert!(authorize_path(&state, &p, "GET", &path).is_err());
            let (status, body) = request(&state, Some(&p), &key, "GET", &path, Value::Null).await;
            assert_eq!(
                (status, body["code"].as_str()),
                (403, Some("grant_denied")),
                "{path}"
            );
        }
    }
    assert!(authorize_path(&state, &owner, "GET", "/api/threads/import-candidates").is_ok());
    assert_eq!(
        request(
            &state,
            Some(&owner),
            &crate::owner_trust::tests::session_key(),
            "GET",
            "/api/threads/import-candidates",
            Value::Null
        )
        .await
        .0,
        200
    );
}

async fn download_request(
    state: &Arc<Supervisor>,
    p: &BrowserPrincipal,
    key: &SigningKey,
    path: &str,
) -> (Value, Vec<u8>) {
    let (r, response_key, aad) = encrypted_with_key(state, "GET", path, Value::Null, Some(p), key);
    crate::owner_trust::tests::decoded_parts(
        crate::tunnel::forward_local(state, r, None).await,
        &response_key,
        &aad,
    )
}

#[tokio::test]
async fn encrypted_streams_require_the_starting_principal_and_valid_scoped_ids() {
    use crate::secure_transport::StreamPrincipal;
    let (_dir, state, owner, mut g, _r, key, p, grantee_account) = fixture_with_account().await;
    let owner_key = crate::owner_trust::tests::session_key();
    g.relay_ref.kind = "grant".into();
    g.scope = "device".into();
    g.workspace_scope = "all".into();
    g.workspace_access = "read".into();
    put(&state.db, &g.grant_id, &g, &owner, owner_trust::now()).unwrap();
    // Generic owner-only streams must not be accessible through invented thread
    // or workspace paths, even when the relay exposes the continuation ID.
    let transport = crate::secure_transport::transport(&state).unwrap();
    let (_, next) = transport
        .streams
        .begin(
            "/api/threads/import-candidates",
            axum::body::Body::from(vec![21u8; 1024 * 1024 + 7]),
            StreamPrincipal::new(Some(&owner), None),
        )
        .await
        .unwrap();
    let next = next.unwrap();
    for prefix in ["/api/threads/zzz", "/api/workspaces/zzz"] {
        let (meta, _) = download_request(&state, &p, &key, &next.replacen("/api", prefix, 1)).await;
        assert_eq!(meta["status"], 410);
    }
    let (meta, bytes) = download_request(&state, &owner, &owner_key, &next).await;
    assert_eq!(meta["status"], 200);
    assert_eq!(bytes, vec![21u8; 7]);

    let ws = state
        .create_workspace(pockymoe_protocol::CreateWorkspaceInput {
            abs_path: Some(state.config.workspace_root.to_string_lossy().into()),
            git_url: None,
            label: Some("Streaming ACL".into()),
        })
        .unwrap();
    std::fs::write(
        state.config.workspace_root.join("large.bin"),
        vec![42u8; 1024 * 1024 + 7],
    )
    .unwrap();
    let path = format!("/api/workspaces/{}/files/download?path=large.bin", ws.id);
    let (meta, bytes) = download_request(&state, &owner, &owner_key, &path).await;
    assert_eq!(meta["status"], 200);
    assert_eq!(bytes.len(), 1024 * 1024);
    let next = meta["streamNext"].as_str().unwrap();
    assert_eq!(
        download_request(&state, &p, &key, next).await.0["status"],
        410
    );
    let (meta, bytes) = download_request(&state, &owner, &owner_key, next).await;
    assert_eq!(meta["status"], 200);
    assert_eq!(bytes, vec![42u8; 7]);
    // Another browser of the same grantee account is a different principal. Only
    // the browser that started a download can finish it.
    let other_key = SigningKey::random(&mut OsRng);
    let other_cert = crate::owner_trust::tests::browser_certificate(&grantee_account, &other_key);
    let (status, body) = register(&state, &other_cert).await;
    assert_eq!((status, body["role"].as_str()), (200, Some("grantee")));
    let other = BrowserPrincipal {
        browser_id: other_cert.browser_id.clone(),
        ..p.clone()
    };
    assert_ne!(other.browser_id, p.browser_id);
    assert!(other.valid(&state));
    let (meta, _) = download_request(&state, &p, &key, &path).await;
    assert_eq!(meta["status"], 200);
    let next = meta["streamNext"].as_str().unwrap();
    assert_eq!(
        download_request(&state, &other, &other_key, next).await.0["status"],
        410
    );
    assert_eq!(
        download_request(&state, &owner, &owner_key, next).await.0["status"],
        410
    );
    let (meta, bytes) = download_request(&state, &p, &key, next).await;
    assert_eq!(meta["status"], 200);
    assert_eq!(bytes, vec![42u8; 7]);
}

#[tokio::test]
async fn grantee_browser_revocations_are_scoped_and_grantee_registrations_are_capped() {
    use p256::ecdsa::{signature::Signer, Signature};
    let (_dir, state, owner, _grant, certificate, _key, principal, account) =
        fixture_with_account().await;
    let mut revocation = crate::owner_trust::tests::browser_revocation(&principal);
    let sig: Signature =
        account.sign(owner_trust::revocation_signing_input(&revocation).as_bytes());
    revocation.signature = B64.encode(sig.to_bytes());
    let mut forged_owner = revocation.clone();
    forged_owner.subject_id = owner.browser_id.clone();
    let sig: Signature =
        account.sign(owner_trust::revocation_signing_input(&forged_owner).as_bytes());
    forged_owner.signature = B64.encode(sig.to_bytes());
    let mut forged_device = revocation.clone();
    forged_device.kind = "device".into();
    forged_device.subject_id = "test-device".into();
    let sig: Signature =
        account.sign(owner_trust::revocation_signing_input(&forged_device).as_bytes());
    forged_device.signature = B64.encode(sig.to_bytes());
    owner_trust::apply_revocations(&state, &[forged_owner, forged_device, revocation]).unwrap();
    assert!(owner.valid(&state));
    assert!(!owner_trust::device_revoked(&state, "test-device").unwrap());
    assert!(!principal.valid(&state));
    assert_eq!(
        register(&state, &certificate).await.1["code"],
        "browser_revoked"
    );
    // A tombstone for a not-yet-registered browser must also survive.
    let fresh =
        crate::owner_trust::tests::browser_certificate(&account, &SigningKey::random(&mut OsRng));
    let mut r = crate::owner_trust::tests::browser_revocation(&principal);
    r.subject_id = fresh.browser_id.clone();
    let sig: Signature = account.sign(owner_trust::revocation_signing_input(&r).as_bytes());
    r.signature = B64.encode(sig.to_bytes());
    owner_trust::apply_revocations(&state, &[r]).unwrap();
    assert_eq!(register(&state, &fresh).await.1["code"], "browser_revoked");
    let mut first = None;
    for _ in 0..32 {
        let c = crate::owner_trust::tests::browser_certificate(
            &account,
            &SigningKey::random(&mut OsRng),
        );
        assert_eq!(register(&state, &c).await.0, 200);
        first.get_or_insert(c);
    }
    assert_eq!(register(&state, first.as_ref().unwrap()).await.0, 200);
    let overflow =
        crate::owner_trust::tests::browser_certificate(&account, &SigningKey::random(&mut OsRng));
    assert_eq!(
        register(&state, &overflow).await,
        (429, json!({"code":"browser_limit"}))
    );
}

#[tokio::test]
async fn grantee_browser_cap_allows_idempotent_registration_at_limit() {
    let (_dir, state, owner, _grant, cert, _key, _principal, account) =
        fixture_with_account().await;
    for _ in 1..32 {
        let c = crate::owner_trust::tests::browser_certificate(
            &account,
            &SigningKey::random(&mut OsRng),
        );
        assert_eq!(register(&state, &c).await.0, 200);
    }
    assert_eq!(register(&state, &cert).await.0, 200);
    let c =
        crate::owner_trust::tests::browser_certificate(&account, &SigningKey::random(&mut OsRng));
    assert_eq!(
        register(&state, &c).await,
        (429, json!({"code":"browser_limit"}))
    );
    assert!(owner.valid(&state));
}
