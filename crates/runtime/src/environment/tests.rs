use super::*;
use async_trait::async_trait;
use serde_json::{json, Value};

#[derive(Default)]
struct FixtureState {
    compose: Option<Value>,
    container: Option<Value>,
    calls: Vec<(String, Value)>,
}

struct FixtureDocker(Arc<Mutex<FixtureState>>);

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
                            json!([{"environments":[{"environmentId":"fixture-env","compose":state.compose.iter().cloned().collect::<Vec<_>>()}]}])
                        }
                        "/api/compose.create" => {
                            let mut compose = payload.clone();
                            compose["composeId"] = json!("fixture-compose");
                            state.compose = Some(compose.clone());
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
                            state.container =
                                Some(container_fixture(state.compose.as_ref().unwrap()));
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
