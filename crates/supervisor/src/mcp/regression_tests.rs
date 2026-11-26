//! Defects found by running real MCP clients against a lab Supervisor.
use super::tests::{form, req};
use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};

const REDIRECT: &str = "http://127.0.0.1:19199/cb";
const JSON: (&str, &str) = ("content-type", "application/json");

async fn client(s: &Arc<Supervisor>) -> String {
    let (status, _, client) = req(
        s,
        "POST",
        "/oauth/mcp/register",
        json!({"client_name":"lab","redirect_uris":[REDIRECT]}),
        &[JSON],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{client}");
    client["client_id"].as_str().unwrap().to_owned()
}

fn authorize_query(client: &str, extra: &[(&str, &str)]) -> (String, String) {
    let verifier = "v".repeat(64);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut pairs = vec![
        ("client_id", client),
        ("redirect_uri", REDIRECT),
        ("response_type", "code"),
        ("code_challenge_method", "S256"),
        ("code_challenge", challenge.as_str()),
        ("state", "st"),
    ];
    for (k, v) in extra {
        pairs.retain(|(key, _)| key != k);
        pairs.push((k, v));
    }
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    (query, verifier)
}

/// Authorize without `resource` (RFC 8707 makes it optional), consent as the
/// owner, and return the code.
async fn code(s: &Arc<Supervisor>, client: &str, scope: &str) -> (String, String) {
    let (query, verifier) = authorize_query(client, &[("scope", scope)]);
    let (status, headers, body) = req(
        s,
        "GET",
        &format!("/oauth/mcp/authorize?{query}"),
        Value::Null,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let ticket = headers["location"]
        .to_str()
        .unwrap()
        .strip_prefix("/?mcpConsent=")
        .unwrap()
        .to_owned();
    let (_, _, consent) = req(
        s,
        "GET",
        &format!("/api/mcp/consent/{ticket}"),
        Value::Null,
        &[],
    )
    .await;
    let (_, _, approval) = req(
        s,
        "POST",
        &format!("/api/mcp/consent/{ticket}"),
        json!({"approve":true,"access":consent["requestedAccess"]}),
        &[JSON],
    )
    .await;
    let redirect = url::Url::parse(approval["redirect"].as_str().unwrap()).unwrap();
    let code = redirect
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    (code, verifier)
}

fn exchange<'a>(client: &'a str, code: &'a str, verifier: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("grant_type", "authorization_code"),
        ("client_id", client),
        ("redirect_uri", REDIRECT),
        ("code", code),
        ("code_verifier", verifier),
    ]
}

