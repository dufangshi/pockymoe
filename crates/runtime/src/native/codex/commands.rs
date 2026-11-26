//! The slash commands codex-acp offers, run natively. `/review…` starts Codex's
//! own review mode (`review/start`) instead of sending the words to the model;
//! `/mcp`, `/skills` and `/status` answer from app-server data without a model
//! call. ACP lists the same commands (its `available_commands_update`).
use pockymoe_protocol::ToolboxItemDto;
use serde_json::{json, Value};

pub(super) enum Command {
    /// A `review/start` target.
    Review(Value),
    Mcp,
    Skills,
    Status,
}

pub(super) fn parse(prompt: &str) -> Option<Command> {
    let prompt = prompt.trim();
    let (name, rest) = prompt
        .strip_prefix('/')?
        .split_once(char::is_whitespace)
        .map(|(n, r)| (n, r.trim()))
        .unwrap_or((prompt.strip_prefix('/')?, ""));
    Some(match name {
        "review" if rest.is_empty() => Command::Review(json!({"type":"uncommittedChanges"})),
        "review" => Command::Review(json!({"type":"custom","instructions":rest})),
        "review-branch" if !rest.is_empty() => {
            Command::Review(json!({"type":"baseBranch","branch":rest}))
        }
        "review-commit" if !rest.is_empty() => {
            let (sha, title) = rest
                .split_once(char::is_whitespace)
                .map(|(s, t)| (s, Some(t.trim())))
                .unwrap_or((rest, None));
            Command::Review(json!({"type":"commit","sha":sha,"title":title}))
        }
        "mcp" if rest.is_empty() => Command::Mcp,
        "skills" if rest.is_empty() => Command::Skills,
        "status" if rest.is_empty() => Command::Status,
        _ => return None,
    })
}

pub(super) fn toolbox() -> Vec<ToolboxItemDto> {
    [
        (
            "review",
            "Review uncommitted changes, or review with custom instructions.",
        ),
        ("review-branch", "Review changes relative to a base branch."),
        ("review-commit", "Review a specific commit."),
        ("mcp", "Show the status of the MCP servers."),
        ("skills", "List available skills."),
        ("status", "Display session configuration and token usage."),
    ]
    .into_iter()
    .map(|(name, description)| ToolboxItemDto {
        action: "prompt".into(),
        command: format!("/{name}"),
        label: name.into(),
        description: Some(description.into()),
        panel: None,
    })
    .collect()
}

pub(super) fn render_mcp(pages: &[Value]) -> String {
    let servers: Vec<&Value> = pages
        .iter()
        .flat_map(|p| p["data"].as_array().into_iter().flatten())
        .collect();
    if servers.is_empty() {
        return "No MCP servers are configured for this session.".into();
    }
    let mut out = String::from("| Server | State | Tools | Sign-in |\n| --- | --- | --- | --- |\n");
    for s in servers {
        let state = s["runtimeStatus"].as_str().unwrap_or("unknown");
        let tools = s["tools"].as_object().map(|t| t.len()).unwrap_or(0);
        let error = s["toolsError"]
            .as_str()
            .map(|e| format!(" ({e})"))
            .unwrap_or_default();
        out.push_str(&format!(
            "| {} | {state}{error} | {tools} | {} |\n",
            s["name"].as_str().unwrap_or("?"),
            s["authStatus"].as_str().unwrap_or("unknown")
        ));
    }
    out
}

pub(super) fn render_skills(response: &Value) -> String {
    let mut lines = vec![];
    for entry in response["data"].as_array().into_iter().flatten() {
        for skill in entry["skills"].as_array().into_iter().flatten() {
            if skill["enabled"] == false {
                continue;
            }
            lines.push(format!(
                "- **{}** — {}",
                skill["name"].as_str().unwrap_or("?"),
                skill["shortDescription"]
                    .as_str()
                    .or(skill["description"].as_str())
                    .unwrap_or("")
            ));
        }
    }
    if lines.is_empty() {
        "No skills are available in this session.".into()
    } else {
        lines.join("\n")
    }
}

pub(super) fn render_status(
    settings: &crate::actor::SessionSettings,
    policy: &Value,
    cwd: &str,
    limits: Option<&Value>,
) -> String {
    let mut lines = vec![
        format!(
            "- Model: {}",
            settings.model.as_deref().unwrap_or("default")
        ),
        format!(
            "- Effort: {}",
            settings.effort.as_deref().unwrap_or("default")
        ),
        format!(
            "- Approval: {}",
            policy["approvalPolicy"].as_str().unwrap_or("?")
        ),
        format!("- Sandbox: {}", policy["sandbox"].as_str().unwrap_or("?")),
        format!(
            "- Mode: {}{}",
            settings.collaboration_mode.as_deref().unwrap_or("default"),
            if settings.performance_mode == Some(true) {
                ", fast"
            } else {
                ""
            }
        ),
        format!("- Directory: {cwd}"),
    ];
    for window in limits
        .and_then(|l| l["windows"].as_array())
        .into_iter()
        .flatten()
    {
        lines.push(format!(
            "- {} limit: {:.0}% used, resets {}",
            window["label"].as_str().unwrap_or("?"),
            window["usedPercent"].as_f64().unwrap_or(0.0),
            window["resetsAt"].as_str().unwrap_or("?")
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_acp_commands_and_leaves_other_text_alone() {
        assert!(
            matches!(parse("/review"), Some(Command::Review(t)) if t["type"] == "uncommittedChanges")
        );
        assert!(
            matches!(parse(" /review focus on errors "), Some(Command::Review(t)) if t["instructions"] == "focus on errors")
        );
        assert!(
            matches!(parse("/review-branch main"), Some(Command::Review(t)) if t["branch"] == "main")
        );
        assert!(
            matches!(parse("/review-commit abc123 Fix login"), Some(Command::Review(t)) if t["sha"] == "abc123" && t["title"] == "Fix login")
        );
        assert!(parse("/review-branch").is_none());
        assert!(matches!(parse("/mcp"), Some(Command::Mcp)));
        assert!(parse("/mcp please").is_none());
        assert!(parse("review the code").is_none());
        assert!(parse("/unknown").is_none());
        assert_eq!(toolbox().len(), 6);
    }

    #[test]
    fn renders_server_skill_and_status_answers() {
        let mcp = render_mcp(&[
            json!({"data":[{"name":"docs","runtimeStatus":"connected","authStatus":"oAuth","tools":{"a":{},"b":{}}},{"name":"db","runtimeStatus":"failed","toolsError":"spawn ENOENT","authStatus":"unsupported","tools":{}}]}),
        ]);
        assert!(
            mcp.contains("| docs | connected | 2 | oAuth |")
                && mcp.contains("failed (spawn ENOENT)"),
            "{mcp}"
        );
        assert_eq!(
            render_mcp(&[]),
            "No MCP servers are configured for this session."
        );
        let skills = render_skills(
            &json!({"data":[{"skills":[{"name":"pdf","description":"PDFs","enabled":true},{"name":"off","description":"x","enabled":false}]}]}),
        );
        assert_eq!(skills, "- **pdf** — PDFs");
        let status = render_status(
            &crate::actor::SessionSettings {
                model: Some("gpt-5.5".into()),
                effort: Some("low".into()),
                ..Default::default()
            },
            &json!({"approvalPolicy":"on-request","sandbox":"workspace-write"}),
            "/w",
            Some(&json!({"windows":[{"label":"5h","usedPercent":12.0,"resetsAt":"later"}]})),
        );
        assert!(
            status.contains("Model: gpt-5.5") && status.contains("5h limit: 12% used"),
            "{status}"
        );
    }
}
