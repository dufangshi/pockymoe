//! Why your other devices refuse this one: the relay names the shares that let
//! other people control agents here (`peer_source_shared`,
//! docs/agent-isolation.md#devices-shared-with-other-people). The message an
//! agent reads lists them, unless one of those shares reaches the agent's own
//! thread: then someone it is shared with could read who else has access, so
//! the agent only learns where the owner can look.
use crate::peer_link::PeerError;
use crate::Supervisor;
use serde_json::Value;

/// Thread lineage depth a share reaches, as in `pockymoe_protocol::access`.
const LINEAGE_DEPTH: usize = 3;

/// Rewrites a `peer_source_shared` refusal into the agent-facing explanation;
/// other errors pass through. `thread` is the calling agent's thread, `None`
/// for the device's own command line (a machine credential).
pub(crate) fn explain(
    state: &Supervisor,
    thread: Option<&str>,
    error: anyhow::Error,
) -> anyhow::Error {
    match error.downcast::<PeerError>() {
        Ok(PeerError::SourceShared { shares, .. }) => PeerError::SourceShared {
            message: message(
                &shares,
                thread.is_some_and(|id| reached(state, id, &shares)),
            ),
            shares,
        }
        .into(),
        Ok(other) => other.into(),
        Err(error) => error,
    }
}

const REFUSED: &str = "This device is shared with other people who can control its agents, so your other devices do not accept its requests.";

fn message(shares: &[Value], private: bool) -> String {
    if shares.is_empty() {
        return format!(
            "{REFUSED} The owner can see which shares cause this on the Devices page under Sharing (Shared by me)."
        );
    }
    if private {
        return format!(
            "{REFUSED} This thread is itself shared, so the shares are not listed here; the owner can see them on the Devices page under Sharing (Shared by me)."
        );
    }
    let lines = shares
        .iter()
        .map(|share| format!("- {}", describe(share)))
        .collect::<Vec<_>>();
    format!(
        "{REFUSED} It is shared for control by:\n{}\nTo let this device reach your other devices again, revoke these shares or change them to view only without new threads (Devices page, Sharing).",
        lines.join("\n")
    )
}

fn quoted(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| {
            format!(
                "\"{}\"",
                text.chars()
                    .filter(|c| !c.is_control())
                    .take(80)
                    .collect::<String>()
            )
        })
}

/// "thread "Fix login" in workspace "Web app" is shared with alice (collaborator)"
fn describe(share: &Value) -> String {
    let what = match share["scope"].as_str() {
        Some("device") => "this whole device".to_owned(),
        Some("workspace") => format!(
            "workspace {}",
            quoted(&share["workspaceLabel"]).unwrap_or_else(|| "(unnamed)".into())
        ),
        _ => {
            let thread = quoted(&share["threadTitle"]).unwrap_or_else(|| "(untitled)".into());
            match quoted(&share["workspaceLabel"]) {
                Some(workspace) => format!("thread {thread} in workspace {workspace}"),
                None => format!("thread {thread}"),
            }
        }
    };
    let who = share["recipient"]
        .as_str()
        .filter(|name| !name.is_empty())
        .map(|name| {
            name.chars()
                .filter(|c| !c.is_control())
                .take(64)
                .collect::<String>()
        })
        .unwrap_or_else(|| "someone".into());
    let mut access = vec![if share["threadAccess"] == "control" {
        "collaborator"
    } else {
        "viewer"
    }];
    if share["canCreateThreads"] == true {
        access.push("can start new threads");
    }
    format!("{what} is shared with {who} ({})", access.join(", "))
}

/// Whether any listed share reaches `thread`: the whole device, its workspace
/// (or an ancestor's), or the thread or one of its ancestors. Unknown threads
/// count as reached.
fn reached(state: &Supervisor, thread: &str, shares: &[Value]) -> bool {
    let mut lineage = Vec::new();
    let mut next = Some(thread.to_owned());
    while let Some(id) = next.take() {
        let Ok(found) = state.get_thread(&id) else {
            if lineage.is_empty() {
                return true;
            }
            break;
        };
        next = found
            .parent_thread_id
            .clone()
            .filter(|_| lineage.len() < LINEAGE_DEPTH);
        lineage.push((found.id, found.workspace_id));
    }
    shares.iter().any(|share| match share["scope"].as_str() {
        Some("device") => true,
        Some("workspace") => lineage
            .iter()
            .any(|(_, workspace)| share["workspaceId"].as_str() == Some(workspace)),
        _ => lineage
            .iter()
            .any(|(id, _)| share["threadId"].as_str() == Some(id)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shares() -> Vec<Value> {
        vec![
            json!({"scope":"thread","threadId":"t1","threadTitle":"Fix login","workspaceId":"web","workspaceLabel":"Web app","recipient":"alice","threadAccess":"control","canCreateThreads":false}),
            json!({"scope":"workspace","threadId":null,"threadTitle":null,"workspaceId":"api","workspaceLabel":"API","recipient":"bob","threadAccess":"control","canCreateThreads":true}),
            json!({"scope":"device","threadId":null,"threadTitle":null,"workspaceId":null,"workspaceLabel":null,"recipient":"carol","threadAccess":"read","canCreateThreads":true}),
        ]
    }

    #[test]
    fn the_refusal_names_each_share_with_its_scope_person_and_access() {
        let text = message(&shares(), false);
        assert!(text.starts_with(REFUSED), "{text}");
        for line in [
            "- thread \"Fix login\" in workspace \"Web app\" is shared with alice (collaborator)",
            "- workspace \"API\" is shared with bob (collaborator, can start new threads)",
            "- this whole device is shared with carol (viewer, can start new threads)",
        ] {
            assert!(text.contains(line), "{line}\n{text}");
        }
        assert!(text.contains("revoke these shares or change them to view only"));
    }

    #[test]
    fn a_thread_reached_by_a_share_learns_no_names() {
        let text = message(&shares(), true);
        for name in ["alice", "bob", "carol", "Fix login", "API"] {
            assert!(!text.contains(name), "{name}: {text}");
        }
        assert!(text.contains("Devices page"));
        assert!(message(&[], false).contains("Shared by me"));
    }

    #[test]
    fn names_are_bounded_and_carry_no_control_characters() {
        let line = describe(&json!({"scope":"thread","threadTitle":"a\nb","recipient":"x\u{1b}y"}));
        assert_eq!(line, "thread \"ab\" is shared with xy (viewer)");
    }
}
