//! Translate app-server notifications into the established transcript vocabulary.
use crate::acp::{mapper::MappedUpdate, TurnMapper};
use serde_json::{json, Value};
pub fn apply(mapper: &mut TurnMapper, event: &Value) -> Vec<MappedUpdate> {
    let p = &event["params"];
    let method = event["method"].as_str().unwrap_or("");
    let update = match method {
        "item/agentMessage/delta"
        | "item/reasoning/summaryTextDelta"
        | "item/reasoning/textDelta" => {
            json!({"sessionUpdate":if method.contains("agentMessage"){"agent_message_chunk"}else{"agent_thought_chunk"},"messageId":p["itemId"],"content":{"type":"text","text":p["delta"]}})
        }
        "item/commandExecution/outputDelta" => {
            json!({"sessionUpdate":"tool_call_update","toolCallId":p["itemId"],"_meta":{"terminal_output_delta":{"data":p["delta"],"terminal_id":p["itemId"]}}})
        }
        "turn/plan/updated" => {
            json!({"sessionUpdate":"plan","entries":p["plan"].as_array().into_iter().flatten().map(|v|json!({"content":v["step"],"status":match v["status"].as_str(){Some("inProgress")=>"in_progress",Some("completed")=>"completed",_=>"pending"},"priority":"medium"})).collect::<Vec<_>>()})
        }
        "thread/goal/updated" => {
            json!({"sessionUpdate":"session_info_update","_meta":{"goal":p["goal"]}})
        }
        "thread/compacted" => json!({"sessionUpdate":"compaction_update","status":"completed"}),
        "thread/tokenUsage/updated" => {
            let mut p = p.clone();
            p["tokenUsage"]["cumulative"] = json!(true);
            json!({"sessionUpdate":"usage_update","used":p["tokenUsage"]["last"]["totalTokens"],"size":p["tokenUsage"]["modelContextWindow"],"tokenUsage":p["tokenUsage"]})
        }
        "item/started" | "item/completed" => {
            let item = &p["item"];
            let kind = item["type"].as_str().unwrap_or("");
            // Text arrives through deltas; completed snapshots would duplicate it.
            if matches!(kind, "agentMessage" | "reasoning" | "userMessage") {
                return vec![];
            }
            if kind == "plan" {
                return if method.ends_with("completed") {
                    vec![mapper.apply(&json!({"sessionUpdate":"agent_message_chunk","messageId":item["id"],"content":{"type":"text","text":item["text"]}}))]
                } else {
                    vec![]
                };
            }
            if kind == "contextCompaction" {
                json!({"sessionUpdate":"compaction_update","status":if method.ends_with("completed"){"completed"}else{"running"}})
            } else {
                let (tool, title) = match kind {
                    "commandExecution" => {
                        ("execute", item["command"].as_str().unwrap_or("Command"))
                    }
                    "fileChange" => ("edit", "Edit files"),
                    "webSearch" => ("fetch", "Web search"),
                    "collabAgentToolCall" | "subAgentActivity" => ("agent", "Subagent"),
                    "mcpToolCall" | "dynamicToolCall" => {
                        ("other", item["tool"].as_str().unwrap_or("Tool"))
                    }
                    "imageGeneration" | "imageView" => ("other", "Image"),
                    _ => ("other", kind),
                };
                let content: Vec<Value> = if kind == "fileChange" {
                    item["changes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|c| json!({"type":"diff","path":c["path"],"diff":c["diff"]}))
                        .collect()
                } else {
                    vec![]
                };
                json!({"sessionUpdate":if method.ends_with("completed"){"tool_call_update"}else{"tool_call"},"toolCallId":item["id"],"kind":tool,"name":item["tool"].as_str().unwrap_or(kind),"title":title,"status":if item["status"]=="failed" || item["success"]==false{"failed"}else if method.ends_with("completed"){"completed"}else{"in_progress"},"rawInput":item,"rawOutput":if kind=="commandExecution"{json!({"formatted_output":item["aggregatedOutput"],"exit_code":item["exitCode"]})}else{item.clone()},"content":content})
            }
        }
        "error" => {
            json!({"sessionUpdate":"tool_call_update","toolCallId":"native-error","kind":"other","title":"Codex error","rawOutput":p["error"],"status":"failed"})
        }
        _ => return vec![],
    };
    vec![mapper.apply(&update)]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_deltas_keep_native_message_identity_without_duplicate_completion() {
        let mut m = TurnMapper::new("t");
        apply(
            &mut m,
            &json!({"method":"item/agentMessage/delta","params":{"itemId":"a","delta":"hello"}}),
        );
        apply(
            &mut m,
            &json!({"method":"item/completed","params":{"item":{"id":"a","type":"agentMessage","text":"hello"}}}),
        );
        let items = m.finish(false);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "hello");
    }
    #[test]
    fn command_and_file_notifications_use_existing_item_kinds() {
        for (native, expected) in [
            ("commandExecution", "commandExecution"),
            ("fileChange", "fileChange"),
            ("webSearch", "webSearch"),
            ("mcpToolCall", "toolCall"),
            ("collabAgentToolCall", "agentToolCall"),
        ] {
            let mut m = TurnMapper::new("t");
            apply(
                &mut m,
                &json!({"method":"item/completed","params":{"item":{"id":"x","type":native,"command":if native=="commandExecution"{json!("ls")}else{Value::Null},"changes":[]}}}),
            );
            assert_eq!(m.finish(false)[0].kind, expected, "{native}");
        }
    }

    #[test]
    fn native_usage_plan_goal_compaction_and_error_survive_mapping() {
        let mut m = TurnMapper::new("t");
        let usage = apply(
            &mut m,
            &json!({"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"totalTokens":100},"last":{"totalTokens":20},"modelContextWindow":200}}}),
        );
        assert_eq!(
            usage[0].usage.as_ref().unwrap()["tokenUsage"]["cumulative"],
            true
        );
        assert_eq!(usage[0].usage.as_ref().unwrap()["used"], 20);
        let plan = apply(
            &mut m,
            &json!({"method":"turn/plan/updated","params":{"plan":[{"step":"Test","status":"inProgress"}]}}),
        );
        assert_eq!(
            plan[0].plan.as_ref().unwrap()[0],
            ("Test".into(), "in_progress".into())
        );
        let goal = apply(
            &mut m,
            &json!({"method":"thread/goal/updated","params":{"goal":{"objective":"Ship","status":"paused"}}}),
        );
        assert_eq!(
            goal[0].goal.as_ref().unwrap().as_ref().unwrap().status,
            "paused"
        );
        apply(&mut m, &json!({"method":"thread/compacted","params":{}}));
        apply(
            &mut m,
            &json!({"method":"error","params":{"error":{"message":"quota exhausted"}}}),
        );
        let items = m.finish(false);
        assert!(items.iter().any(|i| i.kind == "contextCompaction"));
        assert!(items.iter().any(|i| i.status.as_deref() == Some("failed")
            && i.detail_text
                .as_deref()
                .is_some_and(|s| s.contains("quota exhausted"))));
    }

    #[test]
    fn extended_native_items_keep_their_details() {
        for kind in [
            "imageGeneration",
            "imageView",
            "dynamicToolCall",
            "subAgentActivity",
            "hookPrompt",
            "functionCallOutput",
            "sleep",
            "enteredReviewMode",
            "exitedReviewMode",
        ] {
            let mut m = TurnMapper::new("t");
            apply(
                &mut m,
                &json!({"method":"item/completed","params":{"item":{"id":"item","type":kind,"savedPath":"/tmp/picture.png","output":"native detail"}}}),
            );
            let items = m.finish(false);
            assert_eq!(items.len(), 1, "{kind}");
            assert!(
                items[0]
                    .detail_text
                    .as_deref()
                    .unwrap()
                    .contains("native detail"),
                "{kind}"
            );
        }
    }
}
