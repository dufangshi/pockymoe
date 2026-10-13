use super::*;
use crate::actor::AgentRuntime;
use crate::fake::FakeRuntime;
use pockymoe_protocol::ThreadEventEnvelope;

const TEN_SECONDS: Duration = Duration::from_secs(10);

fn at(seconds_ago: i64) -> DateTime<Utc> {
    Utc::now() - chrono::TimeDelta::seconds(seconds_ago)
}

fn member(session: &str, process: &str, reason: HarnessLoadReason) -> Member {
    Member {
        provider: Provider::Codex,
        session: LoadedSession {
            session_id: session.into(),
            thread_id: Some(format!("thread-{session}")),
            process_id: process.into(),
            root_pid: None,
            load_reason: reason,
            requested_at: at(3600),
            loaded_at: at(3600),
            last_activity_at: at(3600),
            prompted: false,
            baseline: Vec::new(),
            busy: Vec::new(),
        },
        thread_id: Some(format!("thread-{session}")),
        displayed: false,
        left_at: None,
        busy: Vec::new(),
        pinned: false,
        bytes: Some(100),
    }
}

fn due(member: &Member, idle: Option<u32>) -> Option<ReleaseReason> {
    release_due(std::slice::from_ref(member), Utc::now(), idle, TEN_SECONDS)
}

#[test]
fn release_view_load_goes_ten_seconds_after_the_last_viewer_leaves() {
    let mut view = member("a", "p1", HarnessLoadReason::View);
    view.session.requested_at = at(60);
    view.session.last_activity_at = at(60);
    assert_eq!(due(&view, Some(10)), None, "never left: idle rule only");
    view.left_at = Some(at(5));
    assert_eq!(due(&view, Some(10)), None, "within the 10 second grace");
    view.left_at = Some(at(11));
    assert_eq!(due(&view, Some(10)), Some(ReleaseReason::View));
    view.displayed = true;
    assert_eq!(due(&view, Some(10)), None, "shown again");
    view.displayed = false;
    view.session.prompted = true;
    assert_eq!(
        due(&view, Some(10)),
        None,
        "a prompt switches to the idle rule"
    );
    view.session.prompted = false;
    view.left_at = Some(at(120));
    assert_eq!(
        due(&view, Some(10)),
        None,
        "a viewer who left before the load started does not count"
    );
    let mut turn = member("b", "p2", HarnessLoadReason::Turn);
    turn.left_at = Some(at(11));
    turn.session.last_activity_at = at(60);
    assert_eq!(
        due(&turn, Some(10)),
        None,
        "only view loads use the view rule"
    );
}

#[test]
fn release_idle_rule_counts_from_activity_or_leaving() {
    let mut idle = member("a", "p1", HarnessLoadReason::Turn);
    idle.session.last_activity_at = at(11 * 60);
    assert_eq!(due(&idle, Some(10)), Some(ReleaseReason::Idle));
    assert_eq!(due(&idle, None), None, "idle release can be turned off");
    assert_eq!(due(&idle, Some(20)), None);
    idle.left_at = Some(at(120));
    assert_eq!(
        due(&idle, Some(10)),
        None,
        "idle counts from leaving the page"
    );
    idle.left_at = Some(at(11 * 60));
    assert_eq!(due(&idle, Some(10)), Some(ReleaseReason::Idle));
    idle.displayed = true;
    assert_eq!(
        due(&idle, Some(10)),
        None,
        "a displayed thread is never released"
    );
    idle.displayed = false;
    idle.pinned = true;
    assert_eq!(due(&idle, Some(10)), None, "an operation is using it");
}

#[test]
fn release_waits_for_every_busy_reason() {
    for reason in [
        Busy::Turn,
        Busy::PendingRequest,
        Busy::QueuedInput,
        Busy::BackgroundTask,
        Busy::Subagent,
        Busy::Goal,
    ] {
        let mut busy = member("a", "p1", HarnessLoadReason::View);
        busy.session.requested_at = at(60);
        busy.left_at = Some(at(30));
        busy.busy = vec![reason];
        assert_eq!(due(&busy, Some(1)), None, "{reason:?}");
        let groups = groups(vec![busy]);
        let settings = HarnessResourceSettingsDto {
            max_loaded_threads: Some(1),
            ..Default::default()
        };
        assert!(
            limit_victims(&groups, &settings, None, None, Utc::now(), TEN_SECONDS).is_empty(),
            "limits never release a busy thread ({reason:?})"
        );
    }
}