#[tokio::test]
async fn mcp_rate_limits_count_failed_attempts() {
    let f = crate::isolation_tests::setup().await;
    // Every attempt is a guess; failures used to roll their count back. Two
    // limits' worth of attempts may straddle a minute, never more.
    let mut limited = false;
    for _ in 0..=240 {
        let (status, body) = form(
            &f.state,
            "/oauth/mcp/token",
            &[
                ("grant_type", "authorization_code"),
                ("client_id", "x"),
                ("code", "guess"),
            ],
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    assert!(limited, "failed token requests were never limited");
    let mut limited = false;
    for _ in 0..=60 {
        let (status, _, _) = req(
            &f.state,
            "POST",
            "/oauth/mcp/register",
            json!({"redirect_uris":["http://evil.example/cb"]}),
            &[JSON],
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
    }
    assert!(limited, "failed registrations were never limited");
}

#[tokio::test]
async fn mcp_oauth_errors_codes_reuse_and_optional_resource() {
    let f = crate::isolation_tests::setup().await;
    let s = &f.state;
    let (_, _, body) = req(
        s,
        "GET",
        "/oauth/mcp/authorize?client_id=unknown&redirect_uri=http://127.0.0.1:1/cb",
        Value::Null,
        &[],
    )
    .await;
    assert_eq!(body["error"], "invalid_client");
    let client = client(s).await;
    // Once client and redirect are known, errors go back to the client.
    let (query, _) = authorize_query(&client, &[("response_type", "token")]);
    let (status, headers, _) = req(
        s,
        "GET",
        &format!("/oauth/mcp/authorize?{query}"),
        Value::Null,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let back = headers["location"].to_str().unwrap();
    assert!(back.starts_with(REDIRECT), "{back}");
    assert!(back.contains("error=unsupported_response_type") && back.contains("state=st"));
    let (query, _) = authorize_query(&client, &[("resource", "https://other.example/mcp")]);
    let (_, headers, _) = req(
        s,
        "GET",
        &format!("/oauth/mcp/authorize?{query}"),
        Value::Null,
        &[],
    )
    .await;
    assert!(headers["location"]
        .to_str()
        .unwrap()
        .contains("error=invalid_target"));
    // Unknown scope values are ignored; the highest known one is requested.
    let (code, verifier) = code(s, &client, "openid mcp full").await;
    let (status, tokens) = form(s, "/oauth/mcp/token", &exchange(&client, &code, &verifier)).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    assert_eq!(tokens["scope"], "full");
    // A replayed code fails and revokes the grant it issued.
    let (status, replay) = form(s, "/oauth/mcp/token", &exchange(&client, &code, &verifier)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(replay["error"], "invalid_grant");
    let lower = format!("bearer {}", tokens["access_token"].as_str().unwrap());
    let headers =
        HeaderMap::from_iter([("authorization".parse().unwrap(), lower.parse().unwrap())]);
    assert!(
        oauth::authenticate(s, &headers).is_err(),
        "grant must be revoked"
    );
    let (_, unsupported) = form(
        s,
        "/oauth/mcp/token",
        &[("grant_type", "password"), ("client_id", &client)],
    )
    .await;
    assert_eq!(unsupported["error"], "unsupported_grant_type");
    let (_, _, bad_redirect) = req(
        s,
        "POST",
        "/oauth/mcp/register",
        json!({"redirect_uris":["http://evil.example/cb"]}),
        &[JSON],
    )
    .await;
    assert_eq!(bad_redirect["error"], "invalid_redirect_uri");
}

/// A grant with a live access token, without going through consent.
fn token(s: &Arc<Supervisor>, access: &str) -> String {
    let token = store::secret();
    let grant = store::secret();
    store::transaction(s, |db| {
        store::put(db,"grant",&grant,&json!({"id":grant,"clientId":"c","access":access,"workspaces":null,"createdAt":store::now()}),900)?;
        store::put(db,"access",&store::hash(&token),&json!({"grant":grant,"clientId":"c","resource":oauth::resource(s),"expiresAt":store::now()+900}),900)
    })
    .unwrap();
    format!("Bearer {token}")
}

#[tokio::test]
async fn mcp_http_versions_batches_and_rebinding_hosts() {
    let f = crate::isolation_tests::setup().await;
    let s = &f.state;
    let bearer = token(s, "read");
    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}});
    for host in [
        "evil.example",
        "evil.example:19190",
        "127.0.0.1.evil.example",
    ] {
        let (status, _, _) = req(
            s,
            "POST",
            "/mcp",
            init.clone(),
            &[JSON, ("authorization", &bearer), ("host", host)],
        )
        .await;
        assert_eq!(status, StatusCode::MISDIRECTED_REQUEST, "{host}");
        let (status, _, _) = req(
            s,
            "GET",
            "/.well-known/oauth-authorization-server",
            Value::Null,
            &[("host", host)],
        )
        .await;
        assert_eq!(status, StatusCode::MISDIRECTED_REQUEST, "{host}");
    }
    let mut session = String::new();
    for host in [
        "127.0.0.1:19190",
        "localhost:19190",
        "[::1]:19190",
        "192.168.1.5",
    ] {
        let (status, headers, body) = req(
            s,
            "POST",
            "/mcp",
            init.clone(),
            &[JSON, ("authorization", &bearer), ("host", host)],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{host}");
        assert_eq!(body["result"]["protocolVersion"], "2025-03-26");
        session = headers["mcp-session-id"].to_str().unwrap().to_owned();
    }
    // Without MCP-Protocol-Version the session's negotiated version applies.
    let ping = json!({"jsonrpc":"2.0","id":2,"method":"ping"});
    let (status, _, body) = req(
        s,
        "POST",
        "/mcp",
        ping.clone(),
        &[
            JSON,
            ("authorization", &bearer),
            ("mcp-session-id", &session),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, _) = req(
        s,
        "POST",
        "/mcp",
        ping.clone(),
        &[
            JSON,
            ("authorization", &bearer),
            ("mcp-session-id", &session),
            ("mcp-protocol-version", "2024-11-05"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, body) = req(
        s,
        "POST",
        "/mcp",
        json!([ping]),
        &[
            JSON,
            ("authorization", &bearer),
            ("mcp-session-id", &session),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], -32600);
}

#[tokio::test]
async fn mcp_tools_list_matches_the_grant_and_unknown_tools_are_protocol_errors() {
    let f = crate::isolation_tests::setup().await;
    let names = |access: &str| {
        let grant = Grant {
            device: None,
            client: "c".into(),
            access: access.into(),
            workspaces: None,
        };
        let state = f.state.clone();
        async move {
            rpc(
                &state,
                &grant,
                json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            )
            .await
            .unwrap()["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        }
    };
    let read = names("read").await;
    assert!(read.contains(&"pockymoe_thread_wait".into()));
    assert!(!read.contains(&"pockymoe_thread_launch".into()));
    let supervised = names("supervised").await;
    assert!(supervised.contains(&"pockymoe_thread_launch".into()));
    assert!(!supervised.contains(&"pockymoe_thread_answer".into()));
    assert!(!supervised.contains(&"pockymoe_thread_switch".into()));
    assert_eq!(
        names("full").await.len(),
        tools::catalog()["tools"].as_array().unwrap().len()
    );
    let grant = Grant {
        device: None,
        client: "c".into(),
        access: "full".into(),
        workspaces: None,
    };
    let reply = rpc(
        &f.state,
        &grant,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"nope","arguments":{}}}),
    )
    .await
    .unwrap();
    assert_eq!(reply["error"]["code"], -32602);
}

#[tokio::test]
async fn mcp_stdio_workspace_allowlist_limits_lists_and_targets() {
    let f = crate::isolation_tests::setup().await;
    let s = &f.state;
    let call = |name: &str, args: Value, workspaces: Value| json!({"access":"supervised","sessionId":"t","workspaces":workspaces,"request":{"jsonrpc":"2.0","id":name,"method":"tools/call","params":{"name":name,"arguments":args}}});
    let reply = local(
        s,
        &call("pockymoe_workspaces", json!({}), json!([f.open])),
        None,
    )
    .await
    .unwrap();
    let listed = reply["result"]["structuredContent"]["workspaces"]
        .as_array()
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], f.open);
    let reply = local(
        s,
        &call(
            "pockymoe_thread_launch",
            json!({"workspaceId":f.isolated}),
            json!([f.open]),
        ),
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply["result"]["isError"], true);
    assert!(local(
        s,
        &call("pockymoe_workspaces", json!({}), json!(["missing"])),
        None
    )
    .await
    .is_err());
}

#[tokio::test]
async fn mcp_events_carry_turn_and_request_ids_but_no_text() {
    let f = crate::isolation_tests::setup().await;
    let thread = crate::isolation_tests::thread(&f.state, &f.open).await;
    f.state.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
        event_type: pockymoe_protocol::ThreadEventType::ThreadRequestCreated,
        thread_id: thread.clone(),
        timestamp: pockymoe_protocol::now_rfc3339(),
        payload: json!({"request":{"id":"r1","turnId":"t1","kind":"permissionRequest","title":"secret command"}}),
    });
    let grant = Grant {
        device: None,
        client: "c".into(),
        access: "read".into(),
        workspaces: None,
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let events = events::poll(&f.state, &grant, "").unwrap();
        if let Some(e) = events["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "thread.request.created")
        {
            assert_eq!(e["requestId"], "r1");
            assert_eq!(e["turnId"], "t1");
            assert_eq!(e["requestKind"], "permissionRequest");
            assert!(!e.to_string().contains("secret"));
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "{events}");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn mcp_abandoned_consent_leaves_no_connected_client() {
    let f = crate::isolation_tests::setup().await;
    let s = &f.state;
    let client = client(s).await;
    let listed = |s: Arc<Supervisor>| async move {
        req(&s, "GET", "/api/mcp/clients", Value::Null, &[]).await.2["clients"]
            .as_array()
            .unwrap()
            .len()
    };
    // Approved, but the client never comes back for its code.
    let (code, verifier) = code(s, &client, "read").await;
    assert_eq!(listed(s.clone()).await, 0);
    let expires: i64 = store::transaction(s, |db| {
        Ok(db.query_row(
            "SELECT MAX(expires) FROM entries WHERE kind='grant'",
            [],
            |r| r.get(0),
        )?)
    })
    .unwrap();
    assert!(expires <= store::now() + 300);
    let (status, tokens) = form(s, "/oauth/mcp/token", &exchange(&client, &code, &verifier)).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    assert_eq!(listed(s.clone()).await, 1);
    let (status, headers, _) = req(
        s,
        "POST",
        "/mcp",
        json!({"jsonrpc":"2.0","id":1,"method":"ping"}),
        &[JSON, ("authorization", "Bearer pmcp_revoked")],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let challenge = headers["www-authenticate"].to_str().unwrap();
    assert!(
        challenge.contains("resource_metadata=") && challenge.contains("error=\"invalid_token\"")
    );
    let (_, headers, _) = req(s, "POST", "/mcp", json!({}), &[JSON]).await;
    assert!(!headers["www-authenticate"]
        .to_str()
        .unwrap()
        .contains("invalid_token"));
}

#[tokio::test]
async fn mcp_arguments_take_the_schema_type() {
    let f = crate::isolation_tests::setup().await;
    let thread = crate::isolation_tests::thread(&f.state, &f.open).await;
    let grant = Grant {
        device: None,
        client: "c".into(),
        access: "read".into(),
        workspaces: None,
    };
    let events = tools::call(&f.state, &grant, "pockymoe_events", &json!({"cursor":0}))
        .await
        .unwrap();
    assert!(events["cursor"].is_string());
    let wait = tools::call(
        &f.state,
        &grant,
        "pockymoe_thread_wait",
        &json!({"threadId":thread,"timeoutSeconds":"1"}),
    )
    .await
    .unwrap();
    assert!(wait.get("status").is_some());
    let err = tools::call(
        &f.state,
        &grant,
        "pockymoe_thread_wait",
        &json!({"threadId":thread,"timeoutSeconds":"soon"}),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("invalid argument type"), "{err}");
}

#[tokio::test]
async fn mcp_settings_names_the_client_of_each_call() {
    let f = crate::isolation_tests::setup().await;
    let s = &f.state;
    store::transaction(s, |db| {
        store::put(db, "grant", "g", &json!({"id":"g","clientId":"pmcp_c","clientName":"Codex","access":"read","workspaces":null,"createdAt":store::now()}), 900)?;
        store::put(db, "audit", "a", &json!({"client":"pmcp_c","tool":"pockymoe_threads","target":null,"time":store::now(),"result":"ok"}), 900)?;
        store::put(db, "audit", "b", &json!({"client":"local-stdio","tool":"pockymoe_threads","target":null,"time":store::now(),"result":"ok"}), 900)
    })
    .unwrap();
    let (_, _, body) = req(s, "GET", "/api/mcp/clients", Value::Null, &[]).await;
    let calls = body["calls"].as_array().unwrap();
    assert_eq!(
        calls.iter().find(|c| c["client"] == "pmcp_c").unwrap()["clientName"],
        "Codex"
    );
    assert!(calls.iter().find(|c| c["client"] == "local-stdio").unwrap()["clientName"].is_null());
}
