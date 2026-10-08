//! Cross-tenant route authorization for shared (non-owner) relay access.
//!
//! Pure predicates: no AppState, no database, no async. Kept in one module so the
//! full allowlist is reviewable in isolation rather than spread through lib.rs.
use super::*;

pub(crate) fn resource_id_from_path(path: &str, resource: &str) -> Option<String> {
    let path = path.split('?').next().unwrap_or(path);
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    while let Some(segment) = segments.next() {
        if segment == resource {
            let id = segments.next()?;
            if id == "start" || id == "import" {
                return None;
            }
            return Some(id.to_string());
        }
    }
    None
}

pub(crate) fn relay_target_allowed(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    path == "/healthz" || path.starts_with("/api/") || path == "/api"
}

pub(crate) fn access_allows(access: &EffectiveAccess, method: &Method, path: &str) -> bool {
    if access.kind == "owner" {
        return true;
    }
    let method = method.as_str();
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
    access.scope == "device"
        && method == "GET"
        // Collection pages cannot use a thread/workspace-scoped handshake before
        // they have loaded the collection. This publishes only the device's signed
        // recipient key; each subsequent request still goes through this ACL.
        && matches!(pathname, "/api/threads" | "/api/workspaces" | "/api/transport/key" | "/api/device/metrics")
}

