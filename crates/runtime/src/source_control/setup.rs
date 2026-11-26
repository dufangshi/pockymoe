//! Opt-in repository setup. Never run repository commands implicitly.
use super::*;
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorktreeSetup {
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub copy_files: Vec<String>,
    #[serde(default)]
    pub submodules: bool,
    #[serde(default)]
    pub sparse_patterns: Vec<String>,
}
impl WorktreeSetup {
    pub fn load(source: &Path) -> Result<Option<Self>> {
        let path = source.join(".pockymoe/worktree.json");
        if !path.exists() {
            return Ok(None);
        }
        ensure!(
            path.canonicalize()?.starts_with(source.canonicalize()?),
            "Setup file must stay inside the source checkout"
        );
        let bytes = std::fs::read(path)?;
        ensure!(bytes.len() <= 64 * 1024, "Worktree setup file too large");
        Ok(Some(serde_json::from_slice(&bytes)?))
    }
    pub async fn run(&self, source: &Path, target: &Path) -> Result<Value> {
        let source = source.canonicalize()?;
        let target = target.canonicalize()?;
        ensure!(source != target, "Setup requires a new worktree");
        for file in &self.copy_files {
            let path = Path::new(file);
            ensure!(
                !file.is_empty()
                    && path
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_))),
                "Copy files must be relative paths without traversal"
            );
            let from = source.join(path).canonicalize()?;
            ensure!(
                from.starts_with(&source) && from.is_file(),
                "Copy source must be a file inside the source checkout"
            );
            let to = target.join(path);
            let parent = to.parent().context("Copy destination required")?;
            let relative = parent.strip_prefix(&target)?;
            let mut checked = target.clone();
            for component in relative.components() {
                checked.push(component);
                ensure!(
                    !checked.is_symlink(),
                    "Copy destination parent cannot be a symlink"
                );
                if !checked.exists() {
                    std::fs::create_dir(&checked)?;
                }
                ensure!(
                    checked.is_dir(),
                    "Copy destination parent must be a directory"
                );
            }
            ensure!(
                parent.canonicalize()?.starts_with(&target) && !to.exists() && !to.is_symlink(),
                "Copy destination exists or escapes the worktree"
            );
            std::fs::copy(from, to)?;
        }
        let mut output = Vec::new();
        if !self.sparse_patterns.is_empty() {
            hosts::command(
                "git",
                &[
                    "sparse-checkout".into(),
                    "set".into(),
                    "--no-cone".into(),
                    "--stdin".into(),
                ],
                &target,
                Some(self.sparse_patterns.join("\n").into_bytes()),
            )
            .await?;
        }
        if self.submodules {
            output.push(
                hosts::command(
                    "git",
                    &[
                        "submodule".into(),
                        "update".into(),
                        "--init".into(),
                        "--recursive".into(),
                    ],
                    &target,
                    None,
                )
                .await?,
            );
        }
        for command in &self.commands {
            #[cfg(unix)]
            let (shell, flag) = ("sh", "-c");
            #[cfg(windows)]
            let (shell, flag) = ("cmd", "/C");
            let (status, stdout, stderr) =
                hosts::command_capture(shell, &[flag.into(), command.clone()], &target, None)
                    .await?;
            output.push(format!("{stdout}{stderr}"));
            if !status.success() {
                return Ok(
                    json!({"completed":false,"output":output,"error":format!("Setup command {} failed ({status})", output.len())}),
                );
            }
        }
        Ok(json!({"completed":true,"output":output}))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn copy_rejects_traversal_and_preserves_existing_files() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join(".env"), "fixture").unwrap();
        let mut setup = WorktreeSetup {
            copy_files: vec!["../escape".into()],
            ..Default::default()
        };
        assert!(setup.run(source.path(), target.path()).await.is_err());
        setup.copy_files = vec![".env".into()];
        setup.run(source.path(), target.path()).await.unwrap();
        assert!(setup.run(source.path(), target.path()).await.is_err());
        assert_eq!(
            std::fs::read_to_string(target.path().join(".env")).unwrap(),
            "fixture"
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn setup_failure_keeps_stdout_and_stderr_and_stops_commands() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let setup = WorktreeSetup {
            commands: vec![
                "printf 'before failure'; printf 'diagnostic' >&2; exit 3".into(),
                "touch should-not-run".into(),
            ],
            ..Default::default()
        };
        let result = setup.run(source.path(), target.path()).await.unwrap();
        assert_eq!(result["completed"], false);
        assert!(result["output"][0]
            .as_str()
            .unwrap()
            .contains("before failure"));
        assert!(result["output"][0].as_str().unwrap().contains("diagnostic"));
        assert!(!target.path().join("should-not-run").exists());
    }
}

/// Creation remains inspectable when setup fails: the copy is already registered
/// and the response includes its path and the failure rather than hiding it.
pub fn run_after_create(source: &Path, target: &Path, isolated: bool) -> Value {
    let source = source.to_owned();
    let target = target.to_owned();
    let result = std::thread::spawn(move || -> Result<Option<Value>> {
        let Some(config) = WorktreeSetup::load(&source)? else {
            return Ok(None);
        };
        ensure!(
            !isolated,
            "Repository setup is unavailable inside an isolated environment"
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(Some(runtime.block_on(config.run(&source, &target))?))
    })
    .join();
    match result {
        Ok(Ok(value)) => json!(value),
        Ok(Err(error)) => json!({"completed":false,"error":error.to_string()}),
        Err(_) => json!({"completed":false,"error":"Setup worker failed"}),
    }
}
