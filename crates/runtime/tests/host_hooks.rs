use anyhow::{ensure, Result};
use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Provider, ThreadDto};
use pockymoe_runtime::{
    fake::FakeRuntime,
    host_hooks::{ThreadHost, ThreadOrigin},
    interaction::AgentOptions,
    Database, RuntimeConfig, Supervisor,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Weak,
};

fn fixture() -> (
    tempfile::TempDir,
    Arc<Supervisor>,
    Arc<FakeRuntime>,
    CreateThreadInput,
) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = RuntimeConfig::from_env().unwrap();
    config.database_url = dir.path().join("db.sqlite");
    config.workspace_root = dir.path().into();
    config.treer_enabled = false;
    config.fake_runtime = true;
    config.relay_server_url = None;
    let runtime = Arc::new(FakeRuntime::new(Provider::Claude));
    let state = Arc::new(Supervisor::new(
        config.clone(),
        Database::open(&config.database_url).unwrap(),
        vec![runtime.clone()],
    ));
    let ws = state
        .create_workspace(CreateWorkspaceInput {
            abs_path: Some(dir.path().to_string_lossy().into()),
            label: None,
            git_url: None,
        })
        .unwrap();
    let input = CreateThreadInput {
        workspace_id: ws.id,
        title: Some("Lead".into()),
        provider: Some(Provider::Claude),
        agent_id: None,
        model: "ios-e2e-stream".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: None,
    };
    (dir, state, runtime, input)
}
struct Host {
    state: Weak<Supervisor>,
    origins: Mutex<Vec<ThreadOrigin>>,
    calls: Mutex<Vec<String>>,
    fail: AtomicBool,
}
impl Host {
    fn new(state: &Arc<Supervisor>) -> Arc<Self> {
        Arc::new(Self {
            state: Arc::downgrade(state),
            origins: Default::default(),
            calls: Default::default(),
            fail: AtomicBool::new(false),
        })
    }
    fn call(&self, event: &str, thread: &ThreadDto) -> Result<()> {
        // The host observes a durable thread before mutation, including deletion.
        ensure!(self.state.upgrade().unwrap().get_thread(&thread.id).is_ok());
        self.calls.lock().unwrap().push(event.into());
        ensure!(!self.fail.load(Ordering::SeqCst), "fixture host refused");
        Ok(())
    }
}
#[async_trait::async_trait]
impl ThreadHost for Host {
    async fn created(&self, t: &ThreadDto, origin: &ThreadOrigin) -> Result<()> {
        self.origins.lock().unwrap().push(origin.clone());
        self.call("create", t)
    }
    fn before_delete(&self, t: &ThreadDto) -> Result<()> {
        self.call("delete", t)
    }
    fn before_close(&self, t: &ThreadDto) -> Result<()> {
        self.call("close", t)
    }
    async fn before_prompt(&self, t: &ThreadDto) -> Result<()> {
        self.call("reopen", t)
    }
    fn before_rename(&self, t: &ThreadDto, _: &str) -> Result<()> {
        self.call("rename", t)
    }
    fn reserve_closed_names(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn creation_origins_and_failed_create_or_fork_roll_back_durable_rows_and_harnesses() {
    use pockymoe_runtime::actor::AgentRuntime;
    let (_dir, s, runtime, input) = fixture();
    let host = Host::new(&s);
    s.set_thread_host(host.clone()).unwrap();
    assert!(s.set_thread_host(host.clone()).is_err());
    let root = s.create_thread(input.clone()).await.unwrap();
    let mut child = input.clone();
    child.parent_thread_id = Some(root.id.clone());
    s.create_thread(child).await.unwrap();
    s.create_thread_with_origin(
        input.clone(),
        Default::default(),
        ThreadOrigin::Browser {
            caller_thread_id: root.id.clone(),
        },
    )
    .await
    .unwrap();
    s.create_attached_thread(input.clone(), vec![], "external".into())
        .await
        .unwrap();
    s.create_thread_with_origin(input.clone(), Default::default(), ThreadOrigin::Hidden)
        .await
        .unwrap();
    s.fork_thread(&root.id).await.unwrap();
    assert_eq!(
        *host.origins.lock().unwrap(),
        vec![
            ThreadOrigin::Local,
            ThreadOrigin::Delegate {
                caller_thread_id: root.id.clone()
            },
            ThreadOrigin::Browser {
                caller_thread_id: root.id.clone()
            },
            ThreadOrigin::Attached {
                external_id: "external".into()
            },
            ThreadOrigin::Hidden,
            ThreadOrigin::Fork {
                source_thread_id: root.id.clone()
            }
        ]
    );
    let count = s.list_threads(None, true).unwrap().len();
    let loaded = runtime.loaded_sessions().await.len();
    host.fail.store(true, Ordering::SeqCst);
    assert!(s
        .create_thread(input)
        .await
        .unwrap_err()
        .to_string()
        .contains("fixture host refused"));
    assert_eq!(s.list_threads(None, true).unwrap().len(), count);
    assert_eq!(runtime.loaded_sessions().await.len(), loaded);
    assert!(s.fork_thread(&root.id).await.is_err());
    assert_eq!(s.list_threads(None, true).unwrap().len(), count);
}

#[tokio::test]
async fn lifecycle_refusals_preserve_thread_and_closed_names_stay_reserved() {
    let (_dir, s, _runtime, input) = fixture();
    let host = Host::new(&s);
    s.set_thread_host(host.clone()).unwrap();
    let root = s.create_thread(input.clone()).await.unwrap();
    let mut input = input;
    input.parent_thread_id = Some(root.id.clone());
    let options = || AgentOptions {
        name: Some("worker".into()),
        ..Default::default()
    };
    let child = s
        .create_thread_with(input.clone(), options())
        .await
        .unwrap();
    host.fail.store(true, Ordering::SeqCst);
    assert!(s.rename_thread(&child.id, "New").is_err());
    assert!(s
        .close_agent_thread(Some(&root.id), &child.id, false)
        .is_err());
    assert!(s.delete_thread_releasing(&child.id).await.is_err());
    assert!(s.get_thread(&child.id).unwrap().closed_at.is_none());
    assert_eq!(s.get_thread(&child.id).unwrap().title, "Lead");
    host.fail.store(false, Ordering::SeqCst);
    s.close_agent_thread(Some(&root.id), &child.id, false)
        .unwrap();
    assert!(s.create_thread_with(input, options()).await.is_err());
    s.prepare_thread_prompt(&child.id).await.unwrap();
    assert_eq!(host.calls.lock().unwrap().last().unwrap(), "reopen");
    s.rename_thread(&child.id, "New").unwrap();
    s.delete_child_thread(&root.id, &child.id).await.unwrap();
    assert!(s.get_thread(&child.id).is_err());
    assert_eq!(host.calls.lock().unwrap().last().unwrap(), "delete");
}

#[tokio::test]
async fn absent_hook_preserves_standalone_close_name_reuse_and_identity_rules() {
    let (_dir, s, _runtime, input) = fixture();
    let root = s.create_thread(input.clone()).await.unwrap();
    let mut input = input;
    input.parent_thread_id = Some(root.id.clone());
    let options = || AgentOptions {
        name: Some("worker".into()),
        ..Default::default()
    };
    let child = s
        .create_thread_with(input.clone(), options())
        .await
        .unwrap();
    assert!(s.set_thread_cli_identity(&child.id, vec![]).is_err());
    s.close_agent_thread(Some(&root.id), &child.id, false)
        .unwrap();
    let replacement = s.create_thread_with(input, options()).await.unwrap();
    s.rename_thread(&replacement.id, "Renamed").unwrap();
    assert_eq!(s.get_thread(&replacement.id).unwrap().title, "Renamed");
    s.delete_child_thread(&root.id, &replacement.id)
        .await
        .unwrap();
    assert!(s.get_thread(&replacement.id).is_err());
    assert!(s.get_thread(&child.id).unwrap().closed_at.is_some());
}
