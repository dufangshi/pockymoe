use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Args)]
#[command(after_long_help = r#"Rules:
  - Create, register and remove worktrees through this interface so the Supervisor
    can keep the thread's working-copy records accurate.
  - Creating a worktree associates it with the thread; it does not change the cwd
    of your shell, running harness or subsequent tool calls. Use the returned path.
  - `use` changes the thread's default checkout only while the thread is idle.
  - `current` reports this CLI process's actual directory. It can differ from the
    thread's configured checkout or another concurrent tool's directory.

More: pockymoe guide worktrees"#)]
pub struct WorktreeArgs {
    /// Local Pockymoe thread; defaults to the authenticated thread.
    #[arg(long, global = true, default_value = "self")]
    pub thread: String,
    #[command(subcommand)]
    pub command: WorktreeCommand,
}

#[derive(Subcommand)]
pub enum WorktreeCommand {
    /// Create and associate a worktree; continue using the returned absolute path.
    Create {
        #[arg(long)]
        name: String,
        /// Starting Git revision; defaults to the source checkout's committed HEAD.
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        /// Reuse the same ID to retry this creation request safely.
        #[arg(long)]
        request_id: Option<String>,
    },
    /// List worktrees associated with this thread, including observed checkouts.
    List,
    /// Inspect this CLI process's actual cwd and its relation to the thread.
    Current,
    /// Inspect an associated working copy by ID.
    Show { id: String },
    /// Associate an existing worktree; relative paths use this CLI process's cwd.
    Adopt {
        #[arg(long)]
        path: PathBuf,
    },
    /// Set an idle thread's default checkout; cannot change a running tool's cwd.
    Use { id: String },
    /// Remove a managed worktree after checking changes and active users.
    Remove { id: String },
}

impl WorktreeCommand {
    fn request(self, cwd: &Path) -> Result<Value> {
        Ok(match self {
            Self::Create {
                name,
                base,
                branch,
                request_id,
            } => json!({
                "operation": "worktreeCreate", "name": name, "base": base,
                "branch": branch, "clientRequestId": request_id, "cwd": cwd,
            }),
            Self::List => json!({"operation": "worktreeList"}),
            Self::Current => json!({"operation": "worktreeCurrent", "cwd": cwd}),
            Self::Show { id } => json!({"operation": "worktreeShow", "workingCopyId": id}),
            Self::Adopt { path } => {
                // Resolve in the caller before transport. Resolving a relative path
                // at the Supervisor would silently select a different checkout.
                let path = if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                };
                let path = std::fs::canonicalize(&path)
                    .with_context(|| format!("Cannot resolve worktree path {}", path.display()))?;
                json!({"operation": "worktreeAdopt", "path": path})
            }
            Self::Use { id } => json!({"operation": "worktreeUse", "workingCopyId": id}),
            Self::Remove { id } => json!({"operation": "worktreeRemove", "workingCopyId": id}),
        })
    }
}

impl crate::threads::Client {
    pub async fn worktree(&self, args: WorktreeArgs) -> Result<Value> {
        let cwd = std::env::current_dir().context("Cannot read this CLI process's cwd")?;
        let mut input = args.command.request(&cwd)?;
        input["threadId"] = json!(self.id(&args.thread).await?);
        input["fromThreadId"] = json!(self.from);
        self.request(input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Commands};
    use clap::Parser;

    fn command(args: &[&str]) -> WorktreeArgs {
        let cli = Cli::try_parse_from(
            ["pockymoe", "worktree"]
                .into_iter()
                .chain(args.iter().copied()),
        )
        .unwrap();
        let Commands::Worktree(args) = cli.command else {
            panic!("Expected worktree command")
        };
        args
    }

    #[test]
    fn worktree_creation_preserves_source_and_explicit_base_without_claiming_a_switch() {
        let args = command(&[
            "create",
            "--name",
            "fix",
            "--base",
            "feature/base",
            "--branch",
            "agent/fix",
            "--request-id",
            "create-fix",
            "--thread",
            "worker",
        ]);
        assert_eq!(args.thread, "worker");
        let cwd = std::env::temp_dir().join("a checkout");
        let input = args.command.request(&cwd).unwrap();
        assert_eq!(input["operation"], "worktreeCreate");
        assert_eq!(input["cwd"], json!(cwd));
        assert_eq!(input["base"], "feature/base");
        assert_eq!(input["branch"], "agent/fix");
        assert_eq!(input["clientRequestId"], "create-fix");
        assert!(input.get("sessionCwdUpdated").is_none());
        assert!(input.get("worktreePath").is_none());
    }

    #[test]
    fn worktree_current_uses_the_callers_cwd_and_use_names_the_working_copy() {
        let args = command(&["current"]);
        assert_eq!(args.thread, "self");
        let cwd = std::env::temp_dir().join("outside-workspace");
        assert_eq!(args.command.request(&cwd).unwrap()["cwd"], json!(cwd));
        let args = command(&["--thread", "worker", "use", "copy-123"]);
        assert_eq!(args.thread, "worker");
        let input = args.command.request(&cwd).unwrap();
        assert_eq!(input["operation"], "worktreeUse");
        assert_eq!(input["workingCopyId"], "copy-123");
        assert!(input.get("cwd").is_none());
    }

    #[test]
    fn worktree_adopt_resolves_relative_paths_before_transport() {
        let root = std::env::temp_dir().join(format!("pockymoe-cli-path-{}", uuid::Uuid::new_v4()));
        let checkout = root.join("outside checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        let args = command(&["adopt", "--path", "outside checkout"]);
        let input = args.command.request(&root).unwrap();
        assert_eq!(input["operation"], "worktreeAdopt");
        assert_eq!(
            input["path"],
            json!(std::fs::canonicalize(checkout).unwrap())
        );
        assert!(command(&["adopt", "--path", "missing"])
            .command
            .request(&root)
            .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
