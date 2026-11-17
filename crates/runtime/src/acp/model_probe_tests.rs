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
