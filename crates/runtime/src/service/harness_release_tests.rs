use super::*;
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
    let config = config(dir);
    let db = Database::open(&config.database_url).unwrap();
    let s = Arc::new(
        Supervisor::new(
            config,
            db,
            vec![Arc::new(FakeRuntime::new(Provider::Codex))],
        )
        .with_local_session_homes(LocalSessionHomes {
            codex_home: dir.join("codex-home"),
            grok_home: dir.join("grok-home"),
            claude_home: dir.join("claude-home"),
        }),
    );
    s.spawn_live_item_persister();
    s
}

async fn thread(s: &Supervisor, dir: &Path, parent: Option<&str>) -> String {
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
        provider: Some(Provider::Codex),
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
    s.runtime(Provider::Codex).unwrap().loaded_sessions().await
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

    let result = s.release_thread_harness(&id, false).await.unwrap();
    assert!(result.released && result.also_released.is_empty());
    let event = next_runtime_event(&mut events, "thread.runtime.released").await;
    assert_eq!(event.payload, json!({"reason":"explicit"}));
    assert!(loaded(&s).await.is_empty());
    assert!(!s.release_thread_harness(&id, false).await.unwrap().released);

    // Capabilities without loading start nothing; the page load is a view.
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
    let error = s.release_thread_harness(&id, false).await.unwrap_err();
    assert!(error.is::<TurnRunning>(), "{error:#}");
    let result = s.release_thread_harness(&id, true).await.unwrap();
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
    s.release_thread_harness(&id, false).await.unwrap();
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
    s.thread_capabilities(&first, true).await.unwrap();
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
