use super::*;
use async_trait::async_trait;
use serde_json::{json, Value};

#[derive(Default)]
struct FixtureState {
    compose: Option<Value>,
    container: Option<Value>,
    calls: Vec<(String, Value)>,
    ignore_lifecycle: bool,
    lose_create_response: bool,
    hide_compose: bool,
}

struct FixtureDocker(Arc<Mutex<FixtureState>>);

struct WrongProofDocker(FixtureDocker);

struct UnavailableDocker;

#[async_trait]
impl EnvironmentDocker for UnavailableDocker {
    async fn host_info(&self, _: &LocalDockerContext) -> Result<DockerHostInfo> {
        bail!("fixture daemon unavailable")
    }
    async fn bind_sha256(
        &self,
        _: &LocalDockerContext,
        _: &Path,
        _: Option<&Path>,
    ) -> Result<String> {
        bail!("fixture daemon unavailable")
    }
    async fn inspect_environment(&self, _: &LocalDockerContext, _: &str) -> Result<Vec<Value>> {
        bail!("fixture daemon unavailable")
    }
}

#[async_trait]
impl EnvironmentDocker for WrongProofDocker {
    async fn host_info(&self, context: &LocalDockerContext) -> Result<DockerHostInfo> {
        self.0.host_info(context).await
    }
    async fn bind_sha256(
        &self,
        _: &LocalDockerContext,
        _: &Path,
        _: Option<&Path>,
    ) -> Result<String> {
        Ok("0".repeat(64))
    }
    async fn inspect_environment(
        &self,
        context: &LocalDockerContext,
        id: &str,
    ) -> Result<Vec<Value>> {
        self.0.inspect_environment(context, id).await
    }
}

#[async_trait]
impl EnvironmentDocker for FixtureDocker {
    async fn host_info(&self, _: &LocalDockerContext) -> Result<DockerHostInfo> {
        Ok(DockerHostInfo {
            daemon_id: "fixture-daemon".into(),
            cpus: 8.0,
            memory_bytes: 16 * 1024 * 1024 * 1024,
            supervisor_container: None,
            host_namespace_verified: true,
        })
    }
    async fn bind_sha256(
        &self,
        _: &LocalDockerContext,
        source: &Path,
        relative: Option<&Path>,
    ) -> Result<String> {
        storage::file_sha256(
            &relative
                .map(|v| source.join(v))
                .unwrap_or_else(|| source.to_owned()),
        )
    }
    async fn inspect_environment(&self, _: &LocalDockerContext, id: &str) -> Result<Vec<Value>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .container
            .iter()
            .filter(|v| v["Config"]["Labels"][docker::ENVIRONMENT_LABEL] == id)
            .cloned()
            .collect())
    }
}

