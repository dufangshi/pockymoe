use std::sync::Arc;

use crate::bounded_channel as mpsc;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use pockymoe_protocol::{now_rfc3339, SupervisorConnectedEnvelope};
use pockymoe_runtime::Supervisor;
use serde_json::{json, Value};
use uuid::Uuid;

pub(crate) struct SocketSession {
    input: mpsc::Sender<Value>,
    task: tokio::task::JoinHandle<()>,
}

impl SocketSession {
    pub(crate) fn spawn(state: Arc<Supervisor>, output: mpsc::Sender<Value>) -> Self {
        Self::spawn_authorized(state, output, None, None)
    }
    pub(crate) fn spawn_authorized(
        state: Arc<Supervisor>,
        output: mpsc::Sender<Value>,
        principal: Option<crate::owner_trust::BrowserPrincipal>,
        thread: Option<String>,
    ) -> Self {
        let (input, incoming) = mpsc::channel();
        let task = tokio::spawn(run_socket_session(
            state, incoming, output, principal, thread,
        ));
        Self { input, task }
    }

    pub(crate) fn send(&self, message: Value) -> bool {
        self.input.send(message).is_ok()
    }
}

impl Drop for SocketSession {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) async fn websocket_loop(socket: WebSocket, state: Arc<Supervisor>) {
    let (output, mut outgoing) = mpsc::channel();
    let session = SocketSession::spawn(state, output);
    let (mut sink, mut stream) = socket.split();

    loop {
        tokio::select! {
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(message) = serde_json::from_str::<Value>(&text) {
                            if !session.send(message) {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    _ => {}
                }
            }
            outgoing = outgoing.recv() => {
                let Some(outgoing) = outgoing else {
                    break;
                };
                if sink.send(Message::Text(outgoing.to_string().into())).await.is_err() {
                    break;
                }
            }
        }
    }
    drop(session);
}

