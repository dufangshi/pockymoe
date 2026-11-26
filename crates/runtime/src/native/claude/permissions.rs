//! Claude's `can_use_tool` callback as a Pockymoe request, and the answer back.
//!
//! The choices follow claude-agent-acp's (`permissions/options*.js`): "Yes"
//! allows once, "Yes, and …" also applies Claude's own `permission_suggestions`
//! (a session edit mode, a rule for this command, a directory), and "No" denies.
//! The label names exactly what the suggestion changes; a suggestion the label
//! cannot describe truthfully is not offered. ExitPlanMode offers plan
//! approval with or without auto-accepted edits, or to keep planning.
//!
//! An answer that matches no offered choice is a denial, never an approval.
use serde_json::{json, Value};

pub(super) const YES: &str = "Yes";
pub(super) const NO: &str = "No";
const PLAN_YES: &str = "Yes, manually approve edits";
const PLAN_AUTO: &str = "Yes, and use auto mode";
const PLAN_EDITS: &str = "Yes, and auto-accept edits";
const PLAN_NO: &str = "No, keep planning";

#[derive(Clone, Debug)]
pub(super) struct Choice {
    pub label: String,
    pub kind: &'static str,
    pub updates: Option<Value>,
}

/// Choices for a tool permission; empty for AskUserQuestion.
pub(super) fn choices(tool: &str, request: &Value, auto: bool) -> Vec<Choice> {
    if tool == "AskUserQuestion" {
        return Vec::new();
    }
    if tool == "ExitPlanMode" {
        // As claude-agent-acp words them; "clear context" restarts the query
        // and bypass is the product policy's call, so neither is offered.
        // Claude would otherwise return to the mode it had before planning.
        let mut plan = vec![choice(
            PLAN_YES,
            "allow_once",
            Some(json!([{"type":"setMode","mode":"default","destination":"session"}])),
        )];
        if auto {
            plan.push(choice(
                PLAN_AUTO,
                "allow_always",
                Some(json!([{"type":"setMode","mode":"auto","destination":"session"}])),
            ));
        }
        plan.extend([
            choice(
                PLAN_EDITS,
                "allow_always",
                Some(json!([{"type":"setMode","mode":"acceptEdits","destination":"session"}])),
            ),
            choice(PLAN_NO, "reject_once", None),
        ]);
        return plan;
    }
    let mut choices = vec![choice(YES, "allow_once", None)];
    if request["suppress_always_allow_rule"] != true {
        if let Some((label, updates)) = durable(tool, &request["permission_suggestions"]) {
            choices.push(choice(&label, "allow_always", Some(updates)));
        }
    }
    choices.push(choice(NO, "reject_once", None));
    if request["default_to_no"] == true {
        // Claude asks to open on the decline: lead with it, as the adapter does.
        choices.rotate_right(1);
    }
    choices
}

fn choice(label: &str, kind: &'static str, updates: Option<Value>) -> Choice {
    Choice {
        label: label.into(),
        kind,
        updates,
    }
}

/// A truthful label for Claude's suggested permission update, or none.
fn durable(tool: &str, suggestions: &Value) -> Option<(String, Value)> {
    let updates = suggestions
        .as_array()
        .filter(|u| !u.is_empty() && u.len() <= 100)?;
    let mut commands = Vec::new();
    let mut directories = Vec::new();
    let mut reads = Vec::new();
    let mut edits_mode = false;
    let mut broad_tool = false;
    for update in updates {
        match update["type"].as_str()? {
            "setMode" if update["mode"] == "acceptEdits" && update["destination"] == "session" => {
                edits_mode = true
            }
            "addDirectories" => {
                for d in update["directories"].as_array()? {
                    directories.push(d.as_str()?.to_owned());
                }
            }
            "addRules" if update["behavior"] == "allow" => {
                for rule in update["rules"].as_array()? {
                    let name = rule["toolName"].as_str()?;
                    match rule["ruleContent"].as_str().map(str::trim) {
                        Some(content) if !content.is_empty() && name == "Read" => {
                            reads.push(content.trim_end_matches("/**").to_owned())
                        }
                        Some(content) if !content.is_empty() && name == tool => {
                            commands.push(content.trim_end_matches(":*").to_owned())
                        }
                        None if name == tool => broad_tool = true,
                        _ => return None,
                    }
                }
            }
            _ => return None,
        }
    }
    let label = if edits_mode && commands.is_empty() && reads.is_empty() && !broad_tool {
        if directories.is_empty() {
            "Yes, allow all edits during this session".to_owned()
        } else {
            format!("Yes, allow all edits and access to {}", list(&directories))
        }
    } else if broad_tool && commands.is_empty() && !edits_mode {
        format!("Yes, and don't ask again for {tool}")
    } else if !commands.is_empty() && !edits_mode && !broad_tool {
        let what = if commands.len() == 1 && commands[0].chars().count() <= 60 {
            format!("`{}`", commands[0])
        } else {
            "similar".to_owned()
        };
        let mut label = format!("Yes, and don't ask again for {what} commands");
        let paths: Vec<_> = directories.iter().chain(&reads).cloned().collect();
        if !paths.is_empty() {
            label.push_str(&format!(" and access to {}", list(&paths)));
        }
        label
    } else if commands.is_empty() && !edits_mode && !broad_tool && !reads.is_empty() {
        format!("Yes, allow reading from {}", list(&reads))
    } else if commands.is_empty() && !edits_mode && !broad_tool && !directories.is_empty() {
        format!("Yes, and always allow access to {}", list(&directories))
    } else {
        return None;
    };
    Some((label, Value::Array(updates.clone())))
}

