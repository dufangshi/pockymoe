//! Pure shared route, grant merge, and WebSocket authorization rules.
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct EffectiveAccess {
    pub kind: &'static str,
    pub grant_id: Option<String>,
    pub share_id: Option<String>,
    pub scope: String,
    pub thread_id: Option<String>,
    pub thread_access: String,
    pub workspace_access: String,
    pub workspace_id: Option<String>,
    pub workspace_scope: Option<String>,
    pub workspace_ids: Vec<String>,
    pub can_create_threads: bool,
}

pub fn owner_access() -> EffectiveAccess {
    EffectiveAccess {
        kind: "owner",
        grant_id: None,
        share_id: None,
        scope: "owner".to_string(),
        thread_id: None,
        thread_access: "control".to_string(),
        workspace_access: "write".to_string(),
        workspace_id: None,
        workspace_scope: None,
        workspace_ids: Vec::new(),
        can_create_threads: true,
    }
}

pub fn resource_id_from_path(path: &str, resource: &str) -> Option<String> {
    let path = path.split('?').next().unwrap_or(path);
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    while let Some(segment) = segments.next() {
        if segment == resource {
            let id = segments.next()?;
            if matches!(id, "start" | "import")
                || (resource == "threads" && id == "import-candidates")
            {
                return None;
            }
            return Some(id.to_string());
        }
    }
    None
}

pub fn relay_target_allowed(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    path == "/healthz" || path.starts_with("/api/") || path == "/api"
}

/// Relay bootstrap exceptions only: browser registration and revocation pushes,
/// whose certificates and signatures the device verifies itself. Callers must
/// first establish a live share or grant on the target device.
pub fn grantee_registration_allowed(method: &str, path: &str, encrypted: bool) -> bool {
    let path = path.split('?').next();
    encrypted
        && matches!(
            (method, path),
            ("POST", Some("/api/transport/browser")) | ("PUT", Some("/api/transport/revocations"))
        )
}

/// Read-only previews of host files a conversation links to outside its workspace
/// (`/api/threads/{id}/linked-files/*`). A whole-device grant with workspace
/// write access needs no workspace boundary, so it reads them like the owner.
pub fn linked_files_allowed(access: &EffectiveAccess) -> bool {
    access.kind == "owner" || (access.scope == "device" && access.workspace_access == "write")
}

/// Terminal and shell routes: a thread's shell sessions (`/api/threads/{id}/shell`,
/// the relay's `/transport/shell-scope` probe), `/api/shells/*`, and any
/// `terminal` plugin. They belong to the device owner only, whatever a share or
/// grant allows; the WebSocket `shell.*` messages follow the same rule
/// (`client_message_allowed`).
pub fn terminal_route(path: &str) -> bool {
    let pathname = path.split('?').next().unwrap_or(path);
    let segments: Vec<&str> = pathname
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    match segments.as_slice() {
        ["api", "shells", ..] => true,
        ["api", "threads", _, "shell", ..] => true,
        ["api", "threads", _, "transport", "shell-scope", ..] => true,
        ["api", "workspaces", _, "terminal" | "terminals" | "shell", ..] => true,
        // One-off shell commands (`/bin/sh -c`) and their output.
        ["api", "threads", _, "commands", ..] => true,
        ["api", "threads", _, "plugins", plugin, ..] | ["api", "plugins", plugin, ..] => {
            plugin_is_terminal(plugin)
        }
        _ => false,
    }
}

fn plugin_is_terminal(plugin: &str) -> bool {
    let plugin = plugin.to_ascii_lowercase();
    plugin
        .rsplit(['.', '/', ':'])
        .next()
        .is_some_and(|name| matches!(name, "terminal" | "shell" | "pty"))
}

/// The thread collection inside one workspace: `GET` lists its conversations,
/// `POST` starts one there. A workspace share needs no file access for them.
fn workspace_threads_allowed(access: &EffectiveAccess, method: &str) -> bool {
    match method {
        "GET" => matches!(access.thread_access.as_str(), "read" | "control"),
        "POST" => access.thread_access == "control" && access.can_create_threads,
        _ => false,
    }
}

