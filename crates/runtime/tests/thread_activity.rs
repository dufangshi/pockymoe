//! Thread map activity: turn counts and running time inside a window.
use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Provider, ThreadActivityDto};
use pockymoe_runtime::{fake::FakeRuntime, Database, RuntimeConfig, Supervisor};
use std::sync::Arc;

fn setup() -> (tempfile::TempDir, Arc<Supervisor>) {
    let dir = tempfile::tempdir().unwrap();
    let config = RuntimeConfig {
        treer_enabled: false,
        host_mode: Default::default(),
        mode: pockymoe_protocol::Mode::Local,
        host: "127.0.0.1".into(),
        port: 0,
        workspace_root: dir.path().into(),
        database_url: dir.path().join("db.sqlite"),
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
        enabled_providers: vec![Provider::Claude],
        acp_command: None,
        acp_startup_timeout_ms: 1000,
        fake_runtime: true,
        supervisor_restart_strategy: Default::default(),
    };
    let db = Database::open(&config.database_url).unwrap();
    let state = Arc::new(Supervisor::new(
        config,
        db,
        vec![Arc::new(FakeRuntime::new(Provider::Claude))],
    ));
    (dir, state)
}

fn workspace(s: &Supervisor) -> String {
    s.list_workspaces()
        .unwrap()
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            s.create_workspace(CreateWorkspaceInput {
                abs_path: Some(s.config.workspace_root.to_string_lossy().into()),
                git_url: None,
                label: Some("test".into()),
            })
            .unwrap()
        })
        .id
}

async fn spawn(s: &Supervisor, ws: &str, parent: Option<&str>) -> pockymoe_protocol::ThreadDto {
    s.create_thread(CreateThreadInput {
        workspace_id: ws.to_string(),
        title: None,
        provider: Some(Provider::Claude),
        agent_id: None,
        model: "ios-e2e-stream".into(),
        reasoning_effort: None,
        approval_mode: "yolo".into(),
        parent_thread_id: parent.map(str::to_owned),
    })
    .await
    .unwrap()
}

fn turn(
    s: &Supervisor,
    thread: &str,
    id: &str,
    status: &str,
    started: Option<&str>,
    completed: Option<&str>,
) {
    s.db.with(|conn| {
        conn.execute(
            "INSERT INTO thread_turns(id,thread_id,status,started_at,completed_at,ordinal) VALUES(?1,?2,?3,?4,?5,1)",
            (id, thread, status, started, completed),
        )?;
        Ok(())
    })
    .unwrap();
}

fn prompt(s: &Supervisor, turn: &str, ordinal: i64, text: &str) {
    s.db.with(|conn| {
        conn.execute(
            "UPDATE thread_turns SET ordinal=?2, display_prompt=?3 WHERE id=?1",
            (turn, ordinal, text),
        )?;
        Ok(())
    })
    .unwrap();
}

fn set_status(s: &Supervisor, thread: &str, status: &str) {
    s.db.with(|conn| {
        conn.execute("UPDATE threads SET status=?2 WHERE id=?1", (thread, status))?;
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
async fn counts_turns_and_running_time_inside_the_window() {
    let (_dir, s) = setup();
    let ws = workspace(&s);
    let busy = spawn(&s, &ws, None).await;
    let crashed = spawn(&s, &ws, None).await;
    let quiet = spawn(&s, &ws, None).await;
    let (since, now) = ("2030-01-02T00:00:00.000Z", "2030-01-03T00:00:00.000Z");
    // Before the window, straddling its start, inside it, and still running.
    turn(
        &s,
        &busy.id,
        "b1",
        "completed",
        Some("2030-01-01T10:00:00.000Z"),
        Some("2030-01-01T11:00:00.000Z"),
    );
    turn(
        &s,
        &busy.id,
        "b2",
        "completed",
        Some("2030-01-01T23:50:00.000Z"),
        Some("2030-01-02T00:10:00.000Z"),
    );
    turn(
        &s,
        &busy.id,
        "b3",
        "completed",
        Some("2030-01-02T12:00:00.000Z"),
        Some("2030-01-02T12:30:00.000Z"),
    );
    turn(
        &s,
        &busy.id,
        "b4",
        "inProgress",
        Some("2030-01-02T23:00:00.000Z"),
        None,
    );
    // Imported history can lack completion times; those turns count, not their time.
    turn(
        &s,
        &busy.id,
        "b5",
        "completed",
        Some("2030-01-02T02:00:00.000Z"),
        None,
    );
    set_status(&s, &busy.id, "running");
    // A turn left open by a crash, on a thread that is not running, adds no time.
    turn(
        &s,
        &crashed.id,
        "c1",
        "inProgress",
        Some("2030-01-02T01:00:00.000Z"),
        None,
    );
    set_status(&s, &crashed.id, "idle");
    // The latest prompt is the newest one that has text.
    prompt(&s, "b2", 2, "older");
    prompt(&s, "b3", 3, "  Fix   the\n login  bug ");
    prompt(&s, "b4", 4, " ");
    let mut activity = s.thread_activity(since, now, 24).unwrap();
    activity.sort_by_key(|a| if a.thread_id == busy.id { 0 } else { 1 });
    assert_eq!(
        activity,
        vec![
            ThreadActivityDto {
                thread_id: busy.id.clone(),
                turns: 5,
                active_ms: (10 + 30 + 60) * 60_000,
                total_ms: (60 + 20 + 30 + 60) * 60_000,
                hourly_ms: (0..24)
                    .map(|hour| match hour {
                        0 => 10 * 60_000,
                        12 => 30 * 60_000,
                        23 => 60 * 60_000,
                        _ => 0,
                    })
                    .collect(),
                last_prompt: Some("Fix the login bug".into()),
            },
            ThreadActivityDto {
                thread_id: crashed.id.clone(),
                turns: 1,
                active_ms: 0,
                total_ms: 0,
                hourly_ms: Vec::new(),
                last_prompt: None,
            },
        ]
    );
    assert!(activity.iter().all(|a| a.thread_id != quiet.id));
}
