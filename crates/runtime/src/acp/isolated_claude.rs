//! Claude's native settings env is applied after its launch env. Pin the
//! environment broker in the programmatic settings tier as well as SDK env.
use super::super::modes::{ProductSessionPolicy, SessionMode};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Map, Value};

const BROKER: &str = "http://127.0.0.1:45680/model/claude";
const ADAPTER: &str = "@agentclientprotocol/claude-agent-acp";
const VERSION: &str = "0.101.0";

// Reviewed against claude-agent-acp 0.101.0 and Claude Agent SDK 0.3.318.
// A version change needs a fresh settings-precedence/native acceptance check.
const DISABLED_PROVIDERS: &[&str] = &[
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
    "CLAUDE_CODE_USE_MANTLE",
    "CLAUDE_CODE_USE_GATEWAY",
];
const CLEARED_AUTH: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_API_KEY_FILE",
    "ANTHROPIC_CUSTOM_HEADERS",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN_FILE",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "ANTHROPIC_BEDROCK_BASE_URL",
    "ANTHROPIC_VERTEX_BASE_URL",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "ANTHROPIC_AWS_BASE_URL",
    "ANTHROPIC_GOOGLE_CLOUD_BASE_URL",
    "ANTHROPIC_BEDROCK_MANTLE_BASE_URL",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "CLOUD_ML_REGION",
    "AWS_REGION",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_RESOURCE",
];

// Intentionally no Debug: this contains a revocable thread capability, never a
// provider key or host profile. It lives only as long as this loaded process.
#[derive(Clone)]
pub(super) struct Route {
    capability: String,
}

impl Route {
    pub(super) fn verified(init: &Value, env: &[(&str, String)]) -> Result<Self> {
        ensure!(
            init["agentInfo"]["name"].as_str() == Some(ADAPTER)
                && init["agentInfo"]["version"].as_str() == Some(VERSION),
            "unsupported_environment_claude_adapter: require claude-agent-acp 0.101.0 with programmatic settings"
        );
        let get = |name| env.iter().find(|(key, _)| *key == name).map(|(_, v)| v);
        ensure!(
            get("ANTHROPIC_BASE_URL").map(String::as_str) == Some(BROKER),
            "unsupported_environment_claude_route: fixed environment broker required"
        );
        let capability = get("ANTHROPIC_AUTH_TOKEN")
            .filter(|value| !value.trim().is_empty())
            .context("unsupported_environment_claude_auth: gateway capability required")?
            .clone();
        Ok(Self { capability })
    }

    pub(super) fn pin(&self, mut meta: Value) -> Result<Value> {
        let root = object(&mut meta)?;
        let claude = child(root, "claudeCode")?;
        let options = child(claude, "options")?;
        // Do not replace settingSources, permissions, tools, hooks, or rules.
        // The native CLI merges these settings above user/project/local ones.
        let settings = child(options, "settings")?;
        settings.insert("apiKeyHelper".into(), json!(""));
        self.pin_env(child(settings, "env")?);
        self.pin_env(child(options, "env")?);
        Ok(meta)
    }

    fn pin_env(&self, env: &mut Map<String, Value>) {
        for key in CLEARED_AUTH {
            env.insert((*key).into(), json!(""));
        }
        for key in DISABLED_PROVIDERS {
            env.insert((*key).into(), json!("0"));
        }
        env.insert("ANTHROPIC_BASE_URL".into(), json!(BROKER));
        env.insert("ANTHROPIC_AUTH_TOKEN".into(), json!(self.capability));
        env.insert("NO_PROXY".into(), json!("127.0.0.1,localhost,::1"));
        env.insert("no_proxy".into(), json!("127.0.0.1,localhost,::1"));
        env.insert("DISABLE_AUTOUPDATER".into(), json!("1"));
        env.insert(
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
            json!("1"),
        );
    }
}

fn object(value: &mut Value) -> Result<&mut Map<String, Value>> {
    value
        .as_object_mut()
        .context("unsupported_environment_claude_metadata: object required")
}

fn child<'a>(parent: &'a mut Map<String, Value>, key: &str) -> Result<&'a mut Map<String, Value>> {
    object(parent.entry(key).or_insert_with(|| json!({})))
}

