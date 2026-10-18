use super::*;
use crate::{
    grants::tests::request,
    owner_trust::tests::{bound_peer, browser_revocation, live_owner_for_device, session_key},
    peer_link,
    tunnel::{forward_local, tests::state_with_relay_url},
};
use axum::{body::Body, http::Request};
use tower::ServiceExt;

fn add(s: &Supervisor) -> saved::Connection {
    let c:saved::Connection=serde_json::from_value(json!({"name":"Shared test","protocol":"openai_chat","baseUrl":"https://example.invalid/v1","apiKey":"fixture-private-key","model":"test-model","headers":{"x-project":"private-project-header"}})).unwrap();
    let v = saved::upsert(&dir(s), c).unwrap();
    saved::get(&dir(s), v["id"].as_str().unwrap()).unwrap()
}
fn protected() -> (tempfile::TempDir, Arc<Supervisor>, BrowserPrincipal, String) {
    let (temp, s) = state_with_relay_url("http://127.0.0.1:1");
    let roots = upstreams::native::HARNESSES
        .into_iter()
        .map(|h| (h.into(), temp.path().join(h)))
        .collect();
    let s = Arc::new(
        Arc::try_unwrap(s)
            .unwrap_or_else(|_| panic!("fixture unexpectedly shared"))
            .with_upstream_homes(roots),
    );
    let id = uuid::Uuid::new_v4().to_string();
    let p = live_owner_for_device(&s, &id);
    bound_peer(&s, &id);
    s.set_peer_access(true).unwrap();
    (temp, s, p, id)
}
async fn grant(
    s: &Arc<Supervisor>,
    p: &BrowserPrincipal,
    c: &saved::Connection,
    target: &str,
) -> Value {
    let (status, v) = request(
        s,
        Some(p),
        &session_key(),
        "POST",
        &format!(
            "/api/management/connections/{}/transfer-authorizations",
            c.id
        ),
        json!({"targetDeviceId":target,"revision":c.revision}),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    v
}
#[tokio::test]
async fn saved_connections_are_owner_only_redacted_and_never_apply() {
    let (_temp, s, p, _id) = protected();
    let c = add(&s);
    let (status, v) = request(
        &s,
        Some(&p),
        &session_key(),
        "GET",
        "/api/management/connections",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    assert!(!v.to_string().contains(&c.api_key));
    assert_eq!(v["bindings"], json!({}));
    let mut update = json!({"id":c.id,"name":"Edited","protocol":c.protocol,"baseUrl":c.base_url,"model":"new-model","revision":c.revision,"apiKey":""});
    let (status, _) = request(
        &s,
        Some(&p),
        &session_key(),
        "POST",
        "/api/management/connections",
        update.clone(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        saved::get(&dir(&s), &c.id).unwrap().headers["x-project"],
        "private-project-header"
    );
    assert!(upstreams::active_profile(&dir(&s), "codex")
        .unwrap()
        .is_none());
    update["name"] = json!("Stale edit");
    let (status, v) = request(
        &s,
        Some(&p),
        &session_key(),
        "POST",
        "/api/management/connections",
        update,
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(v["code"], "edit_conflict");
    let (_temp, s, _owner, _grant, _cert, key, grantee) = crate::grants::tests::fixture().await;
    for (method, path, body) in [
        (
            "GET",
            "/api/management/connections".to_string(),
            Value::Null,
        ),
        (
            "POST",
            format!(
                "/api/management/connections/{}/transfer-authorizations",
                c.id
            ),
            json!({"targetDeviceId":uuid::Uuid::new_v4().to_string(),"revision":c.revision}),
        ),
    ] {
        let (status, _) = request(&s, Some(&grantee), &key, method, &path, body).await;
        assert_eq!(status, 403);
    }
}
#[tokio::test]
async fn authorization_requires_encrypted_owner_and_exact_version() {
    let (_temp, s, p, _id) = protected();
    let c = add(&s);
    let target = uuid::Uuid::new_v4().to_string();
    let response = crate::http::router(s.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/management/connections/{}/transfer-authorizations",
                    c.id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"targetDeviceId":target,"revision":c.revision}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!response.status().is_success());
    let (status, v) = request(
        &s,
        Some(&p),
        &session_key(),
        "POST",
        &format!(
            "/api/management/connections/{}/transfer-authorizations",
            c.id
        ),
        json!({"targetDeviceId":target,"revision":"stale"}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(v["code"], "revision_mismatch");
    let v = grant(&s, &p, &c, &target).await;
    assert!(!v.to_string().contains(&c.api_key));
    let input = Pull {
        token: v["token"].as_str().unwrap().into(),
        revision: c.revision.clone(),
    };
    assert!(consume_grant(&s, "wrong-target", &c.id, &c.revision, &input).is_err());
    assert!(consume_grant(&s, &target, "wrong-connection", &c.revision, &input).is_err());
    consume_grant(&s, &target, &c.id, &c.revision, &input).unwrap();
    assert!(consume_grant(&s, &target, &c.id, &c.revision, &input).is_err());
}
#[tokio::test]
async fn revoked_browser_and_expired_capabilities_cannot_export() {
    let (_temp, s, p, _id) = protected();
    let c = add(&s);
    let target = uuid::Uuid::new_v4().to_string();
    let v = grant(&s, &p, &c, &target).await;
    let input = Pull {
        token: v["token"].as_str().unwrap().into(),
        revision: c.revision.clone(),
    };
    let key = format!("upstream-transfer:{}", owner_trust::hash(&input.token));
    let mut g: Value = serde_json::from_str(&s.db.get_kv(&key).unwrap().unwrap()).unwrap();
    g["expiresAt"] = json!(owner_trust::now() - 1);
    s.db.set_kv(&key, &g.to_string()).unwrap();
    assert!(consume_grant(&s, &target, &c.id, &c.revision, &input).is_err());
    let v = grant(&s, &p, &c, &target).await;
    let input = Pull {
        token: v["token"].as_str().unwrap().into(),
        revision: c.revision.clone(),
    };
    let (status, _) = request(
        &s,
        Some(&p),
        &session_key(),
        "PUT",
        "/api/transport/revocations",
        json!({"revocations":[browser_revocation(&p)]}),
    )
    .await;
    assert_eq!(status, 200);
    assert!(consume_grant(&s, &target, &c.id, &c.revision, &input).is_err());
}
#[tokio::test]
async fn certified_peer_sync_is_encrypted_and_does_not_apply_harness() {
    let (_a, source, owner, source_id) = protected();
    let (_b, target, target_owner, target_id) = protected();
    let c = add(&source);
    let authorization = grant(&source, &owner, &c, &target_id).await;
    let (connection, mut out) = peer_link::tests::connected(&target, &target_id);
    let source_for_task = source.clone();
    let peer_id = target_id.clone();
    let expected_source = source_id.clone();
    let key = c.api_key.clone();
    let token = authorization["token"].as_str().unwrap().to_string();
    let expected_token = token.clone();
    let bridge = tokio::spawn(async move {
        while let Some(frame) = out.recv().await {
            if frame["type"] != "peer.request" {
                continue;
            }
            assert_eq!(frame["targetDeviceId"], expected_source);
            assert!(!frame.to_string().contains(&key));
            assert!(!frame.to_string().contains(&expected_token));
            let response = forward_local(
                &source_for_task,
                frame["payload"].clone(),
                Some(PeerCaller {
                    device_id: peer_id.clone(),
                    device_name: "Target".into(),
                    user_id: "same-owner".into(),
                    shared_control: None,
                }),
            )
            .await;
            assert!(!response.to_string().contains(&key));
            connection.receive(
                &json!({"type":"peer.response","requestId":frame["requestId"],"payload":response}),
            );
        }
    });
    let path = format!("/api/management/connections/{}/sync", c.id);
    let body = json!({"sourceDeviceId":source_id,"revision":c.revision,"token":token});
    let (status, v) = request(
        &target,
        Some(&target_owner),
        &session_key(),
        "POST",
        &path,
        body.clone(),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["local"], true);
    assert!(!v.to_string().contains(&c.api_key));
    assert_eq!(saved::get(&dir(&target), &c.id).unwrap().api_key, c.api_key);
    assert!(upstreams::inventory(&dir(&target)).unwrap()["profiles"]
        .as_array()
        .unwrap()
        .is_empty());
    let (status, v) = request(
        &target,
        Some(&target_owner),
        &session_key(),
        "POST",
        &path,
        body,
    )
    .await;
    assert_eq!(status, 403);
    assert!(v["message"]
        .as_str()
        .unwrap()
        .contains("invalid or expired"));
    bridge.abort();
    let _ = bridge.await;
}
#[tokio::test]
async fn busy_harness_does_not_discard_saved_connection_and_plugin_references_block_delete() {
    let (_temp, s, p, _id) = protected();
    let mut c = add(&s);
    c.protocol = "openai_responses".into();
    let v = saved::upsert(&dir(&s), c.clone()).unwrap();
    c = saved::get(&dir(&s), v["id"].as_str().unwrap()).unwrap();
    let guard = s.harness_gate("codex").read_owned().await;
    let (status, v) = request(
        &s,
        Some(&p),
        &session_key(),
        "POST",
        &format!("/api/management/connections/{}/apply", c.id),
        json!({"harness":"codex"}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(v["code"], "harness_busy");
    assert_eq!(saved::get(&dir(&s), &c.id).unwrap().revision, c.revision);
    drop(guard);
    let plugin = s.plugin_settings("pockymoe.content-quality").unwrap();
    let mut values = plugin.values;
    values["connectionId"] = json!(c.id);
    s.save_plugin_settings(
        "pockymoe.content-quality",
        pockymoe_runtime::plugins::PluginSettings {
            revision: plugin.revision,
            values,
        },
    )
    .unwrap();
    let (status, v) = request(
        &s,
        Some(&p),
        &session_key(),
        "DELETE",
        &format!("/api/management/connections/{}", c.id),
        Value::Null,
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(v["code"], "connection_in_use");
}
#[test]
fn catalog_merges_replicas_without_conflating_name_or_versions() {
    let id = uuid::Uuid::new_v4().to_string();
    let mut v = json!({"connections":[{"id":id,"name":"Local","protocol":"openai_chat","revision":"local-version","local":true,"sources":[]}]});
    merge_catalog(
        &mut v,
        vec![
            json!({"id":id,"name":"Remote","protocol":"openai_chat","revision":"remote-version","deviceId":"A","deviceName":"Device A","online":true}),
            json!({"id":id,"deviceId":"B","revision":"stale-self"}),
        ],
        Some("B"),
    );
    assert_eq!(v["connections"].as_array().unwrap().len(), 1);
    assert_eq!(v["connections"][0]["name"], "Local");
    assert_eq!(v["connections"][0]["conflict"], true);
    assert_eq!(v["connections"][0]["sources"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn native_upstream_owner_api_restores_local_oauth_clears_binding_and_checks_revision_and_busy_gates(
) {
    let (_temp, s, p, _id) = protected();
    let root = s.upstream_home("claude");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("settings.json"),
        json!({"hooks":{"fixture":"keep"}}).to_string(),
    )
    .unwrap();
    std::fs::write(root.join(".credentials.json"),json!({"claudeAiOauth":{"refreshToken":"fixture-refresh","accessToken":"fixture-access","expiresAt":1}}).to_string()).unwrap();
    let (status, list) = request(
        &s,
        Some(&p),
        &session_key(),
        "GET",
        "/api/management/connections",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(list["native"]["claude"]["kind"], "oauth");
    assert_eq!(list["native"]["claude"]["active"], true);
    assert!(!list.to_string().contains("fixture-refresh"));
    assert!(!list.to_string().contains("fixture-access"));
    assert!(!snapshot(&s).to_string().contains("native"));
    let c:saved::Connection=serde_json::from_value(json!({"name":"fixture API","protocol":"anthropic","baseUrl":"https://fixture.invalid","apiKey":"fixture-api-key","model":"fixture-model"})).unwrap();
    let c = saved::upsert(&dir(&s), c).unwrap();
    let path = format!(
        "/api/management/connections/{}/apply",
        c["id"].as_str().unwrap()
    );
    let (status, _) = request(
        &s,
        Some(&p),
        &session_key(),
        "POST",
        &path,
        json!({"harness":"claude"}),
    )
    .await;
    assert_eq!(status, 200);
    let path = "/api/management/upstreams/native/claude/activate";
    let (status, body) = request(
        &s,
        Some(&p),
        &session_key(),
        "POST",
        path,
        json!({"revision":list["native"]["claude"]["revision"]}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(body["code"], "edit_conflict");
    let busy = s.harness_gate("claude").read_owned().await;
    let (status, body) = request(&s, Some(&p), &session_key(), "POST", path, json!({})).await;
    assert_eq!(status, 409);
    assert_eq!(body["code"], "harness_busy");
    drop(busy);
    let maintenance = s.maintenance_gate.clone().write_owned().await;
    let (status, body) = request(&s, Some(&p), &session_key(), "POST", path, json!({})).await;
    assert_eq!(status, 409);
    assert_eq!(body["code"], "harness_busy");
    drop(maintenance);
    let (status, list) = request(
        &s,
        Some(&p),
        &session_key(),
        "GET",
        "/api/management/connections",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(list["native"]["claude"]["active"], false);
    let (status, result) = request(
        &s,
        Some(&p),
        &session_key(),
        "POST",
        path,
        json!({"revision":list["native"]["claude"]["revision"]}),
    )
    .await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["native"], true);
    assert_eq!(result["restartRequired"], false);
    assert_eq!(result["restartedSessions"], 0);
    assert!(saved::binding(&dir(&s), "claude").unwrap().is_none());
    assert!(upstreams::active_profile(&dir(&s), "claude")
        .unwrap()
        .is_none());
    let d: Value =
        serde_json::from_slice(&std::fs::read(root.join("settings.json")).unwrap()).unwrap();
    assert_eq!(d["hooks"]["fixture"], "keep");
    assert!(d["env"].get("ANTHROPIC_API_KEY").is_none());
    let (status, body) = request(
        &s,
        Some(&p),
        &session_key(),
        "POST",
        "/api/management/upstreams/native/gemini/activate",
        json!({}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["code"], "connection_error");
    let (status, _) = request(
        &s,
        Some(&p),
        &session_key(),
        "POST",
        "/api/management/upstreams/native/unknown/activate",
        json!({}),
    )
    .await;
    assert_eq!(status, 400);
}
#[tokio::test]
async fn native_upstream_management_rejects_shared_grantee_http_requests() {
    let (_temp, s, _owner, _grant, _cert, key, grantee) = crate::grants::tests::fixture().await;
    for (method, path, body) in [
        ("GET", "/api/management/connections", Value::Null),
        (
            "POST",
            "/api/management/upstreams/native/claude/activate",
            json!({}),
        ),
    ] {
        let (status, _) = request(&s, Some(&grantee), &key, method, path, body).await;
        assert_eq!(status, 403);
    }
}