async fn api_fixture() -> (
    String,
    Arc<Mutex<FixtureState>>,
    tokio::task::JoinHandle<()>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = Arc::new(Mutex::new(FixtureState::default()));
    let server_state = state.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let state = server_state.clone();
            tokio::spawn(async move {
                let mut data = Vec::new();
                let mut buffer = [0_u8; 8192];
                let (header_len, body_len) = loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        return;
                    }
                    data.extend_from_slice(&buffer[..count]);
                    if let Some(end) = data.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&data[..end]);
                        let len = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        break (end + 4, len);
                    }
                };
                while data.len() < header_len + body_len {
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        return;
                    }
                    data.extend_from_slice(&buffer[..count]);
                }
                let headers = String::from_utf8_lossy(&data[..header_len]);
                let first = headers.lines().next().unwrap();
                let route = first
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .split('?')
                    .next()
                    .unwrap();
                let payload: Value = if body_len == 0 {
                    json!({})
                } else {
                    serde_json::from_slice(&data[header_len..header_len + body_len]).unwrap()
                };
                let response = {
                    let mut state = state.lock().unwrap();
                    state.calls.push((route.into(), payload.clone()));
                    match route {
                        "/api/project.all" => {
                            json!([{"environments":[{"environmentId":"fixture-env"}]}])
                        }
                        "/api/environment.one" => {
                            let entries = state.compose.iter().filter(|_| !state.hide_compose)
                                .map(|compose| json!({"name":compose["name"],"composeId":compose["composeId"]}))
                                .collect::<Vec<_>>();
                            json!({"environmentId":"fixture-env","compose":entries})
                        }
                        "/api/compose.create" => {
                            let mut compose = payload.clone();
                            compose["composeId"] = json!("fixture-compose");
                            compose["appName"] =
                                json!(format!("{}-rg1o4a", payload["appName"].as_str().unwrap()));
                            state.compose = Some(compose.clone());
                            if std::mem::take(&mut state.lose_create_response) {
                                // The server committed; the client never receives its identity.
                                return;
                            }
                            compose
                        }
                        "/api/compose.one" => state.compose.clone().unwrap_or(Value::Null),
                        "/api/compose.update" => {
                            let compose = state.compose.as_mut().unwrap();
                            for (k, v) in payload.as_object().unwrap() {
                                compose[k] = v.clone();
                            }
                            compose.clone()
                        }
                        "/api/compose.deploy" => {
                            let command = state.compose.as_ref().unwrap()["command"]
                                .as_str()
                                .unwrap_or("")
                                .to_owned();
                            if command.ends_with(" down --remove-orphans") {
                                if !state.ignore_lifecycle {
                                    state.container = None;
                                }
                            } else if command.ends_with(" stop") {
                                if !state.ignore_lifecycle {
                                    if let Some(container) = &mut state.container {
                                        container["State"]["Running"] = json!(false);
                                    }
                                }
                            } else {
                                state.container =
                                    Some(container_fixture(state.compose.as_ref().unwrap()));
                            }
                            json!(true)
                        }
                        "/api/compose.cleanQueues" => json!({"success":true}),
                        "/api/deployment.queueList" => json!([]),
                        "/api/compose.stop" => {
                            if let Some(container) = &mut state.container {
                                container["State"]["Running"] = json!(false);
                            }
                            json!(true)
                        }
                        "/api/compose.delete" => {
                            state.compose = None;
                            state.container = None;
                            json!(true)
                        }
                        _ => json!({"message":"fixture-secret-should-not-escape"}),
                    }
                };
                let body = serde_json::to_vec(&response).unwrap();
                let head=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                socket.write_all(head.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            });
        }
    });
    (format!("http://{address}"), state, task)
}

fn container_fixture(compose: &Value) -> Value {
    let yaml: Value = serde_yaml::from_str(compose["composeFile"].as_str().unwrap()).unwrap();
    let service = &yaml["services"]["worker"];
    json!({
        "Id":"a".repeat(64),"State":{"Running":true},
        "Config":{"User":service["user"],"Image":service["image"],"Labels":service["labels"]},
        "HostConfig":{
            "Privileged":false,"NetworkMode":"none","PidMode":"","IpcMode":"private","Init":true,"ReadonlyRootfs":true,
            "CapDrop":["ALL"],"CapAdd":[],"Devices":[],"DeviceRequests":[],"SecurityOpt":["no-new-privileges:true"],"PortBindings":{},
            "NanoCpus":(service["cpus"].as_f64().unwrap()*1_000_000_000.0) as u64,
            "Memory":service["mem_limit"],"MemorySwap":service["memswap_limit"],"PidsLimit":service["pids_limit"],
            "Ulimits":[{"Name":"nofile","Soft":service["ulimits"]["nofile"]["soft"],"Hard":service["ulimits"]["nofile"]["hard"]}]
        },
        "Mounts":service["volumes"].as_array().unwrap().iter().map(|mount|json!({
            "Type":"bind","Source":mount["source"],"Destination":mount["target"],"RW":!mount["read_only"].as_bool().unwrap(),"Propagation":"rprivate"
        })).collect::<Vec<_>>()
    })
}

