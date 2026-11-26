use super::*;

#[tokio::test]
async fn codex_switch_refreshes_adapter_catalog_before_config_validation() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("agent.py");
    std::fs::write(&script, r#"
import json, sys, os, socket, threading
def control():
    config = json.loads(os.environ['POCKYMOE_APP_SERVER_BRIDGE'])
    host, port = config['address'].rsplit(':', 1)
    connection = socket.create_connection((host, int(port)))
    stream = connection.makefile('rw')
    stream.write(config['token'] + '\n'); stream.flush()
    for line in stream:
        request = json.loads(line)
        stream.write(json.dumps({'id':request['id'],'result':{}}) + '\n'); stream.flush()
threading.Thread(target=control, daemon=True).start()
refreshed = False
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    method = request['method']
    if method == 'initialize': result = {'protocolVersion': 1, 'agentCapabilities': {}}
    elif method == 'session/new': result = {'sessionId':'fixture'}
    elif method == 'session/set_model':
        refreshed = True
        result = {}
    elif method == 'session/set_config_option' and refreshed:
        result = {'configOptions':[{'id':'model','category':'model','currentValue':request['params']['value'],'options':[]}]}
    else:
        print(json.dumps({'id':request['id'],'error':{'code':-32602,'message':'model missing from stale catalog'}}), flush=True)
        continue
    print(json.dumps({'id':request['id'],'result':result}), flush=True)
"#).unwrap();
    let command = shell_words::join([
        which::which("python3").unwrap().to_str().unwrap(),
        script.to_str().unwrap(),
    ]);
    let def = AcpAgentDef {
        id: "codex".into(),
        display_name: "Fixture".into(),
        description: String::new(),
        transport: "native".into(),
        base_command: command.clone(),
        server_command: command,
        install_command: None,
        model_list_command: None,
        sign_in_command: None,
    };
    let runtime = AcpRuntime::bound(Provider::Codex, "codex", 5_000);
    let (_, mut live) = runtime
        .spawn_session(
            &def,
            dir.path().to_str().unwrap(),
            ProductSessionPolicy::default(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let _cleanup = StartupProcess(Some(live.process.clone()));
    AcpRuntime::apply_setting_op(
        &live.process.clone(),
        &mut live,
        SessionSettingOp::SetConfig {
            config_id: "model".into(),
            value: "gpt-6.2-sol".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(live.model.as_deref(), Some("gpt-6.2-sol"));
    live.process.shutdown().await.unwrap();
}

/// An agent without an account: initialize succeeds, session/new answers
/// ACP's auth_required (as `agy acp` 1.5.18 and other agents do).
#[tokio::test]
async fn an_unsigned_agent_reports_sign_in_required_instead_of_a_raw_rpc_error() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("agent.py");
    std::fs::write(&script, r#"
import json, sys
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    if request['method'] == 'initialize':
        result = {'protocolVersion': 1, 'agentInfo': {'name': 'Fixture Agent', 'version': '1.0.0'},
                  'agentCapabilities': {'loadSession': True, 'promptCapabilities': {'image': True}},
                  'authMethods': [{'id': 'oauth-personal', 'name': 'Log in'}]}
        print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
    else:
        print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32000, 'message': 'Authentication required'}}), flush=True)
"#).unwrap();
    let command = shell_words::join([
        which::which("python3").unwrap().to_str().unwrap(),
        script.to_str().unwrap(),
    ]);
    let def = AcpAgentDef {
        id: "copilot".into(),
        display_name: "GitHub Copilot CLI".into(),
        description: String::new(),
        transport: "native".into(),
        base_command: command.clone(),
        server_command: command,
        install_command: None,
        model_list_command: None,
        sign_in_command: Some("copilot login --device-code".into()),
    };
    let runtime = AcpRuntime::bound(Provider::Acp, "copilot", 5_000);
    let error = runtime
        .spawn_session(
            &def,
            dir.path().to_str().unwrap(),
            ProductSessionPolicy::default(),
            None,
            None,
            None,
        )
        .await
        .err()
        .unwrap()
        .to_string();
    assert_eq!(
        error,
        "Sign-in required: GitHub Copilot CLI is not signed in. To use it, run \
         `copilot login --device-code` in a terminal on this machine, then try again."
    );
    // Settings and the agent list read it as a sign-in, not a failure.
    use crate::actor::AgentRuntime;
    assert_eq!(runtime.sign_in_error("copilot"), Some(error));
    assert_eq!(runtime.sign_in_error("claude"), None);
}
