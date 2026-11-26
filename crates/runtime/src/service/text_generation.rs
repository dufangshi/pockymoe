//! One-shot text generation, independent of a thread's interactive session.
use super::*;
use anyhow::Context;
use std::time::{Duration, Instant};
impl Supervisor {
    pub fn text_generation_settings(&self) -> Result<Value> {
        self.db.with(|c| {
            let value: Option<String> = c
                .query_row(
                    "SELECT value FROM kv WHERE key='git:textGeneration'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(value
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_else(|| json!({"source":"auto","instructions":""})))
        })
    }
    pub fn set_text_generation_settings(&self, input: &Value) -> Result<Value> {
        ensure!(
            matches!(
                input["source"].as_str().unwrap_or("auto"),
                "auto" | "upstream" | "codex" | "claude"
            ),
            "invalid generation source"
        );
        self.db.with(|c|{c.execute("INSERT INTO kv(key,value) VALUES('git:textGeneration',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[input.to_string()])?;Ok(())})?;
        self.text_generation_settings()
    }
    pub async fn generate_text(&self, system: &str, user: &str, input: &Value) -> Result<String> {
        let mut settings = self.text_generation_settings()?;
        if let Some(fields) = input.as_object() {
            settings.as_object_mut().unwrap().extend(fields.clone());
        }
        let source = settings["source"].as_str().unwrap_or("auto");
        let instructions = settings["instructions"].as_str().unwrap_or("");
        let system = format!("{system}\n{instructions}");
        ensure!(
            system.len() + user.len() <= 256 * 1024,
            "generation context exceeds limit"
        );
        let dir = crate::upstreams::directory(&self.config.database_url);
        let selected = settings["connectionId"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                if matches!(source, "auto" | "upstream") {
                    crate::upstreams::connections::list(&dir)
                        .ok()
                        .and_then(|v| {
                            v["connections"].as_array()?.first()?["id"]
                                .as_str()
                                .map(str::to_owned)
                        })
                } else {
                    None
                }
            });
        if let Some(id) = selected {
            let connection = crate::upstreams::connections::get(&dir, &id)?;
            let model = settings["model"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(&connection.model);
            ensure!(!model.is_empty(), "select a generation model");
            ensure!(
                connection.api_key.is_empty()
                    || (!system.contains(&connection.api_key)
                        && !user.contains(&connection.api_key)),
                "generation context contains a credential"
            );
            return tokio::time::timeout(
                Duration::from_secs(60),
                crate::upstreams::connections::complete(&connection, model, &system, user, 2048),
            )
            .await
            .context("text generation timed out")?;
        }
        ensure!(source != "upstream", "configure an upstream connection");
        let source = if source == "auto" {
            if which::which("codex").is_ok() {
                "codex"
            } else {
                "claude"
            }
        } else {
            source
        }
        .to_owned();
        let prompt = format!("{system}\n\n{user}");
        crate::execution::spawn_blocking(move || {
            let dir = tempfile::tempdir()?;
            let mut command = std::process::Command::new(&source);
            command
                .current_dir(dir.path())
                .stdin(std::process::Stdio::null());
            if source == "codex" {
                command.args([
                    "exec",
                    "--ephemeral",
                    "--skip-git-repo-check",
                    "--sandbox",
                    "read-only",
                    &prompt,
                ]);
            } else {
                command.args([
                    "-p",
                    "--no-session-persistence",
                    "--tools",
                    "",
                    "--output-format",
                    "text",
                    &prompt,
                ]);
            }
            let (status, bytes, truncated) = crate::child_process::run_bounded(
                &mut command,
                64 * 1024,
                Instant::now() + Duration::from_secs(60),
            )?;
            ensure!(
                status.success() && !truncated,
                "one-shot text generation failed"
            );
            Ok(String::from_utf8(bytes)?.trim().into())
        })
        .await?
    }
    pub fn commit_message_context(&self, thread: &str, input: &Value) -> Result<String> {
        self.git_commit_context(thread, input)
    }
    pub async fn generate_commit_message(&self, thread: &str, input: &Value) -> Result<Value> {
        let context = self.commit_message_context(thread, input)?;
        let message=self.generate_text("Write a concise Git commit message for these changes. Follow the recent subjects as conventions. Return only the message, without Markdown fences.",&context,input).await?;
        Ok(
            json!({"message":message,"status":self.worktree_git_status(thread,input["workingCopyId"].as_str())?}),
        )
    }
}