pub(crate) fn shared_runtime_metadata_allowed(method: &str, pathname: &str) -> bool {
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

pub(crate) fn shared_thread_path_allowed(
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

pub(crate) fn shared_workspace_path_allowed(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_file_routes_require_exact_workspace_and_write_for_operations() {
        let base = "/api/workspaces/ws";
        assert!(!shared_workspace_path_allowed(
            "POST",
            &format!("{base}/files"),
            "ws",
            false
        ));
        assert!(shared_workspace_path_allowed(
            "POST",
            &format!("{base}/files"),
            "ws",
            true
        ));
        assert!(!shared_workspace_path_allowed(
            "POST",
            &format!("{base}/files"),
            "other",
            true
        ));
        for suffix in ["/files/capabilities", "/files/document"] {
            assert!(shared_workspace_path_allowed(
                "GET",
                &format!("{base}{suffix}"),
                "ws",
                false
            ));
            assert!(!shared_workspace_path_allowed(
                "GET",
                &format!("{base}{suffix}"),
                "other",
                true
            ));
        }
        assert!(!shared_workspace_path_allowed(
            "POST",
            &format!("{base}/files/save"),
            "ws",
            false
        ));
        assert!(shared_workspace_path_allowed(
            "POST",
            &format!("{base}/files/save"),
            "ws",
            true
        ));
        assert!(!shared_workspace_path_allowed(
            "GET",
            &format!("{base}/files/operations/id"),
            "ws",
            false
        ));
        assert!(shared_workspace_path_allowed(
            "GET",
            &format!("{base}/files/operations/id"),
            "ws",
            true
        ));
        assert!(!shared_workspace_path_allowed(
            "GET",
            &format!("{base}/files/operations/id/extra"),
            "ws",
            true
        ));
    }
    #[test]
    fn global_search_requires_device_conversation_access() {
        let mut access = owner_access();
        for path in [
            "/api/search?q=secret",
            "/api/workspaces/workspace-1/search?q=secret",
        ] {
            assert!(access_allows(&access, &Method::GET, path));
        }
        access.kind = "shared";
        access.thread_id = Some("thread-1".into());
        access.workspace_id = Some("workspace-1".into());
        access.workspace_scope = Some("all".into());
        access.workspace_access = "write".into();
        access.thread_access = "read".into();
        for scope in ["thread", "workspace"] {
            access.scope = scope.into();
            for path in [
                "/api/search?q=secret&threadId=thread-1",
                "/api/workspaces/workspace-1/search?q=secret",
                "/api/workspaces/workspace-2/search?q=secret",
            ] {
                assert!(
                    !access_allows(&access, &Method::GET, path),
                    "{scope}: {path}"
                );
            }
        }
        access.scope = "device".into();
        access.workspace_access = "none".into();
        assert!(access_allows(
            &access,
            &Method::GET,
            "/api/workspaces/workspace-1/transport/key?challenge=fresh"
        ));
        assert!(!access_allows(
            &access,
            &Method::POST,
            "/api/workspaces/workspace-1/transport/key"
        ));
        assert!(!access_allows(
            &access,
            &Method::GET,
            "/api/workspaces/workspace-1/files/raw"
        ));
        assert!(access_allows(&access, &Method::GET, "/api/search"));
        assert!(access_allows(
            &access,
            &Method::GET,
            "/api/workspaces/workspace-1/search"
        ));
        assert!(!access_allows(&access, &Method::POST, "/api/search"));
        assert!(!access_allows(
            &access,
            &Method::GET,
            "/api/workspaces/workspace-1/files/search"
        ));
        access.thread_access = "none".into();
        assert!(!access_allows(&access, &Method::GET, "/api/search"));
        assert!(!access_allows(
            &access,
            &Method::GET,
            "/api/workspaces/workspace-1/search"
        ));
    }

    #[test]
    fn conversation_search_is_scoped_read_access() {
        assert!(shared_thread_path_allowed(
            "GET",
            "/api/threads/thread-1/search",
            "thread-1",
            false
        ));
        assert!(!shared_thread_path_allowed(
            "GET",
            "/api/threads/thread-2/search",
            "thread-1",
            false
        ));
        assert!(!shared_thread_path_allowed(
            "POST",
            "/api/threads/thread-1/search",
            "thread-1",
            true
        ));
    }

    #[test]
    fn history_detail_paths_preserve_wire_encoding_and_shared_read_access() {
        for prefix in ["/relay", "/relay/devices/device-1"] {
            for resource in ["turns", "items"] {
                let encoded =
                    format!("threads/thread-1/{resource}/thread-1%3Ascheduled%3Atimer-1/detail");
                let uri: Uri = format!("{prefix}/api/{encoded}?token=secret&view=full")
                    .parse()
                    .unwrap();
                let decoded = encoded.replace("%3A", ":");
                let target = relay_api_target_path(&decoded, &uri);
                assert_eq!(target, format!("/api/{encoded}?view=full"));
                assert!(shared_thread_path_allowed(
                    "GET",
                    target.split('?').next().unwrap(),
                    "thread-1",
                    false
                ));
                assert!(!shared_thread_path_allowed(
                    "POST",
                    target.split('?').next().unwrap(),
                    "thread-1",
                    false
                ));
                assert!(!shared_thread_path_allowed(
                    "GET",
                    target.split('?').next().unwrap(),
                    "other-thread",
                    false
                ));
            }
        }
    }

    #[test]
    fn shared_access_uses_explicit_route_allowlists() {
        let mut access = EffectiveAccess {
            kind: "shared",
            grant_id: None,
            share_id: Some("share".to_string()),
            scope: "thread".to_string(),
            thread_id: Some("thread-1".to_string()),
            thread_access: "read".to_string(),
            workspace_access: "none".to_string(),
            workspace_id: None,
            workspace_scope: Some("selected".to_string()),
            workspace_ids: Vec::new(),
            can_create_threads: false,
        };
        assert!(!access_allows(&access, &Method::GET, "/api/transport/key"));
        let mut device_access = access.clone();
        device_access.scope = "device".into();
        assert!(access_allows(
            &device_access,
            &Method::GET,
            "/api/device/metrics"
        ));
        assert!(!access_allows(
            &device_access,
            &Method::POST,
            "/api/device/metrics"
        ));
        assert!(!access_allows(&access, &Method::GET, "/api/device/metrics"));
        let mut workspace_access = access.clone();
        workspace_access.scope = "workspace".into();
        assert!(!access_allows(
            &workspace_access,
            &Method::GET,
            "/api/device/metrics"
        ));
        assert!(access_allows(
            &device_access,
            &Method::GET,
            "/api/transport/key?challenge=fresh"
        ));
        for (method, path) in [
            (Method::POST, "/api/transport/key"),
            (Method::POST, "/api/transport/session"),
            (Method::GET, "/api/transport/private"),
            (Method::GET, "/api/management/upstreams"),
            (Method::POST, "/api/management/supervisor/restart"),
        ] {
            assert!(!access_allows(&device_access, &method, path));
        }
        assert!(access_allows(
            &access,
            &Method::GET,
            "/api/threads/thread-1/items/item-1/detail"
        ));
        for action in ["stat", "preview", "raw"] {
            let path = format!("/api/threads/thread-1/linked-files/{action}?path=/private/file");
            assert!(!access_allows(&access, &Method::GET, &path));
            let mut device = access.clone();
            device.scope = "device".into();
            device.thread_access = "control".into();
            device.workspace_access = "write".into();
            assert!(!access_allows(&device, &Method::GET, &path));
            assert!(access_allows(&owner_access(), &Method::GET, &path));
        }

        assert!(!access_allows(
            &access,
            &Method::GET,
            "/api/threads/thread-1/private-debug"
        ));
        assert!(!access_allows(
            &access,
            &Method::POST,
            "/api/threads/thread-1/prompt"
        ));
        assert!(!access_allows(
            &access,
            &Method::POST,
            "/api/threads/thread-1/pending-steers/pending-1/steer"
        ));
        access.thread_access = "control".to_string();
        for route in [
            "/api/management/harnesses",
            "/api/management/upstreams",
            "/api/management/upstreams/profile-1",
            "/api/management/templates",
            "/api/management/supervisor",
            "/api/management/harnesses/codex",
            "/api/management/supervisor/update",
            "/api/management/supervisor/restart",
        ] {
            assert!(!access_allows(&access, &Method::GET, route));
            assert!(!access_allows(&access, &Method::POST, route));
        }
        assert!(access_allows(
            &access,
            &Method::POST,
            "/api/threads/thread-1/prompt"
        ));
        assert!(access_allows(
            &access,
            &Method::POST,
            "/api/threads/thread-1/pending-steers/pending-1/steer"
        ));
        assert!(!access_allows(
            &access,
            &Method::POST,
            "/api/threads/thread-1/pending-steers/pending-1"
        ));
        assert!(!access_allows(
            &access,
            &Method::DELETE,
            "/api/threads/thread-1"
        ));
        let uri: Uri = "/relay/api/threads?token=secret&workspaceId=workspace-1"
            .parse()
            .unwrap();
        assert_eq!(
            relay_api_target_path("threads", &uri),
            "/api/threads?workspaceId=workspace-1"
        );
    }

    #[test]
    fn harness_catalog_is_shared_metadata_like_models() {
        assert!(shared_runtime_metadata_allowed(
            "GET",
            "/api/agent-runtimes/acp/harness"
        ));
        assert!(!shared_runtime_metadata_allowed(
            "POST",
            "/api/agent-runtimes/acp/harness"
        ));
    }

    #[test]
    fn harness_panel_actions_stay_owner_only() {
        // They edit the device's harness profile, not just one shared thread.
        for control in [false, true] {
            assert!(!shared_thread_path_allowed(
                "POST",
                "/api/threads/t/harness",
                "t",
                control
            ));
        }
    }

    #[test]
    fn supervisor_restart_requires_device_owner_even_for_control_grants() {
        let mut access = owner_access();
        let route = "/api/management/supervisor/restart";
        assert!(access_allows(&access, &Method::POST, route));
        access.kind = "shared".into();
        access.thread_access = "control".into();
        access.workspace_access = "write".into();
        access.can_create_threads = true;
        for scope in ["thread", "workspace", "device"] {
            access.scope = scope.into();
            assert!(!access_allows(&access, &Method::POST, route), "{scope}");
            assert!(!access_allows(
                &access,
                &Method::POST,
                &format!("{route}?owner=true")
            ));
        }
    }
}

#[cfg(test)]
mod automation_acl_tests {
    use super::*;
    #[test]
    fn native_subagent_routes_stay_within_the_shared_parent_thread() {
        for suffix in ["/subagents", "/subagents/child-1"] {
            assert!(shared_thread_path_allowed(
                "GET",
                &format!("/api/threads/parent{suffix}"),
                "parent",
                false
            ));
            assert!(!shared_thread_path_allowed(
                "GET",
                &format!("/api/threads/another{suffix}"),
                "parent",
                false
            ));
            assert!(!shared_thread_path_allowed(
                "POST",
                &format!("/api/threads/parent{suffix}"),
                "parent",
                false
            ));
        }
        assert!(!shared_thread_path_allowed(
            "GET",
            "/api/threads/parent/subagents/child-1/settings",
            "parent",
            false
        ));
    }

    #[test]
    fn automation_routes_preserve_read_control_boundaries_and_reject_unknown_actions() {
        for suffix in [
            "/automations",
            "/automations/hook",
            "/automations/hook/runs",
        ] {
            assert!(shared_thread_path_allowed(
                "GET",
                &format!("/api/threads/t{suffix}"),
                "t",
                false
            ));
        }
        for suffix in [
            "/automations",
            "/automations/preview",
            "/automations/hook/pause",
            "/automations/hook/resume",
            "/automations/hook/cancel",
            "/commands",
        ] {
            let path = format!("/api/threads/t{suffix}");
            assert!(!shared_thread_path_allowed("POST", &path, "t", false));
            assert!(shared_thread_path_allowed("POST", &path, "t", true));
        }
        assert!(!shared_thread_path_allowed(
            "POST",
            "/api/threads/t/automations/hook/arbitrary",
            "t",
            true
        ));
        assert!(!shared_thread_path_allowed(
            "GET",
            "/api/threads/t/commands/c",
            "t",
            false
        ));
        assert!(shared_thread_path_allowed(
            "GET",
            "/api/threads/t/commands/c",
            "t",
            true
        ));
        assert!(!shared_workspace_path_allowed(
            "POST",
            "/api/workspaces/w/automations",
            "w",
            true
        ));
    }
}
