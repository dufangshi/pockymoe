//! A new thread that works in its own managed worktree, for the Web and native
//! new-thread forms. Same layout and branch rules as the CLI's
//! `thread create --worktree`: a sibling `REPO.worktrees/NAME` checkout on
//! `agent/NAME` (or a named branch), plus a base revision for a new branch, a
//! retry-safe request id, and the repository's `.pockymoe/worktree.json` setup
//! whose outcome is returned.

use super::*;
use crate::interaction::AgentOptions;
use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewThreadWorktree {
    /// Directory and default branch name; defaults to the title as a slug, else
    /// the thread id's first characters.
    #[serde(default)]
    pub name: Option<String>,
    /// An existing branch is checked out as is; a new one is created.
    #[serde(default)]
    pub branch: Option<String>,
    /// Where a new branch starts; defaults to `HEAD`.
    #[serde(default)]
    pub base: Option<String>,
    /// Retrying with the same id returns the first outcome instead of a second worktree.
    #[serde(default)]
    pub client_request_id: Option<String>,
}

fn trimmed(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// A worktree name from a thread title: lowercase ASCII words joined by `-`,
/// when the result is a valid name.
pub(crate) fn name_from_title(title: &str) -> Option<String> {
    let mut slug = String::new();
    for c in title.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.chars().take(32).collect::<String>();
    let slug = slug.trim_end_matches('-').to_owned();
    crate::interaction::validate_name(&slug).ok().map(|_| slug)
}

impl Supervisor {
    /// Creates a user thread in a new worktree of its workspace and runs the
    /// repository's worktree setup there. A setup failure does not undo the
    /// thread: the response says what failed, like `worktree create`.
    pub async fn start_thread_in_worktree(
        &self,
        input: CreateThreadInput,
        worktree: NewThreadWorktree,
    ) -> Result<Value> {
        ensure!(
            input.parent_thread_id.is_none(),
            "a new-thread form thread has no parent; delegates use thread create --worktree"
        );
        let name = match trimmed(worktree.name.clone()) {
            Some(name) => {
                crate::interaction::validate_name(&name)?;
                Some(name)
            }
            None => input.title.as_deref().and_then(name_from_title),
        };
        let branch = trimmed(worktree.branch.clone());
        let base = trimmed(worktree.base.clone());
        for (label, value) in [("branch", &branch), ("base", &base)] {
            ensure!(
                value
                    .as_ref()
                    .is_none_or(|v| v.len() <= 200 && !v.chars().any(char::is_control)),
                "invalid {label}"
            );
        }
        let request = trimmed(worktree.client_request_id.clone());
        ensure!(
            request.as_ref().is_none_or(|r| r.len() <= 128),
            "invalid clientRequestId"
        );
        let key = request.map(|r| format!("thread.worktree:{r}"));
        let signature = json!({
            "workspaceId": input.workspace_id, "title": input.title, "provider": input.provider,
            "agentId": input.agent_id, "model": input.model, "name": name, "branch": branch, "base": base,
        });
        if let Some(key) = &key {
            let saved = self.db.get_kv(key)?;
            if let Some(saved) = saved {
                let saved: Value = serde_json::from_str(&saved)?;
                ensure!(
                    saved["input"] == signature,
                    "conflict: request ID reused with different input"
                );
                let mut result = saved.get("result").cloned().context(
                    "conflict: creation outcome uncertain; check the thread list before retrying",
                )?;
                if let Some(id) = result["thread"]["id"].as_str() {
                    result["thread"] = serde_json::to_value(self.get_thread(id)?)?;
                }
                result["created"] = json!(false);
                return Ok(result);
            }
            self.db
                .set_kv(key, &json!({ "input": signature }).to_string())?;
        }
        let workspace = self.get_workspace(&input.workspace_id)?;
        let options = AgentOptions {
            worktree: Some(branch),
            worktree_name: name,
            worktree_base: base,
            ..Default::default()
        };
        let thread = match self.create_thread_with(input, options).await {
            Ok(thread) => thread,
            Err(error) => {
                // Nothing was created: the same request may be tried again.
                if let Some(key) = &key {
                    let _ = self.db.with(|c| {
                        c.execute("DELETE FROM kv WHERE key=?1", [key])?;
                        Ok(())
                    });
                }
                return Err(error);
            }
        };
        let path = thread
            .worktree_path
            .clone()
            .context("the thread was created without its worktree")?;
        let branch = std::process::Command::new("git")
            .args(["-C", &path, "rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned());
        let source = workspace.abs_path.clone();
        let target = path.clone();
        let setup = tokio::task::spawn_blocking(move || {
            crate::source_control::setup::run_after_create(
                Path::new(&source),
                Path::new(&target),
                false,
            )
        })
        .await
        .unwrap_or_else(|_| json!({"completed": false, "error": "Setup worker failed"}));
        let result = json!({
            "thread": thread,
            "worktree": { "path": path, "branch": branch },
            "setup": setup,
            "created": true,
        });
        if let Some(key) = &key {
            self.db.set_kv(
                key,
                &json!({ "input": signature, "result": result }).to_string(),
            )?;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::name_from_title;

    #[test]
    fn titles_become_worktree_names_when_they_can() {
        assert_eq!(
            name_from_title("Fix the login bug").as_deref(),
            Some("fix-the-login-bug")
        );
        assert_eq!(
            name_from_title("  API: v2 → v3  ").as_deref(),
            Some("api-v2-v3")
        );
        assert_eq!(name_from_title("修复登录"), None);
        assert_eq!(name_from_title("2026 plan"), None);
        assert_eq!(
            name_from_title("a very long title that keeps going past the limit").map(|n| n.len()),
            Some(32)
        );
    }
}
