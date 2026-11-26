//! Plan Mode follow-through, as codex-acp does it: when a Plan Mode turn
//! finishes with a plan, ask "Implement this plan?"; on yes, leave Plan Mode
//! and run "Implement the approved plan." inside the same product turn.
use pockymoe_protocol::ThreadActionRequestDto;
use serde_json::{json, Value};

pub(super) const IMPLEMENT: &str = "Yes, implement this plan";
pub(super) const REVISE: &str = "No, and tell Codex what to do differently";
pub(super) const PROMPT: &str = "Implement the approved plan.";

pub(super) fn request(
    id: &str,
    turn_id: &str,
    item_id: &str,
    plan: &str,
) -> ThreadActionRequestDto {
    ThreadActionRequestDto {
        id: id.into(),
        kind: "permissionRequest".into(),
        title: "Implement this plan?".into(),
        description: Some(plan.chars().take(4000).collect()),
        turn_id: Some(turn_id.into()),
        item_id: Some(tool_id(item_id)),
        created_at: pockymoe_protocol::now_rfc3339(),
        questions: vec![json!({
            "id": "permission",
            "header": "Plan",
            "question": "Implement this plan?",
            "isOther": false,
            "isSecret": false,
            "options": [
                {"label": IMPLEMENT, "description": "allow once"},
                {"label": REVISE, "description": "reject once"},
            ],
        })],
    }
}

pub(super) fn tool_id(item_id: &str) -> String {
    format!("plan-review:{item_id}")
}

/// The transcript row codex-acp shows for the review, pending then decided.
pub(super) fn row(item_id: &str, plan: &str, decided: Option<bool>) -> Value {
    match decided {
        None => {
            json!({"sessionUpdate":"tool_call","toolCallId":tool_id(item_id),"kind":"switch_mode","title":"Implement this plan?","status":"pending","rawInput":{"plan":plan}})
        }
        Some(approved) => {
            json!({"sessionUpdate":"tool_call_update","toolCallId":tool_id(item_id),"status":"completed",
            "rawOutput": if approved {"User approved the plan."} else {"User kept the session in plan mode."}})
        }
    }
}

/// A named choice decides; a bare allow implements, a bare deny revises.
pub(super) fn approved(allow: bool, answer: Option<&str>) -> bool {
    let structured: Option<Value> = answer.and_then(|text| serde_json::from_str(text).ok());
    let picked = structured
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|obj| {
            obj.values()
                .find_map(|v| v["answers"].as_array()?.first()?.as_str())
        })
        .or(answer)
        .map(str::trim);
    match picked {
        Some(IMPLEMENT) => true,
        Some(REVISE) => false,
        _ => allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_and_request_shape() {
        let card = format!(r#"{{"permission":{{"answers":["{REVISE}"]}}}}"#);
        assert!(!approved(true, Some(&card)));
        assert!(approved(
            true,
            Some(&format!(
                r#"{{"permission":{{"answers":["{IMPLEMENT}"]}}}}"#
            ))
        ));
        assert!(approved(true, None));
        assert!(!approved(false, None));
        let dto = request("id", "turn", "plan-1", "1. Do it");
        assert_eq!(dto.questions[0]["options"].as_array().unwrap().len(), 2);
        assert_eq!(dto.item_id.as_deref(), Some("plan-review:plan-1"));
    }
}
