use crate::context_inspection::{CodexTraceReader, InspectionState};
use anyhow::Result;
use serde_json::Value;
use std::{path::Path, sync::Arc};

/// Where adapter releases keep the ACP v1 application, newest layout last.
const CLAUDE_V1_MODULES: [&str; 2] = ["dist/acp-agent.js", "dist/v1/acp-agent.js"];

/// Only wrap the known installed package. Custom executables keep their own
/// protocol, and so does a release whose layout this build does not know: the
/// bridge is optional, and a session must start without it.
pub(super) fn claude_command(command: &str, root: &Path) -> Result<Option<String>> {
    let parsed = super::rpc::parse_spawn_command(command)?;
    if !parsed.args.is_empty() {
        return Ok(None);
    }
    let Ok(executable) = std::fs::canonicalize(&parsed.program) else {
        return Ok(None);
    };
    let Some(directory) = executable.parent().and_then(Path::parent) else {
        return Ok(None);
    };
    let package = std::fs::read(directory.join("package.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    if package
        .as_ref()
        .is_none_or(|v| v["name"] != "@agentclientprotocol/claude-agent-acp")
        || !["dist/managed-policy.js", "dist/utils.js"]
            .iter()
            .all(|module| directory.join(module).is_file())
    {
        return Ok(None);
    }
    let Some(module) = CLAUDE_V1_MODULES.into_iter().find(|module| {
        std::fs::read_to_string(directory.join(module))
            .is_ok_and(|source| source.contains("export function v1AgentApp"))
    }) else {
        tracing::warn!(
            adapter = %directory.display(),
            version = package.as_ref().and_then(|v| v["version"].as_str()).unwrap_or("unknown"),
            "Claude adapter layout is not recognized; context inspection is unavailable for it"
        );
        return Ok(None);
    };
    std::fs::create_dir_all(root)?;
    let script = root.join("claude-context-bridge.mjs");
    let body = include_str!("claude-context-bridge.mjs");
    if std::fs::read_to_string(&script).ok().as_deref() != Some(body) {
        std::fs::write(&script, body)?;
    }
    Ok(Some(shell_words::join([
        "node",
        &script.to_string_lossy(),
        &directory.to_string_lossy(),
        module,
    ])))
}

pub(super) fn observer(
    reader: Arc<std::sync::Mutex<CodexTraceReader>>,
    state: Arc<InspectionState>,
    session: String,
    thread: String,
    bus: crate::actor::EventBus,
) -> tokio_util::sync::CancellationToken {
    let stop = tokio_util::sync::CancellationToken::new();
    let cancel = stop.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(25));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! { _=cancel.cancelled()=>break,_=tick.tick()=>{} }
            if !state.enabled() {
                continue;
            }
            let reader = reader.clone();
            let snapshots = tokio::task::spawn_blocking(move || reader.lock().unwrap().poll())
                .await
                .unwrap_or_default();
            for snapshot in snapshots {
                crate::context_inspection::emit(&state, &session, &thread, &bus, snapshot);
            }
        }
    });
    stop
}

pub(super) fn native_usage(
    reader: Option<&Arc<std::sync::Mutex<CodexTraceReader>>>,
    usage: &Value,
    state: Option<&Arc<InspectionState>>,
    session: &str,
    thread: &str,
    bus: &crate::actor::EventBus,
) {
    if let (Some(reader), Some(state)) = (reader, state.filter(|s| s.enabled())) {
        let (reader, state, usage, session, thread, bus) = (
            reader.clone(),
            state.clone(),
            usage.clone(),
            session.to_string(),
            thread.to_string(),
            bus.session_observation(),
        );
        tokio::spawn(async move {
            let snapshot = tokio::task::spawn_blocking(move || {
                let mut reader = reader.lock().unwrap();
                reader.native_usage(&usage);
                reader.matching_native_snapshot(&usage)
            })
            .await
            .ok()
            .flatten();
            if let Some(snapshot) = snapshot {
                crate::context_inspection::emit(&state, &session, &thread, &bus, snapshot);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::claude_command;
    use std::path::Path;

    fn adapter(root: &Path, files: &[(&str, &str)]) -> String {
        std::fs::create_dir_all(root.join("dist")).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"@agentclientprotocol/claude-agent-acp","version":"0.0.0"}"#,
        )
        .unwrap();
        std::fs::write(root.join("dist/index.js"), "").unwrap();
        for (path, body) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        root.join("dist/index.js").to_string_lossy().into_owned()
    }

    #[test]
    fn the_claude_bridge_follows_the_adapter_layout_and_is_skipped_for_an_unknown_one() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = dir.path().join("bridge");
        let app = "export function v1AgentApp(logger, onAgent) {}";
        let common = [("dist/managed-policy.js", ""), ("dist/utils.js", "")];
        let with = |name: &str, extra: &[(&str, &str)]| {
            let files: Vec<_> = common.iter().chain(extra).copied().collect();
            claude_command(&adapter(&dir.path().join(name), &files), &bridge).unwrap()
        };
        // Through 0.102 the application sat in dist/; 0.104 moved it under dist/v1/.
        let old = with("old", &[("dist/acp-agent.js", app)]).unwrap();
        assert!(old.ends_with(" dist/acp-agent.js"), "{old}");
        let new = with("new", &[("dist/v1/acp-agent.js", app)]).unwrap();
        assert!(new.ends_with(" dist/v1/acp-agent.js"), "{new}");
        assert!(bridge.join("claude-context-bridge.mjs").is_file());
        // A layout this build does not know, a module without the export, a
        // missing executable: the adapter starts as installed, never an error.
        assert_eq!(with("moved", &[("dist/elsewhere.js", app)]), None);
        assert_eq!(
            with(
                "renamed",
                &[("dist/v1/acp-agent.js", "export function other() {}")]
            ),
            None
        );
        assert_eq!(
            claude_command("/nonexistent/claude-agent-acp", &bridge).unwrap(),
            None
        );
        assert_eq!(
            claude_command("claude-agent-acp --flag", &bridge).unwrap(),
            None
        );
    }
}