pub fn access_allows(access: &EffectiveAccess, method: &str, path: &str) -> bool {
    if access.kind == "owner" {
        return true;
    }
    let pathname = path.split('?').next().unwrap_or(path);
    // No share or grant ever reaches a terminal, read or control.
    if terminal_route(pathname) {
        return false;
    }
    if shared_runtime_metadata_allowed(method, pathname) {
        return true;
    }
    let thread_id = resource_id_from_path(path, "threads");
    let workspace_id = resource_id_from_path(path, "workspaces");

    if pathname == "/api/threads/start" {
        return access.scope == "device"
            && access.can_create_threads
            && access.thread_access == "control"
            && method == "POST";
    }
    // Workspace search may initiate encryption before any thread request. A
    // device conversation reader already has /api/transport/key permission;
    // this alias publishes the same signed descriptor, never workspace files.
    if method == "GET"
        && access.scope == "device"
        && matches!(access.thread_access.as_str(), "read" | "control")
        && workspace_id
            .as_ref()
            .is_some_and(|id| pathname == format!("/api/workspaces/{id}/transport/key"))
    {
        return true;
    }
    // A workspace filesystem grant (including one attached to a thread share)
    // does not grant access to conversations in that workspace. Global search
    // follows the existing device-wide thread collection permission only.
    if pathname == "/api/search"
        || workspace_id
            .as_ref()
            .is_some_and(|id| pathname == format!("/api/workspaces/{id}/search"))
    {
        return method == "GET"
            && access.scope == "device"
            && matches!(access.thread_access.as_str(), "read" | "control");
    }
    if let Some(thread_id) = thread_id {
        if access.scope != "device" && access.thread_id.as_deref() != Some(thread_id.as_str()) {
            return false;
        }
        if pathname
            .strip_prefix(&format!("/api/threads/{thread_id}/linked-files/"))
            .is_some_and(|action| matches!(action, "stat" | "preview" | "raw"))
        {
            return method == "GET" && linked_files_allowed(access);
        }
        return shared_thread_path_allowed(
            method,
            pathname,
            &thread_id,
            access.thread_access == "control",
        );
    }
    if let Some(workspace_id) = workspace_id {
        let workspace_matches = access.scope == "device"
            || access.workspace_id.as_deref() == Some(workspace_id.as_str())
            || (access.workspace_scope.as_deref() == Some("selected")
                && access.workspace_ids.iter().any(|id| id == &workspace_id));
        if !workspace_matches {
            return false;
        }
        let suffix = pathname
            .strip_prefix(&format!("/api/workspaces/{workspace_id}"))
            .unwrap_or("/invalid");
        // Conversations of a shared workspace: listing them and starting new ones
        // there. A thread share's workspace never lists its other conversations.
        if suffix == "/threads" {
            return matches!(access.scope.as_str(), "workspace" | "device")
                && workspace_threads_allowed(access, method);
        }
        // The workspace record and the device's signed transport key, which the
        // browser fetches before its first workspace request.
        if access.scope == "workspace" && method == "GET" && matches!(suffix, "" | "/transport/key")
        {
            return true;
        }
        if access.workspace_access == "none" {
            return false;
        }
        return shared_workspace_path_allowed(
            method,
            pathname,
            &workspace_id,
            access.workspace_access == "write",
        );
    }
    // Harness processes and their release policy list every thread's title.
    if access.scope == "device" && method == "PUT" && pathname == "/api/harness-resources/settings"
    {
        return access.thread_access == "control";
    }
    access.scope == "device"
        && method == "GET"
        // Collection pages cannot use a thread/workspace-scoped handshake before
        // they have loaded the collection. This publishes only the device's signed
        // recipient key; each subsequent request still goes through this ACL.
        && matches!(
            pathname,
            "/api/threads"
                | "/api/workspaces"
                | "/api/transport/key"
                | "/api/device/metrics"
                | "/api/device/processes"
                | "/api/harness-resources/settings"
        )
}

pub fn shared_runtime_metadata_allowed(method: &str, pathname: &str) -> bool {
    if method != "GET" {
        return false;
    }
    if matches!(pathname, "/api/agent-runtimes" | "/api/plugins") {
        return true;
    }
    let segments: Vec<&str> = pathname
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    segments.len() == 4
        && segments[0] == "api"
        && segments[1] == "agent-runtimes"
        && matches!(segments[3], "status" | "models" | "agents" | "harness")
}

pub fn shared_thread_path_allowed(
    method: &str,
    pathname: &str,
    thread_id: &str,
    control: bool,
) -> bool {
    let base = format!("/api/threads/{thread_id}");
    let Some(suffix) = pathname.strip_prefix(&base) else {
        return false;
    };
    if !suffix.is_empty() && !suffix.starts_with('/') {
        return false;
    }
    let automation: Vec<&str> = suffix.split('/').filter(|s| !s.is_empty()).collect();
    if automation.first() == Some(&"automations") {
        return match (method, automation.as_slice()) {
            ("GET", ["automations"])
            | ("GET", ["automations", _])
            | ("GET", ["automations", _, "runs"]) => true,
            ("POST", ["automations"])
            | ("POST", ["automations", "preview"])
            | ("POST", ["automations", _, "pause" | "resume" | "cancel"]) => control,
            _ => false,
        };
    }
    if automation.first() == Some(&"plugins") {
        return match (method, automation.as_slice()) {
            ("GET", ["plugins"]) => true,
            ("GET" | "PUT", ["plugins", _, "settings"]) => control,
            _ => false,
        };
    }
    if automation.first() == Some(&"commands") {
        // Shell commands and their output are terminal use (`terminal_route`).
        return false;
    }
    if method == "GET" && suffix.starts_with("/transport/stream/") {
        return true;
    }
    if method == "GET" {
        if matches!(
            suffix,
            "" | "/transport/key"
                | "/export-turns"
                | "/exports/pdf"
                | "/assets/image"
                | "/goal"
                | "/skills"
                | "/mcp-servers"
                | "/hooks"
                | "/models"
                | "/group"
                | "/watches"
                | "/search"
                | "/summary"
        ) {
            return true;
        }
        let parts: Vec<&str> = suffix.split('/').filter(|part| !part.is_empty()).collect();
        if parts.first() == Some(&"subagents") && matches!(parts.len(), 1 | 2) {
            return true;
        }
        if parts.len() == 3 && matches!(parts[0], "items" | "turns") && parts[2] == "detail" {
            return true;
        }
        return control && matches!(suffix, "/fork-turns" | "/capabilities");
    }
    if method == "POST" && suffix == "/transport/session" {
        return true;
    }
    if !control {
        return false;
    }
    match method {
        "PATCH" => matches!(suffix, "/goal" | "/settings"),
        "DELETE" => suffix == "/goal",
        "PUT" => suffix == "/hooks",
        "POST" => {
            if matches!(
                suffix,
                "/goal"
                    | "/resume"
                    | "/prompt"
                    | "/interrupt"
                    | "/release"
                    | "/compact"
                    | "/fork"
                    | "/hooks"
                    | "/hooks/trust"
                    | "/hooks/untrust"
            ) {
                return true;
            }
            let parts: Vec<&str> = suffix.split('/').filter(|part| !part.is_empty()).collect();
            parts.len() == 3
                && ((parts[0] == "requests" && parts[2] == "respond")
                    || (parts[0] == "pending-steers" && parts[2] == "steer"))
        }
        _ => false,
    }
}

