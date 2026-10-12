//! Coordination over a lineage: names, roles, worktrees, close, tree, wait and wake.
//! Everything here is opt-in on top of the passive inbox; nothing starts a turn
//! except a wake the caller explicitly registered for its own delegates.
use super::{closing_message, enqueue};
use crate::Supervisor;
use anyhow::{anyhow, bail, ensure, Result};
use pockymoe_protocol::{now_rfc3339, ThreadEventEnvelope};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Options only an agent-created thread can carry.
#[derive(Default)]
pub struct AgentOptions {
    pub name: Option<String>,
    pub role: Option<RoleTemplate>,
    /// `Some(branch)` asks for an isolated git worktree; `Some(None)` names the
    /// branch after the agent.
    pub worktree: Option<Option<String>>,
}

pub struct RoleTemplate {
    pub name: String,
    pub description: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub agent: Option<String>,
    pub instructions: String,
}

pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

/// Aliases resolved relative to the caller, so they cannot also be names.
const ALIASES: [&str; 3] = ["self", "parent", "root"];
const MAX_WAIT: Duration = Duration::from_secs(1800);
const MAX_PENDING_WAKES: usize = 3;
const WAKE_EXCERPT: usize = 1500;

pub fn validate_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    ensure!(
        chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && name.len() <= 32
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'),
        "name must match [a-z][a-z0-9_-]{{0,31}}"
    );
    ensure!(!ALIASES.contains(&name), "`{name}` is reserved");
    Ok(())
}

/// `.pockymoe/agents`, then `.remote-codex/agents` from before the rename, in
/// the workspace and then the home directory; the first definition of a name wins.
fn role_dirs(workspace: &Path) -> Vec<PathBuf> {
    let mut roots = vec![workspace.to_path_buf()];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home));
    }
    roots
        .iter()
        .flat_map(|root| {
            [
                root.join(".pockymoe/agents"),
                root.join(".remote-codex/agents"),
            ]
        })
        .collect()
}

/// `.pockymoe/agents/NAME.md` (or the pre-rename `.remote-codex/agents`) in the
/// workspace, then in the home directory.
/// Optional front matter sets `description`, `model`, `effort` and `agent`; the body
/// is prepended to the first prompt the thread runs.
pub fn load_role(workspace: &Path, name: &str) -> Result<RoleTemplate> {
    validate_name(name).map_err(|_| anyhow!("invalid role name `{name}`"))?;
    let text = role_dirs(workspace)
        .iter()
        .find_map(|dir| std::fs::read_to_string(dir.join(format!("{name}.md"))).ok())
        .ok_or_else(|| {
            anyhow!("role `{name}` not found; define .pockymoe/agents/{name}.md in the workspace or home directory, or list roles with `pockymoe thread roles`")
        })?;
    Ok(parse_role(name, &text))
}

fn parse_role(name: &str, text: &str) -> RoleTemplate {
    let mut role = RoleTemplate {
        name: name.into(),
        description: None,
        model: None,
        reasoning_effort: None,
        agent: None,
        instructions: text.trim().into(),
    };
    let Some(rest) = text.strip_prefix("---\n") else {
        return role;
    };
    let Some((head, body)) = rest.split_once("\n---") else {
        return role;
    };
    for line in head.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = Some(value.trim().trim_matches('"').to_string()).filter(|v| !v.is_empty());
        match key.trim() {
            "description" => role.description = value,
            "model" => role.model = value,
            "effort" | "reasoning_effort" | "reasoningEffort" => role.reasoning_effort = value,
            "agent" => role.agent = value,
            _ => {}
        }
    }
    role.instructions = body.trim().into();
    role
}

pub fn list_roles(workspace: &Path) -> Value {
    let mut seen = std::collections::BTreeMap::new();
    for dir in role_dirs(workspace) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|_| path.extension().is_some_and(|e| e == "md"))
            else {
                continue;
            };
            if seen.contains_key(name) || validate_name(name).is_err() {
                continue;
            }
            let role = parse_role(name, &std::fs::read_to_string(&path).unwrap_or_default());
            seen.insert(
                name.to_string(),
                json!({"name":name,"description":role.description,"model":role.model,"reasoningEffort":role.reasoning_effort,"agent":role.agent,"path":path}),
            );
        }
    }
    json!({"roles":seen.into_values().collect::<Vec<_>>()})
}