#[test]
fn release_of_a_shared_process_needs_every_session() {
    let mut parent = member("parent", "shared", HarnessLoadReason::Turn);
    parent.session.last_activity_at = at(20 * 60);
    let mut fork = member("fork", "shared", HarnessLoadReason::Fork);
    fork.session.last_activity_at = at(60);
    let lone = member("lone", "other", HarnessLoadReason::Turn);
    let grouped = groups(vec![parent.clone(), lone, fork.clone()]);
    assert_eq!(grouped.len(), 2);
    assert_eq!(grouped[0].len(), 2);
    let now = Utc::now();
    assert_eq!(release_due(&grouped[0], now, Some(10), TEN_SECONDS), None);
    fork.session.last_activity_at = at(11 * 60);
    let pair = [parent.clone(), fork.clone()];
    assert_eq!(
        release_due(&pair, now, Some(10), TEN_SECONDS),
        Some(ReleaseReason::Idle)
    );
    fork.busy = vec![Busy::Turn];
    assert_eq!(
        release_due(&[parent, fork], now, Some(10), TEN_SECONDS),
        None,
        "a busy fork keeps its parent's process"
    );
}

#[test]
fn release_unattributed_loads_after_a_grace_period() {
    let mut orphan = member("gone", "p1", HarnessLoadReason::Fork);
    orphan.thread_id = None;
    orphan.session.loaded_at = at(10);
    orphan.session.last_activity_at = at(10);
    assert_eq!(due(&orphan, Some(10)), None, "a fork may still be saving");
    orphan.session.loaded_at = at(120);
    assert_eq!(due(&orphan, Some(10)), Some(ReleaseReason::Idle));
}

#[test]
fn release_limits_take_least_recently_used_settled_threads_first() {
    let make = |id: &str, minutes: i64| {
        let mut member = member(id, id, HarnessLoadReason::Turn);
        member.session.last_activity_at = at(minutes * 60);
        member
    };
    let mut busy_oldest = make("oldest", 40);
    busy_oldest.busy = vec![Busy::BackgroundTask];
    let groups = groups(vec![
        make("newest", 1),
        busy_oldest,
        make("old", 30),
        make("middle", 20),
    ]);
    let limit_victims_at =
        |groups: &[Vec<Member>], settings: &HarnessResourceSettingsDto, bytes, exclude| {
            limit_victims(groups, settings, bytes, exclude, Utc::now(), TEN_SECONDS)
        };
    let ids = |victims: Vec<usize>| -> Vec<String> {
        victims
            .into_iter()
            .map(|index| groups[index][0].session.session_id.clone())
            .collect()
    };
    let limit = |threads: Option<u32>, bytes: Option<u64>| HarnessResourceSettingsDto {
        idle_release_minutes: Some(10),
        max_loaded_threads: threads,
        max_harness_memory_bytes: bytes,
    };
    assert_eq!(
        ids(limit_victims_at(&groups, &limit(Some(2), None), None, None)),
        ["old", "middle"]
    );
    assert_eq!(
        ids(limit_victims_at(
            &groups,
            &limit(Some(2), None),
            None,
            Some("old")
        )),
        ["middle", "newest"],
        "the load that triggered the check is kept"
    );
    assert!(limit_victims_at(&groups, &limit(Some(4), None), None, None).is_empty());
    assert_eq!(
        ids(limit_victims_at(
            &groups,
            &limit(None, Some(250)),
            Some(400),
            None
        )),
        ["old", "middle"],
        "release until the harness memory is under the limit"
    );
    assert_eq!(
        ids(limit_victims_at(&groups, &limit(Some(1), None), None, None)),
        ["old", "middle", "newest"],
        "a busy thread may stay over the limit"
    );
    let mut fresh = groups.clone();
    fresh[0][0].session.loaded_at = at(5);
    assert_eq!(
        ids(limit_victims_at(&fresh, &limit(Some(1), None), None, None)),
        ["old", "middle"],
        "a load still finishing its user action is kept"
    );
}

