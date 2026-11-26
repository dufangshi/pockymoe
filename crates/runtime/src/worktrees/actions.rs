use super::*;

pub(super) fn relative(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty()
            && !Path::new(path).is_absolute()
            && !path.contains('\0')
            && !Path::new(path).components().any(|c| matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )),
        "path must be relative to the working copy"
    );
    Ok(())
}
impl Supervisor {
    pub(super) fn extend_git_status(&self, copy: &WorkingCopy, status: &mut Value) -> Result<()> {
        let root = Path::new(&copy.path);
        let upstream = git_text(
            root,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
        )
        .ok();
        let counts = upstream
            .as_ref()
            .and_then(|_| {
                git_text(
                    root,
                    &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
                )
                .ok()
            })
            .unwrap_or_default();
        let mut counts = counts
            .split_whitespace()
            .filter_map(|v| v.parse::<u64>().ok());
        status["upstream"] = json!(upstream);
        status["ahead"] = json!(counts.next().unwrap_or(0));
        status["behind"] = json!(counts.next().unwrap_or(0));
        status["stashCount"] = json!(git_text(root, &["stash", "list", "--format=%gd"])
            .unwrap_or_default()
            .lines()
            .count());
        status["conflicted"] = json!(status["files"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| {
                let pair = format!(
                    "{}{}",
                    f["indexStatus"].as_str().unwrap_or(""),
                    f["worktreeStatus"].as_str().unwrap_or("")
                );
                pair.contains('U') || matches!(pair.as_str(), "AA" | "DD")
            })
            .map(|f| f["path"].clone())
            .collect::<Vec<_>>());
        // rev-parse works in linked worktrees and environment workers too.
        status["operation"] = json!(
            if git(root, &["rev-parse", "--verify", "MERGE_HEAD"]).is_ok() {
                Some("merge")
            } else if git(root, &["rev-parse", "--verify", "REBASE_HEAD"]).is_ok() {
                Some("rebase")
            } else if git(root, &["rev-parse", "--verify", "CHERRY_PICK_HEAD"]).is_ok() {
                Some("cherryPick")
            } else {
                None
            }
        );
        Ok(())
    }
    pub fn worktree_git_stashes(&self, thread: &str, id: Option<&str>) -> Result<Value> {
        let copy = self.selected_copy(thread, id)?;
        let out = git(
            Path::new(&copy.path),
            &["stash", "list", "-z", "--format=%H%x00%gd%x00%gs%x00%cI"],
        )?;
        let fields: Vec<_> = out.bytes.split(|b| *b == 0).collect();
        let stashes:Vec<_>=fields.chunks(4).filter(|r|r.len()==4&&!r[0].is_empty()).map(|r|json!({"oid":String::from_utf8_lossy(r[0]),"name":String::from_utf8_lossy(r[1]),"subject":String::from_utf8_lossy(r[2]),"createdAt":String::from_utf8_lossy(r[3])})).collect();
        Ok(json!({"workingCopyId":copy.id,"stashes":stashes,"truncated":out.truncated}))
    }
    pub fn worktree_git_branches(&self, thread: &str, id: Option<&str>) -> Result<Value> {
        let copy = self.selected_copy(thread, id)?;
        let out = git_text(Path::new(&copy.path), &["for-each-ref", "--format=%(refname:short)%09%(upstream:short)%09%(committerdate:iso-strict)%09%(refname)", "refs/heads", "refs/remotes"])?;
        let mut branches = Vec::new();
        for line in out.lines() {
            let f: Vec<_> = line.split('\t').collect();
            if f.len() != 4 {
                continue;
            }
            let counts = if f[1].is_empty() {
                String::new()
            } else {
                git_text(
                    Path::new(&copy.path),
                    &[
                        "rev-list",
                        "--left-right",
                        "--count",
                        &format!("{}...{}", f[0], f[1]),
                    ],
                )
                .unwrap_or_default()
            };
            let mut counts = counts
                .split_whitespace()
                .filter_map(|s| s.parse::<u64>().ok());
            branches.push(json!({"name":f[0],"upstream":if f[1].is_empty(){None}else{Some(f[1])},"ahead":counts.next().unwrap_or(0),"behind":counts.next().unwrap_or(0),"lastCommitAt":f[2],"remote":f[3].starts_with("refs/remotes/")}));
        }
        Ok(json!({"current":copy.branch,"detached":copy.branch.is_none(),"branches":branches}))
    }
    pub fn worktree_git_write(&self, thread: &str, action: &str, input: &Value) -> Result<Value> {
        let t = self.get_thread(thread)?;
        ensure!(
            !matches!(t.status.as_str(), "running" | "recovering")
                || input["confirmRunning"] == true,
            "conflict: thread is running; confirmRunning is required"
        );
        let copy = self.selected_copy(thread, input["workingCopyId"].as_str())?;
        let _guard = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
        let root = Path::new(&copy.path);
        let flag = |key: &str| input[key] == true;
        let text = |key: &str| -> Result<&str> {
            input[key]
                .as_str()
                .filter(|s| !s.is_empty() && !s.contains('\0'))
                .with_context(|| format!("{key} required"))
        };
        let mut args: Vec<String> = Vec::new();
        match action {
            "stage" | "unstage" | "discard" => {
                if let Some(patch) = input["patch"].as_str() {
                    ensure!(patch.len() <= OUTPUT_LIMIT, "patch exceeds limit");
                    apply_hunk(root, action, patch)?;
                    self.git_changed(thread, "gitChanged");
                    return Ok(json!({"status":self.worktree_git_status(thread,Some(&copy.id))?}));
                } else {
                    let paths = input["paths"]
                        .as_array()
                        .context("paths or patch required")?;
                    ensure!(
                        !paths.is_empty() && paths.len() <= 1000,
                        "select 1 to 1000 paths"
                    );
                    if action == "discard" {
                        for path in paths {
                            let path = path.as_str().context("invalid path")?;
                            relative(path)?;
                        }
                        let untracked =
                            git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
                        ensure!(!untracked.truncated, "untracked paths exceed limit");
                        let mut tracked = Vec::new();
                        for path in paths {
                            let path = path.as_str().unwrap();
                            if untracked
                                .bytes
                                .split(|b| *b == 0)
                                .any(|p| p == path.as_bytes())
                            {
                                git(root, &["clean", "-f", "--", path])?;
                            } else {
                                tracked.push(path);
                            }
                        }
                        if !tracked.is_empty() {
                            let mut restore = vec!["restore", "--worktree", "--"];
                            restore.extend(tracked);
                            git(root, &restore)?;
                        }
                        self.git_changed(thread, "gitChanged");
                        return Ok(
                            json!({"status":self.worktree_git_status(thread,Some(&copy.id))?}),
                        );
                    }
                    args.extend(
                        match action {
                            "stage" => vec!["add", "-A"],
                            "unstage" => vec!["restore", "--staged"],
                            _ => vec!["restore", "--worktree"],
                        }
                        .into_iter()
                        .map(str::to_owned),
                    );
                    args.push("--".into());
                    for path in paths {
                        let path = path.as_str().context("invalid path")?;
                        relative(path)?;
                        args.push(path.into());
                    }
                }
            }
            "commit" => {
                args.extend(["-c", "commit.gpgsign=false", "commit", "-m"].map(str::to_owned));
                args.push(text("message")?.into());
                if flag("amend") {
                    args.push("--amend".into());
                }
                if flag("all") {
                    git(root, &["add", "-A", "--", "."])?;
                }
            }
            "branchCreate" | "checkout" => {
                let name = text("name").or_else(|_| text("branch"))?;
                ensure!(!name.starts_with('-'), "invalid branch");
                git(root, &["check-ref-format", "--branch", name])?;
                args.push(
                    if action == "branchCreate" {
                        "branch"
                    } else {
                        "switch"
                    }
                    .into(),
                );
                args.push(name.into());
            }
            "fetch" => args.extend(["fetch", "--prune"].map(str::to_owned)),
            "pull" => {
                args.push("pull".into());
                args.push(
                    match input["mode"].as_str().unwrap_or("ffOnly") {
                        "ffOnly" => "--ff-only",
                        "rebase" => "--rebase",
                        "merge" => "--no-rebase",
                        _ => bail!("invalid pull mode"),
                    }
                    .into(),
                );
            }
            "push" => {
                args.push("push".into());
                if flag("forceWithLease") {
                    args.push("--force-with-lease".into());
                }
                if flag("setUpstream") {
                    args.push("--set-upstream".into());
                    args.push("origin".into());
                    args.push(copy.branch.as_deref().context("detached HEAD")?.into());
                }
            }
            "stashPush" => {
                args.extend(["stash", "push", "--include-untracked"].map(str::to_owned));
                if let Some(m) = input["message"].as_str() {
                    args.extend(["-m".into(), m.into()]);
                }
            }
            "stashPop" => {
                args.extend(["stash", "pop"].map(str::to_owned));
                if let Some(n) = input["index"].as_u64() {
                    args.push(format!("stash@{{{n}}}"));
                }
            }
            _ => bail!("unknown Git write action"),
        }
        git_within(
            root,
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            Duration::from_secs(if matches!(action, "fetch" | "pull" | "push") {
                120
            } else {
                15
            }),
        )?;
        self.git_changed(thread, "gitChanged");
        Ok(json!({"status":self.worktree_git_status(thread,Some(&copy.id))?}))
    }
    pub(crate) fn git_changed(&self, thread: &str, reason: &str) {
        self.bus.emit(ThreadEventEnvelope {
            event_type: ThreadEventType::ThreadUpdated,
            thread_id: thread.into(),
            timestamp: now_rfc3339(),
            payload: json!({"reason":reason}),
        });
    }
}
impl Supervisor {
    pub(crate) fn git_commit_context(&self, thread: &str, input: &Value) -> Result<String> {
        let copy = self.selected_copy(thread, input["workingCopyId"].as_str())?;
        let root = Path::new(&copy.path);
        let staged = git_text(root, &["diff", "--cached", "--name-status"])?;
        let mut args = vec![
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
        ];
        let temp;
        let oid;
        if staged.is_empty() && input["all"] == true {
            temp = uuid::Uuid::new_v4().to_string();
            let reference = format!("refs/pockymoe/commit-context/{temp}");
            let result = super::checkpoints::snapshot(root, &reference)?;
            oid = result["commit"]
                .as_str()
                .context("snapshot missing")?
                .to_owned();
            git(root, &["update-ref", "-d", &reference])?;
            args = vec![
                "show",
                "--format=",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                &oid,
            ];
        } else {
            ensure!(!staged.is_empty(), "no staged changes");
        }
        let patch = git(root, &args)?;
        let subjects = git_text(root, &["log", "-10", "--format=%s"]).unwrap_or_default();
        let patch = String::from_utf8_lossy(&patch.bytes);
        let end = patch
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|i| *i <= 64 * 1024)
            .last()
            .unwrap_or(0);
        Ok(format!(
            "Recent subjects:\n{subjects}\n\nStaged paths:\n{staged}\n\nPatch:\n{}",
            &patch[..end]
        ))
    }
}
pub(crate) fn apply_hunk(root: &Path, action: &str, patch: &str) -> Result<()> {
    if crate::execution::file_request("gitHunk", root, "", json!({"action":action,"patch":patch}))?
        .is_some()
    {
        return Ok(());
    }
    ensure!(
        matches!(action, "stage" | "unstage" | "discard"),
        "invalid hunk action"
    );
    ensure!(patch.len() <= OUTPUT_LIMIT, "patch exceeds limit");
    let mut source = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-color"];
    if action == "unstage" {
        source.push("--cached");
    }
    let diff = git(root, &source)?;
    ensure!(!diff.truncated, "current diff exceeds limit");
    let diff = String::from_utf8(diff.bytes)?;
    // A hunk must be a verbatim selection from the current diff. In particular,
    // a fabricated preimage cannot turn discard into an arbitrary file write.
    let mut saw = false;
    let mut hunk = String::new();
    for line in patch.split_inclusive('\n') {
        if line.starts_with("@@") || line.starts_with("diff --git ") {
            if !hunk.is_empty() {
                ensure!(diff.contains(&hunk), "conflict: stale or invalid hunk");
                hunk.clear();
            }
            if line.starts_with("@@") {
                saw = true;
                hunk.push_str(line);
            }
        } else if !hunk.is_empty() {
            hunk.push_str(line);
        } else if line.starts_with("--- ") || line.starts_with("+++ ") {
            ensure!(
                diff.lines().any(|l| l == line.trim_end_matches('\n')),
                "conflict: invalid hunk path"
            );
        }
    }
    ensure!(
        saw && !hunk.is_empty() && diff.contains(&hunk),
        "conflict: stale or invalid hunk"
    );
    let mut file = tempfile::NamedTempFile::new()?;
    use std::io::Write;
    file.write_all(patch.as_bytes())?;
    let name = file.path().to_str().context("non UTF-8 patch path")?;
    let mut args = vec!["apply"];
    if action != "discard" {
        args.push("--cached");
    }
    if action != "stage" {
        args.push("--reverse");
    }
    args.extend(["--check", name]);
    git(root, &args)?;
    args.retain(|a| *a != "--check");
    git(root, &args)?;
    Ok(())
}