fn list(paths: &[String]) -> String {
    let names: Vec<_> = paths
        .iter()
        .map(|p| {
            std::path::Path::new(p)
                .file_name()
                .map(|n| format!("{}/", n.to_string_lossy()))
                .unwrap_or_else(|| p.clone())
        })
        .collect();
    match names.len() {
        1 => names[0].clone(),
        2 => format!("{} and {}", names[0], names[1]),
        n => format!("{}, {} and {} more", names[0], names[1], n - 2),
    }
}

/// The card's question: what Claude wants to do, in its own words.
pub(super) fn summary(tool: &str, request: &Value) -> String {
    let input = &request["input"];
    let target = match tool {
        "Bash" | "PowerShell" => input["command"].as_str(),
        "Edit" | "Write" | "MultiEdit" | "Read" => input["file_path"].as_str(),
        "NotebookEdit" => input["notebook_path"].as_str(),
        "WebFetch" => input["url"].as_str(),
        "WebSearch" => input["query"].as_str(),
        "ExitPlanMode" => return "Approve Plan".into(),
        _ => None,
    };
    let name = request["display_name"]
        .as_str()
        .or(request["title"].as_str())
        .unwrap_or(tool);
    match target {
        Some(target) => format!("{name}: {}", truncate(target, 400)),
        None => name.to_owned(),
    }
}

/// Why Claude asks: its description and the decision reason.
pub(super) fn description(request: &Value) -> Option<String> {
    let parts: Vec<_> = [
        request["description"].as_str(),
        request["decision_reason"].as_str(),
        request["blocked_path"].as_str(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|p| !p.is_empty())
    .map(|p| truncate(p, 300))
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_owned()
    } else {
        format!("{}…", text.chars().take(limit).collect::<String>())
    }
}

pub(super) fn questions(choices: &[Choice], question: &str) -> Vec<Value> {
    vec![json!({
        "id":"permission","header":"Permission","question":question,
        "isOther":false,"isSecret":false,
        "options":choices.iter().map(|c| json!({"label":c.label,"description":c.kind.replace('_'," ")})).collect::<Vec<_>>()
    })]
}

/// The first answer label in the UI's `{question: {answers: [...]}}` envelope,
/// or the bare string an API client sent.
fn selected(answer: Option<&str>) -> Option<String> {
    let answer = answer?.trim();
    if answer.is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(answer) {
        Ok(Value::Object(map)) => map.values().find_map(|v| {
            v["answers"]
                .as_array()?
                .first()?
                .as_str()
                .map(str::to_owned)
        }),
        Ok(Value::String(text)) => Some(text),
        _ => Some(answer.to_owned()),
    }
}

/// Claude's `PermissionResult` for a tool permission answer.
pub(super) fn response(
    choices: &[Choice],
    allow: bool,
    answer: Option<&str>,
    input: &Value,
    tool_use: Option<&str>,
) -> Value {
    let picked = match selected(answer) {
        Some(label) => choices.iter().find(|c| c.label == label).cloned(),
        // No answer: the request API's plain allow/deny.
        None if allow => choices.iter().find(|c| c.kind == "allow_once").cloned(),
        None => None,
    };
    let mut result = match picked {
        Some(choice) if allow && choice.kind.starts_with("allow") => {
            let mut result = json!({"behavior":"allow","updatedInput":input});
            if let Some(updates) = choice.updates {
                result["updatedPermissions"] = updates;
            }
            result
        }
        Some(choice) if choice.label == PLAN_NO => {
            json!({"behavior":"deny","message":"The user wants to keep planning. Revise the plan; do not start implementing."})
        }
        _ => json!({"behavior":"deny","message":"The user denied this tool request"}),
    };
    if let Some(id) = tool_use {
        result["toolUseID"] = json!(id);
    }
    result
}

