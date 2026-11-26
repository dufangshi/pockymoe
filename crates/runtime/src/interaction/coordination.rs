//! What a coordinator needs to act on a delegate: whether its queue cannot
//! start (`stalled`), how its last turn ended (`outcome`), how many turns in a
//! row failed, and the contract a delegate reads with its first task
//! (docs/thread-interaction.md, "Coordination outcomes").
use super::agents::clip;
use crate::Supervisor;
use anyhow::{bail, Result};
use pockymoe_protocol::now_rfc3339;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::time::Duration;

/// Durations the coordination worker uses. Tests shorten them through
/// `InteractionState::set_timers`; production uses `PRODUCTION`.
#[derive(Clone, Copy, Debug)]
pub struct CoordinationTimers {
    /// How long a queue must keep failing to start before its thread is `stalled`.
    pub stall_after: Duration,
    /// Failed starts needed as well, so one slow failure is not yet a stall.
    pub stall_failures: u32,
    /// Pause after a failed start before the worker tries the queue again.
    pub start_retry: Duration,
}

impl CoordinationTimers {
    pub const PRODUCTION: Self = Self {
        stall_after: Duration::from_secs(30),
        stall_failures: 3,
        start_retry: Duration::from_secs(5),
    };
}

impl Default for CoordinationTimers {
    fn default() -> Self {
        Self::PRODUCTION
    }
}

/// Trailing failed turns at which a delegate's caller must investigate before
/// sending it more work.
pub const FAILURE_BREAKER: i64 = 3;
const ERROR_EXCERPT: usize = 600;

fn stall_key(thread: &str) -> String {
    format!("cli:stall:{thread}")
}

pub(crate) fn interrupt_key(turn: &str) -> String {
    format!("cli:interrupted-by:{turn}")
}

fn seconds_between(from: &str, to: &str) -> Option<f64> {
    let from = chrono::DateTime::parse_from_rfc3339(from).ok()?;
    let to = chrono::DateTime::parse_from_rfc3339(to).ok()?;
    Some((to - from).num_milliseconds() as f64 / 1000.0)
}

/// Records that the worker could not start `thread`'s queue. A failure that
/// does not follow the previous one directly (the queue was empty or the
/// thread ran in between) starts a new period, so an old failure cannot make a
/// fresh queue look stalled.
pub(crate) fn record_start_failure(
    conn: &Connection,
    thread: &str,
    attempt_started: &str,
    error: &str,
    timers: CoordinationTimers,
) -> Result<()> {
    let now = now_rfc3339();
    let previous: Option<Value> = conn
        .query_row(
            "SELECT value FROM kv WHERE key=?1",
            [stall_key(thread)],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|raw| serde_json::from_str(&raw).ok());
    let slack = (timers.start_retry + Duration::from_secs(5)).as_secs_f64();
    let continues = previous.as_ref().is_some_and(|record| {
        record["lastAt"]
            .as_str()
            .and_then(|last| seconds_between(last, attempt_started))
            .is_some_and(|gap| gap <= slack)
    });
    let record = match previous.filter(|_| continues) {
        Some(mut record) => {
            record["failures"] = json!(record["failures"].as_u64().unwrap_or(0) + 1);
            record["lastAt"] = json!(now);
            record["error"] = json!(clip(error, ERROR_EXCERPT));
            record
        }
        None => json!({"since":attempt_started,"lastAt":now,"failures":1,
            "error":clip(error, ERROR_EXCERPT),"reported":[]}),
    };
    conn.execute(
        "INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![stall_key(thread), record.to_string()],
    )?;
    Ok(())
}

/// A turn started: the queue is no longer failing.
pub(crate) fn clear_stall(conn: &Connection, thread: &str) -> Result<()> {
    conn.execute("DELETE FROM kv WHERE key=?1", [stall_key(thread)])?;
    Ok(())
}

