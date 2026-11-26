//! Disposable-index snapshots. The real index and branch are never written.
use super::*;

pub(crate) fn migrate(c: &Connection) -> Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS turn_checkpoints(
        thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
        turn_id TEXT NOT NULL, copy_id TEXT NOT NULL, phase TEXT NOT NULL,
        commit_oid TEXT, head_oid TEXT, branch TEXT, status TEXT NOT NULL, error TEXT,
        created_at TEXT NOT NULL, PRIMARY KEY(thread_id,turn_id,copy_id,phase));",
    )?;
    Ok(())
}
pub(crate) fn migrate_review(c: &Connection) -> Result<()> {
    for column in ["files", "additions", "deletions"] {
        let exists: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('turn_checkpoints') WHERE name=?1)",
            [column],
            |r| r.get(0),
        )?;
        if !exists {
            c.execute(
                &format!("ALTER TABLE turn_checkpoints ADD COLUMN {column} INTEGER"),
                [],
            )?;
        }
    }
    c.execute_batch("CREATE TABLE IF NOT EXISTS rewound_turn_accounting AS SELECT id,thread_id,status,started_at,completed_at,model,token_usage_json,pricing_model_key,pricing_tier_key,origin,history_copy_of FROM thread_turns WHERE 0;
    CREATE UNIQUE INDEX IF NOT EXISTS rewound_accounting_id ON rewound_turn_accounting(id);
    CREATE INDEX IF NOT EXISTS rewound_accounting_date ON rewound_turn_accounting(julianday(started_at));
    CREATE VIEW IF NOT EXISTS all_turn_accounting AS SELECT id,thread_id,status,started_at,completed_at,model,token_usage_json,pricing_model_key,pricing_tier_key,origin,history_copy_of FROM thread_turns UNION ALL SELECT * FROM rewound_turn_accounting;
    CREATE TRIGGER IF NOT EXISTS rewound_accounting_thread_delete AFTER DELETE ON threads BEGIN DELETE FROM rewound_turn_accounting WHERE thread_id=OLD.id; END;")?;
    Ok(())
}
fn indexed(root: &Path, index: &Path, args: &[&str]) -> Result<String> {
    indexed_until(root, index, args, Instant::now() + Duration::from_secs(5))
}
fn indexed_until(root: &Path, index: &Path, args: &[&str], deadline: Instant) -> Result<String> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    ensure!(!remaining.is_zero(), "checkpoint time budget exceeded");
    let index = index.to_str().context("non UTF-8 index path")?;
    let out = git_options(
        root,
        args,
        remaining,
        &[
            ("GIT_INDEX_FILE", index),
            ("GIT_AUTHOR_NAME", "Pockymoe"),
            ("GIT_AUTHOR_EMAIL", "checkpoint@pockymoe.invalid"),
            ("GIT_COMMITTER_NAME", "Pockymoe"),
            ("GIT_COMMITTER_EMAIL", "checkpoint@pockymoe.invalid"),
        ],
    )?;
    ensure!(!out.truncated, "checkpoint output exceeds limit");
    Ok(String::from_utf8(out.bytes)?.trim_end().into())
}
pub(crate) fn snapshot(root: &Path, reference: &str) -> Result<Value> {
    if let Some(v) =
        crate::execution::file_request("gitCheckpoint", root, "", json!({"reference":reference}))?
    {
        return Ok(v);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let temp = tempfile::tempdir()?;
    let index = temp.path().join("index");
    let run = |args: &[&str]| indexed_until(root, &index, args, deadline);
    let head = run(&["rev-parse", "--verify", "HEAD"]).ok();
    let branch = run(&["symbolic-ref", "--short", "-q", "HEAD"]).ok();
    let original = git_text(root, &["rev-parse", "--git-path", "index"])?;
    let original = root.join(original);
    if original.exists() {
        let modified = original.metadata()?.modified()?;
        std::fs::copy(original, &index)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&index)?
            .set_times(std::fs::FileTimes::new().set_modified(modified))?;
    } else {
        run(&["read-tree", "--empty"])?;
    }
    // A command has a five-second deadline, including walking untracked trees.
    run(&["-c", "core.splitIndex=false", "add", "-A", "--", "."])?;
    let tree = run(&["write-tree"])?;
    let known = run(&[
        "for-each-ref",
        "--format=%(objectname)",
        "refs/heads",
        "refs/remotes",
    ])?;
    ensure!(
        known.len() < 64 * 1024,
        "too many repository refs to checkpoint"
    );
    let message = format!("Pockymoe checkpoint\n\n{known}");
    let mut args = vec![
        "-c",
        "commit.gpgsign=false",
        "commit-tree",
        &tree,
        "-m",
        &message,
    ];
    if let Some(h) = &head {
        args.extend(["-p", h]);
    }
    let commit = run(&args)?;
    run(&["update-ref", reference, &commit])?;
    Ok(json!({"commit":commit,"head":head,"branch":branch}))
}
pub(crate) fn restore(root: &Path, oid: &str) -> Result<()> {
    if crate::execution::file_request("gitCheckpointRestore", root, "", json!({"oid":oid}))?
        .is_some()
    {
        return Ok(());
    }
    let temp = tempfile::tempdir()?;
    let index = temp.path().join("index");
    // Preserve Git's stat cache; hashing every tracked file defeats restore budgets.
    let original = root.join(git_text(root, &["rev-parse", "--git-path", "index"])?);
    if original.exists() {
        let modified = original.metadata()?.modified()?;
        std::fs::copy(original, &index)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&index)?
            .set_times(std::fs::FileTimes::new().set_modified(modified))?;
    } else {
        indexed(root, &index, &["read-tree", "--empty"])?;
    }
    indexed(root, &index, &["add", "-A", "--", "."])?;
    let current = indexed(root, &index, &["write-tree"])?;
    let deleted = git(
        root,
        &[
            "diff",
            "--no-renames",
            "--name-only",
            "-z",
            "--diff-filter=D",
            &current,
            oid,
            "--",
        ],
    )?;
    ensure!(!deleted.truncated, "restore deletion list exceeds limit");
    let changed = git(
        root,
        &[
            "diff",
            "--no-renames",
            "--name-only",
            "-z",
            "--diff-filter=ACMT",
            &current,
            oid,
            "--",
        ],
    )?;
    ensure!(
        !changed.truncated,
        "restore changed path list exceeds limit"
    );
    indexed(root, &index, &["read-tree", oid])?;
    for path in deleted.bytes.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let path = std::str::from_utf8(path)?;
        let target = root.join(path);
        ensure!(
            target
                .parent()
                .context("invalid restore path")?
                .canonicalize()?
                .starts_with(root),
            "restore path escapes checkout"
        );
        if target
            .symlink_metadata()
            .is_ok_and(|m| m.is_file() || m.file_type().is_symlink())
        {
            std::fs::remove_file(&target)?;
            let mut parent = target.parent();
            while let Some(dir) = parent {
                if dir == root || std::fs::remove_dir(dir).is_err() {
                    break;
                }
                parent = dir.parent();
            }
        }
    }
    // A NUL-delimited stdin stream handles arbitrary names without ARG_MAX
    // limits and materializes all changed paths in one Git invocation.
    if !changed.bytes.is_empty() {
        use std::io::{Seek, SeekFrom, Write};
        let mut paths = tempfile::tempfile()?;
        paths.write_all(&changed.bytes)?;
        paths.seek(SeekFrom::Start(0))?;
        git_options_input(
            root,
            &["checkout-index", "--force", "--stdin", "-z"],
            Duration::from_secs(5),
            &[(
                "GIT_INDEX_FILE",
                index.to_str().context("invalid index path")?,
            )],
            Some(paths),
        )?;
    }
    // Verify the actual working tree, including removals and untracked files.
    // This also guards undo and the automatic safety restore: a transport that
    // silently loses checkout-index input must never report success.
    indexed(root, &index, &["add", "-A", "--", "."])?;
    let restored = indexed(root, &index, &["write-tree"])?;
    let target = git_text(root, &["rev-parse", &format!("{oid}^{{tree}}")])?;
    ensure!(
        restored == target,
        "restore verification failed: working tree differs from checkpoint"
    );
    Ok(())
}
// T3 attribution rule: dirty changes at either boundary and locally authored
// commits belong to the turn. Imported history and clean merge contributions do
// not. Remerge diffs retain conflict resolutions made by a local merge.
fn authored_paths(
    root: &Path,
    start: &str,
    end: &str,
) -> Result<Option<std::collections::BTreeSet<String>>> {
    let parent = |oid: &str| git_text(root, &["rev-parse", "--verify", &format!("{oid}^1")]).ok();
    let (Some(a), Some(b)) = (parent(start), parent(end)) else {
        return Ok(None);
    };
    if a == b {
        return Ok(None);
    }
    let mut paths = std::collections::BTreeSet::new();
    let mut add = |args: &[&str]| -> Result<()> {
        let out = git(root, args)?;
        ensure!(!out.truncated, "authored paths exceed limit");
        for path in out.bytes.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            paths.insert(
                std::str::from_utf8(path)?
                    .trim_start_matches('\n')
                    .to_owned(),
            );
        }
        Ok(())
    };
    add(&["diff", "--name-only", "--no-renames", "-z", &a, start, "--"])?;
    add(&["diff", "--name-only", "--no-renames", "-z", &b, end, "--"])?;
    let local = git_text(root, &["reflog", "show", "--all", "--format=%H %gs"])?;
    let local: std::collections::HashSet<&str> = local
        .lines()
        .filter_map(|line| {
            let (oid, reason) = line.split_once(' ')?;
            if ["commit", "merge ", "rebase ", "cherry-pick"]
                .iter()
                .any(|p| reason.starts_with(p))
            {
                Some(oid)
            } else {
                None
            }
        })
        .collect();
    let baseline = git_text(root, &["show", "-s", "--format=%b", start])?;
    let range = format!("{a}..{b}");
    let mut revargs = vec!["rev-list", &range, "--not"];
    revargs.extend(
        baseline
            .lines()
            .filter(|s| s.len() >= 40 && s.bytes().all(|b| b.is_ascii_hexdigit())),
    );
    let commits = git_text(root, &revargs)?;
    for oid in commits.lines().filter(|oid| local.contains(oid)) {
        add(&[
            "show",
            "--format=",
            "--diff-merges=remerge",
            "--name-only",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "-z",
            oid,
            "--",
        ])?;
    }
    Ok(Some(paths))
}
impl Supervisor {
    pub(crate) fn capture_turn_checkpoints(&self, thread: &str, turn: &str, phase: &str) {
        if let Err(error) = self.capture_checkpoints_inner(thread, turn, phase) {
            tracing::warn!(%error,thread,turn,"checkpoint unavailable");
        }
    }
    fn capture_checkpoints_inner(&self, thread: &str, turn: &str, phase: &str) -> Result<()> {
        let context = self.worktree_context(thread)?;
        let Some(copies) = context["workingCopies"].as_array() else {
            return Ok(());
        };
        for entry in copies {
            let Some(id) = entry["id"].as_str() else {
                continue;
            };
            let copy = self.worktree_copy(thread, id)?;
            let exists:bool=self.db.with(|c|Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM turn_checkpoints WHERE thread_id=?1 AND turn_id=?2 AND copy_id=?3 AND phase=?4)",params![thread,turn,id,phase],|r|r.get(0))?))?;
            if exists {
                continue;
            }
            let reference = format!(
                "refs/pockymoe/checkpoints/{thread}/{turn}/{}/{phase}",
                copy.id
            );
            let result = snapshot(Path::new(&copy.path), &reference);
            let (value, error) = match result {
                Ok(v) => (v, None),
                Err(e) => (Value::Null, Some(e.to_string())),
            };
            self.db.with(|c|{c.execute("INSERT OR IGNORE INTO turn_checkpoints(thread_id,turn_id,copy_id,phase,commit_oid,head_oid,branch,status,error,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![thread,turn,id,phase,value["commit"].as_str(),value["head"].as_str(),value["branch"].as_str(),if error.is_some(){"unavailable"}else{"available"},error,now_rfc3339()])?;Ok(())})?;
        }
        if phase == "end" {
            for entry in copies {
                if let Some(id) = entry["id"].as_str() {
                    let _ = self.checkpoint_diff(thread, Some(id), turn, None, false);
                }
            }
        }
        self.prune_checkpoints(thread, false)?;
        self.git_changed(thread, "checkpointsChanged");
        Ok(())
    }
    fn checkpoint_oid(&self, thread: &str, turn: &str, id: &str, phase: &str) -> Result<String> {
        ensure!(
            matches!(phase, "start" | "end") || phase.starts_with("undo-"),
            "invalid checkpoint phase"
        );
        self.db.with(|c|c.query_row("SELECT commit_oid FROM turn_checkpoints WHERE thread_id=?1 AND turn_id=?2 AND copy_id=?3 AND phase=?4 AND status='available'",params![thread,turn,id,phase],|r|r.get(0)).optional()?.context("checkpoint unavailable"))
    }
    pub fn checkpoints_list(&self, thread: &str) -> Result<Value> {
        self.get_thread(thread)?;
        let mut turns=self.db.with(|c|{
            let mut s=c.prepare("SELECT id,status,started_at,completed_at FROM thread_turns WHERE thread_id=?1 ORDER BY started_at,id")?;
            let rows=s.query_map([thread],|r|Ok(json!({"turnId":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?,"startedAt":r.get::<_,Option<String>>(2)?,"completedAt":r.get::<_,Option<String>>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;Ok(rows)
        })?;
        for (i, turn) in turns.iter_mut().enumerate() {
            turn["turnIndex"] = json!(i + 1);
            let id = turn["turnId"].as_str().unwrap();
            let copies=self.db.with(|c|{let mut s=c.prepare("SELECT copy_id,status,error,files,additions,deletions FROM turn_checkpoints WHERE thread_id=?1 AND turn_id=?2 AND phase='start'")?;let rows=s.query_map(params![thread,id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<u64>>(3)?,r.get::<_,Option<u64>>(4)?,r.get::<_,Option<u64>>(5)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;Ok(rows)})?;
            let mut values = Vec::new();
            for (copy, status, reason, files, additions, deletions) in copies {
                let diff: Result<Value> = if files.is_some() {
                    Ok(json!({"files":files,"additions":additions,"deletions":deletions}))
                } else {
                    Err(anyhow::anyhow!("Turn statistics unavailable"))
                };
                let mut value = json!({"workingCopyId":copy,"available":status=="available"&&diff.is_ok(),"reason":reason.or_else(||diff.as_ref().err().map(ToString::to_string)),"files":0,"additions":0,"deletions":0});
                if let Ok(diff) = diff {
                    for key in ["files", "additions", "deletions"] {
                        value[key] = diff[key].clone();
                    }
                }
                values.push(value);
            }
            turn["workingCopies"] = json!(values);
        }
        Ok(json!({"turns":turns}))
    }
    pub fn checkpoint_diff(
        &self,
        thread: &str,
        id: Option<&str>,
        turn: &str,
        path: Option<&str>,
        ignore: bool,
    ) -> Result<Value> {
        let copy = self.selected_copy(thread, id)?;
        let start = self.checkpoint_oid(thread, turn, &copy.id, "start")?;
        let end = self.checkpoint_oid(thread, turn, &copy.id, "end")?;
        let root = Path::new(&copy.path);
        let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-color"];
        if ignore {
            args.push("--ignore-all-space");
        }
        args.extend([&start, &end, "--"]);
        let authored = authored_paths(root, &start, &end)?;
        let selected: Vec<String> = if let Some(path) = path {
            super::actions::relative(path)?;
            if authored.as_ref().is_none_or(|p| p.contains(path)) {
                vec![path.into()]
            } else {
                Vec::new()
            }
        } else {
            authored
                .as_ref()
                .map(|p| p.iter().cloned().collect())
                .unwrap_or_default()
        };
        let filtered_empty = (authored.is_some() || path.is_some()) && selected.is_empty();
        args.extend(selected.iter().map(String::as_str));
        let out = if filtered_empty {
            GitOutput {
                bytes: Vec::new(),
                truncated: false,
            }
        } else {
            git(root, &args)?
        };
        let mut stats_args = vec!["diff", "--numstat", &start, &end, "--"];
        stats_args.extend(selected.iter().map(String::as_str));
        let stats = if filtered_empty {
            String::new()
        } else {
            git_text(root, &stats_args)?
        };
        let (mut files, mut additions, mut deletions) = (0u64, 0u64, 0u64);
        for row in stats.lines() {
            let mut f = row.split('\t');
            additions += f.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            deletions += f.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            files += 1;
        }
        if path.is_none() && !ignore {
            self.db.with(|c|{c.execute("UPDATE turn_checkpoints SET files=?4,additions=?5,deletions=?6 WHERE thread_id=?1 AND turn_id=?2 AND copy_id=?3 AND phase='start'",params![thread,turn,copy.id,files,additions,deletions])?;Ok(())})?;
        }
        Ok(
            json!({"workingCopyId":copy.id,"area":"turn","turnId":turn,"patch":String::from_utf8_lossy(&out.bytes),"truncated":out.truncated,"files":files,"additions":additions,"deletions":deletions}),
        )
    }
    pub async fn checkpoint_restore(
        &self,
        thread: &str,
        input: &Value,
        preview: bool,
    ) -> Result<Value> {
        self.checkpoint_restore_inner(thread, input, preview, false)
            .await
    }
    pub(crate) async fn checkpoint_restore_inner(
        &self,
        thread: &str,
        input: &Value,
        preview: bool,
        claimed: bool,
    ) -> Result<Value> {
        let t = self.get_thread(thread)?;
        ensure!(
            !matches!(t.status.as_str(), "running" | "recovering"),
            "conflict: thread is running"
        );
        let copy = self.selected_copy(thread, input["workingCopyId"].as_str())?;
        let others=self.db.with(|c|{let mut s=c.prepare("SELECT DISTINCT t.id FROM threads t LEFT JOIN thread_working_copies w ON w.thread_id=t.id WHERE t.id!=?1 AND (t.worktree_path=?2 OR w.copy_id=?3 OR (t.worktree_path IS NULL AND t.workspace_id=?4))")?;let rows=s.query_map(params![thread,copy.path,copy.id,t.workspace_id],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;Ok(rows)})?;
        let mut claim_ids = others.clone();
        if !claimed {
            claim_ids.push(thread.into());
        }
        let _claim = self
            .try_claim(&claim_ids)
            .context("conflict: a thread using this working copy is busy")?;
        for other in others {
            let t = self.get_thread(&other)?;
            let loaded = if let Some(session) = &t.provider_session_id {
                self.runtime(t.provider)?
                    .loaded_sessions()
                    .await
                    .iter()
                    .any(|s| crate::import_id::session_ids_match(&s.session_id, session))
            } else {
                false
            };
            ensure!(
                !matches!(t.status.as_str(), "running" | "recovering") && !loaded,
                "conflict: working copy is used by thread {other}"
            );
        }
        let _guard = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
        let turn = input["turnId"].as_str().context("turnId required")?;
        let phase = input["phase"].as_str().unwrap_or("start");
        let oid = self.checkpoint_oid(thread, turn, &copy.id, phase)?;
        let root = Path::new(&copy.path);
        let undo = uuid::Uuid::new_v4().to_string();
        let reference = format!(
            "refs/pockymoe/checkpoints/{thread}/{turn}/{}/undo-{undo}",
            copy.id
        );
        let before = snapshot(root, &reference)?;
        let result = (|| {
            let diff = git(
                root,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                    before["commit"].as_str().unwrap(),
                    &oid,
                    "--",
                ],
            )?;
            if preview {
                let mut changes = std::collections::HashMap::new();
                let names = git(
                    root,
                    &[
                        "diff",
                        "--name-status",
                        "--no-renames",
                        "-z",
                        before["commit"].as_str().unwrap(),
                        &oid,
                        "--",
                    ],
                )?;
                ensure!(!names.truncated, "preview paths exceed limit");
                let mut fields = names.bytes.split(|b| *b == 0);
                while let (Some(change), Some(path)) = (fields.next(), fields.next()) {
                    changes.insert(
                        String::from_utf8_lossy(path).into_owned(),
                        String::from_utf8_lossy(change).into_owned(),
                    );
                }
                let stats = git(
                    root,
                    &[
                        "diff",
                        "--numstat",
                        "--no-renames",
                        "-z",
                        before["commit"].as_str().unwrap(),
                        &oid,
                        "--",
                    ],
                )?;
                ensure!(!stats.truncated, "preview statistics exceed limit");
                let mut files = Vec::new();
                for row in stats.bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
                    let row = std::str::from_utf8(row)?;
                    let mut fields = row.splitn(3, '\t');
                    let a = fields.next().unwrap_or("");
                    let d = fields.next().unwrap_or("");
                    let path = fields.next().unwrap_or("");
                    files.push(json!({"path":path,"change":changes.get(path),"additions":a.parse::<u64>().unwrap_or(0),"deletions":d.parse::<u64>().unwrap_or(0),"binary":a=="-"}));
                }
                return Ok(
                    json!({"files":files,"patch":String::from_utf8_lossy(&diff.bytes),"truncated":diff.truncated}),
                );
            }
            self.db.with(|c|{c.execute("INSERT INTO turn_checkpoints(thread_id,turn_id,copy_id,phase,commit_oid,status,created_at) VALUES(?1,?2,?3,?4,?5,'available',?6)",params![thread,turn,copy.id,format!("undo-{undo}"),before["commit"].as_str(),now_rfc3339()])?;Ok(())})?;
            if let Err(error) = restore(root, &oid) {
                restore(
                    root,
                    before["commit"]
                        .as_str()
                        .context("safety checkpoint missing")?,
                )
                .context(format!(
                    "restore failed ({error}); safety restore also failed"
                ))?;
                return Err(error);
            }
            self.git_changed(thread, "gitChanged");
            Ok(json!({"restored":true,"undoId":format!("{turn}:{}:{undo}",copy.id)}))
        })();
        if preview {
            let _ = git(root, &["update-ref", "-d", &reference]);
        }
        result
    }
    pub async fn checkpoint_undo(&self, thread: &str, undo: &str) -> Result<Value> {
        let parts: Vec<_> = undo.split(':').collect();
        ensure!(parts.len() == 3, "invalid undoId");
        self.checkpoint_restore(thread,&json!({"turnId":parts[0],"workingCopyId":parts[1],"phase":format!("undo-{}",parts[2])}),false).await
    }
}
impl Supervisor {
    pub(crate) fn prune_checkpoints(&self, thread: &str, all: bool) -> Result<()> {
        let rows=self.db.with(|c|{
            let sql=if all {"SELECT turn_id,copy_id,phase FROM turn_checkpoints WHERE thread_id=?1"} else {"SELECT turn_id,copy_id,phase FROM turn_checkpoints WHERE thread_id=?1 AND turn_id NOT IN (SELECT turn_id FROM turn_checkpoints WHERE thread_id=?1 GROUP BY turn_id ORDER BY MAX(created_at) DESC LIMIT 200)"};
            let mut s=c.prepare(sql)?;let rows=s.query_map([thread],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;Ok(rows)
        })?;
        for (turn, id, phase) in rows {
            if let Ok(copy) = self.worktree_copy(thread, &id) {
                let reference = format!("refs/pockymoe/checkpoints/{thread}/{turn}/{id}/{phase}");
                git(Path::new(&copy.path), &["update-ref", "-d", &reference])?;
            }
            self.db.with(|c|{c.execute("DELETE FROM turn_checkpoints WHERE thread_id=?1 AND turn_id=?2 AND copy_id=?3 AND phase=?4",params![thread,turn,id,phase])?;Ok(())})?;
        }
        Ok(())
    }
}
impl Supervisor {
    pub(crate) fn capture_recovery_checkpoints(&self) -> Result<()> {
        let turns = self.db.with(|c| {
            let mut s =
                c.prepare("SELECT thread_id,id FROM thread_turns WHERE status='recovering'")?;
            let rows = s
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })?;
        for (thread, turn) in turns {
            let context = self.execution_context(&thread)?;
            crate::execution::blocking_scope(context, || {
                self.capture_turn_checkpoints(&thread, &turn, "end")
            });
        }
        Ok(())
    }
}
impl Supervisor {
    pub(crate) async fn capture_turn_checkpoints_async(
        &self,
        thread: &str,
        turn: &str,
        phase: &str,
    ) {
        if let Err(error) = self
            .capture_checkpoints_async_inner(thread, turn, phase)
            .await
        {
            tracing::warn!(%error,thread,turn,"checkpoint unavailable");
        }
    }
    async fn capture_checkpoints_async_inner(
        &self,
        thread: &str,
        turn: &str,
        phase: &str,
    ) -> Result<()> {
        let t = self.get_thread(thread)?;
        let mut paths=self.db.with(|c|{let mut s=c.prepare("SELECT w.root FROM working_copies w JOIN thread_working_copies t ON t.copy_id=w.id WHERE t.thread_id=?1")?;let rows=s.query_map([thread],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;Ok(rows)})?;
        if let Some(cwd) = self.session_cwd(&t) {
            paths.push(cwd);
        }
        paths.sort();
        paths.dedup();
        let saved=self.db.with(|c|{let mut s=c.prepare("SELECT copy_id,phase,commit_oid FROM turn_checkpoints WHERE thread_id=?1 AND turn_id=?2")?;let rows=s.query_map(params![thread,turn],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;Ok(rows)})?;
        let owner = thread.to_owned();
        let turn_id = turn.to_owned();
        let phase_name = phase.to_owned();
        let captured = crate::execution::spawn_blocking(move || -> Result<Vec<_>> {
            let mut captured = Vec::new();
            for path in paths {
                let Ok(copy) = probe(Path::new(&path)) else {
                    continue;
                };
                if saved
                    .iter()
                    .any(|(id, phase, _)| id == &copy.id && phase == &phase_name)
                {
                    continue;
                }
                let reference = format!(
                    "refs/pockymoe/checkpoints/{owner}/{turn_id}/{}/{phase_name}",
                    copy.id
                );
                let value = snapshot(Path::new(&copy.path), &reference);
                let counts = if phase_name == "end" {
                    saved
                        .iter()
                        .find(|(id, phase, _)| id == &copy.id && phase == "start")
                        .and_then(|(_, _, oid)| oid.as_ref())
                        .and_then(|start| {
                            value
                                .as_ref()
                                .ok()
                                .and_then(|v| v["commit"].as_str())
                                .and_then(|end| {
                                    checkpoint_counts(Path::new(&copy.path), start, end).ok()
                                })
                        })
                } else {
                    None
                };
                captured.push((copy, value, counts));
            }
            Ok(captured)
        })
        .await??;
        for (copy, result, counts) in captured {
            let copy = self.remember_copy_unlocked(thread, copy, "configured", false)?;
            let (value, error) = match result {
                Ok(v) => (v, None),
                Err(e) => (Value::Null, Some(e.to_string())),
            };
            self.db.with(|c|{c.execute("INSERT OR IGNORE INTO turn_checkpoints(thread_id,turn_id,copy_id,phase,commit_oid,head_oid,branch,status,error,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![thread,turn,copy.id,phase,value["commit"].as_str(),value["head"].as_str(),value["branch"].as_str(),if error.is_some(){"unavailable"}else{"available"},error,now_rfc3339()])?;
                if let Some((files,additions,deletions))=counts {c.execute("UPDATE turn_checkpoints SET files=?4,additions=?5,deletions=?6 WHERE thread_id=?1 AND turn_id=?2 AND copy_id=?3 AND phase='start'",params![thread,turn,copy.id,files,additions,deletions])?;}Ok(())})?;
        }
        // Retention usually has no Git work; defer cleanup to its own bounded job.
        self.prune_checkpoints(thread, false)?;
        self.git_changed(thread, "checkpointsChanged");
        Ok(())
    }
}
fn checkpoint_counts(root: &Path, start: &str, end: &str) -> Result<(u64, u64, u64)> {
    let authored = authored_paths(root, start, end)?;
    if authored.as_ref().is_some_and(|p| p.is_empty()) {
        return Ok((0, 0, 0));
    }
    let mut args = vec!["diff", "--numstat", start, end, "--"];
    if let Some(paths) = &authored {
        args.extend(paths.iter().map(String::as_str));
    }
    let stats = git_text(root, &args)?;
    let (mut files, mut additions, mut deletions) = (0, 0, 0);
    for row in stats.lines() {
        let mut f = row.split('\t');
        additions += f.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        deletions += f.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        files += 1;
    }
    Ok((files, additions, deletions))
}
