use serde_json::{json, Value};

/// Product-level session policy. ACP only exposes a single `currentModeId`,
/// so these axes are resolved into one advertised mode id per harness.
#[derive(Debug, Clone, Default)]
pub struct ProductSessionPolicy {
    pub collaboration_mode: Option<String>,
    pub sandbox_mode: Option<String>,
    pub approval_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMode {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionChoice {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

impl ProductSessionPolicy {
    pub fn auto_approve(&self) -> bool {
        matches!(self.approval_mode.as_deref(), Some("yolo"))
            || matches!(self.sandbox_mode.as_deref(), Some("danger-full-access"))
    }

    pub fn allows_writes_outside_workspace(&self) -> bool {
        matches!(self.sandbox_mode.as_deref(), Some("danger-full-access"))
            || self.sandbox_mode.is_none() && self.approval_mode.as_deref() == Some("yolo")
    }

    pub fn rejects_writes(&self) -> bool {
        matches!(self.sandbox_mode.as_deref(), Some("read-only"))
            || matches!(self.collaboration_mode.as_deref(), Some("plan"))
                && !self.allows_writes_outside_workspace()
    }
}

/// Preferred ACP mode ids for the product controls.
///
/// ACP `session/new` returns `modes.availableModes` and the client switches
/// with `session/set_mode`. IDs are harness-defined:
/// - Codex ACP: `read-only`, `agent` / `auto`, `agent-full-access` / `full-access`, optional `plan`
/// - Claude ACP: `default`, `acceptEdits`, `plan`, optional `auto` and
///   `bypassPermissions`. Its own default is `auto`, so full access must name
///   `bypassPermissions` or the session keeps Claude's classifier on.
/// - Claude-like: `plan` / `architect` / `ask` vs `code` / `build`
/// - Grok and others may omit modes entirely
///
/// Full access wins over Plan: Plan is a collaboration hint, Full is an
/// explicit permission grant. Plan still wins over workspace-write/read-only.
pub fn preferred_mode_ids(policy: &ProductSessionPolicy) -> Vec<&'static str> {
    if policy.allows_writes_outside_workspace() {
        vec![
            "agent-full-access",
            "full-access",
            "danger-full-access",
            "bypassPermissions",
            "yolo",
        ]
    } else if matches!(policy.collaboration_mode.as_deref(), Some("plan")) {
        vec!["plan", "architect", "ask"]
    } else if matches!(policy.sandbox_mode.as_deref(), Some("read-only")) {
        vec!["read-only", "readonly", "ask"]
    } else {
        vec!["agent", "code", "build", "auto", "default"]
    }
}

pub fn resolve_mode(available: &[SessionMode], policy: &ProductSessionPolicy) -> Option<String> {
    if available.is_empty() {
        return None;
    }
    for preferred in preferred_mode_ids(policy) {
        if let Some(found) = available.iter().find(|mode| ids_match(&mode.id, preferred)) {
            return Some(found.id.clone());
        }
    }
    available.iter().find_map(|mode| {
        let name = mode.name.as_deref().unwrap_or("");
        preferred_mode_ids(policy)
            .iter()
            .any(|preferred| ids_match(name, preferred) || name_matches_preferred(name, preferred))
            .then(|| mode.id.clone())
    })
}

pub fn parse_available_modes(session: &Value) -> (Option<String>, Vec<SessionMode>) {
    let modes = match session.get("modes") {
        Some(modes) => modes,
        None => return (None, Vec::new()),
    };
    let current = modes
        .get("currentModeId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let available = modes
        .get("availableModes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(SessionMode {
                id: entry.get("id").and_then(Value::as_str)?.to_string(),
                name: entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect();
    (current, available)
}

pub fn parse_permission_choices(params: &Value) -> Vec<PermissionChoice> {
    params
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(PermissionChoice {
                option_id: entry
                    .get("optionId")
                    .or_else(|| entry.get("id"))
                    .and_then(Value::as_str)?
                    .to_string(),
                name: entry
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                kind: entry
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
}

pub fn permission_title(params: &Value) -> String {
    params
        .pointer("/toolCall/title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .unwrap_or("Permission required")
        .to_string()
}

pub fn permission_description(params: &Value) -> Option<String> {
    let tool_call = params.get("toolCall")?;
    let kind = tool_call
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("operation");
    let raw_input = tool_call.get("rawInput");
    let detail = raw_input
        .and_then(|input| input.get("command"))
        .or_else(|| raw_input.and_then(|input| input.get("path")))
        .or_else(|| raw_input.and_then(|input| input.get("url")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    Some(match detail {
        Some(detail) => format!("{}: {detail}", kind.replace('_', " ")),
        None => format!(
            "Review this {} operation before continuing.",
            kind.replace('_', " ")
        ),
    })
}

pub fn permission_questions(title: &str, choices: &[PermissionChoice]) -> Vec<Value> {
    vec![json!({
        "id": "permission",
        "header": "Permission",
        "question": title,
        "isOther": false,
        "isSecret": false,
        "options": choices.iter().map(|choice| json!({
            "label": if choice.name.is_empty() { choice.option_id.clone() } else { choice.name.clone() },
            "description": choice.kind.replace('_', " "),
        })).collect::<Vec<_>>(),
    })]
}

/// Pick the optionId advertised by the agent. Never invent `allow-once`.
pub fn select_permission_option(
    choices: &[PermissionChoice],
    auto_allow: bool,
    answer: Option<&str>,
) -> Option<String> {
    if let Some(answer) = answer.map(str::trim).filter(|value| !value.is_empty()) {
        if let Some(found) = choices.iter().find(|choice| {
            choice.option_id == answer
                || choice.name == answer
                || ids_match(&choice.option_id, answer)
                || ids_match(&choice.name, answer)
        }) {
            return Some(found.option_id.clone());
        }
    }
    let preferred_kinds = if auto_allow {
        ["allow_always", "allow-always", "allow_once", "allow-once"]
    } else {
        [
            "reject_once",
            "reject-once",
            "reject_always",
            "reject-always",
        ]
    };
    for kind in preferred_kinds {
        if let Some(found) = choices.iter().find(|choice| ids_match(&choice.kind, kind)) {
            return Some(found.option_id.clone());
        }
    }
    if auto_allow {
        choices.first().map(|choice| choice.option_id.clone())
    } else {
        None
    }
}

pub fn resolve_mode_config_value(
    config_options: &Value,
    policy: &ProductSessionPolicy,
) -> Option<(String, String)> {
    let option = config_options.as_array()?.iter().find(|entry| {
        let id = entry.get("id").and_then(Value::as_str).unwrap_or("");
        let category = entry.get("category").and_then(Value::as_str).unwrap_or("");
        ids_match(id, "mode") || ids_match(category, "mode")
    })?;
    let config_id = option.get("id").and_then(Value::as_str)?.to_string();
    let select_options = collect_select_options(option);
    let modes: Vec<SessionMode> = select_options
        .iter()
        .filter_map(|entry| {
            let value = entry.get("value").and_then(Value::as_str)?;
            Some(SessionMode {
                id: value.to_string(),
                name: entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect();
    resolve_mode(&modes, policy).map(|value| (config_id, value))
}

fn collect_select_options(option: &Value) -> Vec<Value> {
    option
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|entry| {
            if let Some(nested) = entry.get("options").and_then(Value::as_array) {
                nested.clone()
            } else {
                vec![entry.clone()]
            }
        })
        .collect()
}

fn ids_match(left: &str, right: &str) -> bool {
    normalize_id(left) == normalize_id(right)
}

fn name_matches_preferred(name: &str, preferred: &str) -> bool {
    normalize_id(name).contains(&normalize_id(preferred))
}

fn normalize_id(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace([' ', '_'], "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_modes() -> Vec<SessionMode> {
        [
            ("default", "Manual"),
            ("acceptEdits", "Accept edits"),
            ("plan", "Plan"),
            ("auto", "Auto"),
            ("bypassPermissions", "Bypass permissions"),
        ]
        .into_iter()
        .map(|(id, name)| SessionMode {
            id: id.into(),
            name: Some(name.into()),
        })
        .collect()
    }

    fn policy(sandbox: Option<&str>, approval: &str) -> ProductSessionPolicy {
        ProductSessionPolicy {
            collaboration_mode: None,
            sandbox_mode: sandbox.map(str::to_string),
            approval_mode: Some(approval.into()),
        }
    }

    #[test]
    fn claude_full_access_selects_bypass_permissions() {
        for full in [
            policy(None, "yolo"),
            policy(Some("danger-full-access"), "guarded"),
        ] {
            assert_eq!(
                resolve_mode(&claude_modes(), &full).as_deref(),
                Some("bypassPermissions")
            );
        }
        // Without bypass (root, or disabled in settings) Claude's own mode stays.
        let without: Vec<_> = claude_modes()
            .into_iter()
            .filter(|mode| mode.id != "bypassPermissions")
            .collect();
        assert_eq!(resolve_mode(&without, &policy(None, "yolo")), None);
        assert_eq!(
            resolve_mode(&claude_modes(), &policy(Some("workspace-write"), "guarded")).as_deref(),
            Some("auto")
        );
    }
}