/// The stall record of a thread whose queue has failed to start often and long
/// enough to need a person or coordinator.
pub(crate) fn stall(
    conn: &Connection,
    thread: &str,
    timers: CoordinationTimers,
) -> Result<Option<Value>> {
    let Some(record) = conn
        .query_row(
            "SELECT value FROM kv WHERE key=?1",
            [stall_key(thread)],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    else {
        return Ok(None);
    };
    // Only failures while the work now queued was waiting count: an old record
    // left after the queue was withdrawn must not mark new work stalled.
    let oldest: Option<String> = conn.query_row(
        "SELECT MIN(created_at) FROM thread_pending_steers WHERE thread_id=?1 AND delivery='continuation'",
        [thread],
        |r| r.get(0),
    )?;
    let current = match (oldest, record["lastAt"].as_str()) {
        (Some(oldest), Some(last)) => seconds_between(&oldest, last).is_some_and(|gap| gap >= 0.0),
        _ => false,
    };
    if !current {
        return Ok(None);
    }
    let long_enough = record["since"]
        .as_str()
        .and_then(|since| seconds_between(since, &now_rfc3339()))
        .is_some_and(|age| age >= timers.stall_after.as_secs_f64());
    let often_enough = record["failures"].as_u64().unwrap_or(0) >= timers.stall_failures as u64;
    Ok((long_enough && often_enough).then_some(record))
}

/// Notes that `caller`'s wake reported this stall, so a wake registered again
/// for the same stall waits for a change instead of firing at once.
pub(crate) fn mark_stall_reported(conn: &Connection, thread: &str, caller: &str) -> Result<()> {
    conn.execute(
        "UPDATE kv SET value=json_set(value,'$.reported[#]',?2) WHERE key=?1
         AND NOT EXISTS (SELECT 1 FROM json_each(json_extract(value,'$.reported')) WHERE value=?2)",
        params![stall_key(thread), caller],
    )?;
    Ok(())
}

pub(crate) fn stall_reported_to(record: &Value, caller: &str) -> bool {
    record["reported"]
        .as_array()
        .is_some_and(|callers| callers.iter().any(|c| c == caller))
}

/// Failed turns in a row, newest first, ignoring a turn still in progress.
/// An interrupted or completed turn ends the run.
pub(crate) fn consecutive_failures(
    conn: &Connection,
    thread: &str,
) -> Result<(i64, Option<String>)> {
    let mut stmt = conn.prepare(
        "SELECT status,error FROM thread_turns WHERE thread_id=?1 ORDER BY ordinal DESC LIMIT 50",
    )?;
    let rows = stmt
        .query_map([thread], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let (mut count, mut last_error) = (0, None);
    for (status, error) in rows {
        match status.as_str() {
            "inProgress" | "recovering" if count == 0 => continue,
            "failed" => {
                if count == 0 {
                    last_error = error;
                }
                count += 1;
            }
            _ => break,
        }
    }
    Ok((count, last_error))
}

/// How the last finished turn ended, in terms a coordinator can act on.
pub(crate) fn turn_outcome(
    conn: &Connection,
    turn: &str,
    status: &str,
    error: Option<&str>,
) -> Result<Value> {
    Ok(match status {
        "completed" => json!({"kind":"completed"}),
        "failed" => json!({"kind":"failed","error":error.map(|e| clip(e, ERROR_EXCERPT))}),
        "interrupted" => {
            let by: Option<Value> = conn
                .query_row(
                    "SELECT value FROM kv WHERE key=?1",
                    [interrupt_key(turn)],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .and_then(|raw| serde_json::from_str(&raw).ok());
            let mut outcome = json!({"kind":"interrupted"});
            match by {
                Some(by) => {
                    outcome["by"] = by["by"].clone();
                    if let Some(thread) = by["threadId"].as_str() {
                        outcome["byThreadId"] = json!(thread);
                    }
                    if let Some(reason) = by["reason"].as_str() {
                        outcome["reason"] = json!(reason);
                    }
                }
                // Startup reconciliation records the lost connection as the error.
                None if error.is_some_and(|e| e.contains("Supervisor")) => {
                    outcome["by"] = json!("restart");
                }
                None => outcome["by"] = json!("unknown"),
            }
            outcome
        }
        other => json!({"kind":other}),
    })
}

/// One line for wake reports and wait output.
pub(crate) fn outcome_line(outcome: &Value) -> Option<String> {
    let kind = outcome["kind"].as_str()?;
    Some(match kind {
        "failed" => format!(
            "failed: {}",
            outcome["error"].as_str().unwrap_or("no error text")
        ),
        "interrupted" => {
            let by = match outcome["by"].as_str() {
                Some("owner") => "by the owner's Stop".to_string(),
                Some("thread") => format!(
                    "by thread {} (thread stop)",
                    outcome["byThreadId"].as_str().unwrap_or("?")
                ),
                Some("restart") => "by a Supervisor restart".into(),
                _ => "by an unknown cause".into(),
            };
            match outcome["reason"].as_str() {
                Some(reason) => format!("interrupted {by}: {reason}"),
                None => format!("interrupted {by}"),
            }
        }
        "stalled" => format!(
            "stalled: its queue has failed to start {} times since {}: {}",
            outcome["failures"],
            outcome["since"].as_str().unwrap_or("?"),
            outcome["error"].as_str().unwrap_or("")
        ),
        "blocked" => "blocked: waiting on an approval or an answer".into(),
        other => other.to_string(),
    })
}

pub(crate) const STALLED_NEXT: &str = "A delegate is stalled: its queue keeps failing to start (see lastError/outcome). Fix the harness or model it needs, or withdraw the work with `pockymoe thread stop NAME --clear-queue --reason ...` (or `thread unqueue NAME --all`), or close it and create a delegate on a harness that works.";

fn contract_key(thread: &str) -> String {
    format!("cli:contract:{thread}")
}
fn contract_sent_key(thread: &str) -> String {
    format!("cli:contract-sent:{thread}")
}

/// Stages the delegate contract for `thread`'s next turn the first time an
/// ancestor gives it a task. It is read only by the harness, never stored as a
/// prompt, so the transcript, sender parsing and summaries never see it.
pub(crate) fn stage_contract(
    conn: &Connection,
    thread: &str,
    sender: &str,
    direct_parent: bool,
) -> Result<()> {
    let known: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM kv WHERE key IN (?1,?2))",
        params![contract_key(thread), contract_sent_key(thread)],
        |r| r.get(0),
    )?;
    if known {
        return Ok(());
    }
    let label: Option<String> = conn
        .query_row(
            "SELECT COALESCE(agent_name,title) FROM threads WHERE id=?1",
            [sender],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let who = match label.filter(|l| !l.trim().is_empty()) {
        Some(name) => format!("{} ({sender})", clip(name.trim(), 60)),
        None => sender.to_string(),
    };
    let ask = if direct_parent { "parent" } else { sender };
    let text = format!(
        "[Pockymoe delegate contract]\nYou work for thread {who}, which delegated this task. Your final message of each turn is reported to it automatically; send no separate \"done\" message unless asked. Put the evidence in that final message: files changed, commit, the exact test command and its result. If you are blocked, run `pockymoe thread ask {ask} --subject \"...\" --text \"...\"` and wait for the answer instead of guessing or stopping. Do not create further delegates unless the task asks for them."
    );
    conn.execute(
        "INSERT INTO kv(key,value) VALUES(?1,?2)",
        params![contract_key(thread), text],
    )?;
    Ok(())
}

impl Supervisor {
    /// The staged contract, once; later turns start without it.
    pub(crate) fn take_contract(&self, thread: &str) -> Option<String> {
        self.db
            .with(|c| {
                let tx = c.unchecked_transaction()?;
                let text: Option<String> = tx
                    .query_row(
                        "SELECT value FROM kv WHERE key=?1",
                        [contract_key(thread)],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(saved) = &text {
                    tx.execute(
                        "INSERT OR REPLACE INTO kv(key,value) VALUES(?1,?2)",
                        params![format!("handoff:contract:{thread}"), saved],
                    )?;
                    tx.execute("DELETE FROM kv WHERE key=?1", [contract_key(thread)])?;
                    tx.execute(
                        "INSERT OR REPLACE INTO kv(key,value) VALUES(?1,?2)",
                        params![contract_sent_key(thread), now_rfc3339()],
                    )?;
                }
                tx.commit()?;
                Ok(text)
            })
            .ok()
            .flatten()
    }

    /// Who stopped a turn, for its outcome. The first record wins, so a
    /// `thread stop` is not overwritten by the generic Stop it calls.
    pub(crate) fn record_interrupt(&self, turn: &str, by: Value) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO kv(key,value) VALUES(?1,?2)",
                params![interrupt_key(turn), by.to_string()],
            )?;
            Ok(())
        })
    }

    /// Refuses more work for a delegate whose turns keep failing, unless the
    /// caller says the retry is intended. Only checked for thread credentials.
    pub(crate) fn ensure_breaker_allows(
        conn: &Connection,
        caller: &str,
        target: &str,
    ) -> Result<()> {
        if !super::agents::is_descendant(conn, caller, target)? {
            return Ok(());
        }
        let (failures, error) = consecutive_failures(conn, target)?;
        if failures >= FAILURE_BREAKER {
            bail!(
                "refused: thread {target} failed its last {failures} turns in a row (last error: {}). Sending the same work again is unlikely to help. Investigate first with `pockymoe thread status {target}` and `pockymoe transcript {target} --limit 1`; fix the cause or close it and use another delegate. If a retry is really intended, send again with --retry-failed.",
                error.as_deref().map(|e| clip(e, 300)).unwrap_or_else(|| "none recorded".into())
            );
        }
        Ok(())
    }
}