#[test]
fn release_display_state_counts_viewers_and_clears_a_disconnect() {
    let mut display = DisplayState::default();
    let set = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<HashSet<_>>();
    let shown = |changes: Vec<DisplayChange>| -> Vec<(String, bool)> {
        changes
            .into_iter()
            .map(|change| (change.thread_id, change.displayed))
            .collect()
    };
    let now = Utc::now();
    assert_eq!(
        shown(display.set(1, set(&["a", "b"]), now)),
        [("a".into(), true), ("b".into(), true)]
    );
    assert!(
        display.set(2, set(&["b"]), now).is_empty(),
        "split view of the same thread"
    );
    assert!(
        display.set(1, set(&["a"]), now).is_empty(),
        "b is still shown by viewer 2"
    );
    assert!(display.displayed("b"));
    assert_eq!(shown(display.clear(2, now)), [("b".into(), false)]);
    assert!(!display.displayed("b"));
    assert_eq!(display.left_at("b"), Some(now));
    assert_eq!(display.left_at("a"), None);
    assert_eq!(shown(display.set(1, set(&[]), now)), [("a".into(), false)]);
    assert!(display.clear(1, now).is_empty());
    assert!(!display.displayed("a"));
}

#[test]
fn release_settings_validation() {
    let valid = HarnessResourceSettingsDto {
        idle_release_minutes: Some(1440),
        max_loaded_threads: Some(200),
        max_harness_memory_bytes: Some(256 * 1024 * 1024),
    };
    assert!(validate_settings(&valid).is_ok());
    assert!(validate_settings(&HarnessResourceSettingsDto {
        idle_release_minutes: None,
        max_loaded_threads: None,
        max_harness_memory_bytes: None,
    })
    .is_ok());
    for invalid in [
        HarnessResourceSettingsDto {
            idle_release_minutes: Some(0),
            ..valid.clone()
        },
        HarnessResourceSettingsDto {
            idle_release_minutes: Some(1441),
            ..valid.clone()
        },
        HarnessResourceSettingsDto {
            max_loaded_threads: Some(0),
            ..valid.clone()
        },
        HarnessResourceSettingsDto {
            max_loaded_threads: Some(201),
            ..valid.clone()
        },
        HarnessResourceSettingsDto {
            max_harness_memory_bytes: Some(256 * 1024 * 1024 - 1),
            ..valid.clone()
        },
    ] {
        assert!(validate_settings(&invalid).is_err(), "{invalid:?}");
    }
}

fn config(dir: &Path) -> RuntimeConfig {
    RuntimeConfig {
        mode: pockymoe_protocol::Mode::Local,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root: dir.into(),
        database_url: dir.join("db.sqlite"),
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
    }
}

fn supervisor(dir: &Path) -> Arc<Supervisor> {
    supervisor_with(dir, Provider::Codex).0
}

fn supervisor_with(dir: &Path, provider: Provider) -> (Arc<Supervisor>, Arc<FakeRuntime>) {
    let mut config = config(dir);
    config.enabled_providers = vec![provider];
    let db = Database::open(&config.database_url).unwrap();
    let fake = Arc::new(FakeRuntime::new(provider));
    let s = Arc::new(
        Supervisor::new(config, db, vec![fake.clone()]).with_local_session_homes(
            LocalSessionHomes {
                codex_home: dir.join("codex-home"),
                grok_home: dir.join("grok-home"),
                claude_home: dir.join("claude-home"),
            },
        ),
    );
    s.spawn_live_item_persister();
    (s, fake)
}

async fn thread(s: &Supervisor, dir: &Path, parent: Option<&str>) -> String {
    thread_for(s, dir, parent, Provider::Codex).await
}

async fn thread_for(
    s: &Supervisor,
    dir: &Path,
    parent: Option<&str>,
    provider: Provider,
) -> String {
    let workspace = s
        .list_workspaces()
        .unwrap()
        .into_iter()
        .next()
        .map(Ok)
        .unwrap_or_else(|| {
            s.create_workspace(CreateWorkspaceInput {
                abs_path: Some(dir.to_string_lossy().into()),
                git_url: None,
                label: None,
            })
        })
        .unwrap();
    s.create_thread(pockymoe_protocol::CreateThreadInput {
        workspace_id: workspace.id,
        title: Some("Release".into()),
        provider: Some(provider),
        agent_id: None,
        model: "ios-e2e-stream".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: parent.map(str::to_owned),
    })
    .await
    .unwrap()
    .id
}

