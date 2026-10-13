//! A task board per lineage root. Work lives here instead of in one agent's context,
//! so it survives that agent ending, and dependencies unblock without anyone polling.
//! Claims are transactional, so two delegates can never take the same task.
use super::inbox;
use crate::Supervisor;
use anyhow::{anyhow, bail, ensure, Result};
use pockymoe_protocol::{now_rfc3339, ThreadEventEnvelope};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use uuid::Uuid;

const MAX_TASKS: i64 = 200;

fn deps(conn: &Connection, root: &str, number: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT depends_on FROM agent_task_deps WHERE root_thread_id=?1 AND number=?2 ORDER BY depends_on",
    )?;
    let rows = stmt
        .query_map(params![root, number], |r| r.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn blocked_by(conn: &Connection, root: &str, number: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT d.depends_on FROM agent_task_deps d JOIN agent_tasks t
           ON t.root_thread_id=d.root_thread_id AND t.number=d.depends_on
         WHERE d.root_thread_id=?1 AND d.number=?2 AND t.status<>'completed' ORDER BY d.depends_on",
    )?;
    let rows = stmt
        .query_map(params![root, number], |r| r.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn name_of(conn: &Connection, thread: Option<&str>) -> Result<Value> {
    let Some(thread) = thread else {
        return Ok(Value::Null);
    };
    let name: Option<String> = conn
        .query_row(
            "SELECT agent_name FROM threads WHERE id=?1",
            [thread],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(json!(name.unwrap_or_else(|| thread.to_string())))
}

fn task_json(conn: &Connection, root: &str, number: i64, with_detail: bool) -> Result<Value> {
    let (title, detail, status, owner, creator, result, updated): (
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    ) = conn
        .query_row(
            "SELECT title,detail,status,owner_thread_id,created_by,result,updated_at FROM agent_tasks WHERE root_thread_id=?1 AND number=?2",
            params![root, number],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .optional()?
        .ok_or_else(|| anyhow!("task #{number} not found in this lineage"))?;
    let blocked = blocked_by(conn, root, number)?;
    let mut value = json!({
        "number": number,
        "title": title,
        "status": status,
        "owner": name_of(conn, owner.as_deref())?,
        "ownerThreadId": owner,
        "createdBy": name_of(conn, creator.as_deref())?,
        "dependsOn": deps(conn, root, number)?,
        "blockedBy": blocked,
        "ready": status == "pending" && blocked.is_empty(),
        "updatedAt": updated,
    });
    if with_detail {
        value["detail"] = json!(detail);
        value["result"] = json!(result);
    } else {
        value["hasDetail"] = json!(detail.is_some());
        value["hasResult"] = json!(result.is_some());
    }
    Ok(value)
}

fn notify(
    conn: &Connection,
    to: &str,
    from: &str,
    kind: &str,
    subject: &str,
    text: &str,
) -> Result<()> {
    if to == from {
        return Ok(());
    }
    inbox::store(
        conn,
        to,
        &Uuid::new_v4().to_string(),
        Some(from),
        text,
        &now_rfc3339(),
        inbox::Envelope {
            subject: Some(subject),
            kind,
            in_reply_to: None,
            topic_key: None,
        },
    )
}

impl Supervisor {
    fn task_scope(&self, caller: &str) -> Result<(String, String)> {
        let thread = self.get_thread(caller)?;
        Ok((
            thread.root_thread_id.unwrap_or(thread.id.clone()),
            thread.id,
        ))
    }

    fn touched(&self, threads: impl IntoIterator<Item = String>) {
        for thread in threads {
            self.bus.emit(ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
                thread_id: thread,
                timestamp: now_rfc3339(),
                payload: json!({"reason":"task_updated"}),
            });
        }
    }

    pub(crate) fn task_counts(&self, root: &str) -> Result<Value> {
        self.db.with(|c| {
            let mut stmt = c.prepare(
                "SELECT status, COUNT(*) FROM agent_tasks WHERE root_thread_id=?1 GROUP BY status",
            )?;
            let mut counts = serde_json::Map::new();
            for row in stmt.query_map([root], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })? {
                let (status, count) = row?;
                counts.insert(status, json!(count));
            }
            Ok(Value::Object(counts))
        })
    }

    pub(crate) fn current_task(&self, root: &str, thread: &str) -> Result<Value> {
        self.db.with(|c| {
            Ok(c.query_row(
                "SELECT number,title FROM agent_tasks WHERE root_thread_id=?1 AND owner_thread_id=?2 AND status='in_progress' ORDER BY number LIMIT 1",
                params![root, thread],
                |r| Ok(json!({"number":r.get::<_, i64>(0)?,"title":r.get::<_, String>(1)?})),
            )
            .optional()?
            .unwrap_or(Value::Null))
        })
    }

    pub fn task_add(
        &self,
        caller: &str,
        title: &str,
        detail: Option<&str>,
        after: &[i64],
        assign: Option<&str>,
    ) -> Result<Value> {
        let (root, me) = self.task_scope(caller)?;
        ensure!(
            !title.trim().is_empty() && title.chars().count() <= 120,
            "title must be nonempty and at most 120 characters"
        );
        ensure!(
            detail.is_none_or(|d| d.len() <= 64 * 1024),
            "detail must be at most 64 KiB"
        );
        if let Some(owner) = assign {
            ensure!(
                self.root_of(owner)? == root,
                "can only assign to a thread in this lineage"
            );
        }
        let number = self.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            let number: i64 = tx.query_row(
                "SELECT COALESCE(MAX(number),0)+1 FROM agent_tasks WHERE root_thread_id=?1",
                [&root],
                |r| r.get(0),
            )?;
            ensure!(number <= MAX_TASKS, "this lineage already has {MAX_TASKS} tasks");
            let now = now_rfc3339();
            tx.execute(
                "INSERT INTO agent_tasks(root_thread_id,number,title,detail,status,owner_thread_id,created_by,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,'pending',?5,?6,?7,?7)",
                params![root, number, title.trim(), detail, assign, me, now],
            )?;
            for dep in after {
                ensure!(*dep < number && *dep > 0, "dependency #{dep} does not exist");
                tx.execute(
                    "INSERT OR IGNORE INTO agent_task_deps(root_thread_id,number,depends_on) VALUES(?1,?2,?3)",
                    params![root, number, dep],
                )?;
            }
            if let Some(owner) = assign {
                let ready = blocked_by(&tx, &root, number)?.is_empty();
                notify(
                    &tx,
                    owner,
                    &me,
                    "task",
                    &format!("Task #{number} assigned: {}", title.trim()),
                    &format!(
                        "Task #{number} \"{}\" is assigned to you{}. Claim it with `pockymoe task claim {number}`, read it with `pockymoe task show {number}`, and finish with `pockymoe task done {number} --result ...`.",
                        title.trim(),
                        if ready { " and ready" } else { "; it is waiting on its dependencies and you will be told when it is ready" }
                    ),
                )?;
            }
            tx.commit()?;
            Ok(number)
        })?;
        self.touched(assign.map(str::to_owned));
        self.db.with(|c| task_json(c, &root, number, false))
    }

    pub fn task_list(&self, caller: &str, all: bool) -> Result<Value> {
        let (root, _) = self.task_scope(caller)?;
        self.db.with(|c| {
            let mut stmt = c.prepare(
                "SELECT number FROM agent_tasks WHERE root_thread_id=?1 AND (?2 OR status NOT IN ('completed','failed')) ORDER BY number",
            )?;
            let numbers = stmt
                .query_map(params![root, all], |r| r.get::<_, i64>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let tasks = numbers
                .into_iter()
                .map(|n| task_json(c, &root, n, false))
                .collect::<Result<Vec<_>>>()?;
            Ok(json!({"rootThreadId":root,"tasks":tasks}))
        })
    }

    pub fn task_show(&self, caller: &str, number: i64) -> Result<Value> {
        let (root, _) = self.task_scope(caller)?;
        self.db.with(|c| task_json(c, &root, number, true))
    }

    /// Claims the given task, or the lowest ready one assigned to the caller or to
    /// nobody. Returns `claimed: null` with a reason when nothing is ready.
    pub fn task_claim(&self, caller: &str, number: Option<i64>) -> Result<Value> {
        let (root, me) = self.task_scope(caller)?;
        let claimed = self.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            let candidates: Vec<(i64, Option<String>, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT number, owner_thread_id, status FROM agent_tasks WHERE root_thread_id=?1 AND (?2 IS NULL OR number=?2) ORDER BY number",
                )?;
                let rows = stmt
                    .query_map(params![root, number], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                rows
            };
            if let Some(n) = number {
                let (_, owner, status) = candidates
                    .first()
                    .ok_or_else(|| anyhow!("task #{n} not found in this lineage"))?;
                if status == "in_progress" && owner.as_deref() == Some(me.as_str()) {
                    return Ok(Some(n));
                }
                ensure!(status == "pending", "conflict: task #{n} is {status}");
                ensure!(
                    owner.as_deref().is_none_or(|o| o == me),
                    "conflict: task #{n} is assigned to another thread"
                );
                let blocked = blocked_by(&tx, &root, n)?;
                ensure!(
                    blocked.is_empty(),
                    "conflict: task #{n} is waiting on {blocked:?}; claim it after they complete"
                );
            }
            let mut pick = None;
            for (n, owner, status) in &candidates {
                if status == "pending"
                    && owner.as_deref().is_none_or(|o| o == me)
                    && blocked_by(&tx, &root, *n)?.is_empty()
                {
                    pick = Some(*n);
                    break;
                }
            }
            let Some(n) = pick else {
                return Ok(None);
            };
            // The status guard makes a concurrent claim of the same task a no-op.
            let changed = tx.execute(
                "UPDATE agent_tasks SET status='in_progress', owner_thread_id=?3, updated_at=?4
                 WHERE root_thread_id=?1 AND number=?2 AND status='pending'",
                params![root, n, me, now_rfc3339()],
            )?;
            if changed != 1 {
                bail!("conflict: task #{n} was claimed concurrently; claim again");
            }
            tx.commit()?;
            Ok(Some(n))
        })?;
        match claimed {
            Some(n) => Ok(json!({"claimed": self.db.with(|c| task_json(c, &root, n, true))?})),
            None => {
                let counts = self.task_counts(&root)?;
                let count = |k: &str| counts[k].as_i64().unwrap_or(0);
                let (pending, running) = (count("pending"), count("in_progress"));
                // `claimed: null` alone reads as "board finished"; say which it is,
                // because a worker that stops while work is merely blocked idles
                // through the rest of the run.
                let (finished, reason) = if pending == 0 && running == 0 {
                    (true, "Board finished: no pending or in-progress tasks.")
                } else if pending == 0 {
                    (
                        true,
                        "Nothing left to claim; the remaining tasks are in progress under others.",
                    )
                } else if running == 0 {
                    (true, "Pending tasks remain but nothing in progress can unblock them (a dependency failed or they are assigned elsewhere).")
                } else {
                    (false, "Pending tasks are blocked on work in progress. Use `task claim --wait` to block until one becomes ready.")
                };
                // `finished`: nothing left this worker could ever claim, so stop.
                // `boardComplete`: no task anywhere is still pending or in progress.
                Ok(json!({
                    "claimed": null,
                    "finished": finished,
                    "boardComplete": pending == 0 && running == 0,
                    "reason": reason,
                    "tasks": counts,
                }))
            }
        }
    }

    /// `task claim` that blocks while pending work is only blocked on in-progress
    /// tasks, so a worker stays available across dependency barriers.
    pub async fn task_claim_wait(
        &self,
        caller: &str,
        number: Option<i64>,
        timeout: std::time::Duration,
    ) -> Result<Value> {
        let started = std::time::Instant::now();
        let deadline = started + timeout;
        let mut events = self.bus.subscribe();
        loop {
            let attempt = match self.task_claim(caller, number) {
                // A named task that is still waiting on dependencies is worth waiting for.
                Err(e) if number.is_some() && e.to_string().contains("is waiting on") => {
                    json!({"claimed": null, "finished": false, "reason": e.to_string()})
                }
                other => other?,
            };
            let ready = !attempt["claimed"].is_null() || attempt["finished"] == true;
            if ready || !super::agents::pause(&mut events, deadline).await {
                let mut attempt = attempt;
                attempt["timedOut"] = json!(!ready);
                attempt["waitedSeconds"] = json!(started.elapsed().as_secs());
                return Ok(attempt);
            }
        }
    }

    /// Completes (or fails) a task. The creator gets passive mail with the result,
    /// and owners of tasks this unblocks are told they are ready.
    pub fn task_done(
        &self,
        caller: &str,
        number: i64,
        result: Option<&str>,
        failed: bool,
    ) -> Result<Value> {
        let (root, me) = self.task_scope(caller)?;
        ensure!(
            result.is_none_or(|r| r.len() <= 16 * 1024),
            "result must be at most 16 KiB; put long output in a file and reference it"
        );
        let mut touched = Vec::new();
        let unblocked = self.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            let (title, status, owner, creator): (String, String, Option<String>, Option<String>) = tx
                .query_row(
                    "SELECT title,status,owner_thread_id,created_by FROM agent_tasks WHERE root_thread_id=?1 AND number=?2",
                    params![root, number],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?
                .ok_or_else(|| anyhow!("task #{number} not found in this lineage"))?;
            ensure!(
                !matches!(status.as_str(), "completed" | "failed"),
                "task #{number} is already {status}"
            );
            ensure!(
                owner.as_deref() == Some(me.as_str()) || creator.as_deref() == Some(me.as_str()),
                "only the task's owner or creator can finish it; claim it first"
            );
            let new_status = if failed { "failed" } else { "completed" };
            tx.execute(
                "UPDATE agent_tasks SET status=?3, result=?4, owner_thread_id=COALESCE(owner_thread_id,?5), updated_at=?6 WHERE root_thread_id=?1 AND number=?2",
                params![root, number, new_status, result, me, now_rfc3339()],
            )?;
            crate::service::automation::record_event(&tx, &format!("task:{root}:{number}:terminal"),
                &json!({"kind":"taskEnded","rootThreadId":root,"taskNumber":number,"status":new_status,"closingMessage":result,"workspaceId":tx.query_row("SELECT workspace_id FROM threads WHERE id=?1",[&root],|r|r.get::<_,String>(0))?,"ancestry":[]}), &now_rfc3339())?;
            if let Some(creator) = creator.as_deref() {
                notify(
                    &tx,
                    creator,
                    &me,
                    "result",
                    &format!("Task #{number} {new_status}: {title}"),
                    &format!(
                        "Task #{number} \"{title}\" is {new_status}.\n{}",
                        result.unwrap_or("(no result text)")
                    ),
                )?;
                touched.push(creator.to_string());
            }
            let mut unblocked = Vec::new();
            if !failed {
                let dependents: Vec<(i64, String, Option<String>)> = {
                    let mut stmt = tx.prepare(
                        "SELECT t.number, t.title, t.owner_thread_id FROM agent_task_deps d JOIN agent_tasks t
                           ON t.root_thread_id=d.root_thread_id AND t.number=d.number
                         WHERE d.root_thread_id=?1 AND d.depends_on=?2 AND t.status='pending'",
                    )?;
                    let rows = stmt
                        .query_map(params![root, number], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    rows
                };
                for (n, title, owner) in dependents {
                    if !blocked_by(&tx, &root, n)?.is_empty() {
                        continue;
                    }
                    unblocked.push(n);
                    if let Some(owner) = owner {
                        notify(
                            &tx,
                            &owner,
                            &me,
                            "task",
                            &format!("Task #{n} ready: {title}"),
                            &format!("Task #{n} \"{title}\" is now unblocked. Claim it with `pockymoe task claim {n}`."),
                        )?;
                        touched.push(owner);
                    }
                }
            }
            tx.commit()?;
            Ok(unblocked)
        })?;
        self.touched(touched);
        Ok(
            json!({"task": self.db.with(|c| task_json(c, &root, number, false))?, "unblocked": unblocked}),
        )
    }

    /// Hands an in-progress task back to the pool.
    pub fn task_release(&self, caller: &str, number: i64) -> Result<Value> {
        let (root, me) = self.task_scope(caller)?;
        let changed = self.db.with(|c| {
            Ok(c.execute(
                "UPDATE agent_tasks SET status='pending', owner_thread_id=NULL, updated_at=?4
                 WHERE root_thread_id=?1 AND number=?2 AND status='in_progress'
                   AND (owner_thread_id=?3 OR created_by=?3)",
                params![root, number, me, now_rfc3339()],
            )?)
        })?;
        ensure!(changed == 1, "task #{number} is not in progress under you");
        self.db.with(|c| task_json(c, &root, number, false))
    }
}
