use pockymoe_protocol::{
    ThreadDto, ThreadEventEnvelope, ThreadEventType, ThreadHistoryItemDto, ThreadItemKind,
    ThreadStatus, ThreadTurnDto, TurnStatus,
};
use serde_json::{json, Value};

#[test]
fn discriminants_round_trip_known_and_unknown_strings() {
    macro_rules! check {
        ($ty:ident) => {
            for wire in $ty::KNOWN_VALUES {
                let value: $ty = serde_json::from_value(json!(wire)).unwrap();
                assert!(!matches!(value, $ty::Unknown(_)));
                assert_eq!(value.as_str(), *wire);
                assert_eq!(serde_json::to_value(value).unwrap(), json!(wire));
            }
            for wire in ["future.value", "", "Future/值"] {
                let value: $ty = serde_json::from_value(json!(wire)).unwrap();
                assert_eq!(value, $ty::Unknown(wire.into()));
                assert_eq!(serde_json::to_value(value).unwrap(), json!(wire));
            }
            assert!(serde_json::from_value::<$ty>(json!(42)).is_err());
            assert!(serde_json::from_value::<$ty>(Value::Null).is_err());
        };
    }
    check!(ThreadItemKind);
    check!(ThreadStatus);
    check!(TurnStatus);
    check!(ThreadEventType);
}

#[test]
fn item_extensions_and_event_payload_round_trip_without_wire_changes() {
    for kind in ["generic", "imageView", "futureItem"] {
        let wire = json!({
            "id": "i", "createdAt": "2030-01-01T00:00:00Z", "kind": kind,
            "text": "body", "status": "providerStatus", "sourceTurnId": "t",
            "detailText": "detail", "sequence": 1,
            "origin": "nativeBackgroundWait", "futurePayload": {"x": [1, true]}
        });
        let item: ThreadHistoryItemDto = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(item.extra["futurePayload"], json!({"x": [1, true]}));
        assert_eq!(serde_json::to_value(item).unwrap(), wire);
    }
    for event_type in ["thread.harness.ready", "future.event"] {
        let wire = json!({
            "type": event_type, "threadId": "t", "timestamp": "now",
            "payload": {"untyped": [1, {"future": true}]}
        });
        let event: ThreadEventEnvelope = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(event).unwrap(), wire);
    }
}

#[test]
fn thread_and_turn_dtos_accept_future_statuses() {
    let turn: ThreadTurnDto = serde_json::from_value(json!({
        "id": "t", "startedAt": null, "status": "futureTurn", "error": null,
        "items": []
    }))
    .unwrap();
    assert_eq!(turn.status, TurnStatus::Unknown("futureTurn".into()));
    assert_eq!(serde_json::to_value(turn).unwrap()["status"], "futureTurn");
    let thread: ThreadDto = serde_json::from_value(json!({
        "id": "t", "workspaceId": "w", "provider": "acp", "source": "supervisor",
        "title": "title", "collaborationMode": "default", "approvalMode": "guarded",
        "status": "futureThread", "isLoaded": false, "isPinned": false,
        "createdAt": "now", "updatedAt": "now"
    }))
    .unwrap();
    assert_eq!(thread.status, ThreadStatus::Unknown("futureThread".into()));
    assert_eq!(
        serde_json::to_value(thread).unwrap()["status"],
        "futureThread"
    );
}
