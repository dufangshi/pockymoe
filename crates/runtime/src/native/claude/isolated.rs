//! Native Claude inside an isolated environment. It uses exactly the route the
//! ACP adapter uses (`acp/isolated_claude.rs`): the same projected credentials
//! (`execution::isolated_harness_environment`), the same broker or approved
//! native file login, and the same pinned values in both the process
//! environment and the programmatic `--settings` tier. Host credentials,
//! the host MCP registry and host harness homes never reach the container.
use crate::acp::runtime::isolated_claude::Route;
use crate::execution::ExecutionContext;
use anyhow::{ensure, Result};
use serde_json::Value;

/// The environment image's native CLI (Claude Agent SDK 0.3.318's own binary).
/// A version change needs a fresh settings-precedence and native acceptance
/// check, as the ACP adapter's pinned version does.
pub(super) const CLI_VERSION: &str = "2.1.318";

pub(super) struct Launch {
    pub context: ExecutionContext,
    /// Projected, nonsecret settings plus the short-lived gateway capability.
    pub env: Vec<(String, String)>,
    pub route: Route,
}

impl Launch {
    /// `None` on the host. Inside an environment, fail closed unless the route
    /// verifies: there is no host fallback.
    pub fn current() -> Result<Option<Self>> {
        let Some(context) = crate::execution::current() else {
            return Ok(None);
        };
        let projected = crate::execution::isolated_harness_environment("claude")?;
        let route = Route::from_environment(&projected)?;
        let mut env = context.launch_env();
        env.extend(projected.into_iter().map(|(k, v)| (k.to_string(), v)));
        // The process tier, like the SDK's options.env. The worker removes API
        // selectors again for a native file login before the CLI starts.
        for (key, value) in route.pinned_env() {
            if let Some(value) = value.as_str() {
                env.push((key, value.to_string()));
            }
        }
        Ok(Some(Self {
            context,
            env,
            route,
        }))
    }

    pub fn args(&self) -> [String; 2] {
        ["--settings".into(), self.route.flag_settings().to_string()]
    }
}

/// The pinned route is only as strong as the CLI that honours it.
pub(super) fn verify(init: &Value) -> Result<()> {
    ensure!(
        init["claude_code_version"].as_str() == Some(CLI_VERSION),
        "unsupported_environment_claude_cli: require the environment image's Claude Code {CLI_VERSION}"
    );
    Ok(())
}

/// An environment-scoped identity for a new session in the current context.
pub(super) fn session_id(context: Option<&ExecutionContext>, raw: &str) -> String {
    match context {
        Some(c) => format!("claude::env/{}/{}/{raw}", c.environment_id, c.generation),
        None => format!("claude::{raw}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_the_reviewed_cli_without_echoing_other_versions() {
        assert!(verify(&serde_json::json!({"claude_code_version": CLI_VERSION})).is_ok());
        for init in [
            serde_json::json!({}),
            serde_json::json!({"claude_code_version": "9.9.9-private"}),
        ] {
            let error = verify(&init).unwrap_err().to_string();
            assert!(error.starts_with("unsupported_environment_claude_cli"));
            assert!(!error.contains("private"));
        }
    }

    #[test]
    fn host_and_environment_identities_resolve_only_in_their_own_context() {
        use super::super::raw_in;
        assert_eq!(session_id(None, "abc"), "claude::abc");
        let scoped = "claude::env/owned/3/abc";
        assert_eq!(raw_in(scoped, Some(("owned", 3))).unwrap(), "abc");
        assert_eq!(raw_in(scoped, Some(("owned", 4))).unwrap(), "abc");
        assert_eq!(raw_in("claude::abc", None).unwrap(), "abc");
        assert_eq!(raw_in("abc", None).unwrap(), "abc");
        for (id, context) in [
            (scoped, None),
            (scoped, Some(("other", 3))),
            (scoped, Some(("owned", 2))),
            ("claude::abc", Some(("owned", 3))),
            ("claude::env/owned/x/abc", Some(("owned", 3))),
            ("claude::env/owned/3/", Some(("owned", 3))),
            ("claude::env/owned/3/a/b", Some(("owned", 3))),
            ("claude::gemini::abc", None),
            ("claude::", None),
        ] {
            assert!(raw_in(id, context).is_err(), "{id} {context:?}");
        }
    }
}