fn connection(base_url: &str, root: &Path) -> UpdateEnvironmentConnectionRequest {
    UpdateEnvironmentConnectionRequest {
        name: "fixture".into(),
        base_url: base_url.into(),
        api_key: Some("fixture-secret-should-not-escape".into()),
        dokploy_environment_id: "fixture-env".into(),
        image: "fixture-worker:1".into(),
        storage_node: "local".into(),
        docker_host: None,
        allowed_roots: vec![root.to_string_lossy().into_owned()],
        allowed_egress_hosts: Vec::new(),
        cpu_limit: 1.0,
        memory_limit_bytes: 256 * 1024 * 1024,
        pids_limit: 128,
        nofile_limit: 1024,
    }
}

fn fixture_project(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir_all(&path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if path.metadata().unwrap().uid() == 0 {
            std::os::unix::fs::chown(&path, Some(1000), Some(1000)).unwrap();
        }
    }
    path
}

#[tokio::test]
async fn lifecycle_is_persistent_generation_fenced_and_retains_data() {
    let root = tempfile::tempdir().unwrap();
    let project = fixture_project(root.path(), "project");
    std::fs::write(project.join("sentinel"), "keep-project").unwrap();
    let (url, fixture, task) = api_fixture().await;
    let database = root.path().join("supervisor.sqlite");
    let manager = EnvironmentManager::new(&database);
    manager
        .set_docker_driver(Arc::new(FixtureDocker(fixture.clone())))
        .unwrap();
    let connection = manager
        .save_connection("new", &connection(&url, &project))
        .await
        .unwrap();
    assert!(manager.test_connection(&connection.id).await.unwrap().ok);
    let input = CreateEnvironmentRequest {
        connection_id: connection.id.clone(),
        thread_id: "thread".into(),
        name: None,
    };
    let created = manager.create(&input, &project, &project).await.unwrap();
    assert!(manager.for_thread("thread").unwrap().is_none());
    manager.bind_thread("thread", Some(&created.id)).unwrap();
    manager.inherit_thread("thread", "child").unwrap();
    assert_eq!(manager.for_thread("child").unwrap().unwrap().id, created.id);
    assert_eq!(created.state, "stopped");
    let running = manager
        .action(&created.id, EnvironmentAction::Start)
        .await
        .unwrap();
    assert_eq!(running.state, "running");
    assert!(running.observed_limits.is_some());
    let record = manager.record(&running.id).unwrap();
    let actual_app_name = format!("{}-rg1o4a", record.app_name);
    assert_eq!(
        record.dokploy_app_name.as_deref(),
        Some(actual_app_name.as_str())
    );
    let original = fixture.lock().unwrap().container.as_ref().unwrap().clone();
    let bridge = running
        .mounts
        .iter()
        .find(|mount| mount.target == compose::BRIDGE_PATH)
        .unwrap();
    assert!(bridge.read_only);
    let mut writable_bridge = original.clone();
    writable_bridge["Mounts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|mount| mount["Destination"] == compose::BRIDGE_PATH)
        .unwrap()["RW"] = json!(true);
    assert!(docker::verify_container(&writable_bridge, &expected(&record)).is_err());
    let bad_fields = [
        ("/HostConfig/Privileged", json!(true)),
        ("/HostConfig/NetworkMode", json!("bridge")),
        ("/HostConfig/PidMode", json!("host")),
        ("/HostConfig/IpcMode", json!("host")),
        ("/HostConfig/Init", json!(false)),
        ("/HostConfig/ReadonlyRootfs", json!(false)),
        ("/HostConfig/CapDrop", json!([])),
        ("/HostConfig/CapAdd", json!(["SYS_ADMIN"])),
        (
            "/HostConfig/SecurityOpt",
            json!(["no-new-privileges:true", "seccomp=unconfined"]),
        ),
        ("/HostConfig/Memory", json!(0)),
        ("/HostConfig/MemorySwap", json!(-1)),
        ("/HostConfig/PidsLimit", json!(0)),
        ("/HostConfig/Ulimits/0/Hard", json!(1048576)),
        (
            "/HostConfig/PortBindings",
            json!({"45680/tcp":[{"HostPort":"45680"}]}),
        ),
        ("/Config/User", json!("0:0")),
        ("/Config/Labels/ca.pockymoe.generation", json!("0")),
        ("/Mounts/0/Source", json!("/etc")),
        ("/Mounts/0/Propagation", json!("rshared")),
    ];
    for (pointer, value) in bad_fields {
        let mut container = original.clone();
        *container.pointer_mut(pointer).unwrap() = value;
        assert!(
            docker::verify_container(&container, &expected(&record)).is_err(),
            "accepted weakened field {pointer}"
        );
    }
    let mut extra_group = original.clone();
    extra_group["HostConfig"]["GroupAdd"] = json!(["0"]);
    assert!(docker::verify_container(&extra_group, &expected(&record)).is_err());
    let mut extra_mount = original.clone();
    extra_mount["Mounts"].as_array_mut().unwrap().push(json!({"Type":"bind","Source":"/var/run/docker.sock","Destination":"/socket","RW":true,"Propagation":"rprivate"}));
    assert!(docker::verify_container(&extra_mount, &expected(&record)).is_err());
    manager
        .validate_generation(&running.id, running.generation)
        .unwrap();
    let state = manager.directory.join("state").join(&running.id);
    std::fs::write(state.join("sentinel"), "keep-state").unwrap();
    let rebuilt = manager
        .action(&running.id, EnvironmentAction::Rebuild)
        .await
        .unwrap();
    assert!(rebuilt.generation > running.generation);
    assert!(manager
        .validate_generation(&running.id, running.generation)
        .is_err());
    assert_eq!(
        std::fs::read_to_string(state.join("sentinel")).unwrap(),
        "keep-state"
    );
    let snapshot = serde_json::to_string(&manager.snapshot().await.unwrap()).unwrap();
    assert!(!snapshot.contains("fixture-secret"));
    drop(manager);
    let manager = EnvironmentManager::new(&database);
    manager
        .set_docker_driver(Arc::new(FixtureDocker(fixture.clone())))
        .unwrap();
    let interrupted = manager.for_thread("thread").unwrap().unwrap();
    assert_eq!(interrupted.state, "interrupted");
    assert!(interrupted.generation > rebuilt.generation);
    assert!(manager
        .validate_generation(&running.id, rebuilt.generation)
        .is_err());
    let stopped = manager
        .action(&running.id, EnvironmentAction::Stop)
        .await
        .unwrap();
    assert_eq!(stopped.state, "stopped");
    let destroyed = manager
        .action(&running.id, EnvironmentAction::Destroy)
        .await
        .unwrap();
    assert_eq!(destroyed.state, "destroyed");
    {
        let state = fixture.lock().unwrap();
        assert!(state.calls.iter().any(|(_, body)| body["command"]
            == format!("compose -p {actual_app_name} -f docker-compose.yml stop")));
        assert!(state.calls.iter().any(|(_, body)| body["command"]
            == format!(
                "compose -p {actual_app_name} -f docker-compose.yml down --remove-orphans"
            )));
    }
    assert_eq!(
        std::fs::read_to_string(project.join("sentinel")).unwrap(),
        "keep-project"
    );
    assert_eq!(
        std::fs::read_to_string(state.join("sentinel")).unwrap(),
        "keep-state"
    );
    assert_eq!(
        manager.for_thread("thread").unwrap().unwrap().state,
        "destroyed"
    );
    assert!(manager
        .action(&running.id, EnvironmentAction::Start)
        .await
        .is_err());
    assert!(fixture
        .lock()
        .unwrap()
        .calls
        .iter()
        .any(|(path, body)| path == "/api/compose.delete" && body["deleteVolumes"] == false));
    task.abort();
}

#[tokio::test]
async fn lost_create_response_recovers_exact_suffixed_identity_without_recreating() {
    let root = tempfile::tempdir().unwrap();
    let project = fixture_project(root.path(), "project");
    let (url, fixture, task) = api_fixture().await;
    let manager = EnvironmentManager::new(&root.path().join("db"));
    manager
        .set_docker_driver(Arc::new(FixtureDocker(fixture.clone())))
        .unwrap();
    let conn = manager
        .save_connection("new", &connection(&url, &project))
        .await
        .unwrap();
    let created = manager
        .create(
            &CreateEnvironmentRequest {
                connection_id: conn.id,
                thread_id: "thread".into(),
                name: None,
            },
            &project,
            &project,
        )
        .await
        .unwrap();
    fixture.lock().unwrap().lose_create_response = true;
    assert!(manager
        .action(&created.id, EnvironmentAction::Start)
        .await
        .is_err());
    assert!(manager.record(&created.id).unwrap().create_pending);
    drop(manager);
    let manager = EnvironmentManager::new(&root.path().join("db"));
    manager
        .set_docker_driver(Arc::new(FixtureDocker(fixture.clone())))
        .unwrap();
    assert!(manager.record(&created.id).unwrap().create_pending);
    fixture.lock().unwrap().hide_compose = true;
    assert!(manager
        .action(&created.id, EnvironmentAction::Start)
        .await
        .unwrap_err()
        .to_string()
        .contains("uncertain"));
    assert!(manager.record(&created.id).unwrap().resource_active);
    fixture.lock().unwrap().hide_compose = false;
    let running = manager
        .action(&created.id, EnvironmentAction::Start)
        .await
        .unwrap();
    assert_eq!(running.state, "running");
    let record = manager.record(&created.id).unwrap();
    assert_eq!(
        record.dokploy_app_name,
        fixture.lock().unwrap().compose.as_ref().unwrap()["appName"]
            .as_str()
            .map(str::to_owned)
    );
    assert!(!record.create_pending);
    let mutations = fixture
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|(route, _)| route == "/api/compose.create")
        .count();
    assert_eq!(mutations, 1);
    fixture.lock().unwrap().compose.as_mut().unwrap()["appName"] =
        json!(format!("{}-another", record.app_name));
    assert!(manager
        .action(&created.id, EnvironmentAction::Stop)
        .await
        .unwrap_err()
        .to_string()
        .contains("identity changed"));
    task.abort();
}

