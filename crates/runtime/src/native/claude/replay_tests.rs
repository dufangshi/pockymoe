//! Mapping tests against real Claude Code 2.1.322 sessions recorded with
//! `haiku` (`fixtures/*.jsonl`, sanitized) and replayed by `fixtures/replay.py`
//! through the whole native runtime: transport, pump, callbacks, completion and
//! mapping. Set `NATIVE_CLAUDE_DUMP=<dir>` to write each turn's items and events.
use super::*;
use crate::fake::FakeRuntime;
use tempfile::TempDir;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/native/claude/fixtures");

pub(super) struct Replay {
    pub dir: TempDir,
    pub runtime: Arc<ClaudeRuntime>,
    pub session: String,
}

pub(super) async fn replay(fixture: &str, approval: &str) -> Replay {
    let dir = TempDir::new().unwrap();
    let log = dir.path().join("inputs.jsonl");
    let command = shell_words::join([
        "env",
        &format!("REPLAY_LOG={}", log.display()),
        "python3",
        &format!("{FIXTURES}/replay.py"),
        &format!("{FIXTURES}/{fixture}.jsonl"),
    ]);
    let runtime = Arc::new(ClaudeRuntime::new(
        Arc::new(FakeRuntime::new(Provider::Acp)),
        command,
        None,
    ));
    let session = runtime
        .start_session(StartSessionInput {
            cwd: dir.path().to_str().unwrap().into(),
            agent_id: Some("claude".into()),
            model: "haiku".into(),
            reasoning_effort: None,
            approval_mode: approval.into(),
            sandbox_mode: Some("workspace-write".into()),
        })
        .await
        .unwrap();
    Replay {
        dir,
        runtime,
        session: session.provider_session_id,
    }
}

pub(super) fn input(session: &str, turn: &str, prompt: &str) -> StartTurnInput {
    StartTurnInput {
        provider_session_id: session.into(),
        prompt: prompt.into(),
        model: None,
        reasoning_effort: None,
        sandbox_mode: None,
        collaboration_mode: None,
        approval_mode: None,
        performance_mode: None,
        thread_id: "thread".into(),
        turn_id: turn.into(),
        hidden: false,
        images: vec![],
        title: None,
        context_delivered: true,
    }
}

pub(super) struct Turn {
    pub result: Result<Vec<ThreadHistoryItemDto>>,
    pub events: Vec<ThreadEventEnvelope>,
}

impl Turn {
    pub fn items(&self) -> &[ThreadHistoryItemDto] {
        self.result.as_ref().unwrap()
    }
    pub fn of(&self, kind: &str) -> Vec<&ThreadHistoryItemDto> {
        self.items().iter().filter(|i| i.kind == kind).collect()
    }
    pub fn events(&self, kind: &str) -> Vec<&ThreadEventEnvelope> {
        self.events
            .iter()
            .filter(|e| e.event_type == kind)
            .collect()
    }
    pub fn text(&self) -> String {
        self.of("agentMessage")
            .iter()
            .map(|i| i.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Runs one turn; `react` sees every event and may answer requests or steer.
pub(super) async fn run<F, Fut>(r: &Replay, name: &str, prompt: &str, react: F) -> Turn
where
    F: Fn(Arc<ClaudeRuntime>, String, ThreadEventEnvelope) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    run_with(
        r,
        input(&r.session, name, prompt),
        CancellationToken::new(),
        react,
    )
    .await
}

pub(super) async fn run_with<F, Fut>(
    r: &Replay,
    input: StartTurnInput,
    cancel: CancellationToken,
    react: F,
) -> Turn
where
    F: Fn(Arc<ClaudeRuntime>, String, ThreadEventEnvelope) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let turn_name = input.turn_id.clone();
    let runtime = r.runtime.clone();
    let session = r.session.clone();
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Ok(event) = rx.recv().await {
            react(runtime.clone(), session.clone(), event.clone()).await;
            let done = event.event_type == "thread.turn.completed";
            events.push(event);
            if done {
                break;
            }
        }
        events
    });
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        r.runtime.start_turn(input, bus.clone(), cancel),
    )
    .await
    .expect("replayed turn finished");
    let events = tokio::time::timeout(Duration::from_secs(5), collector)
        .await
        .unwrap()
        .unwrap();
    if let Ok(dir) = std::env::var("NATIVE_CLAUDE_DUMP") {
        let name = &turn_name;
        let _ = std::fs::create_dir_all(&dir);
        let dump = json!({
            "items": result.as_ref().ok(),
            "error": result.as_ref().err().map(|e| e.to_string()),
            "events": events,
        });
        std::fs::write(
            format!("{dir}/{name}.json"),
            serde_json::to_string_pretty(&dump).unwrap(),
        )
        .unwrap();
    }
    Turn { result, events }
}

