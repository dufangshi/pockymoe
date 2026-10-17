//! Relay HTTP method adapter for the shared pure authorization rules.
use super::*;
pub(crate) use pockymoe_protocol::access::{relay_target_allowed, resource_id_from_path};
#[cfg(test)]
use pockymoe_protocol::access::{
    shared_runtime_metadata_allowed, shared_thread_path_allowed, shared_workspace_path_allowed,
};

pub(crate) fn access_allows(access: &EffectiveAccess, method: &Method, path: &str) -> bool {
    pockymoe_protocol::access::access_allows(access, method.as_str(), path)
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
    fn thread_summary_is_scoped_read_access() {
        let mut access = owner_access();
        access.kind = "shared";
        access.scope = "thread".into();
        access.thread_id = Some("thread-1".into());
        access.thread_access = "read".into();
        for (method, path, allowed) in [
            ("GET", "/api/threads/thread-1/summary", true),
            ("GET", "/api/threads/thread-1/summary?maxChars=2000", true),
            ("GET", "/api/threads/thread-2/summary", false),
            ("GET", "/api/threads/thread-1/summary/extra", false),
            ("POST", "/api/threads/thread-1/summary", false),
        ] {
            assert_eq!(
                access_allows(
                    &access,
                    &Method::from_bytes(method.as_bytes()).unwrap(),
                    path
                ),
                allowed,
                "{method} {path}"
            );
        }
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
            // Only a whole device shared with workspace write reads files outside workspaces.
            let mut device = access.clone();
            device.scope = "device".into();
            device.thread_access = "control".into();
            device.workspace_access = "read".into();
            assert!(!access_allows(&device, &Method::GET, &path));
            device.workspace_access = "write".into();
            assert!(access_allows(&device, &Method::GET, &path));
            assert!(!access_allows(&device, &Method::POST, &path));
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
    #[test]
    fn thread_plugin_scope_relay_uses_control_for_private_settings() {
        for control in [false, true] {
            assert!(shared_thread_path_allowed(
                "GET",
                "/api/threads/t/plugins",
                "t",
                control
            ));
            for method in ["GET", "PUT"] {
                assert_eq!(
                    shared_thread_path_allowed(
                        method,
                        "/api/threads/t/plugins/p/settings",
                        "t",
                        control
                    ),
                    control
                );
                assert!(!shared_thread_path_allowed(
                    method,
                    "/api/threads/other/plugins/p/settings",
                    "t",
                    control
                ));
                assert!(!shared_thread_path_allowed(
                    method,
                    "/api/threads/t/plugins/p/settings/extra",
                    "t",
                    control
                ));
            }
        }
    }
}