#[test]
fn compose_recovery_requires_exact_names_complete_listing_and_unchanged_ownership() {
    let name = "pockymoe-owned";
    let listing = json!({"environmentId":"env","compose":[{"name":name,"composeId":"owned"},{"name":"pockymoe-owned-other","composeId":"other"}]});
    assert_eq!(
        find_owned_compose(&listing, name, "env")
            .unwrap()
            .as_deref(),
        Some("owned")
    );
    assert!(find_owned_compose(&listing, name, "other-env").is_err());
    assert!(find_owned_compose(&json!({"environmentId":"env"}), name, "env").is_err());
    assert!(find_owned_compose(&json!({"environmentId":"env","compose":[{"name":name,"composeId":"a"},{"name":name,"composeId":"b"}]}), name, "env").is_err());
    let owned = json!({"name":name,"appName":"pockymoe-owned-rg1o4a","environmentId":"env","serverId":null,"sourceType":"raw","composeType":"docker-compose"});
    let actual = verify_owned_compose(&owned, name, "env", None).unwrap();
    assert!(verify_owned_compose(&owned, name, "env", Some(&actual)).is_ok());
    for (field, value) in [
        ("name", json!("another")),
        ("environmentId", json!("another")),
        ("serverId", json!("remote")),
        ("sourceType", json!("git")),
        ("composeType", json!("stack")),
        ("appName", json!("pockymoe-owned-renamed")),
    ] {
        let mut changed = owned.clone();
        changed[field] = value;
        assert!(
            verify_owned_compose(&changed, name, "env", Some(&actual)).is_err(),
            "accepted changed {field}"
        );
    }
}