async fn ignore(_: Arc<ClaudeRuntime>, _: String, _: ThreadEventEnvelope) {}

impl Replay {
    /// What the runtime wrote to Claude's stdin.
    fn inputs(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join("inputs.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
    fn controls(&self, subtype: &str) -> Vec<Value> {
        self.inputs()
            .into_iter()
            .filter(|v| v["type"] == "control_request" && v["request"]["subtype"] == subtype)
            .map(|v| v["request"].clone())
            .collect()
    }
    fn answers(&self) -> Vec<Value> {
        self.inputs()
            .into_iter()
            .filter(|v| v["type"] == "control_response")
            .map(|v| v["response"]["response"].clone())
            .filter(|a| a.get("behavior").is_some())
            .collect()
    }
}

/// Answers every permission request with "allow".
async fn allow(runtime: Arc<ClaudeRuntime>, _: String, event: ThreadEventEnvelope) {
    if event.event_type == "thread.request.created" {
        let id = event.payload["request"]["id"].as_str().unwrap().to_owned();
        runtime.respond_permission(&id, true, None).await.unwrap();
    }
}

#[tokio::test]
async fn replay_tools_render_like_acp_with_exact_diffs_and_exit_codes() {
    let r = replay("basic-tools", "guarded").await;
    let t = run(&r, "basic-tools", "tools", allow).await;
    let bash = t.of("commandExecution")[0];
    assert_eq!(bash.text, "echo hi-from-bash");
    let detail = bash.detail_text.as_deref().unwrap();
    // Plain output, never a JSON-quoted string.
    assert!(detail.ends_with("Output:\nhi-from-bash"), "{detail}");
    let read = t.of("fileRead")[0];
    assert_eq!(read.text, "/work/notes.txt:1");
    let edits = t.of("fileChange");
    assert_eq!(edits.len(), 2);
    // The whole-file diff from Claude's structured patch, with real line numbers.
    let edit = edits[0].detail_text.as_deref().unwrap();
    assert!(
        edit.contains("@@ -1,2 +1,2 @@\n alpha\n-beta\n+gamma"),
        "{edit}"
    );
    assert_eq!(edits[0].extra["addedLines"], 1);
    assert_eq!(edits[1].extra["removedLines"], 0);
    assert!(t.text().starts_with("DONE"));
    let message = t.of("agentMessage")[0];
    assert_eq!(
        message.extra["nativeMessageId"].as_str().unwrap().len(),
        36,
        "fork needs the UUID"
    );
    // Each request offered Claude's own "allow edits this session" choice.
    let request = &t.events("thread.request.created")[0].payload["request"];
    let labels: Vec<_> = request["questions"][0]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        ["Yes", "Yes, allow all edits during this session", "No"]
    );
    // Outside the workspace the path stays absolute; inside it is relative.
    assert_eq!(request["title"], "Edit /work/notes.txt");
    assert_eq!(request["questions"][0]["question"], "Edit /work/notes.txt");
    let answers = r.answers();
    assert!(
        answers
            .iter()
            .all(|a| a["behavior"] == "allow" && a["toolUseID"].is_string()),
        "{answers:?}"
    );
    let context = t.events("thread.context.updated");
    assert_eq!(
        context.last().unwrap().payload["contextUsage"]["modelContextWindow"],
        1_000_000
    );
}

#[tokio::test]
async fn replay_reject_denies_and_always_allow_sends_claudes_rule() {
    let r = replay("guarded", "guarded").await;
    let t = run(
        &r,
        "guarded",
        "guarded",
        |runtime: Arc<ClaudeRuntime>, _: String, event: ThreadEventEnvelope| async move {
            if event.event_type == "thread.request.created" {
                let id = event.payload["request"]["id"].as_str().unwrap().to_owned();
                // The UI's envelope; the HTTP API defaults allow to true.
                let answer = json!({"permission":{"answers":["No"]}}).to_string();
                runtime
                    .respond_permission(&id, true, Some(&answer))
                    .await
                    .unwrap();
            }
        },
    )
    .await;
    assert!(t.result.is_ok());
    let answers = r.answers();
    let denial = answers
        .iter()
        .find(|a| a.get("behavior").is_some())
        .unwrap();
    assert_eq!(denial["behavior"], "deny");
    let request = &t.events("thread.request.created")[0].payload["request"];
    assert_eq!(request["questions"][0]["options"][1]["label"], "Yes, and don't ask again for `python3 -c \"open(\\\"guard.txt\\\",\\\"w\\\").write(\\\"x\\\")\"` commands");
    assert_eq!(
        request["description"],
        "Run the requested python one-liner to write guard.txt · This command requires approval"
    );

    let r = replay("guarded", "guarded").await;
    run(
        &r,
        "always",
        "guarded",
        |runtime: Arc<ClaudeRuntime>, _: String, event: ThreadEventEnvelope| async move {
            if event.event_type == "thread.request.created" {
                let request = &event.payload["request"];
                let always = request["questions"][0]["options"][1]["label"]
                    .as_str()
                    .unwrap();
                let answer = json!({"permission":{"answers":[always]}}).to_string();
                runtime
                    .respond_permission(request["id"].as_str().unwrap(), true, Some(&answer))
                    .await
                    .unwrap();
            }
        },
    )
    .await;
    let allow = r
        .answers()
        .into_iter()
        .find(|a| a["behavior"] == "allow")
        .unwrap();
    assert_eq!(
        allow["updatedPermissions"][0]["destination"],
        "localSettings"
    );
    assert_eq!(
        allow["updatedPermissions"][0]["rules"][0]["toolName"],
        "Bash"
    );
}

#[tokio::test]
async fn replay_yolo_auto_approves_and_guarded_runs_in_auto_mode() {
    let r = replay("guarded", "yolo").await;
    let t = run(&r, "yolo", "guarded", ignore).await;
    assert!(t.events("thread.request.created").is_empty());
    assert_eq!(
        r.answers()
            .into_iter()
            .find(|a| a.get("behavior").is_some())
            .unwrap()["behavior"],
        "allow"
    );
    // Guarded workspace work uses Claude's auto mode, as the ACP adapter does.
    let r = replay("guarded", "guarded").await;
    assert_eq!(r.controls("set_permission_mode")[0]["mode"], "auto");
}

#[tokio::test]
async fn replay_background_agent_holds_the_turn_until_its_follow_up() {
    let r = replay("subagent", "guarded").await;
    let t = run(&r, "subagent", "agent", allow).await;
    // The child's own text stays in its result; only the parent speaks.
    let replies: Vec<_> = t
        .of("agentMessage")
        .iter()
        .map(|i| i.text.clone())
        .collect();
    assert_eq!(replies.len(), 2, "{replies:?}");
    assert!(replies[1].contains("CHILD42"));
    assert!(t.of("agentToolCall")[0].status.as_deref() == Some("completed"));
    // The child's command is in the transcript, attributed by its row.
    assert_eq!(t.of("commandExecution")[0].text, "echo CHILD42");
    let lists: Vec<_> = t
        .events("thread.subagents.updated")
        .iter()
        .map(|e| e.payload["activeSubagents"].clone())
        .collect();
    // The backgrounded agent stays listed after its launch result, then ends.
    assert!(lists
        .iter()
        .any(|l| l[0]["isBackground"] == true && l[0]["name"] == "Run echo CHILD42"));
    assert_eq!(lists.last().unwrap(), &json!([]));
    assert_eq!(t.of("generic")[0].extra["origin"], "nativeTaskNotification");
    let last = t.of("agentMessage");
    assert_eq!(last.last().unwrap().extra["responsePhase"], "final");
    assert_eq!(last[0].extra["responsePhase"], "commentary");
}

#[tokio::test]
async fn replay_background_shell_waits_then_wakes_in_the_same_turn() {
    let r = replay("bgbash", "guarded").await;
    let t = run(&r, "bgbash", "bg", allow).await;
    let replies: Vec<_> = t
        .of("agentMessage")
        .iter()
        .map(|i| i.text.clone())
        .collect();
    assert_eq!(replies, ["STARTED", "BGDONE"]);
    let wake = t.of("generic");
    assert_eq!(wake[0].extra["origin"], "nativeTaskNotification");
    assert_eq!(
        wake[0].extra["backgroundTaskCount"], 1,
        "the waiting row became the wake"
    );
    assert!(r.runtime.loaded_sessions().await[0].busy.is_empty());
}

#[tokio::test]
async fn replay_stop_interrupts_and_keeps_the_process() {
    let r = replay("interrupt", "guarded").await;
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let t = run_with(
        &r,
        input(&r.session, "interrupt", "sleep"),
        cancel,
        move |_: Arc<ClaudeRuntime>, _: String, event: ThreadEventEnvelope| {
            let stop = stop.clone();
            async move {
                if event.event_type == "thread.item.started"
                    && event.payload["item"]["kind"] == "commandExecution"
                {
                    stop.cancel();
                }
            }
        },
    )
    .await;
    assert!(t.result.is_ok());
    assert_eq!(
        t.events("thread.turn.completed")[0].payload["status"],
        "interrupted"
    );
    assert_eq!(
        t.of("commandExecution")[0].status.as_deref(),
        Some("interrupted")
    );
    assert_eq!(r.controls("interrupt").len(), 1);
    assert_eq!(
        r.runtime.execution_state(&r.session).await,
        ExecutionState::Idle
    );
}

#[tokio::test]
async fn replay_steering_joins_the_running_turn() {
    let r = replay("steer", "guarded").await;
    let t = run(
        &r,
        "steer",
        "steer",
        |runtime: Arc<ClaudeRuntime>, session: String, event: ThreadEventEnvelope| async move {
            if event.event_type == "thread.item.started"
                && event.payload["item"]["kind"] == "commandExecution"
            {
                runtime
                    .send_input(&session, "steer", "Also include STEERED42")
                    .await
                    .unwrap();
            }
        },
    )
    .await;
    assert!(t.text().contains("STEERED42"));
    let prompts: Vec<_> = r
        .inputs()
        .into_iter()
        .filter(|v| v["type"] == "user")
        .collect();
    assert_eq!(prompts.len(), 2);
}

#[tokio::test]
async fn replay_api_errors_fail_with_claudes_message_and_keep_the_process() {
    let r = replay("badmodel", "guarded").await;
    let t = run(&r, "bad", "hi", ignore).await;
    let error = t.result.as_ref().unwrap_err().to_string();
    assert!(error.contains("issue with the selected model"), "{error}");
    assert_eq!(
        t.events("thread.turn.completed")[0].payload["status"],
        "failed"
    );
    assert_eq!(
        r.runtime.execution_state(&r.session).await,
        ExecutionState::Idle
    );
}

#[tokio::test]
async fn replay_compaction_is_a_compaction_item_with_its_summary() {
    let r = replay("compact", "guarded").await;
    run(&r, "hello", "hello", ignore).await;
    let t = run(&r, "compact", "/compact", ignore).await;
    let compaction = t.of("contextCompaction");
    assert_eq!(compaction.len(), 1, "{:?}", t.items());
    assert!(
        compaction[0].text.contains("greeting"),
        "{}",
        compaction[0].text
    );
    let used = t.events("thread.context.updated").last().unwrap().payload["contextUsage"]
        ["tokensInContextWindow"]
        .clone();
    assert_eq!(used, 8352, "the context meter drops to the retained size");
}

#[tokio::test]
async fn replay_questions_and_plan_approval() {
    let r = replay("ask", "guarded").await;
    let t = run(
        &r,
        "ask",
        "ask",
        |runtime: Arc<ClaudeRuntime>, _: String, event: ThreadEventEnvelope| async move {
            if event.event_type == "thread.request.created" {
                let request = &event.payload["request"];
                assert_eq!(request["kind"], "requestUserInput");
                let answer = json!({"question_0":{"answers":["Red"]}}).to_string();
                runtime
                    .respond_permission(request["id"].as_str().unwrap(), true, Some(&answer))
                    .await
                    .unwrap();
            }
        },
    )
    .await;
    assert!(t.text().contains("Red"));
    let answered = r
        .answers()
        .into_iter()
        .find(|a| a["behavior"] == "allow")
        .unwrap();
    assert_eq!(
        answered["updatedInput"]["answers"]["Which color do you prefer?"],
        "Red"
    );

    let r = replay("plan", "guarded").await;
    let mut plan = input(&r.session, "plan", "plan");
    plan.collaboration_mode = Some("plan".into());
    let t = run_with(&r, plan, CancellationToken::new(), allow).await;
    let requests = t.events("thread.request.created");
    let exit = requests
        .iter()
        .find(|e| e.payload["request"]["title"] == "Approve Plan")
        .unwrap();
    let labels: Vec<_> = exit.payload["request"]["questions"][0]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["label"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        labels,
        [
            "Yes, manually approve edits",
            "Yes, and use auto mode",
            "Yes, and auto-accept edits",
            "No, keep planning"
        ]
    );
    assert_eq!(
        r.controls("set_permission_mode").last().unwrap()["mode"],
        "plan"
    );
    let approved = t
        .items()
        .iter()
        .find(|i| i.text == "Exited Plan Mode")
        .expect("plan row");
    assert!(approved
        .detail_text
        .as_deref()
        .unwrap()
        .contains("# Plan: Add a README"));
    // The plan approval moved Claude to default; the next turn re-asserts plan.
    assert_eq!(
        *r.runtime
            .session(&r.session)
            .await
            .unwrap()
            .mode
            .lock()
            .unwrap(),
        "default"
    );
}

#[tokio::test]
async fn replay_images_and_read_results() {
    let r = replay("image", "guarded").await;
    let mut prompt = input(&r.session, "image", "What color?");
    prompt.images = vec![];
    let t = run_with(&r, prompt, CancellationToken::new(), ignore).await;
    assert_eq!(t.text(), "Red");
    let r = replay("search", "guarded").await;
    let t = run(&r, "search", "search", allow).await;
    let image = t.of("fileRead")[0];
    assert!(
        image.detail_text.as_deref().unwrap().contains("image"),
        "{:?}",
        image.detail_text
    );
    assert_eq!(
        t.of("toolCall")
            .iter()
            .filter(|i| i.status.as_deref() == Some("failed"))
            .count(),
        2
    );
}

#[tokio::test]
async fn replay_foreground_agent_completes_in_its_turn() {
    let r = replay("fgagent", "guarded").await;
    let t = run(&r, "fg", "fg", allow).await;
    assert_eq!(t.text(), "The subagent replied with **KID7**.");
    let agent = t.of("agentToolCall")[0];
    assert!(agent.detail_text.as_deref().unwrap().contains("KID7"));
}

#[tokio::test]
async fn replay_capabilities_toolbox_and_models_follow_claude() {
    let r = replay("basic-tools", "guarded").await;
    let caps = r
        .runtime
        .session_negotiated_caps(Some("claude"), &r.session);
    // What ACP's ClaudeAdapter advertises, plus input during a turn and fork at a reply.
    assert!(caps.usage.cost_usd && caps.usage.token_usage && caps.usage.context_window);
    assert!(caps.controls.permission_requests && caps.controls.live_model_switch);
    assert!(caps.turns.steer && caps.turns.stream_input && caps.turns.compact);
    assert!(caps.branching.fork && caps.branching.fork_at && caps.sessions.import_local);
    // The fixture's account offers fast mode on Opus.
    assert!(caps.controls.performance_mode);
    let snapshot = r
        .runtime
        .session_capabilities(Some("claude"), &r.session)
        .await
        .unwrap();
    assert!(snapshot
        .toolbox_items
        .iter()
        .any(|i| i.command == "/review" && i.action == "prompt"));
    assert!(snapshot
        .toolbox_items
        .iter()
        .any(|i| i.command == "/compact"));
    let models = r.runtime.list_models(Some("claude"), None).await.unwrap();
    let haiku = models.iter().find(|m| m.id == "haiku").unwrap();
    assert!(haiku
        .supported_reasoning_efforts
        .iter()
        .any(|e| e.reasoning_effort == "high"));
}

#[tokio::test]
async fn replay_mcp_elicitation_is_a_question_answered_as_an_mcp_result() {
    let r = replay("elicitation", "guarded").await;
    let t = run(
        &r,
        "elicit",
        "ask",
        |runtime: Arc<ClaudeRuntime>, _: String, event: ThreadEventEnvelope| async move {
            if event.event_type == "thread.request.created" {
                let request = &event.payload["request"];
                assert_eq!(request["kind"], "requestUserInput");
                assert_eq!(request["title"], "tickets");
                assert_eq!(request["questions"][0]["options"][1]["label"], "high");
                let answer = json!({"priority":{"answers":["high"]}}).to_string();
                runtime
                    .respond_permission(request["id"].as_str().unwrap(), true, Some(&answer))
                    .await
                    .unwrap();
            }
        },
    )
    .await;
    assert_eq!(t.text(), "Filed as high.");
    let reply = r
        .inputs()
        .into_iter()
        .find(|v| v["type"] == "control_response" && v["response"]["request_id"] == "e1")
        .unwrap();
    assert_eq!(
        reply["response"]["response"],
        json!({"action":"accept","content":{"priority":"high"}})
    );
}

#[test]
fn mcp_status_becomes_the_panel_list_without_configs() {
    let status = json!({"mcpServers":[
        {"name":"docs","status":"connected","config":{"type":"claudeai-proxy","url":"https://secret.example/mcp","id":"x"},
         "scope":"claudeai","tools":[{"name":"read","annotations":{"readOnly":true,"title":"Read doc"}}]},
        {"name":"tickets","status":"failed","error":"spawn ENOENT","config":{"type":"stdio","command":"/opt/tickets","env":{"TOKEN":"t"}}},
        {"name":"calendar","status":"needs-auth","config":{"type":"http","url":"https://c.example","headers":{"Authorization":"Bearer z"}}}]});
    let list = super::mcp_servers(&status);
    let servers = list["servers"].as_array().unwrap();
    assert_eq!(servers[0]["authStatus"], "oAuth");
    assert_eq!(servers[0]["status"], "connected");
    assert_eq!(servers[0]["tools"][0]["title"], "Read doc");
    assert_eq!(servers[1]["status"], "failed");
    assert_eq!(servers[1]["error"], "spawn ENOENT");
    assert_eq!(servers[2]["authStatus"], "notLoggedIn");
    assert_eq!(servers[2]["status"], "needsAuth");
    let text = list.to_string();
    for secret in ["secret.example", "/opt/tickets", "TOKEN", "Bearer"] {
        assert!(!text.contains(secret), "{secret} leaked");
    }
}
