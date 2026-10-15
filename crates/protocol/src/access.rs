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

pub fn access_allows(access: &EffectiveAccess, method: &str, path: &str) -> bool {
    if access.kind == "owner" {
        return true;
    }
    let pathname = path.split('?').next().unwrap_or(path);
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
        if !workspace_matches || access.workspace_access == "none" {
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
    if automation.first() == Some(&"commands") {
        // Command argv/output belong to device control access, not read-only shares.
        return control
            && matches!(
                (method, automation.as_slice()),
                ("POST", ["commands"]) | ("GET", ["commands", _])
            );
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
    let workspace_access = if matching
        .iter()
        .any(|grant| grant.workspace_access == "write")
    {
        "write"
    } else if matching
        .iter()
        .any(|grant| grant.workspace_access == "read")
    {
        "read"
    } else {
        "none"
    };
    let mut merged = representative;
    merged.scope = merged_scope.to_string();
    merged.thread_id = if merged_scope == "thread" {
        thread_id.map(str::to_string).or(merged.thread_id)
    } else {
        None
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

pub fn grant_matches(
    grant: &EffectiveAccess,
    thread_id: Option<&str>,
    workspace_id: Option<&str>,
    lineage: &[String],
) -> bool {
    match grant.scope.as_str() {
        "device" => workspace_id.is_none() || grant.workspace_access != "none",
        "thread" => thread_id.is_some_and(|id| {
            grant.thread_id.as_deref().is_some_and(|parent| {
                parent == id || lineage.iter().take(3).any(|ancestor| ancestor == parent)
            })
        }),
        "workspace" => {
            grant.workspace_access != "none"
                && workspace_id.is_some_and(|id| Some(id) == grant.workspace_id.as_deref())
        }
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
        .filter(|g| grant_matches(g, thread_id, workspace_id, lineage))
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
    ) && (matches!(kind, "supervisor.ping" | "thread.view")
        || access.kind == "owner"
        || access.thread_access == "control")
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
        assert!(client_message_allowed(&read, "supervisor.ping"));
        // Viewer reports are transport control messages. The Supervisor checks
        // every listed thread and ignores read-only viewers for harness retention.
        assert!(client_message_allowed(&read, "thread.view"));
        assert!(client_message_allowed(&control, "thread.view"));
        let owner = EffectiveAccess {
            kind: "owner".into(),
            ..read.clone()
        };
        assert!(client_message_allowed(&owner, "thread.view"));
        assert!(!client_message_allowed(&read, "shell.attach"));
        assert!(client_message_allowed(&control, "shell.attach"));
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
