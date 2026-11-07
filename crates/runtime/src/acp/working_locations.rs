//! Structured execution facts only. Shell text is not a reliable cwd protocol.
use serde_json::{json, Value};
pub(super) fn extract(body: &Value) -> Vec<Value> {
    let agent = body
        .pointer("/_meta/agentId")
        .or_else(|| body.pointer("/rawInput/agent_id"))
        .and_then(Value::as_str);
    let mut locations = Vec::new();
    for (pointer, source) in [
        ("/_meta/terminal_info/cwd", "terminal"),
        ("/rawInput/cwd", "command"),
        ("/rawInput/new_cwd", "cwdChanged"),
    ] {
        if let Some(path) = body.pointer(pointer).and_then(Value::as_str) {
            locations.push(json!({"path":path,"source":source,"agentId":agent}));
        }
    }
    for key in ["path", "file_path", "target_file", "file"] {
        if let Some(path) = body
            .get("rawInput")
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
        {
            locations.push(json!({"path":path,"source":"file","agentId":agent}));
        }
    }
    if let Some(list) = body["locations"].as_array() {
        for location in list.iter().take(8) {
            if let Some(path) = location["path"].as_str() {
                locations.push(json!({"path":path,"source":"file","agentId":agent}));
            }
        }
    }
    locations.truncate(8);
    locations
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uses_execution_metadata_without_guessing_shell_cd() {
        let locations = extract(
            &json!({"rawInput":{"command":"cd /other && git status","cwd":"/original"},"_meta":{"terminal_info":{"cwd":"/original"},"agentId":"child"},"locations":[{"path":"/other/a.rs"}]}),
        );
        assert!(locations
            .iter()
            .any(|v| v["path"] == "/original" && v["agentId"] == "child"));
        assert!(locations.iter().any(|v| v["path"] == "/other/a.rs"));
        assert!(!locations.iter().any(|v| v["path"] == "/other"));
    }
}