#[tokio::test]
async fn budgets_and_inspect_reject_overcommit_or_weakened_container() {
    let root = tempfile::tempdir().unwrap();
    let project = fixture_project(root.path(), "project");
    let (url, fixture, task) = api_fixture().await;
    let manager = EnvironmentManager::new(&root.path().join("db"));
    manager
        .set_docker_driver(Arc::new(FixtureDocker(fixture.clone())))
        .unwrap();
    let conn = manager
        .save_connection("new", &connection(&url, &project))
        .await
        .unwrap();
    let first = manager
        .create(
            &CreateEnvironmentRequest {
                connection_id: conn.id.clone(),
                thread_id: "a".into(),
                name: None,
            },
            &project,
            &project,
        )
        .await
        .unwrap();
    let second = manager
        .create(
            &CreateEnvironmentRequest {
                connection_id: conn.id,
                thread_id: "b".into(),
                name: None,
            },
            &project,
            &project,
        )
        .await
        .unwrap();
    manager
        .save_budget(&EnvironmentBudgetDto {
            max_running: 1,
            ..EnvironmentBudgetDto::default()
        })
        .await
        .unwrap();
    manager
        .action(&first.id, EnvironmentAction::Start)
        .await
        .unwrap();
    assert!(manager
        .action(&second.id, EnvironmentAction::Start)
        .await
        .unwrap_err()
        .to_string()
        .contains("budget"));
    fixture.lock().unwrap().container.as_mut().unwrap()["HostConfig"]["NetworkMode"] =
        json!("host");
    assert!(manager
        .action(&first.id, EnvironmentAction::Start)
        .await
        .unwrap_err()
        .to_string()
        .contains("network"));
    assert_eq!(manager.get(&first.id).unwrap().state, "error");
    assert!(manager
        .validate_generation(&first.id, manager.get(&first.id).unwrap().generation)
        .is_err());
    task.abort();
}