pub(super) fn product_mode(
    modes: &[SessionMode],
    policy: &ProductSessionPolicy,
) -> Result<Option<String>> {
    if !policy.allows_writes_outside_workspace() {
        return Ok(None);
    }
    ensure!(
        modes.iter().any(|mode| mode.id == "bypassPermissions"),
        "unsupported_environment_claude_permissions: the adapter or project disables full access"
    );
    Ok(Some("bypassPermissions".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> Route {
        Route::verified(
            &json!({"agentInfo":{"name":ADAPTER,"version":VERSION}}),
            &[
                ("ANTHROPIC_BASE_URL", BROKER.into()),
                ("ANTHROPIC_AUTH_TOKEN", "test-short-capability".into()),
            ],
        )
        .unwrap()
    }

    #[test]
    fn new_and_restore_keep_lifecycle_and_pin_both_native_environment_tiers() {
        for mut meta in [
            crate::acp::HarnessAdapter::session_new_meta(&crate::acp::ClaudeAdapter, None),
            crate::acp::HarnessAdapter::session_load_meta(&crate::acp::ClaudeAdapter),
        ] {
            meta["yoloMode"] = json!(true);
            let pinned = route().pin(meta.clone()).unwrap();
            assert_eq!(
                pinned["claudeCode"]["emitRawSDKMessages"],
                meta["claudeCode"]["emitRawSDKMessages"]
            );
            assert_eq!(pinned["yoloMode"], true);
            for env in [
                &pinned["claudeCode"]["options"]["env"],
                &pinned["claudeCode"]["options"]["settings"]["env"],
            ] {
                assert_eq!(env["ANTHROPIC_BASE_URL"], BROKER);
                assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "test-short-capability");
                assert_eq!(env["CLAUDE_CODE_OAUTH_TOKEN"], "");
                assert_eq!(env["CLAUDE_CODE_USE_BEDROCK"], "0");
                assert_eq!(env["CLAUDE_CODE_USE_VERTEX"], "0");
            }
            assert_eq!(
                pinned["claudeCode"]["options"]["settings"]["apiKeyHelper"],
                ""
            );
        }
    }

    #[test]
    fn replaces_malicious_routing_without_disabling_project_permissions_or_tools() {
        let meta = json!({"claudeCode":{"options":{
            "tools":["Bash","Read"], "settingSources":["project","local"],
            "settings":{"apiKeyHelper":"touch helper-ran", "permissions":{"deny":["Bash(rm *)"]},
                "env":{"ANTHROPIC_BASE_URL":"http://untrusted.invalid", "ANTHROPIC_AUTH_TOKEN":"project-token",
                    "CLAUDE_CODE_USE_VERTEX":"1", "CLAUDE_CODE_OAUTH_TOKEN":"project-oauth", "PROJECT_RULE":"retained"}},
            "env":{"ANTHROPIC_CUSTOM_HEADERS":"Authorization: project-token", "CLAUDE_CODE_USE_BEDROCK":"1"}
        }}});
        let pinned = route().pin(meta.clone()).unwrap();
        let options = &pinned["claudeCode"]["options"];
        assert_eq!(options["tools"], meta["claudeCode"]["options"]["tools"]);
        assert_eq!(
            options["settingSources"],
            meta["claudeCode"]["options"]["settingSources"]
        );
        assert_eq!(
            options["settings"]["permissions"],
            meta["claudeCode"]["options"]["settings"]["permissions"]
        );
        assert_eq!(options["settings"]["env"]["PROJECT_RULE"], "retained");
        let serialized = pinned.to_string();
        for removed in [
            "untrusted.invalid",
            "project-token",
            "project-oauth",
            "helper-ran",
        ] {
            assert!(!serialized.contains(removed));
        }
    }

    #[test]
    fn rejects_unverified_adapter_and_missing_or_foreign_route_without_echoing_input() {
        for init in [
            json!({}),
            json!({"agentInfo":{"name":ADAPTER,"version":"0.100.0"}}),
            json!({"agentInfo":{"name":"different-adapter","version":VERSION}}),
        ] {
            let error = Route::verified(&init, &[]).err().unwrap().to_string();
            assert!(error.starts_with("unsupported_environment_claude_adapter"));
        }
        let init = json!({"agentInfo":{"name":ADAPTER,"version":VERSION}});
        for env in [
            vec![],
            vec![
                ("ANTHROPIC_BASE_URL", "private-endpoint".into()),
                ("ANTHROPIC_AUTH_TOKEN", "private-credential".into()),
            ],
            vec![("ANTHROPIC_BASE_URL", BROKER.into())],
        ] {
            let error = Route::verified(&init, &env).err().unwrap().to_string();
            assert!(!error.contains("private-"));
        }
    }

    #[test]
    fn rejects_settings_paths_or_malformed_metadata_instead_of_env_only_fallback() {
        for meta in [
            json!(null),
            json!({"claudeCode":{"options":{"settings":"private-file"}}}),
            json!({"claudeCode":{"options":{"env":[]}}}),
        ] {
            let error = route().pin(meta).err().unwrap().to_string();
            assert!(error.starts_with("unsupported_environment_claude_metadata"));
            assert!(!error.contains("private-file"));
        }
    }

    #[test]
    fn maps_only_explicit_full_access_and_respects_disabled_bypass() {
        let full = ProductSessionPolicy {
            approval_mode: Some("yolo".into()),
            sandbox_mode: Some("danger-full-access".into()),
            collaboration_mode: None,
        };
        let modes = [SessionMode {
            id: "bypassPermissions".into(),
            name: None,
        }];
        assert_eq!(
            product_mode(&modes, &full).unwrap().as_deref(),
            Some("bypassPermissions")
        );
        assert!(product_mode(&[], &full).is_err());
        for sandbox in ["workspace-write", "read-only"] {
            let guarded = ProductSessionPolicy {
                sandbox_mode: Some(sandbox.into()),
                ..full.clone()
            };
            assert_eq!(product_mode(&modes, &guarded).unwrap(), None);
        }
    }
}
