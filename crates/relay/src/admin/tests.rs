//! Admin API tests against a disposable store.
use super::*;

async fn body(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn bearer(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    headers
}

#[tokio::test]
async fn legacy_summary_reports_tunnel_times_lineage_and_no_thread_titles() {
    let (state, dir) = crate::tests::test_app_state("admin-legacy-summary");
    let token = {
        let conn = state.store.conn.lock().await;
        conn.execute_batch(
            "INSERT INTO relay_users VALUES ('admin','admin@x','admin','admin',1,NULL,'2026-01-01T00:00:00Z','s','h');
             INSERT INTO relay_users VALUES ('owner','o@x','owner','user',1,NULL,'2026-01-01T00:00:00Z','s','h');
             INSERT INTO relay_users VALUES ('guest','g@x','guest','user',1,NULL,'2026-01-01T00:00:00Z','s','h');
             INSERT INTO relay_devices VALUES ('dev','owner','Laptop',NULL,'th','rcd_x...y','2026-01-02T00:00:00Z');
             INSERT INTO relay_thread_lineage(device_id,thread_id,parent_thread_id,workspace_id) VALUES
               ('dev','t1',NULL,'w1'),('dev','t2','t1','w1'),('dev','t3',NULL,'w2');
             INSERT INTO relay_shares(id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,thread_id,thread_title,thread_access,workspace_access,created_at)
               VALUES ('share','owner','owner','guest','guest','dev','Laptop','t1','Secret plans','read','none','2026-01-03T00:00:00Z');
             INSERT INTO relay_access_grants(id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,scope,thread_access,workspace_access,created_at)
               VALUES ('grant','owner','owner','guest','guest','dev','Laptop','device','control','write','2026-01-03T00:00:00Z');",
        )
        .unwrap();
        create_session(&conn, &state.store.session_secret, "admin").unwrap()
    };
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    state.sockets.write().await.insert(
        "dev".into(),
        DeviceSocket {
            tx,
            connection_id: Uuid::new_v4(),
            connected_at: "2026-01-04T00:00:00Z".into(),
            last_heartbeat_at: "2026-01-04T00:01:00Z".into(),
        },
    );
    let value = body(
        relay_admin(
            bearer(&token),
            Query(AdminQuery::default()),
            State(state.clone()),
        )
        .await
        .into_response(),
    )
    .await;
    let device = &value["devices"][0];
    assert_eq!(device["connectedAt"], "2026-01-04T00:00:00Z");
    assert_eq!(device["lastHeartbeatAt"], "2026-01-04T00:01:00Z");
    assert_eq!(device["threadCount"], 3);
    assert_eq!(device["agentThreadCount"], 1);
    assert_eq!(device["workspaceCount"], 2);
    assert_eq!(value["shares"][0]["threadTitle"], Value::Null);
    assert_eq!(value["grants"][0]["scope"], "device");
    let owner = value["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|user| user["id"] == "owner")
        .unwrap();
    assert_eq!(owner["deviceCount"], 1);
    let unauthorized = relay_admin(
        HeaderMap::new(),
        Query(AdminQuery::default()),
        State(state.clone()),
    )
    .await
    .into_response();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn rejecting_a_registration_deletes_its_pending_vault() {
    let (state, dir) = crate::tests::test_app_state("admin-reject-vault");
    let token = {
        let conn = state.store.conn.lock().await;
        conn.execute_batch(
            "INSERT INTO relay_users VALUES ('admin','admin@x','admin','admin',1,NULL,'2026-01-01T00:00:00Z','s','h');
             INSERT INTO relay_pending_registrations(id,email,username,password_salt,password_hash,created_at)
               VALUES ('req','n@x','newbie','s','h','2026-01-01T00:00:00Z');
             INSERT INTO relay_pending_account_vaults(request_id,user_id,vault_json,recovery_salt,recovery_hash)
               VALUES ('req','u','{}','s','h');",
        )
        .unwrap();
        create_session(&conn, &state.store.session_secret, "admin").unwrap()
    };
    let response = reject_registration(
        Path("req".into()),
        bearer(&token),
        Query(TokenQuery::default()),
        State(state.clone()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let conn = state.store.conn.lock().await;
    let vaults: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM relay_pending_account_vaults",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM relay_pending_registrations WHERE id='req'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!((vaults, status.as_str()), (0, "rejected"));
    drop(conn);
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A relay serving only the admin API (behind the real browser-security
/// middleware) over a seeded store, with an admin and a user session.
struct Console {
    state: Arc<AppState>,
    base: String,
    client: reqwest::Client,
    task: tokio::task::JoinHandle<()>,
    dir: PathBuf,
    admin: String,
    user: String,
}

impl Drop for Console {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const SECRET_TITLE: &str = "Quarterly secret plans";

fn ago(hours: i64) -> String {
    common::rfc3339_ago(hours * common::HOUR_MS)
}

/// Seeds people, devices, shares, activity, registrations, sessions, security
/// events and public links shaped like a small real relay.
fn seed(conn: &Connection) {
    let user = |id: &str, role: &str, enabled: i64, seen: Option<i64>, created_days: i64| {
        conn.execute(
            "INSERT INTO relay_users(id,email,username,role,enabled,last_seen_at,created_at,password_salt,password_hash)
             VALUES (?1,?2,?1,?3,?4,?5,?6,'','')",
            params![id, format!("{id}@example.test"), role, enabled, seen.map(ago), ago(created_days * 24)],
        )
        .unwrap();
    };
    user("admin", "admin", 1, Some(0), 100);
    user("alice", "user", 1, Some(0), 90);
    user("bob", "user", 1, Some(30), 60);
    user("carol", "user", 0, Some(24 * 40), 50);
    user("dave", "user", 1, None, 3);
    user("erin", "user", 1, Some(2), 10);
    conn.execute_batch(&format!(
        "INSERT INTO relay_account_vaults(user_id,vault_json,revision,account_key_id,account_public_key)
           VALUES ('alice','{{}}',1,'k1','p1'),('bob','{{}}',1,'k2','p2');
         INSERT INTO relay_account_auth(user_id,oauth_only) VALUES ('erin',1);
         INSERT INTO relay_user_identities(id,user_id,provider,provider_subject,provider_email,created_at)
           VALUES ('i1','erin','github','gh-1','erin@example.test','{now}');
         INSERT INTO relay_totp(user_id,secret,enabled,expires_at) VALUES ('alice','s',1,0);
         INSERT INTO relay_passkeys(id,user_id,credential,name,created_at) VALUES ('pk','bob','{{}}','Key',0);
         INSERT INTO relay_devices(id,owner_user_id,name,token_hash,token_preview,created_at) VALUES
           ('d-alice-1','alice','gpu-lab','h1','rcd_a','{d1}'),
           ('d-alice-2','alice','laptop','h2','rcd_b','{d2}'),
           ('d-bob','bob','build-01','h3','rcd_c','{d2}');
         INSERT INTO relay_device_certificates(device_id,certificate_json)
           VALUES ('d-alice-1','{{\"createdAt\":\"2026-05-24T00:00:00.000Z\"}}'),('d-bob','not json');
         INSERT INTO relay_thread_lineage(device_id,thread_id,parent_thread_id,workspace_id) VALUES
           ('d-alice-1','t1',NULL,'w1'),('d-alice-1','t2','t1','w1'),('d-bob','t3',NULL,'w9');
         INSERT INTO relay_upstream_catalog(device_id,connection_id,name,protocol,revision) VALUES
           ('d-alice-1','c1','My private key name','anthropic','r'),('d-alice-1','c2','Other','openai_responses','r');
         INSERT INTO relay_shares(id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,thread_id,thread_title,workspace_label,thread_access,workspace_access,created_at)
           VALUES ('s1','alice','alice','bob','bob','d-alice-1','gpu-lab','t1','{SECRET_TITLE}','paper-figs','read','none','{d1}');
         INSERT INTO relay_access_grants(id,owner_user_id,owner_username,target_user_id,target_username,device_id,device_name,scope,thread_title,thread_access,workspace_access,can_create_threads,created_at,expires_at)
           VALUES ('g1','alice','alice','bob','bob','d-alice-1','gpu-lab','device','{SECRET_TITLE}','control','write',1,'{d1}',NULL),
                  ('g2','bob','bob','alice','alice','d-bob','build-01','workspace',NULL,'control','read',0,'{d1}','{soon}'),
                  ('g3','bob','bob','dave','dave','d-bob','build-01','thread',NULL,'read','none',0,'{d1}',NULL);
         INSERT INTO relay_share_activity(id,resource_id,resource_type,user_id,username,kind,accessed_at)
           VALUES ('a1','g1','grant','bob','bob','send_prompt','{h1}'),('a2','s1','share','bob','bob','open_thread','{h1}');
         INSERT INTO relay_pending_registrations(id,email,username,password_salt,password_hash,created_at,status,reviewed_at,reviewed_by_user_id)
           VALUES ('r1','new@x','newbie','s','h','{h5}','pending',NULL,NULL),
                  ('r2','ok@x','okay','s','h','{d2}','approved','{d1}','admin'),
                  ('r3','no@x','nope','s','h','{d2}','rejected','{d1}','admin');
         INSERT INTO relay_public_links(id,owner_user_id,device_id,thread_id,snapshot_json,created_at) VALUES
           ('l1','alice','d-alice-1','t1','{{\"title\":\"{SECRET_TITLE}\",\"turns\":[]}}','{d1}'),
           ('l2','alice','d-alice-1','t1','{{\"version\":2,\"iv\":\"x\",\"ciphertext\":\"y\"}}','{d1}');",
        now = ago(0),
        d1 = ago(24),
        d2 = ago(48),
        h1 = ago(1),
        h5 = ago(5),
        soon = common::rfc3339_from_ms(common::now_ms() + common::DAY_MS),
    ))
    .unwrap();
    let now = common::now_ms();
    // Oldest first, as the relay appends them.
    for (user, event, resource, minutes) in [
        ("bob", "login.challenge", None, 40),
        ("alice", "passkey.added", None, 30),
        ("bob", "device.token_rotated", Some("d-bob"), 20),
        ("bob", "login.password_or_oauth", None, 10),
        ("alice", "login.mfa", None, 5),
    ] {
        conn.execute(
            "INSERT INTO relay_security_events(user_id,event,resource_id,created_at) VALUES (?1,?2,?3,?4)",
            params![user, event, resource, now - minutes * common::MINUTE_MS],
        )
        .unwrap();
    }
}

impl Console {
    async fn new(name: &str) -> Self {
        let (state, dir) = crate::tests::test_app_state(name);
        let (admin, user) = {
            let conn = state.store.conn.lock().await;
            seed(&conn);
            let admin = create_session(&conn, &state.store.session_secret, "admin").unwrap();
            let user = create_session(&conn, &state.store.session_secret, "bob").unwrap();
            crate::account::mark_session(&conn, &user, crate::account::FULL).unwrap();
            conn.execute(
                "UPDATE relay_auth_sessions SET environment='Chrome · macOS' WHERE user_id='bob'",
                [],
            )
            .unwrap();
            (admin, user)
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .merge(routes())
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                security::browser_security,
            ))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            state,
            base,
            client: reqwest::Client::new(),
            task,
            dir,
            admin,
            user,
        }
    }
    async fn call(&self, method: Method, path: &str, token: Option<&str>) -> (StatusCode, Value) {
        let mut request = self.client.request(method, format!("{}{path}", self.base));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null))
    }
    async fn get(&self, path: &str) -> Value {
        let (status, value) = self
            .call(Method::GET, path, Some(&self.admin.clone()))
            .await;
        assert_eq!(status, StatusCode::OK, "{path}: {value}");
        value
    }
    async fn go_online(&self, device: &str) {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        std::mem::forget(rx);
        self.state.sockets.write().await.insert(
            device.into(),
            DeviceSocket {
                tx,
                connection_id: Uuid::new_v4(),
                connected_at: ago(3),
                last_heartbeat_at: ago(0),
            },
        );
    }
}

const READ_ROUTES: [&str; 13] = [
    "/relay/admin/overview",
    "/relay/admin/search?q=a",
    "/relay/admin/users",
    "/relay/admin/users/alice",
    "/relay/admin/registrations",
    "/relay/admin/devices",
    "/relay/admin/devices/d-alice-1",
    "/relay/admin/shares",
    "/relay/admin/public-links",
    "/relay/admin/previews",
    "/relay/admin/security",
    "/relay/admin/security/events",
    "/relay/admin/system",
];

#[tokio::test]
async fn every_admin_route_needs_the_admin_and_never_carries_thread_titles() {
    let console = Console::new("admin-console-auth").await;
    for path in READ_ROUTES.iter().chain(["/relay/admin/sessions"].iter()) {
        assert_eq!(
            console.call(Method::GET, path, None).await.0,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
        let user = console.user.clone();
        assert_eq!(
            console.call(Method::GET, path, Some(&user)).await.0,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
        let value = console.get(path).await;
        assert!(
            !value.to_string().contains(SECRET_TITLE),
            "{path} leaks a thread title"
        );
        assert!(
            !value.to_string().contains("My private key name"),
            "{path} leaks an upstream name"
        );
    }
    for (method, path) in [
        (Method::DELETE, "/relay/admin/users/bob/sessions"),
        (Method::DELETE, "/relay/admin/shares/grant/g1"),
        (Method::POST, "/relay/admin/devices/d-bob/disconnect"),
    ] {
        let user = console.user.clone();
        assert_eq!(
            console.call(method, path, Some(&user)).await.0,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
}

#[tokio::test]
async fn users_page_filters_counts_and_details() {
    let console = Console::new("admin-console-users").await;
    console.go_online("d-alice-1").await;
    let all = console.get("/relay/admin/users?pageSize=2").await;
    assert_eq!(all["total"], 6);
    assert_eq!(all["items"].as_array().unwrap().len(), 2);
    assert_eq!(all["counts"]["noAccountKey"], 3);
    assert_eq!(all["counts"]["disabled"], 1);
    assert_eq!(all["counts"]["neverSeen"], 1);
    assert_eq!(all["counts"]["noTwoFactor"], 3);
    assert_eq!(all["pendingRegistrations"], 1);
    assert_eq!(all["methods"]["github"], 1);
    assert_eq!(all["methods"]["oauthOnly"], 1);
    let second = console.get("/relay/admin/users?pageSize=2&page=2").await;
    assert_ne!(second["items"][0]["id"], all["items"][0]["id"]);
    let missing_key = console
        .get("/relay/admin/users?filter=noAccountKey&sort=name")
        .await;
    let names: Vec<&str> = missing_key["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["username"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["carol", "dave", "erin"]);
    let found = console.get("/relay/admin/users?q=ALI").await;
    assert_eq!(found["total"], 1);
    let alice = &found["items"][0];
    assert_eq!(alice["accountKey"], true);
    assert_eq!(alice["authenticator"], true);
    assert_eq!(alice["devices"], 2);
    assert_eq!(alice["devicesOnline"], 1);
    let erin = &console.get("/relay/admin/users?q=erin").await["items"][0];
    assert_eq!(erin["loginMethods"], json!(["github"]));
    let admin = &console.get("/relay/admin/users?q=admin%40").await["items"][0];
    assert_eq!(admin["accountKey"], Value::Null);
    // Wildcards in the search are text.
    assert_eq!(console.get("/relay/admin/users?q=%25").await["total"], 0);

    let detail = console.get("/relay/admin/users/bob").await;
    assert_eq!(detail["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(detail["sessions"][0]["environment"], "Chrome · macOS");
    assert_eq!(detail["shares"], json!({ "outgoing": 2, "incoming": 2 }));
    assert_eq!(detail["devices"][0]["name"], "build-01");
    assert_eq!(detail["events"][0]["event"], "login.password_or_oauth");
    let admin_token = console.admin.clone();
    assert_eq!(
        console
            .call(Method::GET, "/relay/admin/users/nobody", Some(&admin_token))
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    let (status, ended) = console
        .call(
            Method::DELETE,
            "/relay/admin/users/bob/sessions",
            Some(&admin_token),
        )
        .await;
    assert_eq!((status, ended["ended"].clone()), (StatusCode::OK, json!(1)));
    let user = console.user.clone();
    assert_eq!(
        console
            .call(Method::GET, "/relay/admin/overview", Some(&user))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, _) = console
        .call(
            Method::DELETE,
            "/relay/admin/users/admin/sessions",
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let pending = console.get("/relay/admin/registrations").await;
    assert_eq!(pending["total"], 1);
    assert_eq!(pending["summary"]["approved30d"], 1);
    assert_eq!(pending["summary"]["rejected30d"], 1);
    assert!(pending["summary"]["medianApprovalHours"].as_f64().unwrap() > 23.0);
    let reviewed = console
        .get("/relay/admin/registrations?status=reviewed")
        .await;
    assert_eq!(reviewed["total"], 2);
    assert_eq!(reviewed["items"][0]["reviewedBy"], "admin");
}

#[tokio::test]
async fn devices_page_joins_tunnels_and_disconnects() {
    let console = Console::new("admin-console-devices").await;
    console.go_online("d-alice-1").await;
    let list = console.get("/relay/admin/devices").await;
    assert_eq!(
        list["counts"],
        json!({ "all": 3, "online": 1, "offline": 2, "protected": 2, "unprotected": 1, "unstable": 0, "outdated": 0 })
    );
    // Online first.
    assert_eq!(list["items"][0]["id"], "d-alice-1");
    let device = &list["items"][0];
    assert_eq!(device["threadCount"], 2);
    assert_eq!(device["agentThreadCount"], 1);
    assert_eq!(device["protectedAt"], "2026-05-24T00:00:00.000Z");
    let bob = console.get("/relay/admin/devices?q=build").await;
    assert_eq!(bob["items"][0]["protected"], true);
    assert_eq!(bob["items"][0]["protectedAt"], Value::Null);
    assert_eq!(
        console.get("/relay/admin/devices?filter=unprotected").await["items"][0]["name"],
        "laptop"
    );
    let detail = console.get("/relay/admin/devices/d-alice-1").await;
    assert_eq!(
        detail["upstreamProtocols"],
        json!(["anthropic", "openai_responses"])
    );
    assert_eq!(detail["shares"].as_array().unwrap().len(), 2);
    let admin = console.admin.clone();
    let (status, value) = console
        .call(
            Method::POST,
            "/relay/admin/devices/d-alice-1/disconnect",
            Some(&admin),
        )
        .await;
    assert_eq!(
        (status, value["wasConnected"].clone()),
        (StatusCode::OK, json!(true))
    );
    assert!(!console.state.sockets.read().await.contains_key("d-alice-1"));
    let (status, _) = console
        .call(
            Method::POST,
            "/relay/admin/devices/missing/disconnect",
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn shares_list_every_scope_and_admin_revocation_stops_relay_access() {
    let console = Console::new("admin-console-shares").await;
    let list = console.get("/relay/admin/shares").await;
    assert_eq!(
        list["counts"],
        json!({ "all": 4, "device": 1, "workspace": 1, "thread": 2, "expiring": 1, "unused": 2 })
    );
    assert_eq!(list["summary"]["permissions"]["canCreateThreads"], 1);
    let grant = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "g1")
        .unwrap();
    assert_eq!(grant["lastAccessKind"], "send_prompt");
    assert_eq!(
        console.get("/relay/admin/shares?filter=unused").await["total"],
        2
    );
    assert_eq!(console.get("/relay/admin/shares?q=dave").await["total"], 1);
    assert_eq!(list["usage"]["days"].as_array().unwrap().len(), 14);
    assert!(effective_access(
        &*console.state.store.conn.lock().await,
        "bob",
        "d-alice-1",
        None,
        None
    )
    .is_some());
    let admin = console.admin.clone();
    let (status, _) = console
        .call(Method::DELETE, "/relay/admin/shares/grant/g1", Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = console
        .call(Method::DELETE, "/relay/admin/shares/grant/g1", Some(&admin))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = console
        .call(Method::DELETE, "/relay/admin/shares/other/g1", Some(&admin))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        console.get("/relay/admin/shares").await["counts"]["device"],
        0
    );

    let links = console.get("/relay/admin/public-links").await;
    assert_eq!(links["summary"]["legacy"], 1);
    assert_eq!(links["summary"]["encrypted"], 1);
    assert_eq!(
        console.get("/relay/admin/public-links?filter=legacy").await["items"][0]["id"],
        "l1"
    );
}

#[tokio::test]
async fn overview_security_and_system_read_existing_records() {
    let console = Console::new("admin-console-overview").await;
    console.go_online("d-bob").await;
    let overview = console.get("/relay/admin/overview?range=7d").await;
    assert_eq!(overview["range"], "7d");
    assert_eq!(overview["attention"]["pendingRegistrations"], 1);
    assert_eq!(overview["attention"]["usersWithoutAccountKey"], 3);
    assert_eq!(overview["attention"]["legacyPublicLinks"], 1);
    assert_eq!(overview["devices"]["online"], 1);
    assert_eq!(overview["users"]["total"], 5);
    let activity: i64 = overview["activity"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["count"].as_i64().unwrap())
        .sum();
    assert_eq!(activity, 5);
    assert_eq!(overview["signupsWeekly"].as_array().unwrap().len(), 12);
    assert_eq!(overview["securityEvents"][0]["event"], "login.mfa");
    assert_eq!(overview["securityEvents"][2]["resourceLabel"], "build-01");

    let security = console.get("/relay/admin/security?range=7d").await;
    assert_eq!(security["logins"], 2);
    assert_eq!(security["mfaLogins"], 1);
    assert_eq!(security["activeSessions"], 2);
    assert_eq!(security["daily"].as_array().unwrap().len(), 7);
    let events = console
        .get("/relay/admin/security/events?type=login&pageSize=1")
        .await;
    assert_eq!(
        (
            events["total"].clone(),
            events["items"].as_array().unwrap().len()
        ),
        (json!(2), 1)
    );
    assert_eq!(
        console
            .get("/relay/admin/security/events?type=credential")
            .await["total"],
        2
    );
    let sessions = console.get("/relay/admin/sessions?q=chrome").await;
    assert_eq!(sessions["items"][0]["username"], "bob");
    let admin = console.admin.clone();
    let id = sessions["items"][0]["id"].as_str().unwrap().to_string();
    let (status, _) = console
        .call(
            Method::DELETE,
            &format!("/relay/admin/sessions/{id}"),
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let own: String = console
        .state
        .store
        .conn
        .lock()
        .await
        .query_row(
            "SELECT token_hash FROM relay_auth_sessions WHERE user_id='admin'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let (status, _) = console
        .call(
            Method::DELETE,
            &format!("/relay/admin/sessions/{own}"),
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let system = console.get("/relay/admin/system").await;
    assert_eq!(system["connections"]["deviceTunnels"], 1);
    let integrity = system["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == "integrity")
        .unwrap();
    assert_eq!(integrity["ok"], true);
    assert!(!system["storage"]["tables"].as_array().unwrap().is_empty());
    assert!(system["relay"]["dataDir"].as_str().is_some());

    let search = console.get("/relay/admin/search?q=build").await;
    assert_eq!(search["devices"][0]["id"], "d-bob");
}

// ── Recorded statistics ─────────────────────────────────────────────────

/// A fake device on a real tunnel: answers `/healthz` with a version.
async fn fake_device(
    state: Arc<AppState>,
    device: &'static str,
    version: &'static str,
) -> (tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>) {
    use tokio_tungstenite::{connect_async, tungstenite::Message as ClientMessage};
    let app_state = state.clone();
    let app = Router::new().route(
        "/tunnel",
        get(move |ws: WebSocketUpgrade| async move {
            ws.on_upgrade(move |socket| {
                handle_supervisor_with_timeout(
                    socket,
                    app_state,
                    device.into(),
                    Duration::from_secs(30),
                )
            })
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (mut socket, _) = connect_async(format!("ws://{address}/tunnel"))
        .await
        .unwrap();
    let client = tokio::spawn(async move {
        while let Some(Ok(message)) = socket.next().await {
            let ClientMessage::Text(text) = message else {
                continue;
            };
            let frame: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if frame["type"] == "relay.request" {
                let body = json!({ "status": "ok", "runningVersion": version }).to_string();
                let reply = json!({ "type": "relay.response", "requestId": frame["requestId"], "payload": { "statusCode": 200, "headers": { "content-type": "application/json" }, "body": body } });
                if socket
                    .send(ClientMessage::Text(reply.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    });
    for _ in 0..100 {
        if state.sockets.read().await.contains_key(device) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (server, client)
}

#[tokio::test]
async fn sampler_records_tunnels_traffic_versions_and_the_overview_reads_them() {
    let console = Console::new("admin-console-stats").await;
    let state = console.state.clone();
    let (server, client) = fake_device(state.clone(), "d-bob", "0.12.9").await;
    // Forwarded traffic and failures are counted in memory until the sample.
    state.metrics.forwarded(
        "d-bob",
        100,
        &crate::metrics::Forwarded::Answered {
            status: 200,
            response_bytes: 2000,
        },
        Duration::from_millis(40),
    );
    state.metrics.forwarded(
        "d-bob",
        50,
        &crate::metrics::Forwarded::Timeout,
        Duration::from_secs(30),
    );
    assert!(state.admission.allow("login:x".into(), 1, 60));
    assert!(!state.admission.allow("login:x".into(), 1, 60));
    let minute = {
        let now = common::now_ms();
        now - now.rem_euclid(common::MINUTE_MS)
    };
    crate::metrics::sample(&state, minute).await;
    // The version probe runs on its own task.
    for _ in 0..100 {
        let known: Option<String> = state
            .store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT version FROM relay_device_versions WHERE device_id='d-bob'",
                [],
                |r| r.get(0),
            )
            .optional()
            .unwrap()
            .flatten();
        if known.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    {
        let conn = state.store.conn.lock().await;
        let (requests, errors): (i64, i64) = conn
            .query_row(
                "SELECT requests,errors FROM relay_device_traffic WHERE device_id='d-bob'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((requests, errors), (2, 1));
        let refused: f64 = conn
            .query_row("SELECT value FROM relay_metrics WHERE resolution='m' AND metric='rate_limited' AND label='login'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(refused, 1.0);
        let open: i64 = conn
            .query_row("SELECT COUNT(*) FROM relay_device_connections WHERE device_id='d-bob' AND ended_at IS NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(open, 1);
        // Nothing about the request beyond counts and sizes was stored.
        let stored: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM relay_metrics WHERE label LIKE '%/%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, 0);
    }
    let overview = console.get("/relay/admin/overview?range=24h").await;
    let online = overview["onlineDevices"]["points"].as_array().unwrap();
    assert!(online.iter().any(|point| point[1] == 1.0));
    // Before recording started the points are unknown, not zero.
    assert_eq!(online[0][1], Value::Null);
    assert!(overview["onlineDevices"]["historyStart"].is_string());
    assert_eq!(overview["tunnels"]["requests"], 2.0);
    assert_eq!(overview["tunnels"]["errorRate"], 0.5);
    assert_eq!(overview["tunnels"]["p95Ms"], 50.0);
    assert_eq!(overview["attention"]["unstableDevices"], 0);
    assert_eq!(overview["versions"][0]["version"], "0.12.9");
    let devices = console.get("/relay/admin/devices?filter=outdated").await;
    assert_eq!(devices["items"][0]["id"], "d-bob");
    assert_eq!(devices["items"][0]["outdated"], true);
    assert_eq!(
        devices["items"][0]["traffic7d"]
            .as_array()
            .unwrap()
            .last()
            .unwrap(),
        2
    );
    let detail = console.get("/relay/admin/devices/d-bob").await;
    assert_eq!(detail["connections"]["spans"].as_array().unwrap().len(), 1);
    assert!(detail["connections"]["uptime"].as_f64().unwrap() > 0.0);
    assert_eq!(
        detail["traffic"]["hours"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["requests"].as_i64().unwrap())
            .sum::<i64>(),
        2
    );
    let security = console.get("/relay/admin/security?range=24h").await;
    assert_eq!(security["rateLimited"]["byKind"]["login"], 1.0);

    // Disconnecting records why the tunnel ended; the device reconnects later.
    let admin = console.admin.clone();
    let (status, _) = console
        .call(
            Method::POST,
            "/relay/admin/devices/d-bob/disconnect",
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // The relay closes the socket at once; the device does not wait for a
    // timeout to notice and reconnect.
    tokio::time::timeout(Duration::from_secs(5), client)
        .await
        .expect("the relay closed the tunnel")
        .unwrap();
    let detail = console.get("/relay/admin/devices/d-bob").await;
    assert_eq!(detail["connections"]["spans"][0]["reason"], "admin");
    assert!(detail["device"]["lastSeenAt"].is_string());
    server.abort();
}

#[tokio::test]
async fn replaced_and_flapping_tunnels_are_recorded_and_flagged() {
    let console = Console::new("admin-console-flapping").await;
    let state = console.state.clone();
    for _ in 0..crate::metrics::UNSTABLE_CONNECTS {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let connection = Uuid::new_v4();
        if let Some(previous) = state.sockets.write().await.insert(
            "d-alice-1".into(),
            DeviceSocket {
                tx,
                connection_id: connection,
                connected_at: ago(0),
                last_heartbeat_at: ago(0),
            },
        ) {
            state
                .metrics
                .disconnected(previous.connection_id, crate::metrics::EndReason::Replaced);
        }
        state.metrics.connected("d-alice-1", connection);
    }
    {
        let conn = state.store.conn.lock().await;
        conn.execute(
            "INSERT INTO relay_settings(key,value) VALUES ('metricsStartedAt','0')",
            [],
        )
        .unwrap();
    }
    let unstable = console.get("/relay/admin/devices?filter=unstable").await;
    assert_eq!(unstable["items"][0]["id"], "d-alice-1");
    assert_eq!(
        unstable["items"][0]["reconnects1h"],
        crate::metrics::UNSTABLE_CONNECTS
    );
    assert_eq!(
        console.get("/relay/admin/overview").await["attention"]["unstableDevices"],
        1
    );
    let conn = state.store.conn.lock().await;
    let replaced: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM relay_device_connections WHERE reason='replaced'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(replaced, crate::metrics::UNSTABLE_CONNECTS - 1);
}

#[tokio::test]
async fn every_admin_change_is_logged_without_secrets() {
    let console = Console::new("admin-console-actions").await;
    let admin = console.admin.clone();
    let send = |method: Method, path: &'static str, body: Value| {
        let request = console
            .client
            .request(method, format!("{}{path}", console.base))
            .bearer_auth(&admin)
            .json(&body);
        async move { request.send().await.unwrap().status() }
    };
    assert_eq!(
        send(
            Method::PATCH,
            "/relay/admin/settings/registration",
            json!({ "approvalRequired": true, "registrationPassword": "invite-code-123" })
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        send(
            Method::PATCH,
            "/relay/admin/users/bob",
            json!({ "enabled": false })
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        send(
            Method::POST,
            "/relay/admin/users/carol/reset-password",
            json!({ "password": "temporary-password-1" })
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        send(
            Method::POST,
            "/relay/admin/registrations/r1/reject",
            json!({})
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        send(Method::DELETE, "/relay/admin/shares/grant/g2", json!({})).await,
        StatusCode::OK
    );
    assert_eq!(
        send(
            Method::DELETE,
            "/relay/admin/users/dave/sessions",
            json!({})
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        send(
            Method::PATCH,
            "/relay/admin/settings/retention",
            json!({ "metricsDays": 30 })
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        send(
            Method::PATCH,
            "/relay/admin/settings/retention",
            json!({ "metricsDays": 31 })
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(Method::DELETE, "/relay/admin/users/erin", json!({})).await,
        StatusCode::OK
    );
    let log = console.get("/relay/admin/actions?pageSize=50").await;
    let actions: Vec<&str> = log["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        [
            "user.deleted",
            "settings.retention",
            "user.sessions_ended",
            "share.revoked",
            "registration.rejected",
            "user.password_reset",
            "user.disabled",
            "settings.registration"
        ]
    );
    let text = log.to_string();
    assert!(!text.contains("invite-code-123") && !text.contains("temporary-password-1"));
    let settings = &log["items"][7]["detail"];
    assert_eq!(settings["approvalRequired"], json!([false, true]));
    assert_eq!(settings["inviteCode"], "set");
    assert_eq!(log["items"][3]["targetLabel"], "bob → alice");
    assert_eq!(log["items"][0]["targetLabel"], "erin");
    assert_eq!(log["items"][1]["detail"]["metricsDays"], json!([90, 30]));
    assert_eq!(
        console.get("/relay/admin/settings/retention").await["metricsDays"],
        30
    );
    assert_eq!(
        console.get("/relay/admin/overview").await["adminActions"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
}

#[tokio::test]
async fn retention_prunes_old_statistics_and_audit_rows() {
    let console = Console::new("admin-console-retention").await;
    let conn = console.state.store.conn.lock().await;
    let now = common::now_ms();
    let old = now - 400 * common::DAY_MS;
    conn.execute_batch(&format!(
        "INSERT INTO relay_metrics VALUES ('m',{minute},'up','',1,1,1),('m',{recent},'up','',1,1,1),
           ('h',{old},'up','',1,1,1),('h',{now},'up','',1,1,1);
         INSERT INTO relay_device_connections(device_id,connection_id,connected_at,ended_at,reason)
           VALUES ('d-bob','old',{old},{old},'closed'),('d-bob','open',{old},NULL,NULL);
         INSERT INTO relay_security_events(user_id,event,created_at) VALUES ('bob','login.mfa',{old});
         INSERT INTO relay_admin_actions(admin_username,action,created_at) VALUES ('admin','user.enabled',{old});
         INSERT INTO relay_daily_active VALUES ('2020-01-01','user','bob');",
        minute = now - 3 * common::DAY_MS,
        recent = now - common::HOUR_MS,
    ))
    .unwrap();
    crate::metrics::prune(&conn).unwrap();
    let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(
        count("SELECT COUNT(*) FROM relay_metrics WHERE resolution='m'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM relay_metrics WHERE resolution='h'"),
        1
    );
    // An open connection is kept however old: it is still the current one.
    assert_eq!(count("SELECT COUNT(*) FROM relay_device_connections"), 1);
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) FROM relay_security_events WHERE created_at={old}"
        )),
        0
    );
    assert_eq!(count("SELECT COUNT(*) FROM relay_admin_actions"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM relay_daily_active"), 0);
}

#[tokio::test]
async fn account_activity_writes_at_most_once_a_minute_and_a_day_row_once() {
    let console = Console::new("admin-console-activity").await;
    let conn = console.state.store.conn.lock().await;
    conn.execute(
        "UPDATE relay_users SET last_seen_at=NULL WHERE id='dave'",
        [],
    )
    .unwrap();
    crate::metrics::note_user_activity(&conn, "dave");
    let first: String = conn
        .query_row(
            "SELECT last_seen_at FROM relay_users WHERE id='dave'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE relay_users SET last_seen_at='marker' WHERE id='dave'",
        [],
    )
    .unwrap();
    crate::metrics::note_user_activity(&conn, "dave");
    let second: String = conn
        .query_row(
            "SELECT last_seen_at FROM relay_users WHERE id='dave'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(first.starts_with("20"));
    assert_eq!(second, "marker");
    let days: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM relay_daily_active WHERE kind='user' AND subject_id='dave'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(days, 1);
}

#[tokio::test]
async fn failed_sign_ins_are_counted_and_audited_only_for_existing_accounts() {
    let console = Console::new("admin-console-login-failed").await;
    let state = console.state.clone();
    for identifier in ["bob", "nobody-here"] {
        let input: LoginInput = serde_json::from_value(
            json!({ "identifier": identifier, "password": "wrong password!" }),
        )
        .unwrap();
        let response = login(State(state.clone()), HeaderMap::new(), Json(input)).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let minute = common::now_ms() - common::now_ms().rem_euclid(common::MINUTE_MS);
    crate::metrics::sample(&state, minute).await;
    let conn = state.store.conn.lock().await;
    let failed: f64 = conn
        .query_row(
            "SELECT value FROM relay_metrics WHERE resolution='m' AND metric='login_failed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(failed, 2.0);
    let audited: Vec<Option<String>> = conn
        .prepare("SELECT user_id FROM relay_security_events WHERE event='login.failed'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(audited, [Some("bob".to_string())]);
}