pub fn shared_workspace_path_allowed(
    method: &str,
    pathname: &str,
    workspace_id: &str,
    write: bool,
) -> bool {
    let base = format!("/api/workspaces/{workspace_id}");
    let Some(suffix) = pathname.strip_prefix(&base) else {
        return false;
    };
    if !suffix.is_empty() && !suffix.starts_with('/') {
        return false;
    }
    if method == "GET" && suffix.starts_with("/transport/stream/") {
        return true;
    }
    if method == "GET" {
        if matches!(
            suffix,
            "" | "/transport/key"
                | "/files/tree"
                | "/files/preview"
                | "/files/capabilities"
                | "/files/document"
                | "/files/raw"
                | "/files/download"
                | "/artifacts"
        ) {
            return true;
        }
        let parts: Vec<&str> = suffix.split('/').filter(|part| !part.is_empty()).collect();
        return (write && parts.len() == 3 && parts[0] == "files" && parts[1] == "operations")
            || (parts.len() == 2 && parts[0] == "artifacts")
            || (parts.len() == 3 && parts[0] == "artifacts" && parts[2] == "download");
    }
    write
        && matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
        && matches!(
            suffix,
            "/files" | "/files/upload" | "/files/move" | "/files/save"
        )
}

/// Inputs are live rows in priority order; lineage matching is resolved locally.
pub fn merge_access(
    owner: bool,
    share: Option<EffectiveAccess>,
    matching: &[EffectiveAccess],
    thread_id: Option<&str>,
    workspace_id: Option<&str>,
) -> Option<EffectiveAccess> {
    if owner {
        return Some(owner_access());
    }
    if let Some(share) = share {
        if workspace_id.is_none_or(|id| {
            share.workspace_id.as_deref() == Some(id) && share.workspace_access != "none"
        }) {
            // A workspace route that a workspace or device grant also covers
            // takes the grants' scope, so a thread share in the same workspace
            // does not hide its conversations; the share adds only its file access.
            if thread_id.is_none() && workspace_id.is_some() && !matching.is_empty() {
                let mut merged = merge_access(false, None, matching, thread_id, workspace_id)?;
                if rank_workspace_access(&share.workspace_access)
                    > rank_workspace_access(&merged.workspace_access)
                {
                    merged.workspace_access = share.workspace_access;
                }
                return Some(merged);
            }
            return Some(share);
        }
    }
    let representative = matching.first()?.clone();
    let merged_scope = if matching.iter().any(|grant| grant.scope == "device") {
        "device"
    } else if matching.iter().any(|grant| grant.scope == "workspace") {
        "workspace"
    } else {
        "thread"
    };
    let thread_access = if matching
        .iter()
        .any(|grant| grant.thread_access == "control")
    {
        "control"
    } else {
        "read"
    };
    let workspace_access = matching
        .iter()
        .map(|grant| grant.workspace_access.as_str())
        .max_by_key(|access| rank_workspace_access(access))
        .unwrap_or("none");
    let mut merged = representative;
    merged.scope = merged_scope.to_string();
    // A workspace grant reaches a thread through the thread's workspace; the
    // merged access names that thread like a thread share does.
    merged.thread_id = match merged_scope {
        "thread" => thread_id.map(str::to_string).or(merged.thread_id),
        "workspace" => thread_id.map(str::to_string),
        _ => None,
    };
    merged.thread_access = thread_access.to_string();
    merged.workspace_access = workspace_access.to_string();
    merged.workspace_id = if merged_scope == "workspace" {
        workspace_id.map(str::to_string).or(merged.workspace_id)
    } else {
        None
    };
    merged.can_create_threads = matching.iter().any(|grant| grant.can_create_threads);
    Some(merged)
}

fn rank_workspace_access(access: &str) -> u8 {
    match access {
        "write" => 2,
        "read" => 1,
        _ => 0,
    }
}

pub fn grant_matches(
    grant: &EffectiveAccess,
    thread_id: Option<&str>,
    workspace_id: Option<&str>,
    lineage: &[String],
) -> bool {
    grant_matches_in(grant, thread_id, workspace_id, lineage, &[])
}

