//! Codex approval callbacks with the decisions codex-acp offers (its
//! `permissions/options.ts`), so the thread's approval card shows the same
//! choices on both protocols and the reply sends Codex's own decision value.
use crate::acp::{permission_questions, select_permission_option, PermissionChoice};
use serde_json::{json, Value};

pub(super) struct Decision {
    pub choice: PermissionChoice,
    pub value: Value,
}

fn decision(id: &str, name: &str, kind: &str, value: Value) -> Decision {
    Decision {
        choice: PermissionChoice {
            option_id: id.into(),
            name: name.into(),
            kind: kind.into(),
        },
        value,
    }
}

/// The ordered choices for one callback; empty for user-input/elicitation.
pub(super) fn decisions(method: &str, params: &Value) -> Vec<Decision> {
    let mut list = match method {
        "item/commandExecution/requestApproval" => command(params),
        "item/fileChange/requestApproval" => vec![
            decision("accept", "Yes, proceed", "allow_once", json!("accept")),
            decision(
                "acceptForSession",
                "Yes, and don't ask again for these files",
                "allow_always",
                json!("acceptForSession"),
            ),
            decision(
                "decline",
                "No, continue without these changes",
                "reject_once",
                json!("decline"),
            ),
            decision(
                "cancel",
                "No, and tell Codex what to do differently",
                "reject_once",
                json!("cancel"),
            ),
        ],
        "item/permissions/requestApproval" => vec![
            decision(
                "turn",
                "Yes, grant these permissions for this turn",
                "allow_once",
                json!({"scope":"turn","strictAutoReview":false}),
            ),
            decision(
                "turnStrict",
                "Yes, grant for this turn with strict auto review",
                "allow_once",
                json!({"scope":"turn","strictAutoReview":true}),
            ),
            decision(
                "session",
                "Yes, grant these permissions for this session",
                "allow_always",
                json!({"scope":"session","strictAutoReview":false}),
            ),
            decision(
                "reject",
                "No, continue without permissions",
                "reject_once",
                Value::Null,
            ),
        ],
        "execCommandApproval" | "applyPatchApproval" => {
            let mut legacy = vec![
                decision("approved", "Yes, proceed", "allow_once", json!("approved")),
                decision(
                    "approved_for_session",
                    "Yes, and don't ask again this session",
                    "allow_always",
                    json!("approved_for_session"),
                ),
            ];
            if let Some(amendment) = params["proposedExecpolicyAmendment"]
                .as_array()
                .filter(|a| !a.is_empty())
            {
                legacy.push(decision(
                    "approved_execpolicy_amendment",
                    &format!(
                        "Yes, and don't ask again for commands that start with `{}`",
                        prefix(amendment)
                    ),
                    "allow_always",
                    json!({"approved_execpolicy_amendment":{"proposed_execpolicy_amendment":amendment}}),
                ));
            }
            legacy.push(decision(
                "denied",
                "No, continue without running it",
                "reject_once",
                json!("denied"),
            ));
            legacy.push(decision(
                "abort",
                "No, and tell Codex what to do differently",
                "reject_once",
                json!("abort"),
            ));
            legacy
        }
        _ => vec![],
    };
    // Allow once, then allow always, then the rejections (codex-acp's order).
    list.sort_by_key(|d| match d.choice.kind.as_str() {
        "allow_once" => 0,
        "allow_always" => 1,
        _ => 2,
    });
    list
}

fn prefix(amendment: &[Value]) -> String {
    amendment
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ")
}