pub(crate) fn stage_role(conn: &Connection, thread: &str, role: &RoleTemplate) -> Result<()> {
    if role.instructions.is_empty() {
        return Ok(());
    }
    conn.execute(
        "INSERT OR REPLACE INTO kv(key,value) VALUES(?1,?2)",
        params![
            format!("cli:role:{thread}"),
            format!("[Pockymoe role: {}]\n{}", role.name, role.instructions)
        ],
    )?;
    Ok(())
}

pub(crate) fn take_role(state: &Supervisor, thread: &str) -> Option<String> {
    state
        .db
        .with(|c| {
            let key = format!("cli:role:{thread}");
            let text: Option<String> = c
                .query_row("SELECT value FROM kv WHERE key=?1", [&key], |r| r.get(0))
                .optional()?;
            c.execute("DELETE FROM kv WHERE key=?1", [&key])?;
            Ok(text)
        })
        .ok()
        .flatten()
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    ensure!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A sibling `REPO.worktrees/SLUG` checkout of the workspace's HEAD. Sibling, not
/// nested, so the main checkout's own tooling never walks into it.
pub fn create_worktree(workspace: &Path, slug: &str, branch: Option<&str>) -> Result<Worktree> {
    let top = PathBuf::from(
        git(workspace, &["rev-parse", "--show-toplevel"])
            .map_err(|_| anyhow!("--worktree requires the workspace to be a git repository"))?,
    );
    let repo = top
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("workspace has no directory name"))?;
    let path = top
        .parent()
        .ok_or_else(|| anyhow!("workspace has no parent directory"))?
        .join(format!("{repo}.worktrees"))
        .join(slug);
    ensure!(
        !path.exists(),
        "worktree path {} already exists; pick another --name or remove it",
        path.display()
    );
    let exists = |b: &str| {
        git(
            &top,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{b}"),
            ],
        )
        .is_ok()
    };
    // A default branch left by an earlier delegate of the same name holds stale
    // work; start fresh rather than silently checking it out. An explicit branch
    // is checked out as is.
    let branch = match branch {
        Some(b) => b.to_owned(),
        None => {
            let mut candidate = format!("agent/{slug}");
            let mut n = 2;
            while exists(&candidate) {
                candidate = format!("agent/{slug}-{n}");
                n += 1;
            }
            candidate
        }
    };
    let target = path.to_string_lossy().into_owned();
    if exists(&branch) {
        git(&top, &["worktree", "add", &target, &branch])?;
    } else {
        git(&top, &["worktree", "add", "-b", &branch, &target, "HEAD"])?;
    }
    Ok(Worktree { path, branch })
}

pub fn remove_worktree(workspace: &Path, path: &Path) -> Result<()> {
    git(workspace, &["worktree", "remove", &path.to_string_lossy()])?;
    Ok(())
}