/// `thread_workspaces` lists the workspace of the requested thread, then those
/// of its ancestors (nearest first), from the caller's own storage. A workspace
/// grant covers every conversation there, including agent threads its
/// conversations started elsewhere (up to three levels, like a thread share),
/// and the workspace's own routes whatever its file access; `access_allows`
/// decides which of those routes need file access.
pub fn grant_matches_in(
    grant: &EffectiveAccess,
    thread_id: Option<&str>,
    workspace_id: Option<&str>,
    lineage: &[String],
    thread_workspaces: &[String],
) -> bool {
    match grant.scope.as_str() {
        "device" => workspace_id.is_none() || grant.workspace_access != "none",
        "thread" => thread_id.is_some_and(|id| {
            grant.thread_id.as_deref().is_some_and(|parent| {
                parent == id || lineage.iter().take(3).any(|ancestor| ancestor == parent)
            })
        }),
        "workspace" => grant.workspace_id.as_deref().is_some_and(|granted| {
            if thread_id.is_some() {
                thread_workspaces.iter().take(4).any(|id| id == granted)
            } else {
                workspace_id == Some(granted)
            }
        }),
        _ => false,
    }
}

/// Resolve live rows, newest shares first and newest grants first within each scope.
/// The caller supplies ancestors from its own storage, nearest ancestor first.
pub fn effective_access_from_rows(
    owner: bool,
    shares: &[EffectiveAccess],
    grants: &[EffectiveAccess],
    thread_id: Option<&str>,
    workspace_id: Option<&str>,
    lineage: &[String],
) -> Option<EffectiveAccess> {
    effective_access_from_rows_in(owner, shares, grants, thread_id, workspace_id, lineage, &[])
}

/// `effective_access_from_rows` with the requested thread's workspaces
/// (`grant_matches_in`), so workspace grants reach that workspace's threads.
pub fn effective_access_from_rows_in(
    owner: bool,
    shares: &[EffectiveAccess],
    grants: &[EffectiveAccess],
    thread_id: Option<&str>,
    workspace_id: Option<&str>,
    lineage: &[String],
    thread_workspaces: &[String],
) -> Option<EffectiveAccess> {
    if owner {
        return Some(owner_access());
    }
    let share = shares
        .iter()
        .find(|share| {
            if let Some(thread) = thread_id {
                share.thread_id.as_deref().is_some_and(|parent| {
                    parent == thread || lineage.iter().take(3).any(|id| id == parent)
                })
            } else {
                workspace_id.is_some_and(|id| {
                    share.workspace_id.as_deref() == Some(id) && share.workspace_access != "none"
                })
            }
        })
        .map(|share| {
            let mut share = share.clone();
            if let Some(thread) = thread_id {
                share.thread_id = Some(thread.into());
            }
            share
        });
    let mut matching: Vec<_> = grants
        .iter()
        .filter(|g| grant_matches_in(g, thread_id, workspace_id, lineage, thread_workspaces))
        .cloned()
        .collect();
    matching.sort_by_key(|g| {
        std::cmp::Reverse(match g.scope.as_str() {
            "device" => 3,
            "workspace" => 2,
            _ => 1,
        })
    });
    merge_access(false, share, &matching, thread_id, workspace_id)
}

/// WebSocket messages a browser may send. Terminal messages (`shell.*`) are the
/// owner's only; shares and grants never attach to, type into or resize a shell.
pub fn client_message_allowed(access: &EffectiveAccess, kind: &str) -> bool {
    matches!(
        kind,
        "supervisor.ping"
            | "thread.view"
            | "shell.attach"
            | "shell.detach"
            | "shell.input"
            | "shell.resize"
            | "shell.clear"
    ) && (matches!(kind, "supervisor.ping" | "thread.view") || access.kind == "owner")
}