fn command(params: &Value) -> Vec<Decision> {
    let offered: Vec<Value> = match params["availableDecisions"].as_array() {
        Some(list) => list.clone(),
        None if !params["networkApprovalContext"].is_null() => {
            let mut list = vec![json!("accept"), json!("acceptForSession")];
            for amendment in params["proposedNetworkPolicyAmendments"]
                .as_array()
                .into_iter()
                .flatten()
            {
                list.push(
                    json!({"applyNetworkPolicyAmendment":{"network_policy_amendment":amendment}}),
                );
            }
            list.extend([json!("decline"), json!("cancel")]);
            list
        }
        None if !params["additionalPermissions"].is_null() => {
            vec![json!("accept"), json!("cancel")]
        }
        None => {
            let mut list = vec![json!("accept"), json!("acceptForSession")];
            if let Some(amendment) = params["proposedExecpolicyAmendment"]
                .as_array()
                .filter(|a| !a.is_empty())
            {
                list.push(
                    json!({"acceptWithExecpolicyAmendment":{"execpolicy_amendment":amendment}}),
                );
            }
            list.extend([json!("decline"), json!("cancel")]);
            list
        }
    };
    let network = !params["networkApprovalContext"].is_null();
    let extra = !params["additionalPermissions"].is_null();
    let mut network_index = 0;
    offered
        .into_iter()
        .filter_map(|value| {
            Some(match value.as_str() {
                Some("accept") => decision(
                    "accept",
                    if network {
                        "Yes, just this once"
                    } else {
                        "Yes, proceed"
                    },
                    "allow_once",
                    value,
                ),
                Some("acceptForSession") => decision(
                    "acceptForSession",
                    if network {
                        "Yes, and allow this host for this conversation"
                    } else if extra {
                        "Yes, and allow these permissions for this session"
                    } else {
                        "Yes, and don't ask again for this command in this session"
                    },
                    "allow_always",
                    value,
                ),
                Some("decline") => decision(
                    "decline",
                    "No, continue without running it",
                    "reject_once",
                    value,
                ),
                Some("cancel") => decision(
                    "cancel",
                    "No, and tell Codex what to do differently",
                    "reject_once",
                    value,
                ),
                Some(_) => return None,
                None => {
                    if let Some(amendment) =
                        value.pointer("/acceptWithExecpolicyAmendment/execpolicy_amendment")
                    {
                        let rendered = prefix(amendment.as_array()?);
                        if rendered.is_empty() || rendered.contains(['\n', '\r']) {
                            return None;
                        }
                        decision(
                            "acceptWithExecpolicyAmendment",
                            &format!("Yes, and don't ask again for commands that start with `{rendered}`"),
                            "allow_always",
                            value,
                        )
                    } else {
                        let rule = value
                            .pointer("/applyNetworkPolicyAmendment/network_policy_amendment")?;
                        let allow = rule["action"] == "allow";
                        network_index += 1;
                        decision(
                            &format!("applyNetworkPolicyAmendment:{}", network_index - 1),
                            if allow {
                                "Yes, and allow this host in the future"
                            } else {
                                "No, and block this host in the future"
                            },
                            if allow { "allow_always" } else { "reject_always" },
                            value,
                        )
                    }
                }
            })
        })
        .collect()
}

pub(super) fn questions(title: &str, decisions: &[Decision]) -> Vec<Value> {
    let choices: Vec<PermissionChoice> = decisions.iter().map(|d| d.choice.clone()).collect();
    permission_questions(title, &choices)
}