/// The request DTO's fields for a permission (not a question).
pub(super) fn dto_parts(
    tool: &str,
    request: &Value,
    choices: &[Choice],
) -> (String, Option<String>, Vec<Value>) {
    let question = summary(tool, request);
    let description = description(request).or_else(|| {
        // The plan itself, so a phone can decide without opening the row.
        (tool == "ExitPlanMode")
            .then(|| {
                request["input"]["plan"]
                    .as_str()
                    .map(|p| truncate(p.trim(), 300))
            })
            .flatten()
    });
    (tool.to_owned(), description, questions(choices, &question))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(c: &[Choice]) -> Vec<&str> {
        c.iter().map(|c| c.label.as_str()).collect()
    }

    #[test]
    fn rejecting_never_allows() {
        let request = json!({"permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}]});
        let c = choices("Edit", &request, false);
        assert_eq!(
            labels(&c),
            [YES, "Yes, allow all edits during this session", NO]
        );
        let input = json!({"file_path":"a"});
        // The UI's envelope with "No", while the HTTP API defaults allow=true.
        let ui = |label: &str| json!({"permission":{"answers":[label]}}).to_string();
        assert_eq!(
            response(&c, true, Some(&ui(NO)), &input, None)["behavior"],
            "deny"
        );
        assert_eq!(
            response(&c, true, Some(&ui("garbage")), &input, None)["behavior"],
            "deny"
        );
        assert_eq!(response(&c, false, None, &input, None)["behavior"], "deny");
        assert_eq!(
            response(&c, false, Some(&ui(YES)), &input, None)["behavior"],
            "deny"
        );
        let yes = response(&c, true, Some(&ui(YES)), &input, Some("t"));
        assert_eq!(yes["behavior"], "allow");
        assert!(yes.get("updatedPermissions").is_none());
        assert_eq!(yes["toolUseID"], "t");
        let always = response(&c, true, Some(&ui(&c[1].label)), &input, None);
        assert_eq!(always["updatedPermissions"][0]["mode"], "acceptEdits");
        assert_eq!(response(&c, true, None, &input, None)["behavior"], "allow");
    }

    #[test]
    fn labels_name_exactly_what_is_remembered() {
        let bash = json!({"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"npm test:*"}],"behavior":"allow","destination":"localSettings"}]});
        assert_eq!(
            labels(&choices("Bash", &bash, false))[1],
            "Yes, and don't ask again for `npm test` commands"
        );
        // A rule for another tool, a deny rule, or an unknown update: no "always".
        for suggestions in [
            json!([{"type":"addRules","rules":[{"toolName":"Write","ruleContent":"x"}],"behavior":"allow","destination":"session"}]),
            json!([{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"x"}],"behavior":"deny","destination":"session"}]),
            json!([{"type":"removeDirectories","directories":["/x"],"destination":"session"}]),
            json!([]),
        ] {
            assert_eq!(
                labels(&choices(
                    "Bash",
                    &json!({"permission_suggestions":suggestions}),
                    false
                )),
                [YES, NO]
            );
        }
        let suppressed = json!({"suppress_always_allow_rule":true,"permission_suggestions":bash["permission_suggestions"]});
        assert_eq!(labels(&choices("Bash", &suppressed, false)), [YES, NO]);
        let first_no = json!({"default_to_no":true});
        assert_eq!(labels(&choices("Bash", &first_no, false)), [NO, YES]);
    }

    #[test]
    fn plan_exit_choices_and_questions_have_no_options() {
        let c = choices("ExitPlanMode", &json!({}), false);
        let input = json!({"plan":"p"});
        let ui = |label: &str| json!({"permission":{"answers":[label]}}).to_string();
        assert_eq!(
            response(&c, true, Some(&ui(PLAN_EDITS)), &input, None)["updatedPermissions"][0]
                ["mode"],
            "acceptEdits"
        );
        let keep = response(&c, true, Some(&ui(PLAN_NO)), &input, None);
        assert_eq!(keep["behavior"], "deny");
        assert!(keep["message"].as_str().unwrap().contains("keep planning"));
        assert!(choices("AskUserQuestion", &json!({}), false).is_empty());
        let request = json!({"display_name":"Bash","input":{"command":"ls"},"description":"List","decision_reason":"This command requires approval"});
        assert_eq!(summary("Bash", &request), "Bash: ls");
        assert_eq!(
            description(&request).unwrap(),
            "List · This command requires approval"
        );
    }
}
