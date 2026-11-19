use serde_json::Value;

// Presence comes from authenticated live supervisor sockets, never a share's
// creation-time snapshot or another device selected by the browser. The rows
// already carry the device's current name from the list query.
pub(super) fn mark_connected(connected: impl Fn(&str) -> bool, shares: &mut [Value]) {
    for share in shares {
        let online = share["deviceId"].as_str().is_some_and(&connected);
        share["deviceConnected"] = Value::Bool(online);
    }
}