/// The JSON-RPC result for the user's choice. `answer` is the label or id the
/// approval card sent; a bare allow/deny picks the first allow/reject choice.
pub(super) fn response(
    method: &str,
    params: &Value,
    decisions: &[Decision],
    allow: bool,
    answer: Option<&str>,
) -> Value {
    let structured: Option<Value> = answer.and_then(|text| serde_json::from_str(text).ok());
    let picked = structured
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|obj| {
            obj.values()
                .find_map(|v| v["answers"].as_array()?.first()?.as_str())
        })
        .or(answer);
    let choices: Vec<PermissionChoice> = decisions.iter().map(|d| d.choice.clone()).collect();
    // A named choice wins. A bare allow means "this once", never a standing
    // rule; a bare deny declines without interrupting the turn.
    let fallback = |kinds: &[&str]| {
        kinds.iter().find_map(|kind| {
            choices
                .iter()
                .find(|c| c.kind == *kind)
                .map(|c| c.option_id.clone())
        })
    };
    let named = picked
        .filter(|p| !p.trim().is_empty())
        .and_then(|p| select_permission_option(&choices, false, Some(p)))
        .filter(|id| {
            picked.is_some_and(|p| {
                choices
                    .iter()
                    .any(|c| &c.option_id == id && (c.option_id == p.trim() || c.name == p.trim()))
            })
        });
    let selected = named.or_else(|| {
        if allow {
            fallback(&["allow_once", "allow_always"])
        } else {
            decisions
                .iter()
                .find(|d| {
                    d.value == json!("decline") || d.value == json!("denied") || d.value.is_null()
                })
                .map(|d| d.choice.option_id.clone())
                .or_else(|| fallback(&["reject_once", "reject_always"]))
        }
    });
    let decision = selected
        .and_then(|id| decisions.iter().find(|d| d.choice.option_id == id))
        .map(|d| d.value.clone());
    match method {
        "item/permissions/requestApproval" => match decision.filter(|v| !v.is_null()) {
            Some(grant) => {
                json!({"permissions":params["permissions"],"scope":grant["scope"],"strictAutoReview":grant["strictAutoReview"]})
            }
            None => json!({"permissions":{},"scope":"turn"}),
        },
        "execCommandApproval" | "applyPatchApproval" => {
            json!({"decision":decision.unwrap_or_else(|| json!("denied"))})
        }
        _ => json!({"decision":decision.unwrap_or_else(|| json!("decline"))}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(d: &[Decision]) -> Vec<&str> {
        d.iter().map(|d| d.choice.option_id.as_str()).collect()
    }

    #[test]
    fn command_defaults_match_codex_acp_and_answers_send_codex_decisions() {
        let params = json!({"command":"rm -rf build","proposedExecpolicyAmendment":["rm","-rf"]});
        let d = decisions("item/commandExecution/requestApproval", &params);
        assert_eq!(
            labels(&d),
            [
                "accept",
                "acceptForSession",
                "acceptWithExecpolicyAmendment",
                "decline",
                "cancel"
            ]
        );
        let m = "item/commandExecution/requestApproval";
        assert_eq!(response(m, &params, &d, true, None)["decision"], "accept");
        assert_eq!(response(m, &params, &d, false, None)["decision"], "decline");
        let session = r#"{"permission":{"answers":["Yes, and don't ask again for this command in this session"]}}"#;
        assert_eq!(
            response(m, &params, &d, true, Some(session))["decision"],
            "acceptForSession"
        );
        let amend = response(m, &params, &d, true, Some("acceptWithExecpolicyAmendment"));
        assert_eq!(
            amend["decision"]["acceptWithExecpolicyAmendment"]["execpolicy_amendment"],
            json!(["rm", "-rf"])
        );
        assert_eq!(
            response(m, &params, &d, false, Some("cancel"))["decision"],
            "cancel"
        );
        let q = questions("rm -rf build", &d);
        assert_eq!(q[0]["options"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn server_offered_and_network_decisions_are_respected() {
        let offered = json!({"availableDecisions":["accept","decline"]});
        assert_eq!(
            labels(&decisions(
                "item/commandExecution/requestApproval",
                &offered
            )),
            ["accept", "decline"]
        );
        let network = json!({"networkApprovalContext":{"host":"example.com"},"proposedNetworkPolicyAmendments":[{"host":"example.com","action":"allow"},{"host":"example.com","action":"deny"}]});
        let d = decisions("item/commandExecution/requestApproval", &network);
        assert_eq!(
            labels(&d),
            [
                "accept",
                "acceptForSession",
                "applyNetworkPolicyAmendment:0",
                "applyNetworkPolicyAmendment:1",
                "decline",
                "cancel"
            ]
        );
        let r = response(
            "item/commandExecution/requestApproval",
            &network,
            &d,
            true,
            Some("Yes, and allow this host in the future"),
        );
        assert_eq!(
            r["decision"]["applyNetworkPolicyAmendment"]["network_policy_amendment"]["action"],
            "allow"
        );
    }

    #[test]
    fn files_permissions_and_legacy_callbacks() {
        let f = decisions("item/fileChange/requestApproval", &json!({}));
        assert_eq!(
            labels(&f),
            ["accept", "acceptForSession", "decline", "cancel"]
        );
        let p = json!({"permissions":{"network":{"enabled":true}}});
        let d = decisions("item/permissions/requestApproval", &p);
        let m = "item/permissions/requestApproval";
        assert_eq!(
            response(m, &p, &d, true, None),
            json!({"permissions":{"network":{"enabled":true}},"scope":"turn","strictAutoReview":false})
        );
        assert_eq!(
            response(m, &p, &d, true, Some("session"))["scope"],
            "session"
        );
        assert_eq!(
            response(m, &p, &d, false, None),
            json!({"permissions":{},"scope":"turn"})
        );
        let l = decisions(
            "execCommandApproval",
            &json!({"proposedExecpolicyAmendment":["ls"]}),
        );
        assert_eq!(
            response("execCommandApproval", &json!({}), &l, true, None)["decision"],
            "approved"
        );
        assert_eq!(
            response("execCommandApproval", &json!({}), &l, false, None)["decision"],
            "denied"
        );
        assert!(decisions("item/tool/requestUserInput", &json!({})).is_empty());
    }
}
