use super::*;
use crate::fake::FakeRuntime;
use tempfile::TempDir;

fn runtime(command: String) -> Arc<ClaudeRuntime> {
    Arc::new(ClaudeRuntime::new(
        Arc::new(FakeRuntime::new(Provider::Acp)),
        command,
        None,
    ))
}
fn fake() -> (TempDir, Arc<ClaudeRuntime>) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cli.py");
    std::fs::write(&path,r#"
import sys,json,os
with open(os.path.join(os.path.dirname(__file__),'argv.jsonl'),'a') as f: f.write(json.dumps(sys.argv[1:])+'\n')
def emit(v): print(json.dumps(v),flush=True)
def result(): emit({'type':'result','subtype':'success','is_error':False})
def assistant(text): emit({'type':'assistant','uuid':'native-uuid','message':{'id':'api-message','model':'fixture','content':[{'type':'text','text':text}],'usage':{'input_tokens':10,'output_tokens':2}}})
for line in sys.stdin:
 v=json.loads(line)
 if v['type']=='control_request':
  r=v['request']; data={}
  if r['subtype']=='initialize':
   data={'models':[{'value':'fixture','displayName':'Fixture','supportedEffortLevels':['low','high']}],'commands':[]}
   hooks=r.get('hooks',{})
  emit({'type':'control_response','response':{'subtype':'success','request_id':v['request_id'],'response':data}})
  if r['subtype']=='interrupt': result()
 elif v['type']=='user':
  text=v['message']['content'][0].get('text','')
  if text=='crash': sys.exit(9)
  if text=='hang': continue
  if text=='write':
   pre=hooks['PreToolUse'][0]; assert pre['matcher']=='Write'
   emit({'type':'control_request','request_id':'hook','request':{'subtype':'hook_callback','callback_id':pre['hookCallbackIds'][0],'tool_use_id':'w1','input':{'hook_event_name':'PreToolUse','tool_name':'Write','tool_input':{'file_path':'lab.txt','content':'after\n'}}}}); continue
  if text=='approval':
   emit({'type':'control_request','request_id':'permission','request':{'subtype':'can_use_tool','tool_name':'Bash','input':{'command':'echo fixture'},'tool_use_id':'tool'}}); continue
  if text=='stream':
   emit({'type':'stream_event','event':{'type':'message_start','message':{'id':'api-message'}}})
   emit({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'Hello'}}})
   assistant('Hello')
  else: assistant(text)
  result()
 elif v['type']=='control_response' and v['response']['request_id']=='hook':
  assert v['response']['response']=={'continue':True}
  emit({'type':'assistant','uuid':'u-w','message':{'id':'api-w','model':'fixture','content':[{'type':'tool_use','id':'w1','name':'Write','input':{'file_path':'lab.txt','content':'after\n'}}],'usage':{'input_tokens':5,'output_tokens':1}}})
  open('lab.txt','w').write('after\n')
  emit({'type':'user','message':{'role':'user','content':[{'type':'tool_result','tool_use_id':'w1','content':'File written'}]}})
  assistant('written'); result()
 elif v['type']=='control_response':
  assistant(v['response']['response']['behavior']); result()