async fn loaded(s: &Supervisor) -> Vec<LoadedSession> {
    let mut all = Vec::new();
    for provider in [Provider::Codex, Provider::Claude] {
        if let Ok(runtime) = s.runtime(provider) {
            all.extend(runtime.loaded_sessions().await);
        }
    }
    all
}

async fn prompt(s: &Supervisor, id: &str, text: &str) {
    s.prompt(
        id,
        pockymoe_protocol::SendThreadPromptInput {
            prompt: text.into(),
            client_request_id: None,
            model: None,
            reasoning_effort: None,
            collaboration_mode: None,
            images: Vec::new(),
        },
    )
    .await
    .unwrap();
}

async fn next_runtime_event(
    events: &mut broadcast::Receiver<ThreadEventEnvelope>,
    kind: &str,
) -> ThreadEventEnvelope {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.event_type == kind {
                return event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {kind} event"))
}

#[tokio::test]
async fn release_settings_default_and_persist_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    assert_eq!(
        s.harness_resource_settings().unwrap(),
        HarnessResourceSettingsDto::default()
    );
    let wanted = HarnessResourceSettingsDto {
        idle_release_minutes: None,
        max_loaded_threads: Some(3),
        max_harness_memory_bytes: Some(2 * 1024 * 1024 * 1024),
    };
    assert_eq!(
        s.update_harness_resource_settings(wanted.clone()).unwrap(),
        wanted
    );
    let error = s
        .update_harness_resource_settings(HarnessResourceSettingsDto {
            max_loaded_threads: Some(0),
            ..wanted.clone()
        })
        .unwrap_err();
    assert!(error.to_string().contains("maxLoadedThreads"));
    // The stored row, as a restarted Supervisor reads it.
    let raw = rusqlite::Connection::open(dir.path().join("db.sqlite")).unwrap();
    let row: (Option<u32>, Option<u32>, Option<i64>) = raw
        .query_row(
            "SELECT idle_release_minutes, max_loaded_threads, max_harness_memory_bytes FROM harness_resource_settings",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, (None, Some(3), Some(2 * 1024 * 1024 * 1024)));
    assert_eq!(s.harness_resource_settings().unwrap(), wanted);
}

#[tokio::test]
async fn release_explicitly_with_load_reasons_and_events() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    s.spawn_harness_release();
    let mut events = s.bus.subscribe();
    let id = thread(&s, dir.path(), None).await;
    let event = next_runtime_event(&mut events, "thread.runtime.loaded").await;
    assert_eq!(
        (event.thread_id.as_str(), &event.payload),
        (id.as_str(), &json!({"loadReason":"create"}))
    );
    assert_eq!(loaded(&s).await[0].load_reason, HarnessLoadReason::Create);

    let result = s.release_thread_harness(&id, false, None).await.unwrap();
    assert!(result.released && result.also_released.is_empty());
    let event = next_runtime_event(&mut events, "thread.runtime.released").await;
    assert_eq!(event.payload, json!({"reason":"explicit"}));
    assert!(loaded(&s).await.is_empty());
    assert!(
        !s.release_thread_harness(&id, false, None)
            .await
            .unwrap()
            .released
    );

    // A thread with history: capabilities without loading start nothing, and
    // the page load is a view.
    prompt(&s, &id, "hello").await;
    next_runtime_event(&mut events, "thread.runtime.loaded").await;
    s.release_thread_harness(&id, false, None).await.unwrap();
    s.thread_capabilities(&id, false).await.unwrap();
    assert!(loaded(&s).await.is_empty());
    s.thread_capabilities(&id, true).await.unwrap();
    assert_eq!(loaded(&s).await[0].load_reason, HarnessLoadReason::View);
    let event = next_runtime_event(&mut events, "thread.runtime.loaded").await;
    assert_eq!(event.payload, json!({"loadReason":"view"}));

    let processes = s.harness_processes().await.unwrap();
    assert_eq!(processes.threads.len(), 1);
    let listed = &processes.threads[0];
    assert_eq!(
        (listed.thread_id.as_str(), listed.title.as_str()),
        (id.as_str(), "Release")
    );
    assert_eq!(listed.load_reason, HarnessLoadReason::View);
    assert!(listed.busy.is_empty() && !listed.displayed);
    assert_eq!(processes.supervisor_pid, std::process::id());
    assert!(processes.supervisor_bytes.is_some());
    assert_eq!(processes.settings, HarnessResourceSettingsDto::default());
}

