use crate::context_inspection::{CodexTraceReader, InspectionState};
use anyhow::Result;
use serde_json::Value;
use std::{path::Path, sync::Arc};

/// Only wrap the known installed package. Custom executables keep their own protocol.
pub(super) fn claude_command(command: &str, root: &Path) -> Result<Option<String>> {
    let parsed = super::rpc::parse_spawn_command(command)?;
    if !parsed.args.is_empty() {
        return Ok(None);
    }
    let executable = std::fs::canonicalize(&parsed.program)?;
    let Some(directory) = executable.parent().and_then(Path::parent) else {
        return Ok(None);
    };
    let package = std::fs::read(directory.join("package.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    if package
        .as_ref()
        .is_none_or(|v| v["name"] != "@agentclientprotocol/claude-agent-acp")
        || !directory.join("dist/managed-policy.js").is_file()
    {
        return Ok(None);
    }
    let source = std::fs::read_to_string(directory.join("dist/acp-agent.js"))?;
    if !source.contains("export function v1AgentApp") {
        return Ok(None);
    }
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