pub fn outbound_event_in_scope(
    configured_thread: Option<&str>,
    attached_shell: Option<&str>,
    event: &Value,
) -> bool {
    let control = matches!(
        event["type"].as_str(),
        Some("supervisor.connected" | "supervisor.pong")
    );
    let attached = attached_shell.is_some_and(|id| event["shellId"].as_str() == Some(id));
    configured_thread.is_none_or(|id| control || attached || event["threadId"].as_str() == Some(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn shared(scope: &str, thread_access: &str, workspace_access: &str) -> EffectiveAccess {
        EffectiveAccess {
            kind: "shared",
            scope: scope.into(),
            thread_access: thread_access.into(),
            workspace_access: workspace_access.into(),
            thread_id: Some("root".into()),
            workspace_id: Some("workspace".into()),
            workspace_scope: Some("selected".into()),
            workspace_ids: vec![],
            grant_id: Some("grant".into()),
            share_id: None,
            can_create_threads: false,
        }
    }
    fn workspace_grant(thread_access: &str, workspace_access: &str) -> EffectiveAccess {
        EffectiveAccess {
            thread_id: None,
            workspace_id: Some("ws".into()),
            ..shared("workspace", thread_access, workspace_access)
        }
    }

    #[test]
    fn workspace_grants_reach_the_conversations_in_their_workspace() {
        let grant = workspace_grant("read", "none");
        let here = ["ws".to_string()];
        // Its own workspace, or an ancestor's within three levels.
        assert!(grant_matches_in(&grant, Some("t"), None, &[], &here));
        assert!(grant_matches_in(
            &grant,
            Some("child"),
            None,
            &[],
            &["elsewhere".into(), "ws".into()]
        ));
        assert!(!grant_matches_in(
            &grant,
            Some("deep"),
            None,
            &[],
            &["a".into(), "b".into(), "c".into(), "d".into(), "ws".into()]
        ));
        assert!(!grant_matches_in(
            &grant,
            Some("t"),
            None,
            &[],
            &["other".into()]
        ));
        // Without the device's report a thread is never in a shared workspace.
        assert!(!grant_matches(&grant, Some("t"), None, &[]));
        // Its workspace routes match whatever the file access; other workspaces never.
        assert!(grant_matches_in(&grant, None, Some("ws"), &[], &[]));
        assert!(!grant_matches_in(&grant, None, Some("other"), &[], &[]));
        assert!(!grant_matches_in(&grant, None, None, &[], &[]));

        let access = effective_access_from_rows_in(
            false,
            &[],
            std::slice::from_ref(&grant),
            Some("t"),
            None,
            &[],
            &here,
        )
        .unwrap();
        assert_eq!(access.scope, "workspace");
        assert_eq!(access.thread_id.as_deref(), Some("t"));
        assert_eq!(access.workspace_id.as_deref(), Some("ws"));
        assert!(access_allows(&access, "GET", "/api/threads/t"));
        assert!(access_allows(&access, "GET", "/api/threads/t/summary"));
        assert!(!access_allows(&access, "POST", "/api/threads/t/prompt"));
        assert!(!access_allows(&access, "GET", "/api/threads/other"));
        let control = effective_access_from_rows_in(
            false,
            &[],
            &[workspace_grant("control", "none")],
            Some("t"),
            None,
            &[],
            &here,
        )
        .unwrap();
        assert!(access_allows(&control, "POST", "/api/threads/t/prompt"));
        assert!(access_allows(
            &control,
            "POST",
            "/api/threads/t/pending-steers/p/steer"
        ));
        assert!(
            effective_access_from_rows_in(false, &[], &[grant], Some("t"), None, &[], &[])
                .is_none()
        );
    }

    #[test]
    fn workspace_shares_list_and_start_threads_only_in_their_workspace() {
        let list = "/api/workspaces/ws/threads";
        let read = workspace_grant("read", "none");
        assert!(access_allows(&read, "GET", list));
        assert!(access_allows(
            &read,
            "GET",
            "/api/workspaces/ws/threads?includeAgentThreads=true"
        ));
        assert!(access_allows(&read, "GET", "/api/workspaces/ws"));
        assert!(access_allows(
            &read,
            "GET",
            "/api/workspaces/ws/transport/key?challenge=x"
        ));
        assert!(!access_allows(&read, "POST", list));
        assert!(!access_allows(
            &read,
            "GET",
            "/api/workspaces/other/threads"
        ));
        assert!(!access_allows(&read, "GET", "/api/workspaces/other"));
        let mut control = workspace_grant("control", "none");
        assert!(!access_allows(&control, "POST", list));
        control.can_create_threads = true;
        assert!(access_allows(&control, "POST", list));
        assert!(!access_allows(
            &control,
            "POST",
            "/api/workspaces/other/threads"
        ));
        for method in ["PUT", "PATCH", "DELETE"] {
            assert!(!access_allows(&control, method, list));
        }
        // Creating threads anywhere else, and the device's thread collection, stay closed.
        for (method, path) in [
            ("POST", "/api/threads/start"),
            ("GET", "/api/threads"),
            ("GET", "/api/threads?workspaceId=ws"),
            ("GET", "/api/workspaces"),
            ("GET", "/api/search"),
            ("GET", "/api/workspaces/ws/search"),
            ("GET", "/api/device/metrics"),
            ("PATCH", "/api/workspaces/ws"),
            ("DELETE", "/api/workspaces/ws"),
        ] {
            assert!(!access_allows(&control, method, path), "{method} {path}");
        }
        // A thread share's workspace gives files at most, never its other conversations.
        let mut thread = shared("thread", "control", "write");
        thread.workspace_id = Some("ws".into());
        thread.can_create_threads = true;
        assert!(!access_allows(&thread, "GET", list));
        assert!(!access_allows(&thread, "POST", list));
        // The whole device lists any workspace's threads.
        assert!(access_allows(
            &shared("device", "read", "read"),
            "GET",
            list
        ));
    }

    #[test]
    fn workspace_files_follow_the_workspace_share_file_access() {
        for (file_access, read, write) in [
            ("none", false, false),
            ("read", true, false),
            ("write", true, true),
        ] {
            let access = workspace_grant("control", file_access);
            for path in [
                "/api/workspaces/ws/files/tree",
                "/api/workspaces/ws/files/raw?path=a",
                "/api/workspaces/ws/files/download?path=a",
            ] {
                assert_eq!(
                    access_allows(&access, "GET", path),
                    read,
                    "{file_access} {path}"
                );
            }
            for (method, path) in [
                ("POST", "/api/workspaces/ws/files/save"),
                ("PUT", "/api/workspaces/ws/files"),
                ("POST", "/api/workspaces/ws/files/upload"),
                ("PATCH", "/api/workspaces/ws/files/move"),
            ] {
                assert_eq!(
                    access_allows(&access, method, path),
                    write,
                    "{file_access} {path}"
                );
            }
            assert!(!access_allows(
                &access,
                "GET",
                "/api/workspaces/other/files/tree"
            ));
            // Files outside the workspace, even ones a conversation links to.
            assert!(!access_allows(
                &EffectiveAccess {
                    thread_id: Some("t".into()),
                    ..access.clone()
                },
                "GET",
                "/api/threads/t/linked-files/raw?path=%2Fetc%2Fpasswd"
            ));
        }
    }

    #[test]
    fn a_thread_share_does_not_hide_a_workspace_share_of_the_same_workspace() {
        let mut share = shared("thread", "control", "write");
        share.share_id = Some("share".into());
        share.grant_id = None;
        share.workspace_id = Some("ws".into());
        let grant = workspace_grant("read", "read");
        let access = effective_access_from_rows_in(
            false,
            &[share.clone()],
            std::slice::from_ref(&grant),
            None,
            Some("ws"),
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(access.scope, "workspace");
        assert_eq!(access.thread_access, "read");
        assert_eq!(access.workspace_access, "write");
        assert!(access_allows(&access, "GET", "/api/workspaces/ws/threads"));
        assert!(access_allows(
            &access,
            "POST",
            "/api/workspaces/ws/files/save"
        ));
        // The shared thread itself keeps the share's own permissions.
        let thread = effective_access_from_rows_in(
            false,
            &[share],
            &[grant],
            Some("root"),
            None,
            &[],
            &["ws".into()],
        )
        .unwrap();
        assert_eq!(thread.scope, "thread");
        assert_eq!(thread.thread_access, "control");
    }

    #[test]
    fn no_share_or_grant_reaches_a_terminal() {
        let routes = [
            ("GET", "/api/threads/root/shell"),
            ("POST", "/api/threads/root/shell"),
            ("GET", "/api/threads/root/transport/shell-scope"),
            ("PATCH", "/api/shells/s1"),
            ("POST", "/api/shells/s1/terminate"),
            ("POST", "/api/threads/root/commands"),
            ("GET", "/api/threads/root/commands/c1"),
            (
                "GET",
                "/api/threads/root/plugins/remote-codex.terminal/settings",
            ),
            (
                "PUT",
                "/api/threads/root/plugins/remote-codex.terminal/settings",
            ),
            ("GET", "/api/plugins/remote-codex.terminal"),
            ("PATCH", "/api/plugins/remote-codex.terminal"),
        ];
        for (method, path) in routes {
            assert!(terminal_route(path), "{path}");
            assert!(access_allows(&owner_access(), method, path), "{path}");
            for scope in ["thread", "workspace", "device"] {
                for level in ["read", "control"] {
                    let mut access = shared(scope, level, "write");
                    access.can_create_threads = true;
                    assert!(
                        !access_allows(&access, method, path),
                        "{scope} {level} {method} {path}"
                    );
                }
            }
        }
        for path in [
            "/api/threads/root/plugins",
            "/api/threads/root/plugins/pockymoe.content-quality/settings",
            "/api/threads/root/summary",
            "/api/plugins",
        ] {
            assert!(!terminal_route(path), "{path}");
        }
    }

    #[test]
    fn literal_thread_routes_do_not_become_thread_ids() {
        for literal in ["start", "import", "import-candidates"] {
            let path = format!("/api/threads/{literal}");
            assert!(resource_id_from_path(&path, "threads").is_none());
            assert!(resource_id_from_path(&format!("{path}?view=all"), "threads").is_none());
            for level in ["read", "control"] {
                assert!(!access_allows(
                    &shared("device", level, "write"),
                    "GET",
                    &path
                ));
            }
            assert!(access_allows(&owner_access(), "GET", &path));
        }
        assert_eq!(
            resource_id_from_path("/api/threads/root/history", "threads").as_deref(),
            Some("root")
        );
    }
    #[test]
    fn linked_host_files_need_the_whole_device_with_workspace_write() {
        let path = "/api/threads/root/linked-files/preview?path=%2Fdata%2Fclip.mp4";
        for action in ["stat", "preview", "raw"] {
            let path = format!("/api/threads/root/linked-files/{action}?path=%2Fdata%2Fclip.mp4");
            assert!(
                access_allows(&shared("device", "read", "write"), "GET", &path),
                "{action}"
            );
            assert!(access_allows(&owner_access(), "GET", &path));
        }
        for access in [
            shared("device", "control", "read"),
            shared("device", "control", "none"),
            shared("thread", "control", "write"),
            shared("workspace", "control", "write"),
        ] {
            assert!(!access_allows(&access, "GET", path), "{access:?}");
        }
        let device = shared("device", "control", "write");
        assert!(!access_allows(&device, "POST", path));
        assert!(!access_allows(
            &device,
            "GET",
            "/api/threads/root/linked-files/delete?path=x"
        ));
    }
    #[test]
    fn grantee_registration_exception_is_encrypted_post_to_one_exact_endpoint() {
        for path in ["/api/transport/browser", "/api/transport/browser?view=full"] {
            assert!(grantee_registration_allowed("POST", path, true));
            assert!(!grantee_registration_allowed("POST", path, false));
            for method in ["GET", "PUT", "PATCH", "DELETE"] {
                assert!(!grantee_registration_allowed(method, path, true));
            }
        }
        for path in [
            "/api/transport/revocations",
            "/api/transport/revocations?all=1",
        ] {
            assert!(grantee_registration_allowed("PUT", path, true));
            assert!(!grantee_registration_allowed("PUT", path, false));
            for method in ["GET", "POST", "PATCH", "DELETE"] {
                assert!(!grantee_registration_allowed(method, path, true));
            }
        }
        for path in [
            "/api/transport/browser/",
            "/api/transport/browser/extra",
            "/api/transport/revocations/extra",
            "/api/transport/session",
            "/api/transport/key",
            "/api/transport/trust",
            "/api/transport/pair",
            "/api/transport/grants",
        ] {
            assert!(!grantee_registration_allowed("POST", path, true), "{path}");
        }
    }

    #[test]
    fn merge_preserves_owner_then_share_then_highest_grant_access() {
        let read = shared("thread", "read", "none");
        let mut device = shared("device", "control", "write");
        device.can_create_threads = true;
        assert_eq!(
            merge_access(
                true,
                Some(read.clone()),
                &[device.clone()],
                Some("child"),
                None
            )
            .unwrap()
            .kind,
            "owner"
        );
        assert_eq!(
            merge_access(
                false,
                Some(read.clone()),
                &[device.clone()],
                Some("child"),
                None
            )
            .unwrap()
            .scope,
            "thread"
        );
        // A share with no filesystem access does not mask a matching workspace grant.
        let merged = merge_access(
            false,
            Some(read.clone()),
            &[device.clone()],
            None,
            Some("workspace"),
        )
        .unwrap();
        assert_eq!(merged.scope, "device");
        assert_eq!(merged.workspace_access, "write");
        let merged =
            merge_access(false, None, &[read.clone(), device], Some("child"), None).unwrap();
        assert_eq!(merged.scope, "device");
        assert_eq!(merged.thread_access, "control");
        assert!(merged.can_create_threads);
        assert!(merged.thread_id.is_none());
        let merged = merge_access(false, None, &[read], Some("child"), None).unwrap();
        assert_eq!(merged.thread_id.as_deref(), Some("child"));
        assert!(merge_access(false, None, &[], None, None).is_none());
    }
    #[test]
    fn row_matching_uses_only_three_provided_ancestors_and_scope() {
        let grant = shared("thread", "read", "none");
        let inherited = effective_access_from_rows(
            false,
            &[grant.clone()],
            &[],
            Some("child"),
            None,
            &["root".into()],
        )
        .unwrap();
        assert_eq!(inherited.thread_id.as_deref(), Some("child"));
        assert!(
            effective_access_from_rows(false, &[grant.clone()], &[], Some("root"), None, &[])
                .is_some()
        );
        assert!(
            effective_access_from_rows(false, &[grant.clone()], &[], Some("other"), None, &[])
                .is_none()
        );

        assert!(grant_matches(&grant, Some("root"), None, &[]));
        assert!(grant_matches(&grant, Some("child"), None, &["root".into()]));
        assert!(!grant_matches(
            &grant,
            Some("child"),
            None,
            &["a".into(), "b".into(), "c".into(), "root".into()]
        ));
        assert!(!grant_matches(&grant, None, Some("workspace"), &[]));
        assert!(grant_matches(
            &shared("workspace", "read", "write"),
            None,
            Some("workspace"),
            &[]
        ));
        assert!(!grant_matches(
            &shared("workspace", "read", "write"),
            Some("root"),
            None,
            &[]
        ));
        assert!(grant_matches(
            &shared("device", "read", "none"),
            Some("root"),
            None,
            &[]
        ));
        assert!(!grant_matches(
            &shared("device", "read", "none"),
            None,
            Some("workspace"),
            &[]
        ));
    }
    #[test]
    fn harness_processes_and_settings_follow_device_metrics_access() {
        let thread = shared("thread", "control", "write");
        let workspace = shared("workspace", "control", "write");
        let device_read = shared("device", "read", "none");
        let device_control = shared("device", "control", "none");
        for path in ["/api/device/processes", "/api/harness-resources/settings"] {
            assert!(!access_allows(&thread, "GET", path), "{path}");
            assert!(!access_allows(&workspace, "GET", path), "{path}");
            assert!(access_allows(&device_read, "GET", path), "{path}");
        }
        let settings = "/api/harness-resources/settings";
        assert!(!access_allows(&thread, "PUT", settings));
        assert!(!access_allows(&device_read, "PUT", settings));
        assert!(access_allows(&device_control, "PUT", settings));
        assert!(!access_allows(
            &device_control,
            "POST",
            "/api/device/processes"
        ));
        // Release is a control action on the thread, like interrupt.
        assert!(access_allows(&thread, "POST", "/api/threads/root/release"));
        assert!(!access_allows(
            &shared("thread", "read", "read"),
            "POST",
            "/api/threads/root/release"
        ));
        assert!(!access_allows(
            &thread,
            "POST",
            "/api/threads/other/release"
        ));
    }

    #[test]
    fn plugin_settings_and_manual_actions_remain_owner_routes() {
        for (method, path) in [
            ("GET", "/api/plugins/pockymoe.content-quality/settings"),
            ("PUT", "/api/plugins/pockymoe.content-quality/settings"),
            (
                "POST",
                "/api/threads/root/plugins/pockymoe.content-quality/actions/reviewDocument",
            ),
            // Installed plugins clone and run code: owner only, on every route.
            ("POST", "/api/plugins/install"),
            ("POST", "/api/plugins/owner.tool/update"),
            ("POST", "/api/plugins/owner.tool/actions/status"),
            ("GET", "/api/plugins/owner.tool/runs"),
            ("DELETE", "/api/plugins/owner.tool"),
        ] {
            assert!(access_allows(&owner_access(), method, path));
            for scope in ["thread", "workspace", "device"] {
                assert!(!access_allows(
                    &shared(scope, "control", "write"),
                    method,
                    path
                ));
            }
        }
    }

    #[test]
    fn thread_plugin_scope_descriptors_are_readable_but_settings_require_control() {
        for scope in ["thread", "device"] {
            let read = shared(scope, "read", "none");
            let control = shared(scope, "control", "none");
            assert!(access_allows(&read, "GET", "/api/threads/root/plugins"));
            for method in ["GET", "PUT"] {
                let path = "/api/threads/root/plugins/pockymoe.content-quality/settings";
                assert!(!access_allows(&read, method, path));
                assert!(access_allows(&control, method, path));
                if scope == "thread" {
                    assert!(!access_allows(
                        &control,
                        method,
                        "/api/threads/other/plugins/pockymoe.content-quality/settings"
                    ));
                }
            }
            for path in [
                "/api/threads/root/plugins/p/settings/extra",
                "/api/threads/root/plugins/p/arbitrary",
                "/api/plugins/p/settings",
            ] {
                assert!(!access_allows(&control, "GET", path));
                assert!(!access_allows(&control, "PUT", path));
            }
        }
    }

    #[test]
    fn native_upstream_management_stays_owner_only() {
        let routes = [
            ("GET", "/api/management/connections"),
            ("POST", "/api/management/upstreams/native/claude/activate"),
            ("POST", "/api/management/upstreams/native/codex/activate"),
        ];
        for (method, path) in routes {
            assert!(access_allows(&owner_access(), method, path));
            for scope in ["device", "thread"] {
                for permission in ["read", "control"] {
                    assert!(
                        !access_allows(&shared(scope, permission, "write"), method, path),
                        "{scope} {permission} {method} {path}"
                    );
                }
            }
        }
    }

    #[test]
    fn read_control_workspace_search_and_owner_routes_share_one_allowlist() {
        let read = shared("thread", "read", "read");
        for path in [
            "/api/threads/root",
            "/api/threads/root/search",
            "/api/workspaces/workspace/files/raw",
        ] {
            assert!(access_allows(&read, "GET", path), "{path}");
        }
        for (method, path) in [
            ("POST", "/api/threads/root/prompt"),
            ("POST", "/api/workspaces/workspace/files/save"),
            ("GET", "/api/threads/other"),
            ("GET", "/api/search"),
            ("GET", "/api/transport/grants"),
            ("GET", "/api/device/metrics"),
        ] {
            assert!(!access_allows(&read, method, path), "{path}");
        }
        let control = shared("thread", "control", "write");
        assert!(access_allows(&control, "POST", "/api/threads/root/prompt"));
        assert!(access_allows(
            &control,
            "POST",
            "/api/workspaces/workspace/files/save"
        ));
        let device = shared("device", "read", "none");
        for path in [
            "/api/search",
            "/api/workspaces/any/search",
            "/api/device/metrics",
        ] {
            assert!(access_allows(&device, "GET", path));
        }
        assert!(!access_allows(
            &device,
            "GET",
            "/api/workspaces/any/files/raw"
        ));
        let owner = EffectiveAccess {
            kind: "owner",
            ..read.clone()
        };
        assert!(client_message_allowed(&read, "supervisor.ping"));
        // Viewer reports are transport control messages. The Supervisor checks
        // every listed thread and ignores read-only viewers for harness retention.
        assert!(client_message_allowed(&read, "thread.view"));
        assert!(client_message_allowed(&control, "thread.view"));
        assert!(client_message_allowed(&owner, "thread.view"));
        // Terminals are the owner's only, even for control access.
        for kind in [
            "shell.attach",
            "shell.input",
            "shell.resize",
            "shell.detach",
            "shell.clear",
        ] {
            assert!(!client_message_allowed(&read, kind));
            assert!(!client_message_allowed(&control, kind));
            assert!(!client_message_allowed(
                &shared("device", "control", "write"),
                kind
            ));
            assert!(client_message_allowed(&owner, kind));
        }
        assert!(!client_message_allowed(&control, "thread.prompt"));
        assert!(outbound_event_in_scope(
            Some("root"),
            None,
            &json!({"type":"supervisor.pong"})
        ));
        assert!(outbound_event_in_scope(
            Some("root"),
            None,
            &json!({"type":"thread.updated","threadId":"root"})
        ));
        assert!(!outbound_event_in_scope(
            Some("root"),
            None,
            &json!({"type":"thread.updated","threadId":"other"})
        ));
    }
}