#[tokio::test]
async fn release_refuses_a_running_turn_unless_interrupting() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    let id = thread(&s, dir.path(), None).await;
    let running = s.clone();
    let turn_thread = id.clone();
    let turn = tokio::spawn(async move {
        running
            .prompt(
                &turn_thread,
                pockymoe_protocol::SendThreadPromptInput {
                    prompt: "inspect this repository slowly".into(),
                    client_request_id: None,
                    model: None,
                    reasoning_effort: None,
                    collaboration_mode: None,
                    images: Vec::new(),
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !s.live.lock().await.contains_key(&id) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (members, _, _) = s.harness_members(false, &HashSet::new()).await.unwrap();
    assert!(members[0].busy.contains(&Busy::Turn));
    let error = s
        .release_thread_harness(&id, false, None)
        .await
        .unwrap_err();
    assert!(error.is::<TurnRunning>(), "{error:#}");
    let result = s.release_thread_harness(&id, true, None).await.unwrap();
    assert!(result.released);
    assert!(loaded(&s).await.is_empty());
    let _ = turn.await;
}

#[tokio::test]
async fn release_busy_reasons_include_queued_input_but_not_held_steers() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    let id = thread(&s, dir.path(), None).await;
    let busy = |s: Arc<Supervisor>| async move {
        s.harness_members(false, &HashSet::new()).await.unwrap().0[0]
            .busy
            .clone()
    };
    assert!(busy(s.clone()).await.is_empty());
    for (delivery, expected) in [
        ("cli-steer", vec![]),
        ("continuation", vec![Busy::QueuedInput]),
    ] {
        s.db.with(|conn| {
            conn.execute("DELETE FROM thread_pending_steers", [])?;
            conn.execute(
                "INSERT INTO thread_pending_steers(id,thread_id,turn_id,display_prompt,submitted_prompt,delivery,created_at,updated_at)
                 VALUES('p1',?1,'','hi','hi',?2,'now','now')",
                params![id, delivery],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(busy(s.clone()).await, expected, "{delivery}");
    }
    let gate = s.hold_harness(&id).await;
    assert!(s.harness_members(false, &HashSet::new()).await.unwrap().0[0].pinned);
    drop(gate);
}

#[tokio::test]
async fn release_when_the_viewer_leaves_a_view_load() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    s.harness.view_delay_ms.store(100, Ordering::Relaxed);
    let id = thread(&s, dir.path(), None).await;
    prompt(&s, &id, "hello").await;
    s.release_thread_harness(&id, false, None).await.unwrap();
    s.spawn_harness_release();
    let mut events = s.bus.subscribe();
    let viewer = s.harness_viewer();
    viewer.set([id.clone()]);
    assert!(s.thread_displayed(&id));
    s.thread_capabilities(&id, true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(loaded(&s).await.len(), 1, "displayed threads stay loaded");
    drop(viewer);
    assert!(!s.thread_displayed(&id));
    let event = next_runtime_event(&mut events, "thread.runtime.released").await;
    assert_eq!(
        (event.thread_id.as_str(), &event.payload),
        (id.as_str(), &json!({"reason":"view"}))
    );
    assert!(loaded(&s).await.is_empty());

    // After a prompt the same thread waits for the idle rule instead.
    let viewer = s.harness_viewer();
    viewer.set([id.clone()]);
    s.thread_capabilities(&id, true).await.unwrap();
    s.prompt(
        &id,
        pockymoe_protocol::SendThreadPromptInput {
            prompt: "hello".into(),
            client_request_id: None,
            model: None,
            reasoning_effort: None,
            collaboration_mode: None,
            images: Vec::new(),
        },
    )
    .await
    .unwrap();
    drop(viewer);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(loaded(&s).await.len(), 1);
    assert!(loaded(&s).await[0].prompted);
}

#[tokio::test]
async fn release_over_the_thread_limit_after_a_load() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    s.update_harness_resource_settings(HarnessResourceSettingsDto {
        idle_release_minutes: Some(10),
        max_loaded_threads: Some(1),
        max_harness_memory_bytes: None,
    })
    .unwrap();
    s.harness.recent_load_ms.store(0, Ordering::Relaxed);
    s.spawn_harness_release();
    let mut events = s.bus.subscribe();
    let first = thread(&s, dir.path(), None).await;
    next_runtime_event(&mut events, "thread.runtime.loaded").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let second = thread(&s, dir.path(), None).await;
    let event = next_runtime_event(&mut events, "thread.runtime.released").await;
    assert_eq!(
        (event.thread_id.as_str(), &event.payload),
        (first.as_str(), &json!({"reason":"limit"}))
    );
    let remaining = loaded(&s).await;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].thread_id.as_deref(), Some(second.as_str()));
    // A displayed thread is not evicted; the limit is then exceeded.
    let viewer = s.harness_viewer();
    viewer.set([second.clone()]);
    // The released `first` was never prompted; its first prompt starts a session.
    prompt(&s, &first, "hello").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let processes = s.harness_processes().await.unwrap();
    assert_eq!(processes.threads.len(), 2);
    assert!(processes.over_thread_limit);
}

#[tokio::test]
async fn release_on_delete_and_close() {
    let dir = tempfile::tempdir().unwrap();
    let s = supervisor(dir.path());
    let root = thread(&s, dir.path(), None).await;
    let child = thread(&s, dir.path(), Some(&root)).await;
    let mut events = s.bus.subscribe();
    s.close_agent_thread_releasing(Some(&root), &child, false)
        .await
        .unwrap();
    let event = next_runtime_event(&mut events, "thread.runtime.released").await;
    assert_eq!(
        (event.thread_id.as_str(), &event.payload),
        (child.as_str(), &json!({"reason":"closed"}))
    );
    s.delete_thread_releasing(&root).await.unwrap();
    let event = next_runtime_event(&mut events, "thread.runtime.released").await;
    assert_eq!(
        (event.thread_id.as_str(), &event.payload),
        (root.as_str(), &json!({"reason":"deleted"}))
    );
    assert!(loaded(&s).await.is_empty());
}

#[test]
fn release_counts_running_native_subagents_and_unfinished_scans() {
    let agent = pockymoe_protocol::NativeSubagentDto {
        agent: pockymoe_protocol::ThreadSubagentDto {
            id: "worker".into(),
            name: None,
            status: "running".into(),
            started_at: None,
            completed_at: None,
            parent_tool_call_id: None,
            is_background: None,
        },
        provider: "codex".into(),
        native_session_id: None,
        model: None,
        prompt: None,
        updated_at: None,
        latest_activity: None,
        token_usage: None,
        price_estimate: None,
        activity_count: 0,
        details_available: false,
    };
    let running = serde_json::to_value(&agent).unwrap();
    assert!(native_activity(
        &json!({"agents":[running],"refreshing":false})
    ));
    let mut done = running.clone();
    done["status"] = json!("completed");
    assert!(!native_activity(
        &json!({"agents":[done.clone()],"refreshing":false})
    ));
    assert!(
        native_activity(&json!({"agents":[done],"refreshing":true})),
        "a large transcript still being read may hold running agents"
    );
}

#[test]
fn release_memory_limit_ignores_partly_rss_samples() {
    let entry = |pid, ppid, bytes| crate::harness_processes::ProcessEntry {
        pid,
        ppid: Some(ppid),
        start: Some(1),
        argv: vec!["node".into()],
        bytes: Some(bytes),
    };
    let mut sample = ProcessSample::from_entries(
        1,
        vec![
            entry(1, 0, 80),
            entry(10, 1, 300),
            entry(11, 10, 200),
            entry(20, 1, 50),
        ],
        pockymoe_protocol::ProcessMemoryMetric::Pss,
    );
    assert_eq!(limit_bytes(&sample, [10, 20, 10]), Some(550));
    sample.degraded = true;
    assert_eq!(limit_bytes(&sample, [10, 20]), None);
}

async fn busy_of(s: &Supervisor, thread: &str) -> Vec<Busy> {
    s.harness_members(false, &HashSet::new())
        .await
        .unwrap()
        .0
        .into_iter()
        .find(|member| member.thread_id.as_deref() == Some(thread))
        .unwrap()
        .busy
}

#[tokio::test]
async fn release_waits_for_claude_session_crons_and_wakeups() {
    let dir = tempfile::tempdir().unwrap();
    let (s, _) = supervisor_with(dir.path(), Provider::Claude);
    let id = thread_for(&s, dir.path(), None, Provider::Claude).await;
    assert!(busy_of(&s, &id).await.is_empty());
    let mut at = Utc::now();
    let record = |at: DateTime<Utc>, text: &str, input: Value, result: &str| {
        let item = json!({"id":format!("{text}-{}", at.timestamp_millis()),"createdAt":rfc3339(at),"kind":"toolCall","text":text,"status":"completed",
            "detailText":format!("Tool: {text}\n\nStatus: completed\n\nInput:\n{input}\n\nResult:\n{result}")});
        s.db.with(|c| {
            c.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,?2,'turn',?1,?3,?4,?4)",
                params![item["id"].as_str().unwrap(), id, item.to_string(), rfc3339(at)])?;
            Ok(())
        })
        .unwrap();
    };
    record(
        at,
        "CronCreate",
        json!({"cron":"*/5 * * * *","prompt":"Check the build","recurring":true}),
        "Scheduled recurring job job1 (*/5 * * * *). Auto-expires after 7 days.",
    );
    assert_eq!(
        busy_of(&s, &id).await,
        [Busy::BackgroundTask],
        "a /loop cron"
    );
    at += chrono::TimeDelta::seconds(1);
    record(
        at,
        "CronDelete",
        json!({"id":"job1"}),
        "Cancelled job job1.",
    );
    assert!(busy_of(&s, &id).await.is_empty(), "the cron was deleted");
    at += chrono::TimeDelta::seconds(1);
    record(
        at,
        "ScheduleWakeup",
        json!({"delaySeconds":600,"prompt":"continue","reason":"waiting for CI"}),
        "Wakeup scheduled.",
    );
    assert_eq!(
        busy_of(&s, &id).await,
        [Busy::BackgroundTask],
        "a pending wakeup"
    );
}