fn is_descendant(conn: &Connection, ancestor: &str, id: &str) -> Result<bool> {
    let mut current = id.to_string();
    for _ in 0..=crate::service::MAX_LINEAGE_DEPTH {
        let parent: Option<String> = conn
            .query_row(
                "SELECT parent_thread_id FROM threads WHERE id=?1",
                [&current],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        match parent {
            Some(p) if p == ancestor => return Ok(true),
            Some(p) => current = p,
            None => return Ok(false),
        }
    }
    Ok(false)
}

/// Waits for the next thread update, or forever if the bus is gone (the caller's
/// timer still ends the wait).
async fn next_update(events: &mut broadcast::Receiver<ThreadEventEnvelope>) {
    loop {
        match events.recv().await {
            Ok(event) if event.event_type == "thread.updated" => return,
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => return,
            Err(broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
        }
    }
}

pub(crate) async fn pause(
    events: &mut broadcast::Receiver<ThreadEventEnvelope>,
    deadline: Instant,
) -> bool {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return false;
    }
    tokio::select! {
        _ = next_update(events) => {}
        _ = tokio::time::sleep(left.min(Duration::from_secs(1))) => {}
    }
    true
}

pub fn clamp_wait(seconds: Option<u64>) -> Duration {
    Duration::from_secs(seconds.unwrap_or(300)).min(MAX_WAIT)
}

impl Supervisor {
    pub(super) fn root_of(&self, thread: &str) -> Result<String> {
        let t = self.get_thread(thread)?;
        Ok(t.root_thread_id.unwrap_or(t.id))
    }

    pub(crate) fn ensure_agent_name_free(&self, root: Option<&str>, name: &str) -> Result<()> {
        validate_name(name)?;
        let Some(root) = root else { return Ok(()) };
        let taken = self.db.with(|c| {
            Ok(c.query_row(
                "SELECT 1 FROM threads WHERE root_thread_id=?1 AND agent_name=?2 AND closed_at IS NULL",
                params![root, name],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
        })?;
        ensure!(
            !taken,
            "conflict: an open thread in this lineage is already named `{name}`; address it by name, or close it first"
        );
        Ok(())
    }

    /// UUID, `self`, `parent`, `root`, or the name of an open thread in the
    /// caller's lineage.
    pub fn resolve_agent(&self, caller: Option<&str>, target: &str) -> Result<String> {
        if let Ok(id) = Uuid::parse_str(target) {
            return Ok(id.to_string());
        }
        let caller = caller.ok_or_else(|| {
            anyhow!("`{target}` is not a thread id; names resolve only from a managed thread (or --from ID)")
        })?;
        let me = self.get_thread(caller)?;
        match target {
            "self" => return Ok(me.id),
            "parent" => {
                return me
                    .parent_thread_id
                    .ok_or_else(|| anyhow!("this thread has no parent"))
            }
            "root" => return Ok(me.root_thread_id.unwrap_or(me.id)),
            _ => {}
        }
        let root = me.root_thread_id.unwrap_or(me.id);
        self.db.with(|c| {
            if let Some(id) = c
                .query_row(
                    "SELECT id FROM threads WHERE root_thread_id=?1 AND agent_name=?2 AND closed_at IS NULL",
                    params![root, target],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                return Ok(id);
            }
            let mut stmt = c.prepare(
                "SELECT agent_name FROM threads WHERE root_thread_id=?1 AND agent_name IS NOT NULL AND closed_at IS NULL ORDER BY created_at",
            )?;
            let names = stmt
                .query_map([&root], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            bail!(
                "no open thread named `{target}` in this lineage (open names: {})",
                if names.is_empty() { "none".into() } else { names.join(", ") }
            )
        })
    }

    /// One thread's coordination state. `queued` and `running` are unsettled;
    /// `blocked` means a running turn waits on an approval, or on an answer to a
    /// `question` it mailed that nobody has acknowledged or replied to.
    async fn member_state(&self, id: &str) -> Result<Value> {
        let (row, last, questions) = self.db.with(|c| {
            let row: (String, Option<String>, Option<String>, String, i64, i64, String) = c.query_row(
                "SELECT COALESCE(t.status,'idle'), t.agent_name, t.closed_at, t.title,
                        (SELECT COUNT(*) FROM thread_turns WHERE thread_id=t.id AND status='inProgress'),
                        (SELECT COUNT(*) FROM thread_pending_steers WHERE thread_id=t.id AND delivery='continuation'),
                        t.provider
                 FROM threads t WHERE t.id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            ).optional()?.ok_or_else(|| anyhow!("thread {id} not found"))?;
            let last: Option<(String, String, Option<String>)> = c
                .query_row(
                    "SELECT id,status,completed_at FROM thread_turns WHERE thread_id=?1 ORDER BY ordinal DESC LIMIT 1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let last = match last {
                Some((turn, status, done)) => {
                    let message = if status == "inProgress" { None } else { closing_message(c, id, &turn)? };
                    Some((turn, status, done, message))
                }
                None => None,
            };
            // A passive question is invisible to the harness, but it is exactly the
            // moment a waiting parent must act, so surface it as blocking.
            let mut stmt = c.prepare(
                "SELECT json_extract(q.value,'$.id'), COALESCE(json_extract(q.value,'$.subject'),''), json_extract(q.value,'$.threadId')
                 FROM kv q WHERE q.key GLOB 'cli:inbox:*'
                   AND json_extract(q.value,'$.fromThreadId')=?1
                   AND json_extract(q.value,'$.kind')='question'
                   AND json_extract(q.value,'$.acknowledgedAt') IS NULL
                   AND NOT EXISTS (SELECT 1 FROM kv r WHERE r.key GLOB 'cli:inbox:' || ?1 || ':*'
                                     AND json_extract(r.value,'$.inReplyTo')=json_extract(q.value,'$.id'))",
            )?;
            let questions = stmt
                .query_map([id], |r| {
                    Ok(format!(
                        "question {} to {}: {}",
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(1)?
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok((row, last, questions))
        })?;
        let (status, name, closed, title, active, queued, provider) = row;
        let mut state = if closed.is_some() {
            "closed".to_string()
        } else if active > 0 || status == "running" {
            "running".into()
        } else if queued > 0 && status != "recovering" {
            "queued".into()
        } else {
            status
        };
        let mut waiting_on = Vec::new();
        if state == "running" {
            let provider = serde_json::from_value(json!(provider))?;
            if let Ok(runtime) = self.runtime(provider) {
                waiting_on = runtime
                    .pending_requests(id)
                    .await
                    .into_iter()
                    .map(|r| r.title)
                    .collect();
            }
            waiting_on.extend(questions);
            if !waiting_on.is_empty() {
                state = "blocked".into();
            }
        }
        let settled = !matches!(state.as_str(), "running" | "queued");
        Ok(json!({
            "threadId": id,
            "name": name,
            "title": title,
            "state": state,
            "settled": settled,
            "waitingOn": if waiting_on.is_empty() { Value::Null } else { json!(waiting_on) },
            "lastTurn": last.map(|(turn, status, done, message)| json!({
                "turnId": turn, "status": status, "completedAt": done,
                "closingMessage": message.map(|m| clip(&m, 4000)),
            })),
        }))
    }

    /// Blocks until the threads settle (all, or any), one blocks on approval, or the
    /// timeout passes. Lets an orchestrator stay in its turn without polling.
    pub async fn wait_threads(
        &self,
        ids: &[String],
        any: bool,
        timeout: Duration,
    ) -> Result<Value> {
        ensure!(
            !ids.is_empty() && ids.len() <= 20,
            "wait for 1 to 20 threads"
        );
        let started = Instant::now();
        let deadline = started + timeout;
        let mut events = self.bus.subscribe();
        loop {
            let mut states = Vec::new();
            for id in ids {
                states.push(self.member_state(id).await?);
            }
            let settled = states.iter().filter(|s| s["settled"] == true).count();
            let blocked = states.iter().any(|s| s["state"] == "blocked");
            let done = if any {
                settled > 0
            } else {
                settled == ids.len()
            };
            if done || blocked || !pause(&mut events, deadline).await {
                return Ok(json!({
                    "done": done,
                    "allSettled": settled == ids.len(),
                    "blocked": blocked,
                    "timedOut": !done && !blocked,
                    "waitedSeconds": started.elapsed().as_secs(),
                    "threads": states,
                    "next": if blocked {
                        "A delegate is blocked: answer its question (reply with --in-reply-to, or acknowledge it) or resolve its approval, then wait again."
                    } else if done {
                        "Read each settled thread's closingMessage; check artifacts before acting on them. Close delegates you no longer need with `pockymoe thread close NAME`."
                    } else {
                        "Still running. Wait again, or continue other work and check `pockymoe thread tree`."
                    },
                }));
            }
        }
    }

    /// Lineage overview: every thread under the root with its state, unread mail,
    /// current task and worktree. Closed threads are hidden unless `all`.
    pub async fn agent_tree(&self, root: &str, all: bool) -> Result<Value> {
        let rows: Vec<(String, Option<String>, i64, Option<String>, Option<String>, Option<String>, String)> =
            self.db.with(|c| {
                let mut stmt = c.prepare(
                    "SELECT id, parent_thread_id, lineage_depth, agent_role, worktree_path, closed_at, updated_at
                     FROM threads WHERE id=?1 OR root_thread_id=?1 ORDER BY created_at",
                )?;
                let rows = stmt
                    .query_map([root], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            })?;
        ensure!(!rows.is_empty(), "thread not found");
        // Depth-first so children print under their parent.
        let mut ordered = Vec::new();
        let mut stack = vec![rows[0].0.clone()];
        while let Some(id) = stack.pop() {
            ordered.push(id.clone());
            for row in rows.iter().rev().filter(|r| r.1.as_deref() == Some(&id)) {
                stack.push(row.0.clone());
            }
        }
        let tasks = self.task_counts(root)?;
        let mut threads = Vec::new();
        for id in ordered {
            let row = rows.iter().find(|r| r.0 == id).unwrap();
            if row.5.is_some() && !all {
                continue;
            }
            let mut state = self.member_state(&id).await?;
            if let Some(turn) = state["lastTurn"].as_object_mut() {
                turn.remove("closingMessage");
            }
            state["depth"] = json!(row.2);
            state["parentThreadId"] = json!(row.1);
            state["role"] = json!(row.3);
            state["worktreePath"] = json!(row.4);
            state["updatedAt"] = json!(row.6);
            state["unreadMessages"] = json!(self.inbox_unread_count(&id)?);
            state["currentTask"] = self.current_task(root, &id)?;
            threads.push(state);
        }
        Ok(json!({"rootThreadId": root, "threads": threads, "tasks": tasks}))
    }

    pub async fn lineage_tree(&self, caller: &str, all: bool) -> Result<Value> {
        let root = self.root_of(caller)?;
        self.agent_tree(&root, all).await
    }

    /// Closes a delegate: frees its slot and name, optionally removing its worktree.
    /// History is kept, and prompting it again reopens it.
    pub fn close_agent_thread(
        &self,
        caller: Option<&str>,
        id: &str,
        drop_worktree: bool,
    ) -> Result<Value> {
        let thread = self.get_thread(id)?;
        ensure!(
            thread.parent_thread_id.is_some(),
            "only agent-created threads can be closed"
        );
        if let Some(caller) = caller.filter(|c| *c != id) {
            ensure!(
                self.db.with(|c| is_descendant(c, caller, id))?,
                "only an ancestor of a thread (or the thread itself) can close it"
            );
        }
        let queued: i64 = self.db.with(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM thread_pending_steers WHERE thread_id=?1",
                [id],
                |r| r.get(0),
            )?)
        })?;
        ensure!(
            thread.status != "running" && thread.active_turn_id.is_none() && queued == 0,
            "conflict: thread is still running or has queued work; wait for it (`pockymoe thread wait {id}`) before closing"
        );
        let mut removed = false;
        if drop_worktree {
            if let Some(path) = &thread.worktree_path {
                let workspace = self.get_workspace(&thread.workspace_id)?;
                remove_worktree(Path::new(&workspace.abs_path), Path::new(path)).map_err(|e| {
                    anyhow!("{e}. Commit or discard the worktree's changes first; the thread was not closed")
                })?;
                removed = true;
            }
        }
        let now = now_rfc3339();
        self.db.with(|c| {
            c.execute(
                "UPDATE threads SET closed_at=?2, updated_at=?2, worktree_path=CASE WHEN ?3 THEN NULL ELSE worktree_path END WHERE id=?1",
                params![id, now, removed],
            )?;
            Ok(())
        })?;
        self.bus.emit(ThreadEventEnvelope {
            event_type: "thread.updated".into(),
            thread_id: id.into(),
            timestamp: now.clone(),
            payload: json!({"reason":"closed"}),
        });
        Ok(
            json!({"threadId":id,"name":thread.agent_name,"closedAt":now,"worktreeRemoved":removed,"worktreePath":if removed {None} else {thread.worktree_path}}),
        )
    }

    /// One-shot: when every listed delegate settles (or one blocks), queue a single
    /// turn on the caller carrying their outcomes. The only path by which delegates
    /// wake their parent, and only because the parent asked.
    pub fn register_wake(&self, caller: &str, ids: &[String]) -> Result<Value> {
        ensure!(
            !ids.is_empty() && ids.len() <= 20,
            "wake on 1 to 20 threads"
        );
        let caller_thread = self.get_thread(caller)?;
        self.ensure_prompt_allowed(&caller_thread)?;
        let id = Uuid::new_v4().to_string();
        self.db.with(|c| {
            for member in ids {
                ensure!(
                    is_descendant(c, caller, member)?,
                    "you can only wake on your own delegates; {member} is not one"
                );
            }
            let pending: i64 = c.query_row(
                "SELECT COUNT(*) FROM kv WHERE key GLOB ?1",
                [format!("cli:wake:{caller}:*")],
                |r| r.get(0),
            )?;
            ensure!(
                (pending as usize) < MAX_PENDING_WAKES,
                "you already have {pending} pending wakes; that is the limit"
            );
            c.execute(
                "INSERT INTO kv(key,value) VALUES(?1,?2)",
                params![
                    format!("cli:wake:{caller}:{id}"),
                    json!({"members":ids,"createdAt":now_rfc3339(),"taskTurnId":caller_thread.active_turn_id.as_deref().map(|turn| crate::service::peer_work::root_turn(c, caller, turn)).transpose()?}).to_string()
                ],
            )?;
            Ok(())
        })?;
        Ok(
            json!({"wakeId":id,"threadId":caller,"members":ids,"behavior":"One turn will be queued on you when all of these settle or one blocks. You may end your turn now."}),
        )
    }

    pub(super) async fn fire_wakes(&self) -> Result<()> {
        let wakes: Vec<(String, String)> = self.db.with(|c| {
            let mut stmt = c.prepare("SELECT key,value FROM kv WHERE key GLOB 'cli:wake:*'")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        for (key, raw) in wakes {
            let caller = key.split(':').nth(2).unwrap_or_default().to_string();
            let registered: Value = serde_json::from_str(&raw)?;
            let members: Vec<String> = serde_json::from_str::<Value>(&raw)
                .ok()
                .and_then(|v| serde_json::from_value(v["members"].clone()).ok())
                .unwrap_or_default();
            let mut states = Vec::new();
            for member in &members {
                if let Ok(state) = self.member_state(member).await {
                    states.push(state);
                }
            }
            let ready = states.iter().all(|s| s["settled"] == true)
                || states.iter().any(|s| s["state"] == "blocked");
            if !ready {
                continue;
            }
            let lines = states
                .iter()
                .map(|s| {
                    let label = s["name"].as_str().map_or_else(
                        || s["threadId"].as_str().unwrap_or("").to_string(),
                        |n| format!("{n} ({})", s["threadId"].as_str().unwrap_or("")),
                    );
                    let mut line = format!("- {label}: {}", s["state"].as_str().unwrap_or(""));
                    if let Some(waiting) = s["waitingOn"].as_array() {
                        line.push_str(&format!(", waiting on: {waiting:?}"));
                    }
                    if let Some(message) = s["lastTurn"]["closingMessage"].as_str() {
                        line.push_str(&format!(
                            "\n  closing message: {}",
                            clip(message, WAKE_EXCERPT)
                        ));
                    }
                    line
                })
                .collect::<Vec<_>>()
                .join("\n");
            let text = format!(
                "[Pockymoe wake: the delegates you registered a wake for have settled]\n{lines}\n\nVerify results before acting on them. Full transcripts: pockymoe transcript ID --limit 1"
            );
            let now = now_rfc3339();
            self.db.with(|c| {
                let tx = c.unchecked_transaction()?;
                let exists = tx
                    .query_row("SELECT 1 FROM threads WHERE id=?1", [&caller], |_| Ok(()))
                    .optional()?
                    .is_some();
                if exists {
                    let pending = Uuid::new_v4().to_string();
                    enqueue(&tx, &pending, &caller, &text, None, &now)?;
                    if let Some(task) = registered["taskTurnId"].as_str() {
                        if let Some(context) =
                            crate::service::peer_work::context(&tx, &caller, task, &text)?
                        {
                            tx.execute(
                                "INSERT INTO kv(key,value) VALUES(?1,?2)",
                                params![
                                    format!("cli:peer-work:pending:{pending}"),
                                    context.to_string()
                                ],
                            )?;
                        }
                    }
                }
                tx.execute("DELETE FROM kv WHERE key=?1", [&key])?;
                tx.commit()?;
                Ok(())
            })?;
            self.bus.emit(ThreadEventEnvelope {
                event_type: "thread.updated".into(),
                thread_id: caller,
                timestamp: now,
                payload: json!({"reason":"pending_steer_updated"}),
            });
        }
        Ok(())
    }
}

pub(crate) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.into();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}… [truncated]")
}
