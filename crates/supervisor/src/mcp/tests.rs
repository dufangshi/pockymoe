use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

pub(super) async fn req(
    s: &Arc<Supervisor>,
    method: &str,
    path: &str,
    body: Value,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Value) {
    let app = crate::http::router(s.clone());
    let mut b = Request::builder().method(method).uri(path);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let r = app
        .oneshot(
            b.body(if body.is_null() {
                Body::empty()
            } else {
                Body::from(body.to_string())
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let status = r.status();
    let headers = r.headers().clone();
    let bytes = to_bytes(r.into_body(), 2 * 1024 * 1024).await.unwrap();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
pub(super) async fn form(
    s: &Arc<Supervisor>,
    path: &str,
    values: &[(&str, &str)],
) -> (StatusCode, Value) {
    let data = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(values.iter().copied())
        .finish();
    let r = crate::http::router(s.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(data))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = r.status();
    let bytes = to_bytes(r.into_body(), 2 * 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[tokio::test]
async fn mcp_oauth_pkce_rotation_reuse_and_audience() {
    let f = crate::isolation_tests::setup().await;
    let s = &f.state;
    let (status, _, meta) = req(
        s,
        "GET",
        "/.well-known/oauth-authorization-server",
        Value::Null,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["code_challenge_methods_supported"], json!(["S256"]));
    let (_, h, _) = req(
        s,
        "POST",
        "/mcp",
        json!({}),
        &[("content-type", "application/json")],
    )
    .await;
    assert!(h.contains_key("www-authenticate"));
    let (status, _, _) = req(
        s,
        "POST",
        "/oauth/mcp/register",
        json!({"redirect_uris":["http://192.168.1.2/cb"]}),
        &[("content-type", "application/json")],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, _, client) = req(
        s,
        "POST",
        "/oauth/mcp/register",
        json!({"client_name":"test","redirect_uris":["http://127.0.0.1:19140/cb"]}),
        &[("content-type", "application/json")],
    )
    .await;
    let client = client["client_id"].as_str().unwrap();
    let verifier = "a".repeat(64);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let resource = oauth::resource(s);
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("client_id", client),
            ("redirect_uri", "http://127.0.0.1:19140/cb"),
            ("response_type", "code"),
            ("code_challenge_method", "S256"),
            ("code_challenge", &challenge),
            ("resource", &resource),
            ("scope", "read supervised full"),
            ("state", "test-state"),
        ])
        .finish();
    let (status, headers, _) = req(
        s,
        "GET",
        &format!("/oauth/mcp/authorize?{query}"),
        Value::Null,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let location = headers["location"].to_str().unwrap();
    let ticket = location.strip_prefix("/?mcpConsent=").unwrap();
    let (_, _, approval) = req(
        s,
        "POST",
        &format!("/api/mcp/consent/{ticket}"),
        json!({"approve":true,"access":"supervised","workspaces":[f.open]}),
        &[("content-type", "application/json")],
    )
    .await;
    let redirect = url::Url::parse(approval["redirect"].as_str().unwrap()).unwrap();
    let params: std::collections::HashMap<_, _> = redirect.query_pairs().into_owned().collect();
    assert_eq!(params["state"], "test-state");
    let code = &params["code"];
    let token_args = |v| {
        vec![
            ("grant_type", "authorization_code"),
            ("client_id", client),
            ("redirect_uri", "http://127.0.0.1:19140/cb"),
            ("code", code.as_str()),
            ("resource", resource.as_str()),
            ("code_verifier", v),
        ]
    };
    let wrong_verifier = "b".repeat(64);
    assert_eq!(
        form(s, "/oauth/mcp/token", &token_args(&wrong_verifier))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut wrong_resource = token_args(&verifier);
    wrong_resource.retain(|(k, _)| *k != "resource");
    wrong_resource.push(("resource", "https://wrong.example/mcp"));
    assert_eq!(
        form(s, "/oauth/mcp/token", &wrong_resource).await.0,
        StatusCode::BAD_REQUEST
    );
    let (status, tokens) = form(s, "/oauth/mcp/token", &token_args(&verifier)).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    let bearer = format!("Bearer {}", tokens["access_token"].as_str().unwrap());
    let headers = [
        ("authorization", bearer.as_str()),
        ("content-type", "application/json"),
    ];
    assert_eq!(
        req(s, "GET", "/api/workspaces", Value::Null, &headers)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status,head,init)=req(s,"POST","/mcp",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),&headers).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    let session = head["mcp-session-id"].to_str().unwrap();
    let headers = [
        ("authorization", bearer.as_str()),
        ("content-type", "application/json"),
        ("mcp-session-id", session),
        ("mcp-protocol-version", "2025-06-18"),
    ];
    let (_,_,result)=req(s,"POST","/mcp",json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"pockymoe_workspaces"}}),&headers).await;
    assert_eq!(
        result["result"]["structuredContent"]["workspaces"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        req(
            s,
            "POST",
            "/mcp",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            &headers
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let mut bad = headers.to_vec();
    bad.push(("origin", "https://evil.example"));
    assert_eq!(
        req(s, "POST", "/mcp", json!({}), &bad).await.0,
        StatusCode::FORBIDDEN
    );
    let refresh = tokens["refresh_token"].as_str().unwrap();
    let args = [
        ("grant_type", "refresh_token"),
        ("client_id", client),
        ("resource", resource.as_str()),
        ("refresh_token", refresh),
    ];
    let (status, new) = form(s, "/oauth/mcp/token", &args).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(new["refresh_token"], tokens["refresh_token"]);
    assert_eq!(
        form(s, "/oauth/mcp/token", &args).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        req(s, "POST", "/mcp", json!({}), &headers).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(!store::transaction(s, |db| store::list(db, "audit"))
        .unwrap()
        .is_empty());
}
#[tokio::test]
async fn mcp_tools_and_grants_fail_closed() {
    let f = crate::isolation_tests::setup().await;
    let grant = Grant {
        device: None,
        client: "test".into(),
        access: "read".into(),
        workspaces: Some(vec![f.open.clone()]),
    };
    let catalog = tools::catalog();
    for tool in catalog["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        let read = tool["annotations"]["readOnlyHint"] == true;
        assert_eq!(
            grant.check(name, Some(&f.open), Some("guarded")).is_ok(),
            read,
            "{name}"
        );
        assert!(
            grant
                .check(name, Some(&f.isolated), Some("guarded"))
                .is_err(),
            "{name}"
        );
    }
    assert!(grant.check("unknown", None, None).is_err());
    let supervised = Grant {
        access: "supervised".into(),
        ..grant.clone()
    };
    for tool in [
        "pockymoe_thread_send",
        "pockymoe_thread_stop",
        "pockymoe_thread_launch",
    ] {
        assert!(supervised.check(tool, Some(&f.open), Some("yolo")).is_err());
        assert!(supervised
            .check(tool, Some(&f.open), Some("guarded"))
            .is_ok());
    }
    let result=rpc(&f.state,&grant,json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"pockymoe_thread_launch","arguments":{"workspaceId":f.open}}})).await.unwrap();
    assert_eq!(result["result"]["isError"], true);
    let result = rpc(
        &f.state,
        &grant,
        json!({"jsonrpc":"2.0","id":2,"method":"bogus"}),
    )
    .await
    .unwrap();
    assert_eq!(result["error"]["code"], -32601);
    assert!(rpc(
        &f.state,
        &grant,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .await
    .is_none());
    assert!(local(&f.state, &json!({}), Some("agent")).await.is_err());
}
#[test]
fn mcp_url_rules() {
    for u in [
        "https://example.org/callback",
        "http://127.0.0.1:123/cb",
        "http://localhost/cb",
        "http://[::1]/cb",
    ] {
        assert!(oauth::safe_url(u), "{u}");
    }
    for u in [
        "http://example.org",
        "http://127.1/cb#x",
        "https://user:password@example.org",
        "file:///tmp/cb",
        "http://192.168.1.1",
    ] {
        assert!(!oauth::safe_url(u), "{u}");
    }
}

#[tokio::test]
async fn mcp_deny_expiry_revocation_sessions_and_cancellation() {
    let f = crate::isolation_tests::setup().await;
    let s = &f.state;
    let thread = crate::isolation_tests::thread(s, &f.open).await;
    s.db.with(|db| {
        db.execute("UPDATE threads SET status='running' WHERE id=?", [&thread])?;
        Ok(())
    })
    .unwrap();
    let token = store::secret();
    let session = store::secret();
    let resource = oauth::resource(s);
    store::transaction(s,|db|{
        store::put(db,"grant","g",&json!({"id":"g","clientId":"test","access":"read","workspaces":[f.open],"createdAt":store::now()}),1000)?;
        store::put(db,"access",&store::hash(&token),&json!({"grant":"g","resource":resource,"expiresAt":store::now()+900}),900)?;
        store::put(db,"session",&session,&json!({"grant":"g"}),900)
    }).unwrap();
    let bearer = format!("Bearer {token}");
    let headers = [
        ("authorization", bearer.as_str()),
        ("content-type", "application/json"),
        ("mcp-session-id", session.as_str()),
        ("mcp-protocol-version", "2025-06-18"),
    ];
    let other = [
        ("authorization", bearer.as_str()),
        ("content-type", "application/json"),
        ("mcp-session-id", "wrong"),
        ("mcp-protocol-version", "2025-06-18"),
    ];
    assert_eq!(
        req(
            s,
            "POST",
            "/mcp",
            json!({"jsonrpc":"2.0","id":1,"method":"ping"}),
            &other
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let state = s.clone();
    let b = bearer.clone();
    let sess = session.clone();
    let waiting = tokio::spawn(async move {
        req(&state,"POST","/mcp",json!({"jsonrpc":"2.0","id":88,"method":"tools/call","params":{"name":"pockymoe_thread_wait","arguments":{"threadId":thread,"timeoutSeconds":600}}}),&[("authorization",&b),("content-type","application/json"),("mcp-session-id",&sess),("mcp-protocol-version","2025-06-18")]).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        req(
            s,
            "POST",
            "/mcp",
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":88}}),
            &headers
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let (_, _, reply) = tokio::time::timeout(std::time::Duration::from_secs(2), waiting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply["error"]["code"], -32800, "{reply}");
    assert_eq!(
        req(s, "DELETE", "/mcp", Value::Null, &headers).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        req(
            s,
            "POST",
            "/mcp",
            json!({"jsonrpc":"2.0","id":1,"method":"ping"}),
            &headers
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    store::transaction(s, |db| {
        store::put(
            db,
            "access",
            &store::hash(&token),
            &json!({"grant":"g","resource":resource,"expiresAt":store::now()-1}),
            900,
        )
    })
    .unwrap();
    assert!(oauth::authenticate(
        s,
        &HeaderMap::from_iter([("authorization".parse().unwrap(), bearer.parse().unwrap())])
    )
    .is_err());
    store::transaction(s, |db| {
        store::put(
            db,
            "consent",
            "deny",
            &json!({"query":{"redirect_uri":"http://127.0.0.1/cb","state":"s"}}),
            60,
        )
    })
    .unwrap();
    let (status, _, denied) = req(
        s,
        "POST",
        "/api/mcp/consent/deny",
        json!({"approve":false}),
        &[("content-type", "application/json")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(denied["redirect"]
        .as_str()
        .unwrap()
        .contains("error=access_denied"));
    assert!(store::transaction(s, |db| store::get(db, "consent", "deny")).is_err());
}

#[test]
fn mcp_acl_is_an_exclusive_audience_and_management_is_owner_only() {
    use pockymoe_protocol::access::{access_allows, owner_access};
    let mut mcp = owner_access();
    mcp.kind = "mcp";
    assert!(access_allows(&mcp, "POST", "/mcp"));
    for path in [
        "/api/threads",
        "/api/cli",
        "/ws",
        "/api/mcp/clients",
        "/api/mcp/consent/abc",
        "/oauth/mcp/token",
    ] {
        assert!(!access_allows(&mcp, "GET", path), "{path}");
    }
    let mut shared = owner_access();
    shared.kind = "grant";
    shared.scope = "device".into();
    for path in [
        "/mcp",
        "/api/mcp/clients",
        "/api/mcp/clients/g",
        "/api/mcp/consent/abc",
    ] {
        for method in ["GET", "POST", "DELETE"] {
            assert!(!access_allows(&shared, method, path), "{method} {path}");
        }
    }
}

#[tokio::test]
async fn mcp_launch_wait_reads_the_completed_queued_prompt() {
    let f = crate::isolation_tests::setup().await;
    // This fixture enables Codex rather than ACP; create through the fixture and
    // exercise the MCP send/wait/read boundary against the deterministic runtime.
    let id = crate::isolation_tests::thread(&f.state, &f.open).await;
    let grant = Grant {
        client: "lab".into(),
        access: "supervised".into(),
        workspaces: Some(vec![f.open]),
        device: None,
    };
    tools::call(
        &f.state,
        &grant,
        "pockymoe_thread_send",
        &json!({"threadId":id,"text":"hello","delivery":"queue"}),
    )
    .await
    .unwrap();
    let reply = tools::call(
        &f.state,
        &grant,
        "pockymoe_thread_wait",
        &json!({"threadId":id,"timeoutSeconds":10}),
    )
    .await
    .unwrap();
    assert_eq!(reply["status"]["queuedCount"], 0);
    assert!(
        !reply["thread"]["turns"].as_array().unwrap().is_empty(),
        "{reply}"
    );
    assert_eq!(reply["status"]["outcome"]["kind"], "completed");
    let transcript = tools::call(
        &f.state,
        &grant,
        "pockymoe_thread_read",
        &json!({"threadId":id,"view":"summary"}),
    )
    .await
    .unwrap();
    assert!(transcript.get("transcript").is_some());
}

#[tokio::test]
async fn mcp_revoke_and_event_allowlist_are_enforced() {
    let f = crate::isolation_tests::setup().await;
    let allowed = crate::isolation_tests::thread(&f.state, &f.open).await;
    let denied = crate::isolation_tests::thread(&f.state, &f.isolated).await;
    let token = store::secret();
    store::transaction(&f.state,|db|{
        store::put(db,"grant","g",&json!({"createdAt":store::now(),"clientId":"client","access":"full","workspaces":null}),900)?;
        store::put(db,"access",&store::hash(&token),&json!({"grant":"g","clientId":"client","resource":oauth::resource(&f.state),"expiresAt":store::now()+900}),900)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS milestones(id INTEGER PRIMARY KEY AUTOINCREMENT,thread TEXT NOT NULL,value TEXT NOT NULL)")?;
        for thread in [&allowed,&denied] {db.execute("INSERT INTO milestones(thread,value) VALUES(?,?)",rusqlite::params![thread,json!({"threadId":thread,"type":"thread.turn.completed"}).to_string()])?;}Ok(())
    }).unwrap();
    let grant = Grant {
        client: "client".into(),
        access: "read".into(),
        workspaces: Some(vec![f.open]),
        device: None,
    };
    let events = events::poll(&f.state, &grant, "").unwrap();
    assert_eq!(events["events"].as_array().unwrap().len(), 1);
    assert_eq!(events["events"][0]["threadId"], allowed);
    assert!(
        events::poll(&f.state, &grant, events["cursor"].as_str().unwrap()).unwrap()["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        form(
            &f.state,
            "/oauth/mcp/revoke",
            &[("token", &token), ("client_id", "client")]
        )
        .await
        .0,
        StatusCode::OK
    );
    let bearer = format!("Bearer {token}");
    let headers =
        HeaderMap::from_iter([("authorization".parse().unwrap(), bearer.parse().unwrap())]);
    assert!(oauth::authenticate(&f.state, &headers).is_err());
}