#[tokio::test]
async fn release_counts_a_scheduled_claude_turn_as_activity() {
    let dir = tempfile::tempdir().unwrap();
    let (s, fake) = supervisor_with(dir.path(), Provider::Claude);
    let id = thread_for(&s, dir.path(), None, Provider::Claude).await;
    let session = Uuid::new_v4().to_string();
    s.db.with(|c| {
        c.execute(
            "UPDATE threads SET provider_session_id=?1 WHERE id=?2",
            params![session, id],
        )?;
        Ok(())
    })
    .unwrap();
    fake.resume_session(&session, None, Default::default())
        .await
        .unwrap();
    let activity = |loaded: Vec<LoadedSession>| {
        loaded
            .into_iter()
            .find(|load| load.session_id == session)
            .unwrap()
            .last_activity_at
    };
    let before = activity(loaded(&s).await);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let at = rfc3339(Utc::now());
    let rows = [
        json!({"type":"user","uuid":"timer-1","turnOrigin":"scheduled","sessionId":session,"timestamp":at,"message":{"content":"Check the build"}}),
        json!({"type":"assistant","sessionId":session,"timestamp":at,"message":{"id":"reply","content":[{"type":"text","text":"Still green."}],"stop_reason":"end_turn"}}),
    ];
    let project = dir.path().join("claude-home/projects/workspace");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join(format!("{session}.jsonl")),
        rows.iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    s.sync_claude_scheduled_history(&id).await.unwrap();
    assert!(
        activity(loaded(&s).await) > before,
        "a timer turn between prompts restarts the idle clock"
    );
}

