//! Managed working copies and observed tool locations. A binding is not a shell cd.
use crate::Supervisor;
use anyhow::{bail, ensure, Context, Result};
use pockymoe_protocol::{now_rfc3339, ThreadEventEnvelope, ThreadEventType};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 512 * 1024;
static MUTATION: Mutex<()> = Mutex::new(());

pub(crate) fn migrate(c: &Connection) -> Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS working_copies (
        id TEXT PRIMARY KEY, root TEXT NOT NULL UNIQUE, common_dir TEXT NOT NULL,
        git_dir TEXT NOT NULL, owner_thread_id TEXT, created_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS thread_working_copies (
        thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
        copy_id TEXT NOT NULL REFERENCES working_copies(id) ON DELETE CASCADE,
        source TEXT NOT NULL, PRIMARY KEY(thread_id,copy_id));
        CREATE TABLE IF NOT EXISTS working_copy_observations (
        thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
        copy_id TEXT NOT NULL REFERENCES working_copies(id) ON DELETE CASCADE,
        cwd TEXT NOT NULL, source TEXT NOT NULL, tool_id TEXT NOT NULL,
        agent_id TEXT NOT NULL, observed_at TEXT NOT NULL,
        PRIMARY KEY(thread_id,tool_id,agent_id,cwd));
        CREATE INDEX IF NOT EXISTS working_copy_observations_thread ON working_copy_observations(thread_id,observed_at);
        INSERT OR IGNORE INTO kv(key,value)
        SELECT 'legacy-worktree-owner:' || id, worktree_path FROM threads
        WHERE worktree_path IS NOT NULL AND NOT EXISTS
          (SELECT 1 FROM thread_working_copies WHERE thread_id=threads.id);")?;
    Ok(())
}