#[tokio::test]
async fn sidecar_is_private_locked_and_invalid_input_never_contacts_docker() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("db");
    let manager = EnvironmentManager::new(&database);
    let other = EnvironmentManager::new(&database);
    assert!(other.snapshot().await.is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            manager.directory.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            manager
                .directory
                .join("resources.json")
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let mut input = connection("https://dokploy.invalid", root.path());
    input.storage_node = "remote".into();
    assert!(manager.save_connection("new", &input).await.is_err());
    input.storage_node = "local".into();
    input.docker_host = Some("tcp://127.0.0.1:2375".into());
    assert!(manager.save_connection("new", &input).await.is_err());
    input.docker_host = None;
    input.base_url = "https://secret@dokploy.invalid".into();
    assert!(manager.save_connection("new", &input).await.is_err());
    assert!(manager
        .save_budget(&EnvironmentBudgetDto {
            max_cpu: f64::NAN,
            ..EnvironmentBudgetDto::default()
        })
        .await
        .is_err());
}

#[tokio::test]
async fn wrong_storage_identity_never_reaches_a_dokploy_mutation() {
    let root = tempfile::tempdir().unwrap();
    let project = fixture_project(root.path(), "project");
    let (url, fixture, task) = api_fixture().await;
    let manager = EnvironmentManager::new(&root.path().join("db"));
    manager
        .set_docker_driver(Arc::new(WrongProofDocker(FixtureDocker(fixture.clone()))))
        .unwrap();
    let conn = manager
        .save_connection("new", &connection(&url, &project))
        .await
        .unwrap();
    let environment = manager
        .create(
            &CreateEnvironmentRequest {
                connection_id: conn.id,
                thread_id: "thread".into(),
                name: None,
            },
            &project,
            &project,
        )
        .await
        .unwrap();
    assert!(manager
        .action(&environment.id, EnvironmentAction::Start)
        .await
        .unwrap_err()
        .to_string()
        .contains("not the Supervisor storage"));
    assert_eq!(manager.get(&environment.id).unwrap().state, "error");
    assert!(fixture.lock().unwrap().calls.is_empty());
    assert!(!std::fs::read_dir(&project).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".pockymoe-storage-proof-")));
    task.abort();
}

