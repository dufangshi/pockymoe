use anyhow::{ensure, Context, Result};
use clap::{Args, Subcommand};
use serde_json::{json, Value};
use std::{io::Read, path::PathBuf};

#[derive(Args)]
pub struct DefinitionArgs {
    #[arg(long, default_value = "self")]
    pub thread: String,
    /// Full typed definition as JSON; threadEnded needs only sourceThreadId (no turn ID).
    #[arg(long, conflicts_with = "file")]
    pub json: Option<String>,
    /// JSON file, or - to read stdin. Conditions support all/any/not.
    #[arg(long, required_unless_present = "json")]
    pub file: Option<PathBuf>,
    #[arg(long)]
    pub request_id: Option<String>,
}
impl DefinitionArgs {
    fn value(&self) -> Result<Value> {
        let text = if let Some(s) = &self.json {
            s.clone()
        } else {
            let p = self.file.as_ref().context("--file or --json required")?;
            if p.as_os_str() == "-" {
                let mut s = String::new();
                std::io::stdin().take(262145).read_to_string(&mut s)?;
                s
            } else {
                std::fs::read_to_string(p)?
            }
        };
        ensure!(text.len() <= 262144, "definition exceeds 256 KiB");
        Ok(serde_json::from_str(&text)?)
    }
}
#[derive(Args)]
pub struct Target {
    #[arg(long, default_value = "self")]
    pub thread: String,
    pub id: String,
}
#[derive(Subcommand)]
pub enum AutomationCommand {
    /// Register a durable hook. Only an explicit prompt action wakes a thread.
    Create(DefinitionArgs),
    /// Validate a definition and show upcoming UTC times without registering it.
    Preview(DefinitionArgs),
    List {
        #[arg(long, default_value = "self")]
        thread: String,
    },
    Show(Target),
    /// Pause future triggers and remove only this hook's unexecuted prompts.
    Pause(Target),
    /// Resume from the next future tick; missed paused ticks are discarded.
    Resume(Target),
    /// Permanently cancel future triggers and unexecuted actions; running work finishes.
    Cancel(Target),
    Runs {
        #[arg(long, default_value = "self")]
        thread: String,
        id: String,
        #[arg(long, default_value_t = 20)]
        limit: u64,
    },
}
#[derive(Subcommand)]
pub enum CommandCommand {
    /// Spawn and wait for a controlled command; ordinary PTY commands are not observed.
    Run {
        #[arg(long, default_value = "self")]
        thread: String,
        #[arg(long)]
        command_key: Option<String>,
        #[arg(long)]
        request_id: Option<String>,
        #[arg(long, conflicts_with = "worktree")]
        cwd: Option<String>,
        /// Associated working-copy ID; the Supervisor resolves its execution cwd.
        #[arg(long, conflicts_with = "cwd")]
        worktree: Option<String>,
        #[arg(long, default_value_t = 60)]
        timeout_seconds: u64,
        /// Explicit shell program text, mutually exclusive with trailing argv.
        #[arg(long, conflicts_with = "argv")]
        shell: Option<String>,
        #[arg(last = true, required_unless_present = "shell")]
        argv: Vec<String>,
    },
    Show {
        #[arg(long, default_value = "self")]
        thread: String,
        id: String,
    },
}
impl crate::threads::Client {
    pub async fn automation(&self, command: AutomationCommand) -> Result<Value> {
        let mut v = match command {
            AutomationCommand::Create(a) => {
                json!({"operation":"automationCreate","threadId":self.id(&a.thread).await?,"definition":a.value()?,"clientRequestId":a.request_id})
            }
            AutomationCommand::Preview(a) => {
                json!({"operation":"automationPreview","threadId":self.id(&a.thread).await?,"definition":a.value()?})
            }
            AutomationCommand::List { thread } => {
                json!({"operation":"automationList","threadId":self.id(&thread).await?})
            }
            AutomationCommand::Runs { thread, id, limit } => {
                json!({"operation":"automationRuns","threadId":self.id(&thread).await?,"automationId":id,"limit":limit})
            }
            AutomationCommand::Show(t) => self.automation_target("automationShow", t).await?,
            AutomationCommand::Pause(t) => self.automation_target("automationPause", t).await?,
            AutomationCommand::Resume(t) => self.automation_target("automationResume", t).await?,
            AutomationCommand::Cancel(t) => self.automation_target("automationCancel", t).await?,
        };
        v["fromThreadId"] = json!(self.from);
        self.request(v).await
    }
    async fn automation_target(&self, operation: &str, t: Target) -> Result<Value> {
        Ok(json!({"operation":operation,"threadId":self.id(&t.thread).await?,"automationId":t.id}))
    }
    pub async fn command_execution(&self, command: CommandCommand) -> Result<Value> {
        let mut v = match command {
            CommandCommand::Run {
                thread,
                command_key,
                request_id,
                cwd,
                worktree,
                timeout_seconds,
                shell,
                argv,
            } => {
                let input = command_input(
                    command_key,
                    request_id,
                    cwd,
                    worktree,
                    timeout_seconds,
                    shell,
                    argv,
                );
                json!({"operation":"commandRun","threadId":self.id(&thread).await?,"input":input})
            }
            CommandCommand::Show { thread, id } => {
                json!({"operation":"commandShow","threadId":self.id(&thread).await?,"commandId":id})
            }
        };
        v["fromThreadId"] = json!(self.from);
        self.request_for(v, 300).await
    }
}

fn command_input(
    command_key: Option<String>,
    request_id: Option<String>,
    cwd: Option<String>,
    worktree: Option<String>,
    timeout_seconds: u64,
    shell: Option<String>,
    argv: Vec<String>,
) -> Value {
    let mut input = json!({"commandKey":command_key,"clientRequestId":request_id,"timeoutSeconds":timeout_seconds,"shell":shell,"argv":argv});
    if let Some(id) = worktree {
        input["workingCopyId"] = json!(id);
    } else {
        input["cwd"] = json!(cwd.unwrap_or_else(|| ".".into()));
    }
    input
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn worktree_command_execution_preserves_argv_and_rejects_conflicting_cwd() {
        let cli = Cli::try_parse_from([
            "pockymoe",
            "command",
            "run",
            "--worktree",
            "copy-123",
            "--",
            "git",
            "diff",
            "--",
            "a file.txt",
        ])
        .unwrap();
        let Commands::Command {
            command:
                CommandCommand::Run {
                    command_key,
                    request_id,
                    cwd,
                    worktree,
                    timeout_seconds,
                    shell,
                    argv,
                    ..
                },
        } = cli.command
        else {
            panic!("Expected command run")
        };
        let input = command_input(
            command_key,
            request_id,
            cwd,
            worktree,
            timeout_seconds,
            shell,
            argv,
        );
        assert_eq!(input["workingCopyId"], "copy-123");
        assert_eq!(input["argv"], json!(["git", "diff", "--", "a file.txt"]));
        assert!(input.get("cwd").is_none());
        assert!(Cli::try_parse_from([
            "pockymoe",
            "command",
            "run",
            "--worktree",
            "copy-123",
            "--cwd",
            ".",
            "--",
            "pwd",
        ])
        .is_err());
    }

    #[test]
    fn command_execution_keeps_the_existing_default_cwd() {
        let input = command_input(None, None, None, None, 60, None, vec!["pwd".into()]);
        assert_eq!(input["cwd"], ".");
        assert!(input.get("workingCopyId").is_none());
    }
}