#[tokio::test]
async fn release_by_a_shared_caller_never_ends_a_thread_outside_its_access() {
    let dir = tempfile::tempdir().unwrap();
    let (s, fake) = supervisor_with(dir.path(), Provider::Codex);
    let parent = thread(&s, dir.path(), None).await;
    let fork = thread(&s, dir.path(), None).await;
    let session = |id: &str| s.get_thread(id).unwrap().provider_session_id.unwrap();
    fake.share_process(&session(&parent), &session(&fork));
    let only_parent = |thread: &str| thread == parent;
    let error = s
        .release_thread_harness(
            &parent,
            false,
            Some(&only_parent as &(dyn Fn(&str) -> bool + Send + Sync)),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().starts_with("conflict:"), "{error:#}");
    assert_eq!(loaded(&s).await.len(), 2);
    let both = |thread: &str| thread == parent || thread == fork;
    let result = s
        .release_thread_harness(
            &parent,
            false,
            Some(&both as &(dyn Fn(&str) -> bool + Send + Sync)),
        )
        .await
        .unwrap();
    assert!(result.released);
    assert_eq!(result.also_released, std::slice::from_ref(&fork));
    assert!(loaded(&s).await.is_empty());
}

#[tokio::test]
async fn release_forgets_an_unsaved_first_session_and_the_first_prompt_starts_one() {
    let dir = tempfile::tempdir().unwrap();
    let (s, fake) = supervisor_with(dir.path(), Provider::Codex);
    let id = thread(&s, dir.path(), None).await;
    s.update_settings(
        &id,
        Some("gpt-5.4".into()),
        Some("high".into()),
        None,
        Some("plan".into()),
        Some("workspace-write".into()),
    )
    .await
    .unwrap();
    let original = s.get_thread(&id).unwrap().provider_session_id.unwrap();
    assert!(
        s.release_thread_harness(&id, false, None)
            .await
            .unwrap()
            .released
    );
    // Claude and Codex save a session with its first message: this one is gone.
    assert_eq!(s.get_thread(&id).unwrap().provider_session_id, None);
    let caps = s.thread_capabilities(&id, true).await.unwrap();
    assert!(caps.effective_capabilities.is_some());
    assert!(loaded(&s).await.is_empty(), "capabilities start no process");
    assert_eq!(
        s.thread_goal(&id, None, None, None, false).await.unwrap()["goal"],
        Value::Null
    );

    prompt(&s, &id, "hello").await;
    let thread = s.get_thread(&id).unwrap();
    let session = thread.provider_session_id.clone().unwrap();
    assert_ne!(session, original);
    let detail = s.get_thread_detail(&id, None).await.unwrap();
    assert_eq!(detail.turns.len(), 1);
    assert_eq!(detail.turns[0].status, "completed");
    assert!(thread.last_error.is_none());
    let start = fake.started_sessions().pop().unwrap();
    assert_eq!(
        (
            start.model.as_str(),
            start.reasoning_effort.as_deref(),
            start.approval_mode.as_str(),
            start.sandbox_mode.as_deref()
        ),
        ("gpt-5.4", Some("high"), "guarded", Some("workspace-write")),
        "the new session keeps the thread's saved settings"
    );
    let load = loaded(&s).await.into_iter().next().unwrap();
    assert_eq!(load.load_reason, HarnessLoadReason::Create);
    assert!(load.prompted);
    // Now it has history: a release keeps the session.
    s.release_thread_harness(&id, false, None).await.unwrap();
    assert_eq!(
        s.get_thread(&id).unwrap().provider_session_id,
        Some(session)
    );
}