#[tokio::test]
async fn upstream_error_bodies_and_credentials_are_not_public() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let root = tempfile::tempdir().unwrap();
    let project = fixture_project(root.path(), "project");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        socket.read(&mut request).await.unwrap();
        let body = "sensitive upstream body fixture-secret-should-not-escape";
        socket.write_all(format!("HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    });
    let manager = EnvironmentManager::new(&root.path().join("db"));
    let conn = manager
        .save_connection("new", &connection(&url, &project))
        .await
        .unwrap();
    let response = manager.test_connection(&conn.id).await.unwrap();
    assert!(!response.ok);
    assert!(response.detail.contains("401"));
    assert!(!response.detail.contains("sensitive"));
    assert!(!response.detail.contains("fixture-secret"));
    task.await.unwrap();
}

#[tokio::test]
async fn reconcile_fences_oom_exit_replacement_and_unreachable_daemon() {
    let root = tempfile::tempdir().unwrap();
    let project = fixture_project(root.path(), "project");
    let (url, fixture, task) = api_fixture().await;
    let manager = EnvironmentManager::new(&root.path().join("db"));
    manager
        .set_docker_driver(Arc::new(FixtureDocker(fixture.clone())))
        .unwrap();
    let conn = manager
        .save_connection("new", &connection(&url, &project))
        .await
        .unwrap();
    let environment = manager
        .create(
            &CreateEnvironmentRequest {
                connection_id: conn.id,
                thread_id: "thread".into(),
                name: None,
            },
            &project,
            &project,
        )
        .await
        .unwrap();
    for fault in ["oom", "exit", "replacement", "unreachable"] {
        let running = manager
            .action(&environment.id, EnvironmentAction::Start)
            .await
            .unwrap();
        assert_eq!(
            manager.reconcile(&environment.id).await.unwrap().generation,
            running.generation
        );
        match fault {
            "oom" => {
                fixture.lock().unwrap().container.as_mut().unwrap()["State"] =
                    json!({"Running":false,"OOMKilled":true,"ExitCode":137})
            }
            "exit" => {
                fixture.lock().unwrap().container.as_mut().unwrap()["State"] =
                    json!({"Running":false,"OOMKilled":false,"ExitCode":42})
            }
            "replacement" => {
                fixture.lock().unwrap().container.as_mut().unwrap()["Id"] = json!("b".repeat(64))
            }
            "unreachable" => manager
                .set_docker_driver(Arc::new(UnavailableDocker))
                .unwrap(),
            _ => unreachable!(),
        }
        let snapshot = manager.reconcile_all().await.unwrap();
        let interrupted = snapshot
            .environments
            .iter()
            .find(|e| e.id == environment.id)
            .unwrap();
        assert_eq!(interrupted.state, "interrupted");
        assert!(interrupted.generation > running.generation);
        assert!(interrupted.container_id.is_none() && interrupted.observed_limits.is_none());
        assert!(interrupted.last_error.is_some());
        assert!(manager
            .validate_generation(&environment.id, running.generation)
            .is_err());
        assert_eq!(
            manager.reconcile(&environment.id).await.unwrap().generation,
            interrupted.generation
        );
        assert!(manager.record(&environment.id).unwrap().resource_active);
        manager
            .set_docker_driver(Arc::new(FixtureDocker(fixture.clone())))
            .unwrap();
    }
    task.abort();
}

#[tokio::test]
async fn incomplete_cleanup_retains_provider_record_and_capacity() {
    let root = tempfile::tempdir().unwrap();
    let project = fixture_project(root.path(), "project");
    let (url, fixture, task) = api_fixture().await;
    let manager = EnvironmentManager::new(&root.path().join("db"));
    manager
        .set_docker_driver(Arc::new(FixtureDocker(fixture.clone())))
        .unwrap();
    let conn = manager
        .save_connection("new", &connection(&url, &project))
        .await
        .unwrap();
    let environment = manager
        .create(
            &CreateEnvironmentRequest {
                connection_id: conn.id,
                thread_id: "thread".into(),
                name: None,
            },
            &project,
            &project,
        )
        .await
        .unwrap();
    let running = manager
        .action(&environment.id, EnvironmentAction::Start)
        .await
        .unwrap();
    fixture.lock().unwrap().ignore_lifecycle = true;
    assert!(manager
        .action(&running.id, EnvironmentAction::Destroy)
        .await
        .is_err());
    assert!(fixture.lock().unwrap().compose.is_some());
    assert!(manager.record(&running.id).unwrap().resource_active);
    assert!(manager
        .validate_generation(&running.id, running.generation)
        .is_err());
    fixture.lock().unwrap().ignore_lifecycle = false;
    assert_eq!(
        manager
            .action(&running.id, EnvironmentAction::Destroy)
            .await
            .unwrap()
            .state,
        "destroyed"
    );
    assert!(!manager.record(&running.id).unwrap().resource_active);
    task.abort();
}

#[tokio::test]
async fn persistence_failure_disables_all_execution_authority() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(&root.path().join("db"));
    let path = manager.directory.join("resources.json");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(path).unwrap();
    assert!(manager
        .save_budget(&EnvironmentBudgetDto {
            max_running: 3,
            ..EnvironmentBudgetDto::default()
        })
        .await
        .is_err());
    assert!(manager.snapshot().await.is_err());
    assert!(manager.for_thread("anything").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn changed_git_closure_cannot_mount_private_control_plane_storage() {
    let root = tempfile::tempdir().unwrap();
    let project = fixture_project(root.path(), "project");
    let (url, fixture, task) = api_fixture().await;
    let manager = EnvironmentManager::new(&root.path().join("db"));
    manager
        .set_docker_driver(Arc::new(FixtureDocker(fixture.clone())))
        .unwrap();
    // Even a broad explicit storage grant must not override the private DB/key
    // boundary when a checkout's .git pointer changes after creation.
    let conn = manager
        .save_connection("new", &connection(&url, root.path()))
        .await
        .unwrap();
    let environment = manager
        .create(
            &CreateEnvironmentRequest {
                connection_id: conn.id,
                thread_id: "thread".into(),
                name: None,
            },
            &project,
            &project,
        )
        .await
        .unwrap();
    std::fs::write(
        project.join(".git"),
        format!("gitdir: {}", manager.directory.display()),
    )
    .unwrap();
    assert!(manager
        .action(&environment.id, EnvironmentAction::Start)
        .await
        .unwrap_err()
        .to_string()
        .contains("private sidecar"));
    assert!(fixture.lock().unwrap().calls.is_empty());
    task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn replacing_the_canonical_checkout_path_is_not_silently_adopted() {
    let root = tempfile::tempdir().unwrap();
    let project = fixture_project(root.path(), "project");
    let replacement = fixture_project(root.path(), "replacement");
    let (url, fixture, task) = api_fixture().await;
    let manager = EnvironmentManager::new(&root.path().join("db"));
    manager
        .set_docker_driver(Arc::new(FixtureDocker(fixture.clone())))
        .unwrap();
    let conn = manager
        .save_connection("new", &connection(&url, root.path()))
        .await
        .unwrap();
    let environment = manager
        .create(
            &CreateEnvironmentRequest {
                connection_id: conn.id,
                thread_id: "thread".into(),
                name: None,
            },
            &project,
            &project,
        )
        .await
        .unwrap();
    std::fs::rename(&project, root.path().join("original")).unwrap();
    std::os::unix::fs::symlink(&replacement, &project).unwrap();
    assert!(manager
        .action(&environment.id, EnvironmentAction::Start)
        .await
        .unwrap_err()
        .to_string()
        .contains("path changed"));
    assert!(fixture.lock().unwrap().calls.is_empty());
    task.abort();
}