struct GitOutput {
    bytes: Vec<u8>,
    truncated: bool,
}
// Drain both pipes even after the response cap. Commands are bounded and cannot
// retain an unbounded diff in memory or block a runtime worker indefinitely.
fn git(root: &Path, args: &[&str]) -> Result<GitOutput> {
    if let Some(value) = crate::execution::file_request("git", root, "", json!({"args":args}))? {
        use base64::Engine;
        return Ok(GitOutput {
            bytes: base64::engine::general_purpose::STANDARD
                .decode(value["bytes"].as_str().context("missing Git response")?)?,
            truncated: value["truncated"].as_bool().unwrap_or(false),
        });
    }
    fn drain(mut pipe: impl Read) -> std::io::Result<(Vec<u8>, bool)> {
        let mut kept = Vec::new();
        let mut truncated = false;
        let mut buf = [0; 8192];
        loop {
            let n = pipe.read(&mut buf)?;
            if n == 0 {
                break;
            }
            let take = n.min(OUTPUT_LIMIT.saturating_sub(kept.len()));
            kept.extend_from_slice(&buf[..take]);
            truncated |= take < n;
        }
        Ok((kept, truncated))
    }
    let mut cmd = Command::new("git");
    let hooks = tempfile::tempdir()?;
    cmd.current_dir(root)
        .args(["-c", "core.fsmonitor=false", "-c", "core.pager=cat"])
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.path().display()))
        .arg("--literal-pathspecs")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in crate::child_process::inherited_host_identity() {
        cmd.env_remove(key);
    }
    // Inherited git -C/worktree overrides would make the observed path inaccurate.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        cmd.env_remove(key);
    }
    let mut child = cmd.spawn().context("Git is unavailable")?;
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let stdout = std::thread::spawn(move || drain(out));
    let stderr = std::thread::spawn(move || drain(err));
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let (bytes, truncated) = stdout
        .join()
        .map_err(|_| anyhow::anyhow!("Git reader failed"))??;
    let (errors, _) = stderr
        .join()
        .map_err(|_| anyhow::anyhow!("Git reader failed"))??;
    ensure!(status.is_some(), "Git operation timed out");
    ensure!(
        status.unwrap().success(),
        "Git operation failed: {}",
        String::from_utf8_lossy(&errors)
    );
    Ok(GitOutput { bytes, truncated })
}
fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let out = git(root, args)?;
    ensure!(!out.truncated, "Git metadata exceeds limit");
    Ok(String::from_utf8(out.bytes)?
        .trim_end_matches(['\r', '\n'])
        .to_owned())
}
fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingCopy {
    pub id: String,
    pub repository_id: String,
    pub path: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub managed: bool,
    pub source: String,
    pub last_observed_at: Option<String>,
    #[serde(skip)]
    common_dir: String,
    #[serde(skip)]
    git_dir: String,
}
fn probe(path: &Path) -> Result<WorkingCopy> {
    let path = std::fs::canonicalize(path)?;
    let cwd = if path.is_dir() {
        path
    } else {
        path.parent().context("path has no parent")?.into()
    };
    let root = std::fs::canonicalize(git_text(&cwd, &["rev-parse", "--show-toplevel"])?)?;
    let path = root.to_string_lossy().into_owned();
    ensure!(
        !path.contains(['\r', '\n']),
        "Git paths with newlines are unsupported"
    );
    let dirs = git_text(&root, &["rev-parse", "--git-dir", "--git-common-dir"])?;
    let mut dirs = dirs.lines();
    let resolve = |v: &str| -> Result<String> {
        Ok(std::fs::canonicalize(root.join(v))?
            .to_string_lossy()
            .into_owned())
    };
    let git_dir = resolve(dirs.next().context("missing gitdir")?)?;
    let common_dir = resolve(dirs.next().context("missing common gitdir")?)?;
    let branch = git_text(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    let head = git_text(&root, &["rev-parse", "--verify", "HEAD"]).ok();
    Ok(WorkingCopy {
        id: hash(&format!("{common_dir}\0{path}")),
        repository_id: hash(&common_dir),
        path,
        branch,
        head,
        managed: false,
        source: "observed".into(),
        last_observed_at: None,
        common_dir,
        git_dir,
    })
}

impl Supervisor {
    /// The older delegate-close entry point shares ownership and usage checks.
    /// A path selected with `use` must never become deletable merely by closing.
    pub(crate) fn remove_configured_worktree(&self, thread: &str, path: &Path) -> Result<()> {
        let _guard = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
        let copy = probe(path)?;
        self.db.with(|c| {
            let owner:Option<Option<String>>=c.query_row("SELECT owner_thread_id FROM working_copies WHERE id=?1",[&copy.id],|r|r.get(0)).optional()?;
            let legacy:Option<String>=c.query_row("SELECT value FROM kv WHERE key=?1",[format!("legacy-worktree-owner:{thread}")],|r|r.get(0)).optional()?;
            ensure!(owner==Some(Some(thread.into())) || (owner.flatten().is_none() && legacy.as_deref()==Some(copy.path.as_str())),"forbidden: this thread did not create the worktree");
            let users:i64=c.query_row("SELECT COUNT(*) FROM threads t WHERE t.id!=?1 AND (t.worktree_path=?2 OR EXISTS(SELECT 1 FROM thread_working_copies a WHERE a.thread_id=t.id AND a.copy_id=?3))",params![thread,copy.path,copy.id],|r|r.get(0))?;
            ensure!(users==0,"conflict: other threads use this worktree");Ok(())
        })?;
        ensure!(
            git(
                Path::new(&copy.path),
                &[
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--untracked-files=all",
                    "--ignored=matching"
                ]
            )?
            .bytes
            .is_empty(),
            "conflict: worktree has uncommitted, untracked or ignored files"
        );
        git(
            Path::new(&copy.common_dir),
            &["worktree", "remove", &copy.path],
        )?;
        self.forget_worktree(&copy.id, &copy.path)
    }
    fn forget_worktree(&self, id: &str, path: &str) -> Result<()> {
        self.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute(
                "DELETE FROM kv WHERE key LIKE 'legacy-worktree-owner:%' AND value=?1",
                [path],
            )?;
            tx.execute(
                "DELETE FROM working_copy_observations WHERE copy_id=?1",
                [id],
            )?;
            tx.execute("DELETE FROM thread_working_copies WHERE copy_id=?1", [id])?;
            tx.execute("DELETE FROM working_copies WHERE id=?1", [id])?;
            tx.commit()?;
            Ok(())
        })
    }
    pub(crate) fn register_created_worktree(&self, thread: &str, path: &Path) -> Result<()> {
        self.remember_copy(thread, probe(path)?, "created", true)?;
        Ok(())
    }
    fn remember_copy(
        &self,
        thread: &str,
        copy: WorkingCopy,
        source: &str,
        managed: bool,
    ) -> Result<WorkingCopy> {
        let _guard = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
        self.remember_copy_unlocked(thread, copy, source, managed)
    }
    fn remember_copy_unlocked(
        &self,
        thread: &str,
        mut copy: WorkingCopy,
        source: &str,
        managed: bool,
    ) -> Result<WorkingCopy> {
        self.get_thread(thread)?;
        self.db.with(|c| {
            // Only the upgrade snapshot can establish legacy ownership. A
            // later `use` or ordinary path observation must not grant it.
            let legacy:Option<String>=c.query_row("SELECT value FROM kv WHERE key=?1",[format!("legacy-worktree-owner:{thread}")],|r|r.get(0)).optional()?;
            let managed = managed || legacy.as_deref()==Some(copy.path.as_str());
            c.execute("INSERT INTO working_copies(id,root,common_dir,git_dir,owner_thread_id,created_at) VALUES(?1,?2,?3,?4,?5,?6)
                ON CONFLICT(id) DO UPDATE SET git_dir=excluded.git_dir, owner_thread_id=COALESCE(working_copies.owner_thread_id,excluded.owner_thread_id)",params![copy.id,copy.path,copy.common_dir,copy.git_dir,managed.then_some(thread),now_rfc3339()])?;
            c.execute("INSERT INTO thread_working_copies(thread_id,copy_id,source) VALUES(?1,?2,?3)
                ON CONFLICT(thread_id,copy_id) DO UPDATE SET source=CASE WHEN source='observed' THEN excluded.source ELSE source END",params![thread,copy.id,source])?;
            copy.managed = c.query_row("SELECT owner_thread_id IS NOT NULL FROM working_copies WHERE id=?1",[&copy.id],|r|r.get(0))?;
            Ok(())
        })?;
        copy.source = source.into();
        Ok(copy)
    }
    pub fn worktree_copy(&self, thread: &str, id: &str) -> Result<WorkingCopy> {
        let (root,source,managed,last): (String,String,bool,Option<String>) = self.db.with(|c| Ok(c.query_row(
            "SELECT w.root,t.source,w.owner_thread_id IS NOT NULL,(SELECT MAX(observed_at) FROM working_copy_observations WHERE thread_id=?1 AND copy_id=w.id)
            FROM working_copies w JOIN thread_working_copies t ON t.copy_id=w.id WHERE t.thread_id=?1 AND w.id=?2",params![thread,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?.context("working copy is not associated with this thread")?))?;
        let mut copy = probe(Path::new(&root))?;
        ensure!(
            copy.id == id,
            "conflict: working copy was replaced; register it again"
        );
        copy.source = source;
        copy.managed = managed;
        copy.last_observed_at = last;
        Ok(copy)
    }
    pub fn worktree_context(&self, thread: &str) -> Result<Value> {
        let target = self.get_thread(thread)?;
        let cwd = self
            .session_cwd(&target)
            .context("thread has no working directory")?;
        let primary = probe(Path::new(&cwd))
            .ok()
            .and_then(|c| self.remember_copy(thread, c, "configured", false).ok());
        let ids: Vec<String> = self.db.with(|c| {
            let mut q = c.prepare(
                "SELECT copy_id FROM thread_working_copies WHERE thread_id=?1 ORDER BY rowid",
            )?;
            let rows = q
                .query_map([thread], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })?;
        let copies: Vec<_> = ids
            .iter()
            .filter_map(|id| self.worktree_copy(thread, id).ok())
            .collect();
        let observations: Vec<Value> = self.db.with(|c| { let mut q=c.prepare("SELECT copy_id,cwd,source,tool_id,agent_id,observed_at FROM working_copy_observations WHERE thread_id=?1 ORDER BY observed_at DESC LIMIT 64")?;
            let rows=q.query_map([thread],|r| Ok(json!({"workingCopyId":r.get::<_,String>(0)?,"cwd":r.get::<_,String>(1)?,"source":r.get::<_,String>(2)?,"toolCallId":r.get::<_,String>(3)?,"agentId":r.get::<_,String>(4)?,"observedAt":r.get::<_,String>(5)?})))?.collect::<rusqlite::Result<_>>()?; Ok(rows) })?;
        Ok(
            json!({"configuredCwd":cwd,"primaryWorkingCopyId":primary.map(|c|c.id),"workingCopies":copies,"observations":observations}),
        )
    }
    pub fn worktree_current(&self, thread: &str, cwd: &str) -> Result<Value> {
        ensure!(Path::new(cwd).is_absolute(), "cwd must be absolute");
        let mut context = self.worktree_context(thread)?;
        let current = probe(Path::new(cwd))
            .ok()
            .map(|copy| self.worktree_copy(thread, &copy.id).unwrap_or(copy));
        context["currentWorkingCopy"] = serde_json::to_value(current)?;
        context["cwd"] = json!(cwd);
        Ok(context)
    }
    pub fn worktree_adopt(&self, thread: &str, path: &str) -> Result<WorkingCopy> {
        ensure!(Path::new(path).is_absolute(), "path must be absolute");
        let target = self.get_thread(thread)?;
        let workspace = self.get_workspace(&target.workspace_id)?;
        let copy = probe(Path::new(path))?;
        ensure!(
            copy.repository_id == probe(Path::new(&workspace.abs_path))?.repository_id,
            "worktree must belong to the thread workspace repository"
        );
        let result = self.remember_copy(thread, copy, "adopted", false)?;
        self.emit_worktree_changed(thread);
        Ok(result)
    }
    pub fn worktree_create(&self, thread: &str, input: &Value) -> Result<Value> {
        let _guard = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
        let name = input["name"].as_str().context("name required")?;
        crate::interaction::validate_name(name)?;
        let target = self.get_thread(thread)?;
        let workspace = self.get_workspace(&target.workspace_id)?;
        let original = probe(Path::new(&workspace.abs_path))?;
        let source = input["cwd"].as_str().map(PathBuf::from).unwrap_or_else(|| {
            PathBuf::from(self.session_cwd(&target).unwrap_or(workspace.abs_path))
        });
        ensure!(source.is_absolute(), "cwd must be absolute");
        let source = probe(&source)?;
        ensure!(
            source.repository_id == original.repository_id,
            "cwd belongs to a different repository"
        );
        let request = input["clientRequestId"].as_str();
        ensure!(
            request.is_none_or(|s| !s.is_empty() && s.len() <= 128),
            "invalid clientRequestId"
        );
        let key = request.map(|r| format!("worktree.create:{thread}:{r}"));
        let signature =
            json!({"name":name,"base":input["base"],"branch":input["branch"],"source":source.path});
        if let Some(key) = &key {
            if let Some(raw) = self.db.with(|c| {
                Ok(
                    c.query_row("SELECT value FROM kv WHERE key=?1", [key], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()?,
                )
            })? {
                let saved: Value = serde_json::from_str(&raw)?;
                ensure!(
                    saved["input"] == signature,
                    "conflict: request ID reused with different input"
                );
                let id = saved["workingCopyId"]
                    .as_str()
                    .context("conflict: creation outcome uncertain; inspect before retrying")?;
                return Ok(
                    json!({"workingCopy":self.worktree_copy(thread,id)?,"created":false,"sessionCwdUpdated":false}),
                );
            }
        }
        let base = input["base"].as_str().unwrap_or("HEAD");
        let commit = git_text(
            Path::new(&source.path),
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{base}^{{commit}}"),
            ],
        )?;
        let branch = input["branch"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("agent/{name}-{}", &hash(thread)[..8]));
        git_text(
            Path::new(&source.path),
            &["check-ref-format", "--branch", &branch],
        )?;
        let repo = Path::new(&original.path);
        let parent = repo
            .parent()
            .context("repository has no parent")?
            .join(format!(
                "{}.worktrees",
                repo.file_name()
                    .context("missing repo name")?
                    .to_string_lossy()
            ));
        let parent = if let Some(context) = self.execution_context(thread)? {
            context
                .manager
                .managed_worktree_root(&context.environment_id)?
        } else {
            parent
        };
        if parent.exists() {
            ensure!(
                !parent.symlink_metadata()?.file_type().is_symlink(),
                "worktree parent cannot be a symlink"
            );
        }
        std::fs::create_dir_all(&parent)?;
        let path = parent.join(format!("{name}-{}", &hash(thread)[..8]));
        ensure!(
            !path.exists() && !path.is_symlink(),
            "conflict: target already exists; adopt it or choose another name"
        );
        if let Some(key) = &key {
            self.db.with(|c| {
                c.execute(
                    "INSERT INTO kv(key,value) VALUES(?1,?2)",
                    params![key, json!({"input":signature,"path":path}).to_string()],
                )?;
                Ok(())
            })?;
        }
        // Refuse existing branches: reuse is explicit through adopt, never an
        // accidental checkout of old task work. Intent remains on uncertain failure.
        git(
            Path::new(&source.path),
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                path.to_str().context("non UTF-8 worktree path")?,
                &commit,
            ],
        )?;
        let copy = self.remember_copy_unlocked(thread, probe(&path)?, "created", true)?;
        if let Some(key) = &key {
            self.db.with(|c| {
                c.execute(
                    "UPDATE kv SET value=?2 WHERE key=?1",
                    params![
                        key,
                        json!({"input":signature,"workingCopyId":copy.id}).to_string()
                    ],
                )?;
                Ok(())
            })?;
        }
        self.emit_worktree_changed(thread);
        Ok(json!({"workingCopy":copy,"created":true,"sessionCwdUpdated":false}))
    }
    pub fn observe_worktree_location(
        &self,
        thread: &str,
        cwd: &str,
        source: &str,
        tool: &str,
        agent: &str,
    ) -> Result<()> {
        if !Path::new(cwd).is_absolute() {
            return Ok(());
        }
        let known: Option<String>=self.db.with(|c|Ok(c.query_row("SELECT copy_id FROM working_copy_observations WHERE thread_id=?1 AND tool_id=?2 AND agent_id=?3 AND cwd=?4",params![thread,tool,agent,cwd],|r|r.get(0)).optional()?))?;
        if known.is_some() {
            return Ok(());
        }
        let copy = match probe(Path::new(cwd)) {
            Ok(copy) => copy,
            Err(_) => return Ok(()),
        };
        let _guard = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
        let associated: bool = self.db.with(|c| {
            Ok(c.query_row(
            "SELECT EXISTS(SELECT 1 FROM thread_working_copies WHERE thread_id=?1 AND copy_id=?2)",
            params![thread,copy.id], |r|r.get(0))?)
        })?;
        let copy = self.remember_copy_unlocked(thread, copy, "observed", false)?;
        self.db.with(|c| {
            c.execute("INSERT OR REPLACE INTO working_copy_observations(thread_id,copy_id,cwd,source,tool_id,agent_id,observed_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![thread,copy.id,cwd,source,tool,agent,now_rfc3339()])?;
            c.execute("DELETE FROM working_copy_observations WHERE thread_id=?1 AND rowid NOT IN (SELECT rowid FROM working_copy_observations WHERE thread_id=?1 ORDER BY observed_at DESC LIMIT 256)",[thread])?; Ok(())
        })?;
        drop(_guard);
        if !associated {
            self.emit_worktree_changed(thread);
        }
        Ok(())
    }
    pub(crate) fn observe_worktree_locations(&self, thread: &str, tool: &str, locations: &[Value]) {
        for location in locations.iter().take(8) {
            if let Some(path) = location["path"].as_str() {
                let _ = self.observe_worktree_location(
                    thread,
                    path,
                    location["source"].as_str().unwrap_or("tool"),
                    tool,
                    location["agentId"].as_str().unwrap_or(""),
                );
            }
        }
    }
    fn emit_worktree_changed(&self, thread: &str) {
        let _ = self.db.with(|c| {
            c.execute(
                "UPDATE threads SET updated_at=?2 WHERE id=?1",
                params![thread, now_rfc3339()],
            )?;
            Ok(())
        });
        self.bus.emit(ThreadEventEnvelope {
            event_type: ThreadEventType::ThreadUpdated,
            thread_id: thread.into(),
            timestamp: now_rfc3339(),
            payload: json!({"reason":"workingCopyChanged"}),
        });
    }
    pub async fn worktree_use(&self, thread: &str, id: &str) -> Result<Value> {
        self.worktree_copy(thread, id)?;
        let target = self.get_thread(thread)?;
        ensure!(
            target.active_turn_id.is_none() && target.status != "running",
            "conflict: thread is running; use explicit tool cwd until idle"
        );
        self.db.with(|c| {
            let pending: i64 = c.query_row(
                "SELECT COUNT(*) FROM thread_pending_steers WHERE thread_id=?1",
                [thread],
                |r| r.get(0),
            )?;
            ensure!(pending == 0, "conflict: thread has queued work");
            Ok(())
        })?;
        let may_release = |other: &str| other == thread;
        self.release_thread_harness(thread, false, Some(&may_release))
            .await?;
        let _guard = self
            .try_claim(&[thread.into()])
            .context("conflict: harness is busy")?;
        let _mutation = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
        let copy = self.worktree_copy(thread, id)?;
        self.db.with(|c| { let tx=c.unchecked_transaction()?; let n=tx.execute("UPDATE threads SET worktree_path=?2,updated_at=?3 WHERE id=?1 AND status NOT IN ('running','recovering') AND NOT EXISTS(SELECT 1 FROM thread_turns WHERE thread_id=?1 AND status IN ('inProgress','recovering')) AND NOT EXISTS(SELECT 1 FROM thread_pending_steers WHERE thread_id=?1)",params![thread,copy.path,now_rfc3339()])?;ensure!(n==1,"conflict: thread has active or queued work");tx.execute("DELETE FROM kv WHERE key=?1",[format!("legacy-worktree-owner:{thread}")])?;tx.commit()?;Ok(()) })?;
        self.emit_worktree_changed(thread);
        Ok(json!({"workingCopy":copy,"sessionCwdUpdated":false,"appliesOnNextSession":true}))
    }
    pub fn worktree_remove(&self, thread: &str, id: &str) -> Result<Value> {
        let _guard = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
        let copy = self.worktree_copy(thread, id)?;
        self.db.with(|c| {let owner:Option<String>=c.query_row("SELECT owner_thread_id FROM working_copies WHERE id=?1",[id],|r|r.get(0))?;
            ensure!(owner.as_deref()==Some(thread),"forbidden: only the creating thread may remove a managed worktree");
            let users:i64=c.query_row("SELECT COUNT(*) FROM thread_working_copies WHERE copy_id=?1 AND thread_id!=?2",params![id,thread],|r|r.get(0))?;
            ensure!(users==0,"conflict: other threads use this worktree");
            let active:i64=c.query_row("SELECT COUNT(*) FROM threads WHERE worktree_path=?1 OR (id=?2 AND (status IN ('running','recovering') OR EXISTS(SELECT 1 FROM thread_turns WHERE thread_id=?2 AND status IN ('inProgress','recovering')) OR EXISTS(SELECT 1 FROM thread_pending_steers WHERE thread_id=?2)))",params![copy.path,thread],|r|r.get(0))?;
            ensure!(active==0,"conflict: worktree is configured or its thread is running");Ok(()) })?;
        let root = Path::new(&copy.path);
        ensure!(
            git(
                root,
                &[
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--untracked-files=all",
                    "--ignored=matching"
                ]
            )?
            .bytes
            .is_empty(),
            "conflict: worktree has uncommitted or untracked files"
        );
        git(
            Path::new(&copy.common_dir),
            &["worktree", "remove", &copy.path],
        )?;
        self.forget_worktree(id, &copy.path)?;
        self.emit_worktree_changed(thread);
        Ok(json!({"removed":true,"workingCopyId":id,"branchDeleted":false}))
    }
    fn selected_copy(&self, thread: &str, id: Option<&str>) -> Result<WorkingCopy> {
        if let Some(id) = id {
            return self.worktree_copy(thread, id);
        }
        let context = self.worktree_context(thread)?;
        self.worktree_copy(
            thread,
            context["primaryWorkingCopyId"]
                .as_str()
                .context("thread directory is not a Git working copy")?,
        )
    }
    pub fn worktree_git_status(&self, thread: &str, id: Option<&str>) -> Result<Value> {
        let copy = self.selected_copy(thread, id)?;
        let out = git(
            Path::new(&copy.path),
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        let mut records = out.bytes.split(|b| *b == 0);
        let mut files = Vec::new();
        while let Some(record) = records.next() {
            if record.len() < 4 {
                continue;
            }
            let x = record[0] as char;
            let y = record[1] as char;
            let path = String::from_utf8_lossy(&record[3..]);
            let old = if x == 'R' || x == 'C' || y == 'R' || y == 'C' {
                records
                    .next()
                    .map(|r| String::from_utf8_lossy(r).into_owned())
            } else {
                None
            };
            files.push(json!({"path":path,"indexStatus":x.to_string(),"worktreeStatus":y.to_string(),"originalPath":old}));
        }
        Ok(
            json!({"workingCopyId":copy.id,"branch":copy.branch,"head":copy.head,"files":files,"truncated":out.truncated}),
        )
    }
    pub fn worktree_git_diff(
        &self,
        thread: &str,
        id: Option<&str>,
        area: &str,
        base: Option<&str>,
        path: Option<&str>,
    ) -> Result<Value> {
        let copy = self.selected_copy(thread, id)?;
        let root = Path::new(&copy.path);
        let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-color"];
        let resolved;
        match area {
            "worktree" => {}
            "index" => args.push("--cached"),
            "branch" => {
                let base = base.unwrap_or("HEAD");
                let commit = git_text(
                    root,
                    &[
                        "rev-parse",
                        "--verify",
                        "--end-of-options",
                        &format!("{base}^{{commit}}"),
                    ],
                )?;
                resolved = git_text(root, &["merge-base", &commit, "HEAD"])?;
                args.push(&resolved);
            }
            _ => bail!("invalid diff area"),
        }
        args.push("--");
        if let Some(path) = path {
            ensure!(
                !Path::new(path).is_absolute()
                    && !Path::new(path).components().any(|p| matches!(
                        p,
                        std::path::Component::ParentDir | std::path::Component::Prefix(_)
                    ))
                    && !path.contains('\0'),
                "path must be relative to the working copy"
            );
            args.push(path);
            if area == "worktree" {
                let names = git(
                    root,
                    &[
                        "ls-files",
                        "--others",
                        "--exclude-standard",
                        "-z",
                        "--",
                        path,
                    ],
                )?;
                if names
                    .bytes
                    .split(|b| *b == 0)
                    .any(|name| name == path.as_bytes())
                {
                    let bytes = if let Some(value) = crate::execution::file_request(
                        "diffBytes",
                        root,
                        path,
                        json!({"limit":OUTPUT_LIMIT}),
                    )? {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD
                            .decode(value["bytes"].as_str().context("invalid diff bytes")?)?
                    } else {
                        let target = root.join(path);
                        let parent = target
                            .parent()
                            .context("file has no parent")?
                            .canonicalize()?;
                        ensure!(
                            parent.starts_with(root),
                            "file resolves outside the working copy"
                        );
                        let metadata = target.symlink_metadata()?;
                        let bytes = if metadata.file_type().is_symlink() {
                            std::fs::read_link(&target)?
                                .to_string_lossy()
                                .as_bytes()
                                .to_vec()
                        } else {
                            ensure!(metadata.is_file(), "diff target is not a regular file");
                            let mut bytes = Vec::new();
                            std::fs::File::open(target)?
                                .take(OUTPUT_LIMIT as u64 + 1)
                                .read_to_end(&mut bytes)?;
                            bytes
                        };
                        bytes
                    };
                    let mut truncated = bytes.len() > OUTPUT_LIMIT;
                    let mut patch = if bytes.contains(&0) {
                        format!("Binary file: {path}\n")
                    } else {
                        let content =
                            String::from_utf8_lossy(&bytes[..bytes.len().min(OUTPUT_LIMIT)]);
                        similar::TextDiff::from_lines("", content.as_ref())
                            .unified_diff()
                            .header("/dev/null", &format!("b/{path}"))
                            .to_string()
                    };
                    if patch.len() > OUTPUT_LIMIT {
                        let mut end = OUTPUT_LIMIT;
                        while !patch.is_char_boundary(end) {
                            end -= 1;
                        }
                        patch.truncate(end);
                        truncated = true;
                    }
                    return Ok(
                        json!({"workingCopyId":copy.id,"area":area,"patch":patch,"truncated":truncated}),
                    );
                }
            }
        }
        let out = git(root, &args)?;
        Ok(
            json!({"workingCopyId":copy.id,"area":area,"patch":String::from_utf8_lossy(&out.bytes),"truncated":out.truncated}),
        )
    }
    pub fn worktree_git_commits(
        &self,
        thread: &str,
        id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Value> {
        let copy = self.selected_copy(thread, id)?;
        let limit = limit.clamp(1, 100);
        let offset = offset.min(100_000);
        let out = git(
            Path::new(&copy.path),
            &[
                "log",
                "-z",
                "--format=%H%x00%s%x00%an%x00%aI",
                &format!("--max-count={}", limit + 1),
                &format!("--skip={offset}"),
            ],
        )?;
        let fields: Vec<_> = out.bytes.split(|b| *b == 0).collect();
        let mut commits = Vec::new();
        for row in fields.chunks(4) {
            if row.len() != 4 || row[0].is_empty() {
                continue;
            }
            commits.push(json!({"oid":String::from_utf8_lossy(row[0]),"subject":String::from_utf8_lossy(row[1]),"author":String::from_utf8_lossy(row[2]),"authoredAt":String::from_utf8_lossy(row[3])}));
        }
        let more = commits.len() > limit || out.truncated;
        commits.truncate(limit);
        Ok(json!({"commits":commits,"hasMore":more}))
    }
}

#[cfg(test)]
mod tests;
