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
        // Notices codex-acp forwards with the same severity, title and text.
        "configWarning" => notice(
            "warning",
            p["summary"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("Configuration warning"),
            p["details"].as_str(),
        ),
        "deprecationNotice" => notice(
            "warning",
            p["summary"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("Deprecated configuration"),
            p["details"].as_str(),
        ),
        "warning" => notice(
            "warning",
            p["message"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("Codex warning"),
            None,
        ),
        "model/rerouted" => notice(
            "info",
            "Model rerouted",
            Some(&format!(
                "Switched from {} to {} ({}).",
                p["fromModel"].as_str().unwrap_or("?"),
                p["toModel"].as_str().unwrap_or("?"),
                p["reason"].as_str().unwrap_or("unspecified")
            )),
        ),
        // Only a hook that blocks or stops Codex changes the turn; it explains
        // an otherwise empty turn. Other hook results stay silent, as in ACP.
        "hook/completed" => {
            let run = &p["run"];
            let status = run["status"].as_str().unwrap_or("");
            if !matches!(status, "blocked" | "stopped") {
                return vec![];
            }
            let event = run["eventName"].as_str().unwrap_or("hook");
            let mut name = event.to_string();
            if let Some(first) = name.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            let title = if status == "blocked" {
                format!("{name} hook blocked {}", hook_target(event))
            } else {
                format!("{name} hook stopped the turn")
            };
            let reason = run["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|e| e["kind"] != "context")
                .filter_map(|e| e["text"].as_str().map(str::trim))
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            notice(
                "warning",
                &title,
                (!reason.is_empty()).then_some(reason.as_str()),
            )
        }
        "thread/tokenUsage/updated" => {
            let mut p = p.clone();
            p["tokenUsage"]["cumulative"] = json!(true);
            json!({"sessionUpdate":"usage_update","used":p["tokenUsage"]["last"]["totalTokens"],"size":p["tokenUsage"]["modelContextWindow"],"tokenUsage":p["tokenUsage"]})
        }
        "item/started" | "item/completed" => {
            let item = &p["item"];
            let kind = item["type"].as_str().unwrap_or("");
            // Text arrives through deltas; completed snapshots would duplicate it.
            // Bookkeeping items have no transcript row in codex-acp either.
            if matches!(
                kind,
                "agentMessage"
                    | "reasoning"
                    | "userMessage"
                    | "sleep"
                    | "functionCallOutput"
                    | "hookPrompt"
                    | "enteredReviewMode"
            ) {
                return vec![];
            }
            // A finished review is the reviewer's answer.
            if kind == "exitedReviewMode" {
                let review = item["review"].as_str().map(str::trim).unwrap_or("");
                return if method.ends_with("completed") && !review.is_empty() {
                    vec![mapper.apply(&json!({"sessionUpdate":"agent_message_chunk","messageId":item["id"],"content":{"type":"text","text":review}}))]
                } else {
                    vec![]
                };
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
            } else if let Some(update) = reported_tool(item, method.ends_with("completed")) {
                update
            } else {
                let (tool, title) = match kind {
                    "commandExecution" => {
                        ("execute", item["command"].as_str().unwrap_or("Command"))
                    }
                    "fileChange" => ("edit", "Edit files"),
                    "collabAgentToolCall" | "subAgentActivity" => ("agent", "Subagent"),
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
fn notice(severity: &str, title: &str, description: Option<&str>) -> Value {
    let mut notice = json!({"sessionUpdate":"notice","severity":severity,"title":title});
    if let Some(description) = description {
        notice["description"] = json!(description);
    }
    notice
}

fn hook_target(event: &str) -> &'static str {
    match event {
        "userPromptSubmit" => "the prompt",
        "preToolUse" | "permissionRequest" => "the tool call",
        "postToolUse" => "the tool result",
        "preModelRequest" => "the model request",
        "preCompact" => "the compaction",
        _ => "the action",
    }
}

fn web_search_title(item: &Value) -> String {
    let action = &item["action"];
    let query = item["query"].as_str().filter(|q| !q.is_empty());
    let titled = |label: &str, value: Option<&str>| match value {
        Some(value) => format!("{label}: {value}"),
        None => label.to_string(),
    };
    match action["type"].as_str() {
        None | Some("other") => titled("Web search", query),
        Some("search") => {
            let queries: Vec<&str> = action["queries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|q| !q.is_empty())
                .collect();
            let joined = queries.join(", ");
            let query = action["query"]
                .as_str()
                .or((!joined.is_empty()).then_some(joined.as_str()))
                .or(query);
            titled("Web search", query)
        }
        Some("openPage") => titled("Open page", action["url"].as_str()),
        Some("findInPage") => {
            let mut title = "Find in page".to_string();
            if let Some(pattern) = action["pattern"].as_str() {
                title.push_str(&format!(" for '{pattern}'"));
            }
            if let Some(url) = action["url"].as_str() {
                title.push_str(&format!(" in {url}"));
            }
            title
        }
        Some(_) => "Web search".into(),
    }
}

fn status(item: &Value, completed: bool) -> &'static str {
    if item["status"] == "failed" || item["success"] == false {
        "failed"
    } else if completed {
        "completed"
    } else {
        "in_progress"
    }
}

/// The ACP tool reports codex-acp builds for these items (its McpToolReporter,
/// DynamicToolReporter, WebSearchReporter, ImageViewReporter and
/// ImageGenerationReporter), so both protocols show the same rows.
fn reported_tool(item: &Value, completed: bool) -> Option<Value> {
    let id = &item["id"];
    let update = if completed {
        "tool_call_update"
    } else {
        "tool_call"
    };
    let report = match item["type"].as_str()? {
        "mcpToolCall" => {
            let mut report = json!({"kind":"execute","title":format!("mcp.{}.{}",item["server"].as_str().unwrap_or("server"),item["tool"].as_str().unwrap_or("tool")),
                "rawInput":{"server":item["server"],"tool":item["tool"],"arguments":item["arguments"]}});
            if !(item["result"].is_null() && item["error"].is_null()) {
                report["rawOutput"] = json!({"result":item["result"],"error":item["error"]});
            }
            report
        }
        "dynamicToolCall" => {
            let tool = item["tool"].as_str().unwrap_or("tool");
            let name = match item["namespace"].as_str().filter(|n| !n.is_empty()) {
                Some(namespace) => format!("{namespace}.{tool}"),
                None => tool.to_string(),
            };
            let mut report = json!({"kind":"execute","title":tool,"name":name,"rawInput":{"arguments":item["arguments"]}});
            if let Some(items) = item["contentItems"].as_array() {
                report["content"] = json!(items
                    .iter()
                    .filter_map(|c| Some(json!({"type":"content","content":match c["type"].as_str()? {
                        "inputText" => json!({"type":"text","text":c["text"]}),
                        "inputImage" => json!({"type":"resource_link","uri":c["imageUrl"],"name":"image"}),
                        "inputAudio" => json!({"type":"resource_link","uri":c["audioUrl"],"name":"audio"}),
                        _ => return None,
                    }})))
                    .collect::<Vec<_>>());
            }
            report
        }
        "webSearch" => json!({"kind":"search","title":web_search_title(item),
            "rawInput":{"type":item["type"],"id":id,"query":item["query"],"action":item["action"]}}),
        "imageView" => {
            let path = item["path"].as_str().unwrap_or("");
            json!({"kind":"read","name":"view_image","title":format!("View Image {path}"),
                "content":[{"type":"content","content":{"type":"resource_link","name":path,"uri":path}}],
                "locations":[{"path":path}],"rawInput":{"path":path}})
        }
        "imageGeneration" => {
            let mut content = vec![];
            if let Some(prompt) = item["revisedPrompt"]
                .as_str()
                .filter(|p| !p.trim().is_empty())
            {
                content.push(json!({"type":"content","content":{"type":"text","text":format!("Revised prompt: {prompt}")}}));
            }
            let saved = item["savedPath"].as_str().filter(|p| !p.trim().is_empty());
            if let Some(data) = item["result"].as_str().filter(|r| !r.trim().is_empty()) {
                let mut image = json!({"type":"image","data":data,"mimeType":"image/png"});
                if let Some(saved) = saved {
                    image["uri"] = json!(saved);
                }
                content.push(json!({"type":"content","content":image}));
            } else if let Some(saved) = saved {
                content.push(json!({"type":"content","content":{"type":"resource_link","name":saved,"uri":saved}}));
            }
            let mut output = json!({"status":item["status"],"revisedPrompt":item["revisedPrompt"],"savedPath":item["savedPath"]});
            if !item["failure"].is_null() {
                output["failure"] = item["failure"].clone();
            }
            json!({"kind":"other","title":"Image generation","rawInput":{"id":id},"content":content,"rawOutput":output})
        }
        _ => return None,
    };
    let mut report = report;
    report["sessionUpdate"] = json!(update);
    report["toolCallId"] = id.clone();
    report["status"] = json!(status(item, completed));
    Some(report)
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
            // codex-acp reports MCP calls as `execute` tools named mcp.server.tool.
            ("mcpToolCall", "commandExecution"),
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
    fn extended_items_render_like_codex_acp() {
        let row = |item: Value| {
            let mut m = TurnMapper::new("t");
            apply(
                &mut m,
                &json!({"method":"item/started","params":{"item":item.clone()}}),
            );
            apply(
                &mut m,
                &json!({"method":"item/completed","params":{"item":item}}),
            );
            m.finish(false)
        };
        let mcp = row(
            json!({"id":"m","type":"mcpToolCall","server":"docs","tool":"search","arguments":{"q":"x"},"result":{"content":[{"type":"text","text":"found"}]},"error":null,"status":"completed"}),
        );
        assert_eq!(
            (mcp[0].kind.as_str(), mcp[0].text.as_str()),
            ("commandExecution", "mcp.docs.search")
        );
        assert!(mcp[0].detail_text.as_deref().unwrap().contains("found"));
        let web = row(
            json!({"id":"w","type":"webSearch","query":"rust","action":{"type":"search","query":"rust async"}}),
        );
        assert_eq!(
            (web[0].kind.as_str(), web[0].text.as_str()),
            ("webSearch", "Web search: rust async")
        );
        let page = row(
            json!({"id":"p","type":"webSearch","query":"","action":{"type":"openPage","url":"https://example.com"}}),
        );
        assert_eq!(page[0].text, "Open page: https://example.com");
        let view = row(json!({"id":"v","type":"imageView","path":"/w/shot.png"}));
        // The transcript names a read by its location, as for codex-acp's report.
        assert_eq!(
            (view[0].kind.as_str(), view[0].text.as_str()),
            ("fileRead", "/w/shot.png")
        );
        let image = row(
            json!({"id":"i","type":"imageGeneration","status":"completed","revisedPrompt":"a red square","savedPath":"/w/out.png"}),
        );
        assert_eq!(image[0].text, "Image generation");
        let detail = image[0].detail_text.as_deref().unwrap();
        assert!(
            detail.contains("Revised prompt: a red square") && detail.contains("/w/out.png"),
            "{detail}"
        );
        let dynamic = row(
            json!({"id":"d","type":"dynamicToolCall","tool":"lookup","namespace":"app","arguments":{},"contentItems":[{"type":"inputText","text":"looked up"}],"success":true,"status":"completed"}),
        );
        assert_eq!(
            (dynamic[0].kind.as_str(), dynamic[0].text.as_str()),
            ("commandExecution", "lookup")
        );
        assert!(dynamic[0]
            .detail_text
            .as_deref()
            .unwrap()
            .contains("looked up"));
        let failed = row(
            json!({"id":"f","type":"dynamicToolCall","tool":"lookup","success":false,"status":"failed"}),
        );
        assert_eq!(failed[0].status.as_deref(), Some("failed"));
        for hidden in [
            "sleep",
            "functionCallOutput",
            "hookPrompt",
            "enteredReviewMode",
        ] {
            // Only the mapper's empty-turn placeholder remains.
            let rows = row(json!({"id":"h","type":hidden,"review":"r","output":"o"}));
            assert!(rows.iter().all(|i| i.text == "(no output)"), "{hidden}");
        }
        let review =
            row(json!({"id":"r","type":"exitedReviewMode","review":"  Looks good: one nit.  "}));
        assert_eq!(
            (review[0].kind.as_str(), review[0].text.as_str()),
            ("agentMessage", "Looks good: one nit.")
        );
        assert!(
            row(json!({"id":"r","type":"exitedReviewMode","review":" "}))
                .iter()
                .all(|i| i.text == "(no output)")
        );
    }

    #[test]
    fn hooks_reroutes_and_warnings_become_codex_acp_notices() {
        let notices = |events: Vec<Value>| {
            let mut m = TurnMapper::new("t");
            for event in events {
                apply(&mut m, &event);
            }
            m.finish(false)
        };
        let items = notices(vec![
            json!({"method":"hook/completed","params":{"run":{"eventName":"preToolUse","status":"blocked","entries":[{"kind":"feedback","text":"no rm"},{"kind":"context","text":"hidden"}]}}}),
            json!({"method":"hook/completed","params":{"run":{"eventName":"postToolUse","status":"completed","entries":[]}}}),
            json!({"method":"hook/completed","params":{"run":{"eventName":"stop","status":"stopped","entries":[]}}}),
            json!({"method":"model/rerouted","params":{"fromModel":"a","toModel":"b","reason":"highRiskCyberActivity"}}),
            json!({"method":"configWarning","params":{"summary":" ","details":"unknown key"}}),
            json!({"method":"warning","params":{"message":"slow disk"}}),
            json!({"method":"deprecationNotice","params":{"summary":"old flag","details":null}}),
        ]);
        let texts: Vec<String> = items
            .iter()
            .map(|i| format!("{} | {}", i.text, i.detail_text.clone().unwrap_or_default()))
            .collect();
        let all = texts.join("\n");
        for expected in [
            "PreToolUse hook blocked the tool call",
            "no rm",
            "Stop hook stopped the turn",
            "Model rerouted",
            "Switched from a to b (highRiskCyberActivity).",
            "Configuration warning",
            "unknown key",
            "slow disk",
            "old flag",
        ] {
            assert!(all.contains(expected), "{expected} missing from {all}");
        }
        assert!(
            !all.contains("hidden") && !all.contains("PostToolUse"),
            "{all}"
        );
    }
}