async fn run_socket_session(
    state: Arc<Supervisor>,
    mut incoming: mpsc::Receiver<Value>,
    output: mpsc::Sender<Value>,
    principal: Option<crate::owner_trust::BrowserPrincipal>,
    thread: Option<String>,
) {
    let mut client_state = SocketClientState::default();
    // Dropped with this task, also when a relay client or the tunnel goes away.
    let viewer = state.harness_viewer();
    let mut events = state.bus.subscribe();
    let mut shells = crate::shells::hub().subscribe_all();
    if output
        .send(
            serde_json::to_value(SupervisorConnectedEnvelope {
                event_type: "supervisor.connected".into(),
                timestamp: now_rfc3339(),
            })
            .unwrap_or_else(
                |_| json!({ "type": "supervisor.connected", "timestamp": now_rfc3339() }),
            ),
        )
        .is_err()
    {
        return;
    }
    let mut auth_tick = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        tokio::select! {
            _ = auth_tick.tick() => {
                if principal.as_ref().is_some_and(|p| !p.valid(&state)) { break; }
            }
            message = incoming.recv() => {
                let Some(message) = message else {
                    break;
                };
                if message["type"] == "thread.view" {
                    if principal.as_ref().is_some_and(|p| !p.valid(&state)) { break; }
                    viewer.set(viewed_threads(&state, principal.as_ref(), thread.as_deref(), &message));
                    continue;
                }
                if principal.as_ref().is_some_and(|p| !p.valid(&state) || !crate::grants::websocket_message_allowed(&state,p,thread.as_deref(),&message)) { break; }
                for response in handle_client_message(&message, &mut client_state) {
                    if principal.as_ref().is_some_and(|p| !crate::grants::websocket_event_allowed(&state,p,thread.as_deref(),&response)) { continue; }
                    if output.send(response).is_err() {
                        return;
                    }
                }
            }
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        let event = serde_json::to_value(event).unwrap_or_else(|_| json!({}));
                        if principal.as_ref().is_some_and(|p| !p.valid(&state)) { break; }
                        if principal.as_ref().is_some_and(|p| !crate::grants::websocket_event_allowed(&state,p,thread.as_deref(),&event)) { continue; }
                        if output.send(event).is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "supervisor socket client lagged thread events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            shell = shells.recv() => {
                match shell {
                    Ok(shell) => {
                        let Some(message) = client_state.shell_message(shell) else {
                            continue;
                        };
                        if principal.as_ref().is_some_and(|p| !p.valid(&state)) { break; }
                        if principal.as_ref().is_some_and(|p| !crate::grants::websocket_event_allowed(&state,p,thread.as_deref(),&message)) { continue; }
                        if output.send(message).is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "supervisor socket client lagged terminal output");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

/// The threads a `thread.view` message lists that this connection controls,
/// judged like the interrupt endpoint: a read-only viewer must not keep the
/// owner's harness loaded.
fn viewed_threads(
    state: &Supervisor,
    principal: Option<&crate::owner_trust::BrowserPrincipal>,
    configured_thread: Option<&str>,
    message: &Value,
) -> Vec<String> {
    message["threadIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 128 && !id.contains(['/', '?', '#']))
        // Bound the per-thread access checks before running them.
        .take(pockymoe_runtime::service::MAX_VIEWED_THREADS)
        .filter(|id| match principal {
            Some(p) if !p.is_owner() => {
                configured_thread.is_none_or(|configured| configured == *id)
                    && crate::grants::authorize_path(
                        state,
                        p,
                        "POST",
                        &format!("/api/threads/{id}/interrupt"),
                    )
                    .is_ok()
            }
            _ => true,
        })
        .map(str::to_owned)
        .collect()
}

#[derive(Default)]
struct SocketClientState {
    attached_shell_id: Option<String>,
    viewer_id: Option<String>,
    /// Output up to this sequence was already sent as the attach replay.
    replayed_seq: u64,
}

impl SocketClientState {
    fn shell_message(&mut self, shell: crate::shells::ShellOutput) -> Option<Value> {
        if self.attached_shell_id.as_deref() != Some(&shell.shell_id)
            || shell.seq <= self.replayed_seq
        {
            return None;
        }
        if shell.exited {
            self.attached_shell_id = None;
            self.viewer_id = None;
            return Some(json!({
                "type": "shell.exited",
                "shellId": shell.shell_id,
                "timestamp": now_rfc3339(),
                "payload": { "threadId": shell.thread_id, "state": "exited" }
            }));
        }
        Some(json!({
            "type": "shell.output",
            "shellId": shell.shell_id,
            "timestamp": now_rfc3339(),
            "payload": { "data": shell.data }
        }))
    }
}

fn handle_client_message(message: &Value, state: &mut SocketClientState) -> Vec<Value> {
    match message.get("type").and_then(Value::as_str) {
        Some("supervisor.ping") => vec![json!({
            "type": "supervisor.pong",
            "timestamp": now_rfc3339(),
            "payload": {
                "requestTimestamp": message.get("timestamp").and_then(Value::as_str)
            }
        })],
        Some("shell.attach") => {
            let (Some(id), Some(cols), Some(rows)) = (
                message.get("shellId").and_then(Value::as_str),
                message.get("cols").and_then(Value::as_u64),
                message.get("rows").and_then(Value::as_u64),
            ) else {
                return shell_error(message, "invalid_request", "Invalid shell attach request");
            };
            let shell = match crate::shells::hub().get(id) {
                Ok(shell) => shell,
                Err(error) => return shell_error(message, "shell_not_found", &error.to_string()),
            };
            if let Err(error) = crate::shells::hub().resize(id, cols as u16, rows as u16) {
                return shell_error(message, "resize_failed", &error.to_string());
            }
            let (replay, replayed_seq) = match crate::shells::hub().replay(id) {
                Ok(replay) => replay,
                Err(error) => return shell_error(message, "shell_not_found", &error.to_string()),
            };
            let viewer_id = Uuid::new_v4().to_string();
            state.attached_shell_id = Some(id.to_string());
            state.viewer_id = Some(viewer_id.clone());
            state.replayed_seq = replayed_seq;
            let mut messages = vec![
                json!({
                    "type": "shell.connected",
                    "shellId": id,
                    "timestamp": now_rfc3339(),
                    "payload": { "viewerId": viewer_id }
                }),
                json!({
                    "type": "shell.status",
                    "shellId": id,
                    "timestamp": now_rfc3339(),
                    "payload": {
                        "threadId": shell.get("threadId").cloned().unwrap_or(Value::Null),
                        "state": "attached",
                        "viewerId": viewer_id
                    }
                }),
            ];
            if !replay.is_empty() {
                // The browser resets xterm before attaching; replay restores the screen.
                messages.push(json!({
                    "type": "shell.output",
                    "shellId": id,
                    "timestamp": now_rfc3339(),
                    "payload": { "data": replay, "replay": true }
                }));
            }
            messages
        }
        Some("shell.detach") => {
            let Some((shell_id, viewer_id)) = owned_shell(message, state) else {
                return shell_error(
                    message,
                    "invalid_viewer",
                    "This browser session does not own the shell attachment",
                );
            };
            let shell_id = shell_id.to_string();
            let viewer_id = viewer_id.to_string();
            state.attached_shell_id = None;
            state.viewer_id = None;
            vec![json!({
                "type": "shell.detached",
                "shellId": shell_id,
                "timestamp": now_rfc3339(),
                "payload": {
                    "state": "detached",
                    "viewerId": viewer_id
                }
            })]
        }
        Some("shell.input") => {
            let Some((shell_id, _)) = owned_shell(message, state) else {
                return shell_error(
                    message,
                    "invalid_viewer",
                    "This browser session does not own the shell attachment",
                );
            };
            let Some(data) = message.get("data").and_then(Value::as_str) else {
                return shell_error(message, "invalid_request", "Shell input data is required");
            };
            if let Err(error) = crate::shells::hub().write(shell_id, data) {
                return shell_error(message, "input_failed", &error.to_string());
            }
            Vec::new()
        }
        Some("shell.resize") => {
            let Some((shell_id, _)) = owned_shell(message, state) else {
                return shell_error(
                    message,
                    "invalid_viewer",
                    "This browser session does not own the shell attachment",
                );
            };
            let (Some(cols), Some(rows)) = (
                message.get("cols").and_then(Value::as_u64),
                message.get("rows").and_then(Value::as_u64),
            ) else {
                return shell_error(message, "invalid_request", "Invalid shell size");
            };
            if let Err(error) = crate::shells::hub().resize(shell_id, cols as u16, rows as u16) {
                return shell_error(message, "resize_failed", &error.to_string());
            }
            Vec::new()
        }
        Some("shell.clear") => {
            let Some((shell_id, _)) = owned_shell(message, state) else {
                return shell_error(
                    message,
                    "invalid_viewer",
                    "This browser session does not own the shell attachment",
                );
            };
            if let Err(error) = crate::shells::hub().write(shell_id, "\u{000c}") {
                return shell_error(message, "clear_failed", &error.to_string());
            }
            vec![json!({
                "type": "shell.output",
                "shellId": shell_id,
                "timestamp": now_rfc3339(),
                "payload": { "data": "", "replace": true }
            })]
        }
        _ => Vec::new(),
    }
}

fn owned_shell<'a>(message: &'a Value, state: &'a SocketClientState) -> Option<(&'a str, &'a str)> {
    let shell_id = message.get("shellId").and_then(Value::as_str)?;
    let viewer_id = message.get("viewerId").and_then(Value::as_str)?;
    (state.attached_shell_id.as_deref() == Some(shell_id)
        && state.viewer_id.as_deref() == Some(viewer_id))
    .then_some((shell_id, viewer_id))
}

fn shell_error(message: &Value, code: &str, error_message: &str) -> Vec<Value> {
    let Some(shell_id) = message.get("shellId").and_then(Value::as_str) else {
        return Vec::new();
    };
    vec![json!({
        "type": "shell.error",
        "shellId": shell_id,
        "timestamp": now_rfc3339(),
        "payload": { "code": code, "message": error_message }
    })]
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tokio::sync::broadcast::error::TryRecvError;

    fn wait_for_replay(id: &str, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if crate::shells::hub().replay(id).unwrap().0.contains(needle) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("shell output never contained {needle}");
    }

    fn next_message(
        state: &mut SocketClientState,
        output: &mut tokio::sync::broadcast::Receiver<crate::shells::ShellOutput>,
        until: impl Fn(&Value) -> bool,
    ) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            match output.try_recv() {
                Ok(chunk) => {
                    if let Some(message) = state.shell_message(chunk) {
                        let done = until(&message);
                        seen.push(message);
                        if done {
                            return seen;
                        }
                    }
                }
                Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(20)),
                Err(error) => panic!("shell output channel failed: {error}"),
            }
        }
        panic!("expected shell message never arrived; saw {seen:?}");
    }

    async fn eventually(check: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !check() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("condition never held");
    }

    #[tokio::test]
    async fn thread_view_replaces_the_set_and_clears_when_the_connection_ends() {
        let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
        let (output, _outgoing) = mpsc::channel();
        let session = SocketSession::spawn(state.clone(), output);
        assert!(session.send(json!({"type":"thread.view","threadIds":["a","b"]})));
        eventually(|| state.thread_displayed("a") && state.thread_displayed("b")).await;
        assert!(session.send(json!({"type":"thread.view","threadIds":["b"]})));
        eventually(|| !state.thread_displayed("a")).await;
        assert!(state.thread_displayed("b"));
        // A closed browser tab or a relay client that went away.
        drop(session);
        eventually(|| !state.thread_displayed("b")).await;
    }

    #[tokio::test]
    async fn thread_view_keeps_at_most_64_threads_before_checking_access() {
        let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
        let ids: Vec<_> = (0..100).map(|n| format!("t{n}")).collect();
        let message = json!({"type":"thread.view","threadIds":ids});
        assert_eq!(viewed_threads(&state, None, None, &message).len(), 64);
        let (output, _outgoing) = mpsc::channel();
        let session = SocketSession::spawn(state.clone(), output);
        assert!(session.send(message));
        eventually(|| state.thread_displayed("t63")).await;
        assert!(!state.thread_displayed("t64"));
    }

    #[test]
    fn attach_replays_recent_output_once_and_reports_exit() {
        let cwd = tempfile::tempdir().unwrap();
        let hub = crate::shells::hub();
        let mut output = hub.subscribe_all();
        let (id, _) = hub
            .create(
                "thread-replay",
                "workspace-replay",
                cwd.path().to_str().unwrap(),
                80,
                24,
                None,
            )
            .unwrap();
        hub.write(&id, "printf 'before-%s\\n' attach\n").unwrap();
        wait_for_replay(&id, "before-attach");

        let mut state = SocketClientState::default();
        let attached = handle_client_message(
            &json!({ "type": "shell.attach", "shellId": id, "cols": 100, "rows": 30 }),
            &mut state,
        );
        let replay = attached
            .iter()
            .find(|message| message["type"] == "shell.output")
            .expect("attach replays existing output");
        assert_eq!(replay["payload"]["replay"], true);
        assert!(replay["payload"]["data"]
            .as_str()
            .unwrap()
            .contains("before-attach"));

        // Output queued before attach is already part of the replay and must not repeat.
        hub.write(&id, "printf 'after-%s\\n' attach\n").unwrap();
        let live = next_message(&mut state, &mut output, |message| {
            message["payload"]["data"]
                .as_str()
                .unwrap_or_default()
                .contains("after-attach")
        });
        assert!(live.iter().all(|message| !message["payload"]["data"]
            .as_str()
            .unwrap_or_default()
            .contains("before-attach")));

        hub.write(&id, "exit\n").unwrap();
        let exited = next_message(&mut state, &mut output, |message| {
            message["type"] == "shell.exited"
        });
        let last = exited.last().unwrap();
        assert_eq!(last["payload"]["threadId"], "thread-replay");
        assert_eq!(last["payload"]["state"], "exited");
        assert!(hub.get(&id).is_err(), "an exited shell is no longer listed");
        assert!(hub.list_for_thread("thread-replay").is_empty());
    }
}