"#).unwrap();
    let command = shell_words::join(["python3", path.to_str().unwrap()]);
    (dir, runtime(command))
}
async fn start(runtime: &ClaudeRuntime, cwd: &str) -> String {
    runtime
        .start_session(StartSessionInput {
            cwd: cwd.into(),
            agent_id: Some("claude".into()),
            model: "fixture".into(),
            reasoning_effort: None,
            approval_mode: "guarded".into(),
            sandbox_mode: Some("workspace-write".into()),
        })
        .await
        .unwrap()
        .provider_session_id
}
fn turn(id: &str, prompt: &str) -> StartTurnInput {
    StartTurnInput {
        provider_session_id: id.into(),
        prompt: prompt.into(),
        model: None,
        reasoning_effort: None,
        sandbox_mode: None,
        collaboration_mode: None,
        approval_mode: None,
        performance_mode: None,
        thread_id: "thread".into(),
        turn_id: "turn".into(),
        hidden: false,
        images: vec![],
        title: None,
        context_delivered: true,
    }
}
#[tokio::test]
async fn native_claude_stream_reconciles_and_releases() {
    let (dir, runtime) = fake();
    let id = start(&runtime, dir.path().to_str().unwrap()).await;
    let items = runtime
        .start_turn(
            turn(&id, "stream"),
            EventBus::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        items
            .iter()
            .filter(|i| i.kind == ThreadItemKind::AgentMessage)
            .map(|i| i.text.as_str())
            .collect::<Vec<_>>(),
        ["Hello"]
    );
    assert_eq!(items[0].extra["nativeMessageId"], "native-uuid");
    assert_eq!(runtime.execution_state(&id).await, ExecutionState::Idle);
    assert_eq!(runtime.loaded_sessions().await.len(), 1);
    runtime.release_session(&id).await.unwrap();
    assert!(runtime.loaded_sessions().await.is_empty());
}
#[tokio::test]
async fn native_claude_permission_is_scoped_and_answered() {
    let (dir, runtime) = fake();
    let id = start(&runtime, dir.path().to_str().unwrap()).await;
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let running = runtime.clone();
    let input = turn(&id, "approval");
    let task = tokio::spawn(async move {
        running
            .start_turn(input, bus, CancellationToken::new())
            .await
    });
    let mut before = vec![];
    let request = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let e = rx.recv().await.unwrap();
            before.push(e.event_type.as_str().to_owned());
            if e.event_type == "thread.request.created" {
                break e.payload["request"]["id"].as_str().unwrap().to_owned();
            }
        }
    })
    .await
    .unwrap();
    assert!(
        before.iter().any(|t| t == "thread.turn.started"),
        "{before:?}"
    );
    assert!(runtime.pending_requests("other-thread").await.is_empty());
    assert_eq!(runtime.pending_requests("thread").await.len(), 1);
    runtime
        .respond_permission(&request, false, None)
        .await
        .unwrap();
    let items = task.await.unwrap().unwrap();
    assert!(items.iter().any(|i| i.text == "deny"));
    assert!(runtime.pending_requests("thread").await.is_empty());
    // The lifecycle events the Supervisor needs, exactly as ACP emits them:
    // started before the request, completed after the final items.
    let mut after = vec![];
    while let Ok(e) = rx.try_recv() {
        after.push(e.event_type.as_str().to_owned());
    }
    let completed = after
        .iter()
        .position(|t| t == "thread.turn.completed")
        .unwrap();
    assert!(after[..completed]
        .iter()
        .any(|t| t == "thread.item.completed"));
    assert_eq!(
        after
            .iter()
            .filter(|t| *t == "thread.turn.completed")
            .count(),
        1
    );
    runtime.release_session(&id).await.unwrap();
}
#[tokio::test]
async fn native_claude_interrupt_and_live_steering() {
    let (dir, runtime) = fake();
    let id = start(&runtime, dir.path().to_str().unwrap()).await;
    for steering in [true, false] {
        let running = runtime.clone();
        let input = turn(&id, "hang");
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let task = tokio::spawn(async move {
            running
                .start_turn(input, bus, CancellationToken::new())
                .await
        });
        rx.recv().await.unwrap();
        if steering {
            runtime.send_input(&id, "turn", "steered").await.unwrap();
        } else {
            runtime.interrupt(&id, "turn").await.unwrap();
        }
        let items = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if steering {
            assert!(items.iter().any(|i| i.text == "steered"));
        }
    }
    runtime.release_session(&id).await.unwrap();
}
#[tokio::test]
async fn native_claude_crash_is_uncertain_and_resume_uses_same_store_identity() {
    let (dir, runtime) = fake();
    let cwd = dir.path().to_str().unwrap();
    let id = start(&runtime, cwd).await;
    let e = runtime
        .start_turn(
            turn(&id, "crash"),
            EventBus::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(e.downcast_ref::<ExecutionUncertain>().is_some(), "{e:#}");
    assert_eq!(runtime.execution_state(&id).await, ExecutionState::Unknown);
    let resumed = runtime
        .resume_session(&id, Some(cwd), SessionSettings::default())
        .await
        .unwrap();
    assert_eq!(resumed.provider_session_id, id);
    runtime
        .start_turn(
            turn(&id, "resumed"),
            EventBus::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let argv = std::fs::read_to_string(dir.path().join("argv.jsonl")).unwrap();
    assert!(argv.contains("--resume"));
    assert!(argv.contains(raw_id(&id).unwrap()));
    runtime.release_session(&id).await.unwrap();
}
#[tokio::test]
async fn native_claude_model_effort_fork_and_images() {
    let (dir, runtime) = fake();
    let id = start(&runtime, dir.path().to_str().unwrap()).await;
    let models = runtime.list_models(None, None).await.unwrap();
    assert_eq!(models[0].supported_reasoning_efforts.len(), 2);
    runtime
        .apply_session_settings(
            &id,
            SessionSettings {
                model: Some("fixture-2".into()),
                effort: Some("high".into()),
                collaboration_mode: Some("plan".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let fork = runtime
        .fork_session_at(
            &id,
            &ForkPoint {
                rollback_count: 1,
                reply: Some(ForkReply {
                    message_id: Some("uuid".into()),
                    text: "text".into(),
                    occurrence: 1,
                }),
            },
        )
        .await
        .unwrap();
    assert_ne!(fork.session.provider_session_id, id);
    let argv = std::fs::read_to_string(dir.path().join("argv.jsonl")).unwrap();
    assert!(argv.contains("--fork-session"));
    assert!(argv.contains("--resume-session-at"));
    let mut input = turn(&id, "image");
    input.images.push(PromptImage {
        mime_type: "image/png".into(),
        data: "fixture".into(),
    });
    assert_eq!(
        prompt_blocks(&input, ".").unwrap()[1]["source"]["media_type"],
        "image/png"
    );
    runtime.release_session(&id).await.unwrap();
    runtime
        .release_session(&fork.session.provider_session_id)
        .await
        .unwrap();
}
#[test]
fn native_claude_maps_tools_plans_usage_and_background() {
    let mut mapper = mapping::Mapper::new("turn");
    for (name, kind) in [
        ("Bash", ThreadItemKind::CommandExecution),
        ("Read", ThreadItemKind::FileRead),
        ("Write", ThreadItemKind::FileChange),
        ("WebSearch", ThreadItemKind::WebSearch),
        ("Task", ThreadItemKind::AgentToolCall),
    ] {
        let updates=mapper.apply(&json!({"type":"assistant","message":{"id":name,"content":[{"type":"tool_use","id":name,"name":name,"input":{"command": if name=="Bash" {json!("echo test")} else {Value::Null}}}]}}));
        assert_eq!(updates[0].items[0].kind, kind, "{name}");
        let updates=mapper.apply(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":name,"content":"output","is_error":false}]}}));
        assert_eq!(updates[0].items[0].status.as_deref(), Some("completed"));
    }
    let updates=mapper.apply(&json!({"type":"assistant","message":{"id":"plan","content":[{"type":"tool_use","name":"TodoWrite","input":{"todos":[{"content":"test","status":"in_progress"}]}}]}}));
    assert!(updates[0].plan.is_some());
    let message = json!({"type":"assistant","message":{"id":"usage","model":"fixture","content":[],"usage":{"input_tokens":10,"output_tokens":2}}});
    mapper.apply(&message);
    let updates = mapper.apply(&message);
    assert_eq!(
        updates[0].usage.as_ref().unwrap()["total"]["outputTokens"],
        2
    );
}
#[test]
fn native_claude_product_policy_and_shared_ids() {
    assert_eq!(raw_id("claude::shared").unwrap(), "shared");
    assert_eq!(raw_id("shared").unwrap(), "shared");
    assert!(raw_id("claude::env/e/1/s").is_err());
    let mut s = SessionSettings {
        collaboration_mode: Some("plan".into()),
        ..Default::default()
    };
    assert_eq!(permission_mode(&s), "plan");
    s.sandbox_mode = Some("danger-full-access".into());
    assert_eq!(permission_mode(&s), "bypassPermissions");
}

/// Run only under an isolated HOME with explicit opt-in. No credentials are
/// accessed by this test; the CLI uses the lab's own Claude configuration.
#[tokio::test]
#[ignore = "requires an explicitly isolated real Claude lab"]
async fn native_claude_real_lab() {
    let command =
        std::env::var("NATIVE_CLAUDE_REAL_COMMAND").expect("explicit real Claude command");
    let evidence = PathBuf::from(std::env::var("NATIVE_CLAUDE_EVIDENCE").unwrap());
    std::fs::create_dir_all(&evidence).unwrap();
    let dir = TempDir::new().unwrap();
    let cwd = dir.path().to_str().unwrap();
    std::fs::write(evidence.join("roundtrip-cwd.txt"), cwd).unwrap();
    let acp: SharedRuntime = Arc::new(crate::acp::AcpRuntime::bound(
        Provider::Acp,
        "claude",
        45000,
    ));
    let runtime = ClaudeRuntime::new(acp.clone(), command, None);
    let session = runtime
        .start_session(StartSessionInput {
            cwd: cwd.into(),
            agent_id: Some("claude".into()),
            model: "haiku".into(),
            reasoning_effort: None,
            approval_mode: "yolo".into(),
            sandbox_mode: Some("danger-full-access".into()),
        })
        .await
        .unwrap();
    let id = session.provider_session_id;
    let items=tokio::time::timeout(Duration::from_secs(120),runtime.start_turn(turn(&id,"Remember the word plum. Use Bash to run printf native-ok, then use Write to create lab.txt containing native-ok. Reply briefly."),EventBus::new(),CancellationToken::new())).await.unwrap().unwrap();
    std::fs::write(
        evidence.join("native-tools.json"),
        serde_json::to_vec_pretty(&items).unwrap(),
    )
    .unwrap();
    assert!(items
        .iter()
        .any(|i| i.kind == ThreadItemKind::CommandExecution));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("lab.txt"))
            .unwrap()
            .trim(),
        "native-ok"
    );
    let point = ForkPoint {
        rollback_count: 0,
        reply: items
            .iter()
            .rev()
            .find(|i| i.kind == ThreadItemKind::AgentMessage)
            .map(|i| ForkReply {
                message_id: i
                    .extra
                    .get("nativeMessageId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                text: i.text.clone(),
                occurrence: 1,
            }),
    };
    let fork = runtime.fork_session_at(&id, &point).await.unwrap();
    runtime
        .release_session(&fork.session.provider_session_id)
        .await
        .unwrap();
    runtime.release_session(&id).await.unwrap();
    // Load a fresh runtime, using precisely the ID that ACP and Claude share.
    runtime
        .resume_session(
            &id,
            Some(cwd),
            SessionSettings {
                model: Some("haiku".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let items = tokio::time::timeout(
        Duration::from_secs(120),
        runtime.start_turn(
            turn(
                &id,
                "What word did I ask you to remember? Reply with only that word.",
            ),
            EventBus::new(),
            CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(items.iter().any(|i| i.text.to_lowercase().contains("plum")));
    std::fs::write(
        evidence.join("native-resumed.json"),
        serde_json::to_vec_pretty(&items).unwrap(),
    )
    .unwrap();
    runtime.release_session(&id).await.unwrap();
    acp.resume_session(
        &id,
        Some(cwd),
        SessionSettings {
            model: Some("haiku".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let items = tokio::time::timeout(
        Duration::from_secs(120),
        acp.start_turn(
            turn(
                &id,
                "What word did I ask you to remember? Reply with only that word.",
            ),
            EventBus::new(),
            CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(items.iter().any(|i| i.text.to_lowercase().contains("plum")));
    std::fs::write(
        evidence.join("acp-resumed.json"),
        serde_json::to_vec_pretty(&items).unwrap(),
    )
    .unwrap();
    acp.release_session(&id).await.unwrap();
    runtime
        .resume_session(
            &id,
            Some(cwd),
            SessionSettings {
                model: Some("haiku".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let items = tokio::time::timeout(
        Duration::from_secs(120),
        runtime.start_turn(
            turn(
                &id,
                "What word did I ask you to remember? Reply with only that word.",
            ),
            EventBus::new(),
            CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(items.iter().any(|i| i.text.to_lowercase().contains("plum")));
    std::fs::write(
        evidence.join("native-from-acp.json"),
        serde_json::to_vec_pretty(&items).unwrap(),
    )
    .unwrap();
    runtime.release_session(&id).await.unwrap();
}

#[tokio::test]
#[ignore = "requires an explicitly isolated real Claude lab"]
async fn native_claude_real_controls_lab() {
    let command = std::env::var("NATIVE_CLAUDE_REAL_COMMAND").unwrap();
    let evidence = PathBuf::from(std::env::var("NATIVE_CLAUDE_EVIDENCE").unwrap());
    let dir = TempDir::new().unwrap();
    let cwd = dir.path().to_str().unwrap();
    std::fs::write(evidence.join("controls-cwd.txt"), cwd).unwrap();
    let runtime = Arc::new(ClaudeRuntime::new(
        Arc::new(FakeRuntime::new(Provider::Acp)),
        command,
        None,
    ));
    let id = runtime
        .start_session(StartSessionInput {
            cwd: cwd.into(),
            agent_id: Some("claude".into()),
            model: "haiku".into(),
            reasoning_effort: None,
            approval_mode: "guarded".into(),
            sandbox_mode: Some("workspace-write".into()),
        })
        .await
        .unwrap()
        .provider_session_id;
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let running = runtime.clone();
    let input = turn(
        &id,
        "Use Bash to run python3 -c 'print(12345)'. Do not use any other tool. Reply briefly.",
    );
    let task = tokio::spawn(async move {
        running
            .start_turn(input, bus, CancellationToken::new())
            .await
    });
    let request = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let event = rx.recv().await.unwrap();
            if event.event_type == "thread.request.created" {
                break event.payload["request"]["id"].as_str().unwrap().to_owned();
            }
        }
    })
    .await
    .unwrap();
    runtime
        .respond_permission(&request, true, None)
        .await
        .unwrap();
    let items = tokio::time::timeout(Duration::from_secs(90), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    std::fs::write(
        evidence.join("guarded.json"),
        serde_json::to_vec_pretty(&items).unwrap(),
    )
    .unwrap();
    runtime
        .apply_session_settings(
            &id,
            SessionSettings {
                collaboration_mode: Some("plan".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let items=tokio::time::timeout(Duration::from_secs(90),runtime.start_turn(turn(&id,"Give a one sentence plan for a hello world program. Do not call tools or ExitPlanMode."),EventBus::new(),CancellationToken::new())).await.unwrap().unwrap();
    std::fs::write(
        evidence.join("plan.json"),
        serde_json::to_vec_pretty(&items).unwrap(),
    )
    .unwrap();
    runtime
        .apply_session_settings(
            &id,
            SessionSettings {
                model: Some("default".into()),
                effort: Some("low".into()),
                collaboration_mode: Some("default".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let items = tokio::time::timeout(
        Duration::from_secs(90),
        runtime.start_turn(
            turn(&id, "Reply exactly: default-ok. Do not call tools."),
            EventBus::new(),
            CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    std::fs::write(
        evidence.join("default-model.json"),
        serde_json::to_vec_pretty(&items).unwrap(),
    )
    .unwrap();
    runtime
        .apply_session_settings(
            &id,
            SessionSettings {
                model: Some("haiku".into()),
                sandbox_mode: Some("danger-full-access".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut image = turn(&id, "Describe the image briefly. Do not call tools.");
    use base64::Engine;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(16, 16)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    image.images.push(PromptImage {
        mime_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
    });
    let items = tokio::time::timeout(
        Duration::from_secs(90),
        runtime.start_turn(image, EventBus::new(), CancellationToken::new()),
    )
    .await
    .unwrap()
    .unwrap();
    std::fs::write(
        evidence.join("image.json"),
        serde_json::to_vec_pretty(&items).unwrap(),
    )
    .unwrap();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let running = runtime.clone();
    let input = turn(&id, "Use Bash to run sleep 3, then reply briefly.");
    let task = tokio::spawn(async move {
        running
            .start_turn(input, bus, CancellationToken::new())
            .await
    });
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let e = rx.recv().await.unwrap();
            if e.event_type == "thread.item.started"
                && e.payload["item"]["kind"] == "commandExecution"
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    runtime
        .send_input(
            &id,
            "turn",
            "Reply exactly steered-ok after the command. Do not call any further tools.",
        )
        .await
        .unwrap();
    let items = tokio::time::timeout(Duration::from_secs(90), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(items.iter().any(|i| i.text.contains("steered-ok")));
    std::fs::write(
        evidence.join("steering.json"),
        serde_json::to_vec_pretty(&items).unwrap(),
    )
    .unwrap();
    let items=tokio::time::timeout(Duration::from_secs(120),runtime.start_turn(turn(&id,"Use the Agent tool to create one general-purpose subagent using model haiku, ask it to reply exactly child-ok. Then reply child-ok yourself. Do not do other work."),EventBus::new(),CancellationToken::new())).await.unwrap().unwrap();
    assert!(items
        .iter()
        .any(|i| i.kind == ThreadItemKind::AgentToolCall));
    std::fs::write(
        evidence.join("subagent.json"),
        serde_json::to_vec_pretty(&items).unwrap(),
    )
    .unwrap();
    // Interrupt a real shell operation, then recover from the shared session.
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let running = runtime.clone();
    let input = turn(&id, "Use Bash to run sleep 30, then reply slept.");
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let task = tokio::spawn(async move { running.start_turn(input, bus, token).await });
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let e = rx.recv().await.unwrap();
            if e.event_type == "thread.item.started"
                && e.payload["item"]["kind"] == "commandExecution"
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(runtime.execution_state(&id).await, ExecutionState::Unknown);
    runtime
        .resume_session(
            &id,
            Some(cwd),
            SessionSettings {
                model: Some("haiku".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    runtime.release_session(&id).await.unwrap();
}

#[test]
fn native_claude_foreground_agent_does_not_wait_for_background_notification() {
    let mut c = completion::Completion::default();
    c.record(&json!({"type":"system","subtype":"task_started","task_id":"child","is_backgrounded":false}));
    c.record(&json!({"type":"result","is_error":false}));
    assert!(c.complete());
    let mut c = completion::Completion::default();
    c.record(&json!({"type":"system","subtype":"session_state_changed","state":"running"}));
    c.record(
        &json!({"type":"system","subtype":"task_started","task_id":"child","is_backgrounded":true}),
    );
    c.record(&json!({"type":"result","is_error":false}));
    assert!(!c.complete());
    c.record(&json!({"type":"system","subtype":"task_notification","task_id":"child","status":"completed"}));
    assert!(!c.complete());
    c.record(&json!({"type":"system","subtype":"session_state_changed","state":"idle"}));
    assert!(c.complete());
}

#[tokio::test]
async fn native_claude_cancellation_cleans_pending_permissions_and_owned_process() {
    let (dir, runtime) = fake();
    let id = start(&runtime, dir.path().to_str().unwrap()).await;
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let running = runtime.clone();
    let input = turn(&id, "approval");
    let task = tokio::spawn(async move { running.start_turn(input, bus, token).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if rx.recv().await.unwrap().event_type == "thread.request.created" {
                break;
            }
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(runtime.pending_requests("thread").await.is_empty());
    assert!(runtime.loaded_sessions().await.is_empty());
    assert_eq!(runtime.execution_state(&id).await, ExecutionState::Unknown);
}

#[test]
fn native_claude_task_plan_and_edit_snippet_diff() {
    let mut m = mapping::Mapper::new("turn");
    m.apply(&json!({"type":"assistant","message":{"id":"plan","content":[{"type":"tool_use","id":"create","name":"TaskCreate","input":{"subject":"Run tests"}}]}}));
    let u=m.apply(&json!({"type":"user","tool_use_result":{"task":{"id":"1","subject":"Run tests"}},"message":{"content":[{"type":"tool_result","tool_use_id":"create","content":"Task created"}]}}));
    assert_eq!(u[0].plan.as_ref().unwrap()[0].0, "Run tests");
    m.apply(&json!({"type":"assistant","message":{"id":"edit","content":[{"type":"tool_use","id":"edit","name":"Edit","input":{"file_path":"file.rs","old_string":"old\n","new_string":"new\n"}}]}}));
    let u=m.apply(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"edit","content":"edited"}]}}));
    assert_eq!(u[0].items[0].kind, ThreadItemKind::FileChange);
    assert!(u[0].items[0].detail_text.as_ref().unwrap().contains("-old"));
}

#[tokio::test]
async fn native_claude_router_delivers_permission_to_owning_backend() {
    let (dir, native) = fake();
    let cwd = dir.path().to_str().unwrap();
    let id = start(&native, cwd).await;
    // Actual ACP deliberately accepts unknown request ids. Routing must not let
    // that acknowledgment hide an unanswered native Claude request.
    let acp = Arc::new(crate::acp::AcpRuntime::catalog(None, 5000));
    assert!(acp
        .respond_permission("claude-native:unknown", true, None)
        .await
        .is_ok());
    crate::native::ProtocolSettings {
        claude: crate::native::HarnessProtocol::Native,
        ..Default::default()
    }
    .save(&dir.path().join("protocol.json"))
    .unwrap();
    let router = Arc::new(
        crate::native::ProtocolRouter::new(acp, None, dir.path().join("protocol.json"))
            .with_backend("claude", native.clone()),
    );
    router
        .resume_session(&id, Some(cwd), SessionSettings::default())
        .await
        .unwrap();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let token = CancellationToken::new();
    let cancel = token.clone();
    let input = turn(&id, "approval");
    let running = router.clone();
    let task = tokio::spawn(async move { running.start_turn(input, bus, cancel).await });
    let request = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = rx.recv().await.unwrap();
            if event.event_type == "thread.request.created" {
                break event.payload["request"]["id"].as_str().unwrap().to_owned();
            }
        }
    })
    .await
    .unwrap();
    assert!(request.starts_with("claude-native:"));
    let pending = router.pending_requests("thread").await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, request);
    router
        .respond_permission(&request, true, None)
        .await
        .unwrap();
    let done = tokio::time::timeout(Duration::from_secs(2), task).await;
    if done.is_err() {
        token.cancel();
    }
    let items = done
        .expect("router swallowed native Claude approval")
        .unwrap()
        .unwrap();
    assert!(items.iter().any(|item| item.text == "allow"));
    assert!(router.pending_requests("thread").await.is_empty());
    crate::native::ProtocolSettings::default()
        .save(&dir.path().join("protocol.json"))
        .unwrap();
    // Stale/native-invalid requests must retain the native validation error,
    // even after the device owner changes the selected protocol back to ACP.
    assert!(router
        .respond_permission(&request, true, None)
        .await
        .is_err());
    assert!(router
        .respond_permission("claude-native:unknown", false, None)
        .await
        .is_err());
    assert!(router
        .respond_permission("acp-request", false, None)
        .await
        .is_ok());
    native.release_session(&id).await.unwrap();
}

#[tokio::test]
async fn native_claude_write_diff_comes_from_the_pre_tool_use_hook() {
    let (dir, runtime) = fake();
    std::fs::write(dir.path().join("lab.txt"), "before\n").unwrap();
    let id = start(&runtime, dir.path().to_str().unwrap()).await;
    let items = runtime
        .start_turn(
            turn(&id, "write"),
            EventBus::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let write = items
        .iter()
        .find(|i| i.kind == "fileChange")
        .expect("write row");
    let detail = serde_json::to_string(write).unwrap();
    assert!(
        detail.contains("before") && detail.contains("after"),
        "{detail}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("lab.txt")).unwrap(),
        "after\n"
    );
    runtime.release_session(&id).await.unwrap();
}
