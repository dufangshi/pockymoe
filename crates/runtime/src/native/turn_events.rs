//! The turn lifecycle events every runtime emits around a visible turn, exactly
//! as the ACP runtime does. The Supervisor relies on them beyond the transcript:
//! generation speed and context steps start on `thread.turn.started`; relay
//! activity, model pricing sync, outside-agent MCP events, the desktop map and
//! the event bus's turn scope end on `thread.turn.completed`.
use crate::actor::{EventBus, ExecutionUncertain, StartTurnInput};
use pockymoe_protocol::{now_rfc3339, ThreadEventEnvelope, ThreadEventType, ThreadHistoryItemDto};
use serde_json::json;

pub(crate) fn started(bus: &EventBus, input: &StartTurnInput) {
    if input.hidden {
        return;
    }
    bus.emit(ThreadEventEnvelope {
        event_type: ThreadEventType::ThreadTurnStarted,
        thread_id: input.thread_id.clone(),
        timestamp: now_rfc3339(),
        payload: json!({ "turnId": input.turn_id }),
    });
}

/// `items` are emitted as completed first when the runtime has not already
/// done so, then the turn's end with the same status/error vocabulary as ACP.
pub(crate) fn finished<T>(
    bus: &EventBus,
    input: &StartTurnInput,
    outcome: &anyhow::Result<T>,
    interrupted: bool,
    items: Option<&[ThreadHistoryItemDto]>,
) {
    if input.hidden {
        return;
    }
    let (status, error) = match outcome {
        Ok(_) if interrupted => ("interrupted", None),
        Ok(_) => ("completed", None),
        Err(error) if error.is::<ExecutionUncertain>() => ("recovering", Some(error.to_string())),
        Err(error) => ("failed", Some(error.to_string())),
    };
    bus.emit(ThreadEventEnvelope {
        event_type: ThreadEventType::ThreadSubagentsUpdated,
        thread_id: input.thread_id.clone(),
        timestamp: now_rfc3339(),
        payload: json!({ "turnId": input.turn_id, "activeSubagents": [] }),
    });
    for item in items.into_iter().flatten() {
        bus.emit(ThreadEventEnvelope {
            event_type: ThreadEventType::ThreadItemCompleted,
            thread_id: input.thread_id.clone(),
            timestamp: now_rfc3339(),
            payload: json!({ "turnId": input.turn_id, "item": item }),
        });
    }
    bus.emit(ThreadEventEnvelope {
        event_type: ThreadEventType::ThreadTurnCompleted,
        thread_id: input.thread_id.clone(),
        timestamp: now_rfc3339(),
        payload: json!({ "turnId": input.turn_id, "status": status, "error": error }),
    });
}
