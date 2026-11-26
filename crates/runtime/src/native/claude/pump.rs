//! One reader per Claude process. While a turn runs, every frame goes to that
//! turn. Between turns (a background task finishing after an interrupt, a
//! scheduled wake) frames only update the session's lifecycle state, and the
//! CLI's callbacks are answered so it never waits on a reply: a hook observes
//! and continues, a tool permission is denied (no one can approve it), any
//! other callback fails closed. Nothing from between turns leaks into the next.
use super::Session;
use serde_json::{json, Value};
use std::sync::Arc;

pub(super) fn spawn(session: &Arc<Session>) {
    let weak = Arc::downgrade(session);
    let process = session.process.clone();
    tokio::spawn(async move {
        let mut events = process.events.lock().await;
        while let Some(frame) = events.recv().await {
            let Some(session) = weak.upgrade() else { break };
            let unsent = {
                let sink = session.sink.lock().unwrap();
                match sink.as_ref() {
                    Some(sink) => sink.send(frame).err().map(|e| e.0),
                    None => Some(frame),
                }
            };
            if let Some(frame) = unsent {
                session.completion.lock().unwrap().record(&frame);
                session.observe(&frame);
                if frame["type"] == "control_request" {
                    let _ = answer_outside_turn(&session, &frame).await;
                }
            }
        }
        // EOF: the turn's inbox closes, which the turn reports as uncertain.
        if let Some(session) = weak.upgrade() {
            session.sink.lock().unwrap().take();
        }
    });
}

async fn answer_outside_turn(session: &Session, frame: &Value) -> anyhow::Result<()> {
    let id = frame["request_id"].as_str().unwrap_or_default();
    let response = match frame["request"]["subtype"].as_str() {
        Some("hook_callback") => {
            json!({"subtype":"success","request_id":id,"response":{"continue":true}})
        }
        Some("can_use_tool") => json!({"subtype":"success","request_id":id,"response":{
            "behavior":"deny",
            "message":"No Pockymoe turn is running to approve this tool. Ask the user to send a message."
        }}),
        _ => {
            json!({"subtype":"error","request_id":id,"error":"Unsupported native Claude control callback"})
        }
    };
    session
        .process
        .write(json!({"type":"control_response","response":response}))
        .await
}