#[tokio::test]
async fn release_keeps_sessions_of_forks_imports_and_threads_with_history() {
    let dir = tempfile::tempdir().unwrap();
    let (s, _) = supervisor_with(dir.path(), Provider::Claude);
    let session = |id: &str| s.get_thread(id).unwrap().provider_session_id;
    // A fork of an empty thread has no turns but a session made by session/fork.
    let source = thread_for(&s, dir.path(), None, Provider::Claude).await;
    let fork = s.fork_thread(&source).await.unwrap();
    s.release_thread_harness(&source, false, None)
        .await
        .unwrap();
    let forked = session(&fork.id);
    assert!(forked.is_some());
    s.thread_capabilities(&fork.id, true).await.unwrap();
    s.release_thread_harness(&fork.id, false, None)
        .await
        .unwrap();
    assert_eq!(session(&fork.id), forked, "forks keep their session");
    // An import, even before any turn is copied.
    let imported = thread_for(&s, dir.path(), None, Provider::Claude).await;
    s.db.with(|c| {
        c.execute(
            "UPDATE threads SET source='local_provider_import' WHERE id=?1",
            [&imported],
        )?;
        Ok(())
    })
    .unwrap();
    let kept = session(&imported);
    s.release_thread_harness(&imported, false, None)
        .await
        .unwrap();
    assert_eq!(session(&imported), kept);
    // Any turn, even a failed one, means the session was written.
    let with_turn = thread_for(&s, dir.path(), None, Provider::Claude).await;
    s.db.with(|c| {
        c.execute(
            "INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES('t1',?1,'failed',1)",
            [&with_turn],
        )?;
        Ok(())
    })
    .unwrap();
    let kept = session(&with_turn);
    s.release_thread_harness(&with_turn, false, None)
        .await
        .unwrap();
    assert_eq!(session(&with_turn), kept);
}
