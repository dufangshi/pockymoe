//! One device-owned ledger. SQLite owns trigger cursors and action acceptance;
//! the existing continuation executor owns prompt execution. Commands are never
//! retried after spawn: a lost completion is explicitly uncertain.
use super::*;
use anyhow::{ensure, Context};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use pockymoe_protocol::{
    AutomationAction as Action, AutomationCondition as Condition,
    AutomationDefinition as Definition, AutomationTrigger as Trigger, CommandRunInput, CommandSpec,
    MissedRunPolicy,
};
use tokio::io::AsyncReadExt;

fn date(s: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)?.with_timezone(&Utc))
}
fn stamp(d: DateTime<Utc>) -> String {
    d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn terminal(s: &str) -> bool {
    matches!(s, "completed" | "failed" | "interrupted")
}
fn ancestry(event: &Value) -> Vec<String> {
    event["ancestry"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn record_event(c: &Connection, key: &str, event: &Value, now: &str) -> Result<()> {
    c.execute("INSERT OR IGNORE INTO automation_events(event_key,payload_json,occurred_at) VALUES(?1,?2,?3)", params![key,event.to_string(),now])?;
    Ok(())
}
/// Shared admission check: remove only a cancelled/expired hook's continuation.
/// It runs before harness preflight and is rechecked in the admission transaction.
pub(crate) fn pending_allowed(c: &Connection, pending: &str, now: &str) -> Result<bool> {
    let owned:Option<(String,String,String,String)>=c.query_row("SELECT r.id,r.scheduled_at,r.definition_json,a.state FROM automation_runs r JOIN automations a ON a.id=r.automation_id WHERE r.pending_id=?1 AND r.state='queued'",[pending],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((run, at, raw, state)) = owned else {
        return Ok(true);
    };
    let d = definition(&raw)?;
    let expired = (date(now)? - date(&at)?).num_seconds() > d.max_lateness_seconds as i64;
    if state != "enabled" || expired {
        c.execute("DELETE FROM thread_pending_steers WHERE id=?1", [pending])?;
        c.execute(
            "UPDATE automation_runs SET state=?2,completed_at=?3,error=?4 WHERE id=?1",
            params![
                run,
                if expired { "skipped" } else { "cancelled" },
                now,
                if expired {
                    "maxLateness"
                } else {
                    "automationPaused"
                }
            ],
        )?;
        return Ok(false);
    }
    Ok(true)
}
pub(crate) fn bind_prompt(c: &Connection, pending: &str, turn: &str, now: &str) -> Result<()> {
    ensure!(
        pending_allowed(c, pending, now)?,
        "conflict: automation continuation expired or was cancelled"
    );
    let state: Option<String> = c.query_row("SELECT a.state FROM automation_runs r JOIN automations a ON a.id=r.automation_id WHERE r.pending_id=?1 AND r.state='queued'", [pending], |r| r.get(0)).optional()?;
    if let Some(state) = state {
        ensure!(
            state == "enabled",
            "conflict: automation is paused or cancelled"
        );
    }
    c.execute("UPDATE automation_runs SET state='running',turn_id=?2,started_at=?3 WHERE pending_id=?1 AND state='queued'",params![pending,turn,now])?;
    Ok(())
}
pub(crate) fn turn_ended(
    c: &Connection,
    thread: &str,
    turn: &str,
    status: &str,
    now: &str,
) -> Result<()> {
    if !terminal(status) {
        return Ok(());
    }
    let origin: Option<(String, String)> = c
        .query_row(
            "SELECT automation_id,event_json FROM automation_runs WHERE turn_id=?1",
            [turn],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let mut chain = vec![];
    if let Some((id, raw)) = origin {
        chain = ancestry(&serde_json::from_str::<Value>(&raw)?);
        chain.push(id);
    }
    c.execute("UPDATE automation_runs SET state=?2,completed_at=?3 WHERE turn_id=?1 AND state IN ('queued','running')",params![turn,status,now])?;
    let workspace: Option<String> = c
        .query_row(
            "SELECT workspace_id FROM threads WHERE id=?1",
            [thread],
            |r| r.get(0),
        )
        .optional()?;
    record_event(
        c,
        &format!("turn:{turn}:terminal"),
        &json!({"kind":"turnEnded","sourceThreadId":thread,"turnId":turn,"workspaceId":workspace,"status":status,"closingMessage":crate::interaction::closing_message(c,thread,turn)?,"ancestry":chain}),
        now,
    )
}

fn condition_ok(c: &Condition, e: &Value) -> bool {
    match c {
        Condition::ReplyLengthAtLeast { chars } => e["closingMessage"]
            .as_str()
            .is_some_and(|text| reply_prose_chars(text) >= *chars),
        Condition::All { conditions } => conditions.iter().all(|c| condition_ok(c, e)),
        Condition::Any { conditions } => conditions.iter().any(|c| condition_ok(c, e)),
        Condition::Not { condition } => !condition_ok(condition, e),
        Condition::StatusIn { values } => e["status"]
            .as_str()
            .is_some_and(|s| values.iter().any(|v| v == s)),
        Condition::ExitCodeEquals { value } => {
            matches!(e["status"].as_str(), Some("completed" | "failed"))
                && e["exitCode"].as_i64() == Some(i64::from(*value))
        }
        Condition::WorkspaceId { value } => e["workspaceId"] == *value,
        Condition::CommandId { value } => e["commandId"] == *value,
    }
}
/// Count visible prose in Unicode scalar values, excluding fenced code and inline code.
pub(crate) fn reply_prose_chars(text: &str) -> u64 {
    let mut fence: Option<(char, usize)> = None;
    let mut count = 0;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next().unwrap_or(' ');
        let width = trimmed.chars().take_while(|c| *c == marker).count();
        if matches!(marker, '`' | '~') && width >= 3 {
            match fence {
                Some((open, len))
                    if marker == open && width >= len && trimmed[width..].trim().is_empty() =>
                {
                    fence = None
                }
                None => fence = Some((marker, width)),
                _ => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let mut rest = line;
        while let Some(start) = rest.find('`') {
            count += rest[..start].chars().count() as u64;
            let span = &rest[start..];
            let width = span.bytes().take_while(|b| *b == b'`').count();
            let mut tail = &span[width..];
            let mut closed = false;
            while let Some(end) = tail.find('`') {
                let run = tail[end..].bytes().take_while(|b| *b == b'`').count();
                tail = &tail[end + run..];
                if run == width {
                    rest = tail;
                    closed = true;
                    break;
                }
            }
            if !closed {
                count += span.chars().count() as u64;
                rest = "";
                break;
            }
        }
        count += rest.chars().count() as u64;
    }
    count
}

fn validate_condition(c: &Condition, depth: usize) -> Result<()> {
    ensure!(depth <= 8, "condition nesting exceeds 8");
    match c {
        Condition::ReplyLengthAtLeast { chars } => ensure!(
            *chars <= 1000000,
            "reply length threshold exceeds one million characters"
        ),
        Condition::All { conditions } | Condition::Any { conditions } => {
            ensure!(conditions.len() <= 32, "too many conditions");
            for c in conditions {
                validate_condition(c, depth + 1)?;
            }
        }
        Condition::Not { condition } => validate_condition(condition, depth + 1)?,
        Condition::StatusIn { values } => ensure!(
            !values.is_empty()
                && values
                    .iter()
                    .all(|v| terminal(v) || matches!(v.as_str(), "timedOut" | "uncertain")),
            "invalid terminal statuses"
        ),
        _ => {}
    }
    Ok(())
}
fn matches_event(t: &Trigger, e: &Value) -> bool {
    match t {
        Trigger::ThreadEnded { source_thread_id } => {
            e["kind"] == "turnEnded"
                && e["sourceThreadId"] == *source_thread_id
                && e["turnId"].as_str().is_some_and(|id| !id.is_empty())
                && e["status"].as_str().is_some_and(terminal)
        }
        Trigger::TurnEnded {
            source_thread_id,
            turn_id,
        } => {
            e["kind"] == "turnEnded"
                && e["sourceThreadId"] == *source_thread_id
                && e["turnId"] == *turn_id
        }
        Trigger::TaskEnded {
            root_thread_id,
            task_number,
        } => {
            e["kind"] == "taskEnded"
                && e["rootThreadId"] == *root_thread_id
                && e["taskNumber"] == *task_number
        }
        Trigger::CommandEnded {
            source_thread_id,
            command_id,
            command_key,
        } => {
            e["kind"] == "commandEnded"
                && e["sourceThreadId"] == *source_thread_id
                && command_id.as_ref().is_none_or(|s| e["commandId"] == *s)
                && command_key.as_ref().is_none_or(|s| e["commandKey"] == *s)
        }
        _ => false,
    }
}
fn next_time(t: &Trigger, now: DateTime<Utc>, future: bool) -> Result<Option<String>> {
    Ok(match t {
        Trigger::At { at } => {
            let at = date(at)?;
            if future && at <= now {
                None
            } else {
                Some(stamp(at))
            }
        }
        Trigger::Interval {
            every_seconds,
            anchor_at,
        } => {
            ensure!(
                (1..=31536000).contains(every_seconds),
                "everySeconds must be 1..31536000"
            );
            let anchor = anchor_at
                .as_deref()
                .map(date)
                .transpose()?
                .unwrap_or(now + ChronoDuration::seconds(*every_seconds as i64));
            let step = *every_seconds as i64;
            let n = if future && anchor <= now {
                (now - anchor).num_seconds() / step + 1
            } else {
                0
            };
            Some(stamp(anchor + ChronoDuration::seconds(n * step)))
        }
        _ => None,
    })
}
/// Set by the Supervisor for every command; a definition may not override them.
const RESERVED_COMMAND_ENV: &[&str] = &["POCKYMOE_COMMAND_ID", "REMOTE_CODEX_COMMAND_ID"];

/// Non-secret process basics a command inherits. Everything else in the
/// Supervisor's environment (CLI config, managed credentials, upstream keys)
/// stays out; a definition passes anything more through `env` explicitly.
#[cfg(unix)]
const INHERITED_COMMAND_ENV: &[&str] = &[
    "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LANGUAGE", "TZ", "TMPDIR", "TERM",
];
#[cfg(windows)]
const INHERITED_COMMAND_ENV: &[&str] = &[
    "SystemRoot",
    "WINDIR",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "USERNAME",
    "APPDATA",
    "LOCALAPPDATA",
    "PATHEXT",
    "ComSpec",
];

pub(crate) fn inherited_command_env() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    std::env::vars_os()
        .filter(|(key, _)| {
            let Some(key) = key.to_str() else {
                return false;
            };
            INHERITED_COMMAND_ENV.iter().any(|k| {
                if cfg!(windows) {
                    k.eq_ignore_ascii_case(key)
                } else {
                    *k == key
                }
            }) || (cfg!(unix) && (key.starts_with("LC_") || key.starts_with("XDG_")))
        })
        .collect()
}

fn validate_command(spec: &CommandSpec, workspace: &str) -> Result<PathBuf> {
    ensure!(
        spec.shell.as_ref().is_some_and(|s| !s.trim().is_empty()) ^ !spec.argv.is_empty(),
        "provide either nonempty argv or explicit shell"
    );
    ensure!(
        spec.argv.len() <= 128
            && spec
                .argv
                .iter()
                .all(|s| s.len() <= 16384 && !s.contains('\0')),
        "invalid argv"
    );
    ensure!(
        spec.shell
            .as_ref()
            .is_none_or(|s| s.len() <= 65536 && !s.contains('\0')),
        "invalid shell"
    );
    ensure!(
        (1..=300).contains(&spec.timeout_seconds),
        "timeoutSeconds must be 1..300"
    );
    ensure!(spec.env.len() <= 64, "at most 64 env entries");
    for (key, value) in &spec.env {
        ensure!(
            !key.is_empty()
                && key.len() <= 128
                && !key.starts_with(|c: char| c.is_ascii_digit())
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "invalid env name {key:?}"
        );
        ensure!(
            !RESERVED_COMMAND_ENV.contains(&key.as_str()),
            "env {key} is set by the Supervisor"
        );
        ensure!(
            value.len() <= 16384 && !value.contains('\0'),
            "invalid env value for {key}"
        );
    }
    ensure!(!spec.cwd.trim().is_empty(), "cwd is required");
    let path = Path::new(&spec.cwd);
    let cwd = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(workspace).join(path)
    };
    let cwd = std::fs::canonicalize(cwd).context("command cwd unavailable")?;
    ensure!(cwd.is_dir(), "command cwd is not a directory");
    Ok(cwd)
}
fn cancelled_pending(c: &Connection, id: &str, now: &str) -> Result<()> {
    c.execute("DELETE FROM thread_pending_steers WHERE id IN (SELECT pending_id FROM automation_runs WHERE automation_id=?1 AND state='queued')",[id])?;
    c.execute("UPDATE automation_runs SET state='cancelled',completed_at=?2 WHERE automation_id=?1 AND state IN ('due','queued')",params![id,now])?;
    Ok(())
}
fn auto_value(c: &Connection, id: &str, thread: &str) -> Result<Value> {
    let (raw,state,next,created,updated,error):(String,String,Option<String>,String,String,Option<String>)=c.query_row("SELECT definition_json,state,next_run_at,created_at,updated_at,error FROM automations WHERE id=?1 AND thread_id=?2",params![id,thread],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?.context("automation not found")?;
    let pending: i64 = c.query_row(
        "SELECT count(*) FROM automation_runs WHERE automation_id=?1 AND state IN ('due','queued')",
        [id],
        |r| r.get(0),
    )?;
    let missed: i64 = c.query_row(
        "SELECT coalesce(sum(missed_count),0) FROM automation_runs WHERE automation_id=?1",
        [id],
        |r| r.get(0),
    )?;
    Ok(
        json!({"id":id,"threadId":thread,"sourceKind":"supervisor","definition":serde_json::from_str::<Value>(&raw)?,"state":state,"nextRunAt":next,"createdAt":created,"updatedAt":updated,"error":error,"pendingCount":pending,"missedCount":missed}),
    )
}
fn run_value(r: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let receipt: Option<String> = r.get(11)?;
    let receipt = receipt
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .map(|mut value| {
            if let Some(object) = value.as_object_mut() {
                object.remove("settings");
            }
            value
        });
    Ok(
        json!({"id":r.get::<_,String>(0)?,"automationId":r.get::<_,String>(1)?,"occurrenceKey":r.get::<_,String>(2)?,"state":r.get::<_,String>(3)?,"scheduledAt":r.get::<_,String>(4)?,"observedAt":r.get::<_,String>(5)?,"missedCount":r.get::<_,i64>(6)?,"pendingSteerId":r.get::<_,Option<String>>(7)?,"turnId":r.get::<_,Option<String>>(8)?,"startedAt":r.get::<_,Option<String>>(9)?,"completedAt":r.get::<_,Option<String>>(10)?,"deliveryReceipt":receipt,"error":r.get::<_,Option<String>>(12)?,"attemptCount":r.get::<_,i64>(13)?,"commandId":r.get::<_,Option<String>>(14)?}),
    )
}
const RUN_SELECT:&str="SELECT id,automation_id,occurrence_key,state,scheduled_at,observed_at,missed_count,pending_id,turn_id,started_at,completed_at,receipt_json,error,attempt_count,command_id FROM automation_runs";

impl Supervisor {
    pub fn automation_preview(&self, thread: &str, definition: Definition) -> Result<Value> {
        let target = self.get_thread(thread)?;
        ensure!(
            target.closed_at.is_none(),
            "targetUnavailable: thread is closed"
        );
        ensure!(
            !definition.name.trim().is_empty() && definition.name.len() <= 120,
            "name must be 1..120 bytes"
        );
        ensure!(
            definition.max_lateness_seconds <= 31536000,
            "maxLatenessSeconds exceeds one year"
        );
        validate_condition(&definition.condition, 0)?;
        match &definition.action {
            Action::PluginAction {
                plugin_id,
                action,
                input,
                timeout_seconds,
            } => {
                self.validate_thread_plugin_action(
                    thread,
                    plugin_id,
                    action,
                    input,
                    *timeout_seconds,
                )?;
            }
            Action::Prompt { text } => {
                ensure!(
                    !text.trim().is_empty() && text.len() <= 262144,
                    "invalid prompt text"
                );
                self.ensure_prompt_allowed(&target)?;
                ensure!(!self.recorded_native_watch_conflict(thread)?,"nativeConflict: confirm native watches are cancelled before enabling supervisor prompts");
            }
            Action::NotifyInbox {
                subject,
                text,
                message_kind,
                ..
            } => {
                ensure!(
                    matches!(message_kind.as_str(), "result" | "status"),
                    "ordinary automation notifications must be result or status"
                );
                ensure!(
                    !subject.trim().is_empty()
                        && subject.chars().count() <= 120
                        && text.len() <= 262144,
                    "invalid inbox subject/text"
                );
            }
            Action::RunScript { command } => {
                validate_command(command, &self.get_workspace(&target.workspace_id)?.abs_path)?;
            }
        }
        match &definition.trigger {
            Trigger::ThreadEnded { source_thread_id } => {
                ensure!(
                    !definition.replay_existing,
                    "replayUnsupported: threadEnded only observes turns ending after registration"
                );
                ensure!(
                    self.get_thread(source_thread_id)?.closed_at.is_none(),
                    "sourceUnavailable: thread is closed"
                );
            }
            Trigger::TurnEnded {
                source_thread_id,
                turn_id,
            } => {
                self.get_thread(source_thread_id)?;
                self.db.with(|c|{ensure!(c.query_row("SELECT EXISTS(SELECT 1 FROM thread_turns WHERE id=?1 AND thread_id=?2)",params![turn_id,source_thread_id],|r|r.get::<_,bool>(0))?,"source turn not found");Ok(())})?;
            }
            Trigger::TaskEnded {
                root_thread_id,
                task_number,
            } => {
                self.get_thread(root_thread_id)?;
                self.db.with(|c|{ensure!(c.query_row("SELECT EXISTS(SELECT 1 FROM agent_tasks WHERE root_thread_id=?1 AND number=?2)",params![root_thread_id,task_number],|r|r.get::<_,bool>(0))?,"source task not found");Ok(())})?;
            }
            Trigger::CommandEnded {
                source_thread_id,
                command_id,
                command_key,
            } => {
                self.get_thread(source_thread_id)?;
                ensure!(
                    command_id.as_ref().is_some_and(|s| !s.is_empty())
                        || command_key.as_ref().is_some_and(|s| !s.is_empty()),
                    "commandId or commandKey is required"
                );
            }
            _ => {}
        }
        let now = Utc::now();
        let next = next_time(&definition.trigger, now, false)?;
        let mut times = vec![];
        if let Some(next) = &next {
            times.push(next.clone());
            if let Trigger::Interval { every_seconds, .. } = definition.trigger {
                for n in 1..5 {
                    times.push(stamp(
                        date(next)? + ChronoDuration::seconds((n * every_seconds) as i64),
                    ));
                }
            }
        }
        Ok(
            json!({"definition":definition,"nextRunAt":next,"nextRuns":times,"busyPolicy":"coalesce","commandObservability":"supervisorWrapperOnly"}),
        )
    }
    pub fn automation_create(
        &self,
        thread: &str,
        mut d: Definition,
        request: Option<&str>,
    ) -> Result<Value> {
        ensure!(
            request.is_none_or(|s| !s.is_empty() && s.len() <= 128),
            "invalid clientRequestId"
        );
        // Normalise only after idempotency lookup, preserving the caller's original definition.
        let fingerprint = serde_json::to_string(&d)?;
        let existing=self.db.with(|c|{if let Some(key)=request {if let Some((id,saved))=c.query_row("SELECT id,request_definition FROM automations WHERE thread_id=?1 AND request_id=?2",params![thread,key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()? {
            ensure!(saved==fingerprint,"conflict: clientRequestId reused with different definition");return Ok(Some(auto_value(c,&id,thread)?));}}Ok(None)})?;
        if let Some(v) = existing {
            return self.automation_show(thread, v["id"].as_str().unwrap_or_default());
        }
        self.automation_preview(thread, d.clone())?;
        if let Trigger::Interval {
            every_seconds,
            anchor_at,
        } = &mut d.trigger
        {
            if anchor_at.is_none() {
                *anchor_at = Some(stamp(
                    Utc::now() + ChronoDuration::seconds(*every_seconds as i64),
                ));
            }
        }
        let now = now_rfc3339();
        let next = next_time(&d.trigger, date(&now)?, false)?;
        let id = Uuid::new_v4().to_string();
        self.db.with(|c|{
            let tx=c.unchecked_transaction()?;
            let source=existing_source_event(&tx,&d.trigger)?;
            ensure!(source.is_none() || d.replay_existing,"sourceAlreadyEnded: use replayExisting=true for an explicit immediate occurrence");
            let cursor:i64=tx.query_row("SELECT coalesce(max(sequence),0) FROM automation_events",[],|r|r.get(0))?;
            let raw=serde_json::to_value(&d)?;
            tx.execute("INSERT INTO automations(id,thread_id,definition_json,state,next_run_at,event_cursor,created_at,updated_at,request_id,request_definition) VALUES(?1,?2,?3,?4,?5,?6,?7,?7,?8,?9)",params![id,thread,raw.to_string(),if d.enabled {"enabled"}else{"paused"},next,cursor,now,request,fingerprint])?;
            if d.enabled {if let Some(e)=source {create_run(&tx,&id,&d,"existing",&now,&now,&e,0)?;}}
            tx.commit()?;auto_value(c,&id,thread)
        })?;
        self.automation_show(thread, &id)
    }
    pub fn automation_list(&self, thread: &str) -> Result<Value> {
        self.get_thread(thread)?;
        let mut items = self.db.with(|c| {
            let mut q = c.prepare(
                "SELECT id FROM automations WHERE thread_id=?1 ORDER BY created_at DESC",
            )?;
            let ids = q
                .query_map([thread], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids.iter()
                .map(|id| auto_value(c, id, thread))
                .collect::<Result<Vec<_>>>()
        })?;
        if !items.is_empty() {
            let statistics = self.automation_statistics(thread)?;
            for item in &mut items {
                item["statistics"] = statistics
                    .get(item["id"].as_str().unwrap_or_default())
                    .cloned()
                    .unwrap_or_default();
            }
        }
        Ok(json!({"automations": items}))
    }
    pub fn automation_show(&self, thread: &str, id: &str) -> Result<Value> {
        let mut item = self.db.with(|c| auto_value(c, id, thread))?;
        item["statistics"] = self
            .automation_statistics(thread)?
            .remove(id)
            .unwrap_or_default();
        Ok(item)
    }
    pub fn automation_runs(&self, thread: &str, id: &str, limit: u64) -> Result<Value> {
        self.db.with(|c| {
            auto_value(c, id, thread)?;
            let mut q = c.prepare(&format!(
                "{RUN_SELECT} WHERE automation_id=?1 ORDER BY observed_at DESC,rowid DESC LIMIT ?2"
            ))?;
            let rows = q
                .query_map(params![id, limit.clamp(1, 100)], run_value)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(json!({"runs":rows}))
        })
    }
    pub fn automation_control(&self, thread: &str, id: &str, op: &str) -> Result<Value> {
        let a = self.automation_show(thread, id)?;
        ensure!(
            a["state"] != "cancelled" || op == "cancel",
            "cancelled automation cannot resume"
        );
        let d = definition(&a["definition"].to_string())?;
        if op == "resume" && a["state"] != "enabled" {
            self.automation_preview(thread, d.clone())?;
        }
        let now = now_rfc3339();
        self.db.with(|c|{let tx=c.unchecked_transaction()?;
            let current:String=tx.query_row("SELECT state FROM automations WHERE id=?1 AND thread_id=?2",params![id,thread],|r|r.get(0))?;
            ensure!(current!="cancelled" || op=="cancel","cancelled automation cannot resume");
            // A retried resume must preserve unconsumed events and the timer anchor.
            if op=="resume" && current=="enabled" {tx.commit()?;return auto_value(c,id,thread);}
            let (state,next)=match op {"pause"=>("paused",None),"cancel"=>("cancelled",None),"resume"=>("enabled",next_time(&d.trigger,date(&now)?,true)?),_=>bail!("unknown automation operation")};
            if op!="resume" {cancelled_pending(&tx,id,&now)?;}
            let cursor:i64=tx.query_row("SELECT coalesce(max(sequence),0) FROM automation_events",[],|r|r.get(0))?;
            tx.execute("UPDATE automations SET state=?2,next_run_at=?3,updated_at=?4,event_cursor=?5,error=NULL WHERE id=?1",params![id,state,next,now,cursor])?;tx.commit()?;auto_value(c,id,thread)
        })?;
        self.automation_show(thread, id)
    }
    pub(crate) fn pause_thread_prompt_automations(&self, thread: &str, reason: &str) -> Result<()> {
        self.db.with(|c|{let tx=c.unchecked_transaction()?;let ids={let mut q=tx.prepare("SELECT id FROM automations WHERE thread_id=?1 AND state='enabled' AND json_extract(definition_json,'$.action.kind')='prompt'")?;let v=q.query_map([thread],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;v};let now=now_rfc3339();for id in ids {cancelled_pending(&tx,&id,&now)?;tx.execute("UPDATE automations SET state='paused',next_run_at=NULL,error=?2,updated_at=?3 WHERE id=?1",params![id,reason,now])?;}tx.commit()?;Ok(())})
    }
    /// Materialize accepted occurrences before cancelling, so an off/on toggle
    /// cannot resurrect an unclaimed manual action or an already observed event.
    pub(crate) fn cancel_pending_plugin_actions(
        c: &Connection,
        thread: Option<&str>,
        plugin: &str,
        now: &str,
    ) -> Result<()> {
        prepare_runs(c, now, Some((thread, plugin)))?;
        c.execute("UPDATE automation_runs SET state='cancelled',completed_at=?3,error='pluginDisabled' WHERE state='due' AND automation_id IN (SELECT id FROM automations WHERE ?1 IS NULL OR thread_id=?1) AND json_extract(definition_json,'$.action.kind')='pluginAction' AND json_extract(definition_json,'$.action.pluginId')=?2", params![thread,plugin,now])?;
        Ok(())
    }

    /// Deterministic clock seam for regressions; production calls with UTC now.
    pub async fn automation_tick(self: &Arc<Self>, now: &str) -> Result<()> {
        let _guard = self.automation_gate.lock().await;
        if self
            .update_draining
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(());
        }
        let available = self.plugins.availability(self);
        let live = self.live.lock().await;
        let jobs = self.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            prepare_runs(&tx, now, None)?;
            let jobs = dispatch_runs(
                &tx,
                now,
                &live.keys().cloned().collect(),
                &self.plugins,
                &available,
            )?;
            tx.commit()?;
            Ok(jobs)
        })?;
        drop(live);
        for job in jobs {
            let s = self.clone();
            tokio::spawn(async move {
                let result = match job {
                    BackgroundJob::Command(id) => s.execute_command(&id).await,
                    BackgroundJob::Plugin(id) => s.execute_plugin_run(&id).await,
                };
                if let Err(error) = result {
                    tracing::warn!(%error, "background completion could not be saved; recovery will mark uncertain");
                }
            });
        }
        Ok(())
    }
}

fn definition(raw: &str) -> Result<Definition> {
    Ok(serde_json::from_str(raw)?)
}
fn existing_source_event(c: &Connection, t: &Trigger) -> Result<Option<Value>> {
    let (key, source) = match t {
        Trigger::TurnEnded {
            source_thread_id,
            turn_id,
        } => (format!("turn:{turn_id}:terminal"), source_thread_id),
        Trigger::TaskEnded {
            root_thread_id,
            task_number,
        } => (
            format!("task:{root_thread_id}:{task_number}:terminal"),
            root_thread_id,
        ),
        Trigger::CommandEnded {
            source_thread_id,
            command_id: Some(id),
            ..
        } => (format!("command:{id}:terminal"), source_thread_id),
        _ => return Ok(None),
    };
    if let Some(raw) = c
        .query_row(
            "SELECT payload_json FROM automation_events WHERE event_key=?1",
            [key],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        let e: Value = serde_json::from_str(&raw)?;
        ensure!(
            matches_event(t, &e),
            "sourceMismatch: selected source does not match the recorded event"
        );
        return Ok(Some(e));
    }
    // Pre-migration history is eligible only through explicit replayExisting.
    let mut event=match t {
        Trigger::TurnEnded {source_thread_id,turn_id}=>c.query_row("SELECT status FROM thread_turns WHERE id=?1 AND thread_id=?2",params![turn_id,source_thread_id],|r|r.get::<_,String>(0)).optional()?.filter(|s|terminal(s)).map(|s|json!({"kind":"turnEnded","sourceThreadId":source_thread_id,"turnId":turn_id,"status":s,"closingMessage":crate::interaction::closing_message(c,source_thread_id,turn_id).ok().flatten(),"ancestry":[]})),
        Trigger::TaskEnded {root_thread_id,task_number}=>c.query_row("SELECT status,result FROM agent_tasks WHERE root_thread_id=?1 AND number=?2",params![root_thread_id,task_number],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?))).optional()?.filter(|(s,_)|terminal(s)).map(|(s,r)|json!({"kind":"taskEnded","rootThreadId":root_thread_id,"taskNumber":task_number,"status":s,"closingMessage":r,"ancestry":[]})),
        _=>None,
    };
    if let Some(e) = &mut event {
        e["workspaceId"] = json!(c.query_row(
            "SELECT workspace_id FROM threads WHERE id=?1",
            [source],
            |r| r.get::<_, String>(0)
        )?);
    }
    Ok(event)
}
fn create_run(
    c: &Connection,
    id: &str,
    d: &Definition,
    key: &str,
    scheduled: &str,
    now: &str,
    e: &Value,
    missed: i64,
) -> Result<()> {
    let chain = ancestry(e);
    let state = if chain.len() >= 3 || chain.iter().any(|s| s == id) {
        "loopSkipped"
    } else if condition_ok(&d.condition, e) {
        "due"
    } else {
        "conditionSkipped"
    };
    c.execute("INSERT OR IGNORE INTO automation_runs(id,automation_id,occurrence_key,state,scheduled_at,observed_at,missed_count,definition_json,event_json,completed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![Uuid::new_v4().to_string(),id,key,state,scheduled,now,missed,serde_json::to_string(d)?,e.to_string(),if state=="due"{None}else{Some(now)}])?;
    Ok(())
}
fn prepare_runs(
    c: &Connection,
    now: &str,
    plugin_scope: Option<(Option<&str>, &str)>,
) -> Result<()> {
    let autos = {
        let mut q=c.prepare("SELECT id,thread_id,definition_json,next_run_at,event_cursor FROM automations WHERE state='enabled' AND (?1 IS NULL OR thread_id=?1) AND (?2 IS NULL OR (json_extract(definition_json,'$.action.kind')='pluginAction' AND json_extract(definition_json,'$.action.pluginId')=?2))")?;
        let v = q
            .query_map(
                params![plugin_scope.and_then(|s| s.0), plugin_scope.map(|s| s.1)],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let now_dt = date(now)?;
    for (id, thread, raw, next, cursor) in autos {
        let d = definition(&raw)?;
        let available: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM threads WHERE id=?1 AND closed_at IS NULL)",
            [&thread],
            |r| r.get(0),
        )?;
        if !available {
            cancelled_pending(c, &id, now)?;
            c.execute("UPDATE automations SET state='paused',error='targetUnavailable',next_run_at=NULL WHERE id=?1",[&id])?;
            continue;
        }
        let source = match &d.trigger {
            Trigger::ThreadEnded { source_thread_id }
            | Trigger::TurnEnded {
                source_thread_id, ..
            }
            | Trigger::CommandEnded {
                source_thread_id, ..
            } => Some(source_thread_id),
            Trigger::TaskEnded { root_thread_id, .. } => Some(root_thread_id),
            _ => None,
        };
        if let Some(source) = source {
            let exists: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM threads WHERE id=?1 AND (?2=0 OR closed_at IS NULL))",
                params![source, matches!(d.trigger, Trigger::ThreadEnded { .. })],
                |r| r.get(0),
            )?;
            if !exists {
                cancelled_pending(c, &id, now)?;
                c.execute(
                    "UPDATE automations SET state='paused',error='sourceUnavailable' WHERE id=?1",
                    [&id],
                )?;
                continue;
            }
        }
        if matches!(d.trigger, Trigger::At { .. } | Trigger::Interval { .. }) {
            if let Some(next) = next {
                let due = date(&next)?;
                if due > now_dt {
                    continue;
                }
                let (latest, count, following) =
                    if let Trigger::Interval { every_seconds, .. } = &d.trigger {
                        let step = *every_seconds as i64;
                        let n = (now_dt - due).num_seconds() / step;
                        let latest = due + ChronoDuration::seconds(n * step);
                        (
                            latest,
                            n + 1,
                            Some(stamp(latest + ChronoDuration::seconds(step))),
                        )
                    } else {
                        (due, 1, None)
                    };
                c.execute(
                    "UPDATE automations SET next_run_at=?2,updated_at=?3 WHERE id=?1",
                    params![id, following, now],
                )?;
                let e = json!({"kind":"time","workspaceId":c.query_row("SELECT workspace_id FROM threads WHERE id=?1",[&thread],|r|r.get::<_,String>(0))?,"ancestry":[]});
                let skipped = (now_dt - latest).num_seconds() > d.max_lateness_seconds as i64
                    || (d.missed_run_policy == MissedRunPolicy::Skip && count > 1);
                if skipped {
                    create_run(
                        c,
                        &id,
                        &d,
                        &format!("time:{}", stamp(latest)),
                        &stamp(latest),
                        now,
                        &e,
                        count - 1,
                    )?;
                    c.execute("UPDATE automation_runs SET state='skipped',error='maxLatenessOrMissedRun',completed_at=?3 WHERE automation_id=?1 AND occurrence_key=?2",params![id,format!("time:{}",stamp(latest)),now])?;
                    continue;
                }
                let pending:Option<String>=c.query_row("SELECT id FROM automation_runs WHERE automation_id=?1 AND state IN ('due','queued') ORDER BY rowid LIMIT 1",[&id],|r|r.get(0)).optional()?;
                if let Some(run) = pending {
                    c.execute("UPDATE automation_runs SET missed_count=missed_count+?2,scheduled_at=?3 WHERE id=?1",params![run,count,stamp(latest)])?;
                } else {
                    create_run(
                        c,
                        &id,
                        &d,
                        &format!("time:{}", stamp(latest)),
                        &stamp(latest),
                        now,
                        &e,
                        count - 1,
                    )?;
                }
            }
        } else {
            let events = {
                let mut q=c.prepare("SELECT sequence,event_key,payload_json,occurred_at FROM automation_events WHERE sequence>?1 ORDER BY sequence LIMIT 256")?;
                let v = q
                    .query_map([cursor], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                v
            };
            for (seq, key, raw, at) in events {
                let e: Value = serde_json::from_str(&raw)?;
                if matches_event(&d.trigger, &e) {
                    create_run(c, &id, &d, &key, &at, now, &e, 0)?;
                    // Exact turn/task/command subscriptions are immutable and single occurrence.
                    // Thread-wide and named command subscriptions consume every new event.
                    if !matches!(
                        d.trigger,
                        Trigger::ThreadEnded { .. }
                            | Trigger::CommandEnded {
                                command_id: None,
                                ..
                            }
                    ) {
                        c.execute(
                            "UPDATE automations SET event_cursor=?2 WHERE id=?1",
                            params![id, seq],
                        )?;
                        break;
                    }
                }
                c.execute(
                    "UPDATE automations SET event_cursor=?2 WHERE id=?1",
                    params![id, seq],
                )?;
            }
        }
    }
    // Only the regular worker performs device-wide queue reconciliation. A
    // thread plugin settings write must not mutate another thread's runs.
    if plugin_scope.is_none() {
        c.execute("UPDATE automation_runs SET state='cancelled',completed_at=?1,error='queueRemoved' WHERE state='queued' AND pending_id NOT IN (SELECT id FROM thread_pending_steers)",[now])?;
    }
    Ok(())
}
enum BackgroundJob {
    Command(String),
    Plugin(String),
}
fn dispatch_runs(
    c: &Connection,
    now: &str,
    live: &HashSet<String>,
    plugins: &crate::plugins::PluginHost,
    available: &HashMap<String, bool>,
) -> Result<Vec<BackgroundJob>> {
    let runs = {
        let mut q=c.prepare("SELECT r.id,r.automation_id,a.thread_id,r.definition_json,r.event_json,r.scheduled_at FROM automation_runs r JOIN automations a ON a.id=r.automation_id WHERE r.state='due' AND a.state='enabled' ORDER BY r.rowid LIMIT 64")?;
        let v = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let mut jobs = vec![];
    for (run, id, thread, raw, event, scheduled) in runs {
        let d = definition(&raw)?;
        let e: Value = serde_json::from_str(&event)?;
        let (status, closed): (String, Option<String>) = c.query_row(
            "SELECT status,closed_at FROM threads WHERE id=?1",
            [&thread],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if closed.is_some() || status == "recovering" {
            continue;
        }
        if (date(now)? - date(&scheduled)?).num_seconds() > d.max_lateness_seconds as i64 {
            c.execute("UPDATE automation_runs SET state='skipped',error='maxLateness',completed_at=?2 WHERE id=?1",params![run,now])?;
            continue;
        }
        let request = format!("automation-{run}");
        match &d.action {
            Action::PluginAction {
                plugin_id, action, ..
            } => {
                let snapshot = if let Some(available) = available.get(plugin_id) {
                    Some(plugins.thread_settings(c, &thread, plugin_id, *available)?)
                } else {
                    None
                };
                let Some(snapshot) = snapshot.filter(|s| s.effective.enabled) else {
                    c.execute("UPDATE automation_runs SET state='cancelled',completed_at=?2,error='pluginDisabledOrUnavailable' WHERE id=?1", params![run,now])?;
                    continue;
                };
                let settings = crate::plugins::PluginSettings {
                    revision: if snapshot.values.is_some() {
                        snapshot.revision
                    } else {
                        snapshot.global_revision
                    },
                    values: snapshot.effective.values,
                };
                let (running, same): (i64, bool) = c.query_row("SELECT count(*),coalesce(max(json_extract(definition_json,'$.action.pluginId')=?1),0) FROM automation_runs WHERE state='running' AND json_extract(definition_json,'$.action.kind')='pluginAction'", [plugin_id], |r| Ok((r.get(0)?,r.get(1)?)))?;
                let busy: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM thread_turns WHERE thread_id=?1 AND status IN ('inProgress','recovering')) OR EXISTS(SELECT 1 FROM thread_pending_steers WHERE thread_id=?1 AND delivery='continuation')", [&thread], |r| r.get(0))?;
                if running >= 4 || same || live.contains(&thread) || status == "running" || busy {
                    continue;
                }
                c.execute("UPDATE automation_runs SET state='running',started_at=?2,attempt_count=attempt_count+1,receipt_json=?3 WHERE id=?1 AND state='due'", params![run,now,json!({"pluginId":plugin_id,"action":action,"settings":settings,"settingsRevisions":{"thread":snapshot.revision,"global":snapshot.global_revision}}).to_string()])?;
                jobs.push(BackgroundJob::Plugin(run));
            }
            Action::NotifyInbox {
                subject,
                text,
                message_kind,
                include_closing_message,
            } => {
                let text = if *include_closing_message {
                    format!("{text}\n{}", e["closingMessage"].as_str().unwrap_or(""))
                } else {
                    text.clone()
                };
                crate::interaction::inbox::store(
                    c,
                    &thread,
                    &request,
                    None,
                    &text,
                    now,
                    crate::interaction::inbox::Envelope {
                        subject: Some(subject),
                        kind: message_kind,
                        in_reply_to: None,
                        topic_key: None,
                    },
                )?;
                c.execute("UPDATE automation_runs SET state='completed',completed_at=?2,attempt_count=attempt_count+1,receipt_json=?3 WHERE id=?1",params![run,now,json!({"delivery":"inbox","messageId":request,"acceptedAt":now}).to_string()])?;
            }
            Action::Prompt { text } => {
                let active:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM automation_runs WHERE automation_id=?1 AND state='running')",[&id],|r|r.get(0))?;
                if active {
                    continue;
                }
                crate::interaction::enqueue(c, &request, &thread, text, Some(&request), now)?;
                c.execute("UPDATE automation_runs SET state='queued',pending_id=?2,attempt_count=attempt_count+1,receipt_json=?3 WHERE id=?1",params![run,request,json!({"requestedDelivery":"queue","delivery":"queued","pendingSteerId":request,"clientRequestId":request,"acceptedAt":now}).to_string()])?;
            }
            Action::RunScript { command } => {
                let busy:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM thread_turns WHERE thread_id=?1 AND status IN ('inProgress','recovering')) OR EXISTS(SELECT 1 FROM thread_pending_steers WHERE thread_id=?1 AND delivery='continuation') OR EXISTS(SELECT 1 FROM automation_runs WHERE automation_id=?2 AND state='running')",params![thread,id],|r|r.get(0))?;
                let commands: i64 = c.query_row(
                    "SELECT count(*) FROM command_executions WHERE state IN ('starting','running')",
                    [],
                    |r| r.get(0),
                )?;
                if live.contains(&thread) || status == "running" || busy || commands >= 4 {
                    continue;
                }
                let mut chain = ancestry(&e);
                chain.push(id.clone());
                let input = CommandRunInput {
                    command: command.clone(),
                    command_key: None,
                    client_request_id: Some(request),
                };
                let command_id = insert_command(c, &thread, &input, Some(&run), &chain, now)?;
                c.execute("UPDATE automation_runs SET state='running',started_at=?2,command_id=?3,attempt_count=attempt_count+1 WHERE id=?1 AND state='due'",params![run,now,command_id])?;
                jobs.push(BackgroundJob::Command(command_id));
            }
        }
    }
    Ok(jobs)
}

fn insert_command(
    c: &Connection,
    thread: &str,
    input: &CommandRunInput,
    run: Option<&str>,
    chain: &[String],
    now: &str,
) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    c.execute("INSERT INTO command_executions(id,thread_id,command_key,request_id,input_json,state,automation_run_id,ancestry_json,started_at) VALUES(?1,?2,?3,?4,?5,'starting',?6,?7,?8)",params![id,thread,input.command_key,input.client_request_id,serde_json::to_string(input)?,run,serde_json::to_string(chain)?,now])?;
    Ok(id)
}
impl Supervisor {
    pub fn command_show(&self, thread: &str, id: &str) -> Result<Value> {
        self.db.with(|c|{Ok(c.query_row("SELECT state,command_key,started_at,completed_at,exit_code,stdout,stderr,error,automation_run_id,input_json FROM command_executions WHERE id=?1 AND thread_id=?2",params![id,thread],|r|Ok(json!({"id":id,"threadId":thread,"state":r.get::<_,String>(0)?,"commandKey":r.get::<_,Option<String>>(1)?,"startedAt":r.get::<_,String>(2)?,"completedAt":r.get::<_,Option<String>>(3)?,"exitCode":r.get::<_,Option<i32>>(4)?,"stdout":r.get::<_,Option<String>>(5)?,"stderr":r.get::<_,Option<String>>(6)?,"error":r.get::<_,Option<String>>(7)?,"automationRunId":r.get::<_,Option<String>>(8)?,"input":serde_json::from_str::<Value>(&r.get::<_,String>(9)?).unwrap_or(Value::Null)}))).optional()?.context("command not found")?)})
    }
    pub async fn command_run(&self, thread: &str, input: CommandRunInput) -> Result<Value> {
        let target = self.get_thread(thread)?;
        ensure!(
            target.closed_at.is_none(),
            "targetUnavailable: thread is closed"
        );
        ensure!(
            input
                .client_request_id
                .as_ref()
                .is_none_or(|s| !s.is_empty() && s.len() <= 128),
            "invalid clientRequestId"
        );
        ensure!(
            input
                .command_key
                .as_ref()
                .is_none_or(|s| !s.is_empty() && s.len() <= 120),
            "invalid commandKey"
        );
        let raw = serde_json::to_string(&input)?;
        let (id,new)=self.db.with(|c|{let tx=c.unchecked_transaction()?;
            if let Some(key)=&input.client_request_id {
                if let Some((id,saved))=tx.query_row("SELECT id,input_json FROM command_executions WHERE thread_id=?1 AND request_id=?2",params![thread,key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()? {ensure!(saved==raw,"conflict: command clientRequestId reused with different input");return Ok((id,false));}
            }
            let n:i64=tx.query_row("SELECT count(*) FROM command_executions WHERE state IN ('starting','running')",[],|r|r.get(0))?;ensure!(n<4,"conflict: device command concurrency limit reached");
            let workspace:String=tx.query_row("SELECT abs_path FROM workspaces WHERE id=?1",[&target.workspace_id],|r|r.get(0))?;
            validate_command(&input.command,&workspace)?;
            let id=insert_command(&tx,thread,&input,None,&[],&now_rfc3339())?;tx.commit()?;Ok((id,true))
        })?;
        if new {
            self.execute_command(&id).await?;
        }
        self.command_show(thread, &id)
    }
    pub(crate) fn recover_automation_commands(&self) -> Result<()> {
        self.db.with(|c|{let tx=c.unchecked_transaction()?;
            let ids={let mut q=tx.prepare("SELECT id FROM command_executions WHERE state IN ('starting','running')")?;let ids=q.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;ids};
            for id in ids {finish_command(&tx,&id,"uncertain",None,"","",Some("Supervisor restarted after command intent; external effects are unknown. Inspect before retrying."),&now_rfc3339())?;}
            tx.execute("UPDATE automation_runs SET state='uncertain',completed_at=?1,error='Supervisor restarted during plugin action; inspect before retrying.' WHERE state='running' AND json_extract(definition_json,'$.action.kind')='pluginAction'", [now_rfc3339()])?;
            // Prompt turns reconciled at boot remain interrupted, never resent.
            tx.execute("UPDATE automation_runs SET state=(SELECT status FROM thread_turns WHERE id=automation_runs.turn_id),completed_at=?1 WHERE state='running' AND turn_id IN (SELECT id FROM thread_turns WHERE status IN ('completed','failed','interrupted'))",[now_rfc3339()])?;
            tx.commit()?;Ok(())
        })
    }
    async fn execute_command(&self, id: &str) -> Result<()> {
        let prepared = self.db.with(|c| {
            let (raw, path): (String, String) = c.query_row(
                "SELECT x.input_json,w.abs_path FROM command_executions x
                 JOIN threads t ON t.id=x.thread_id JOIN workspaces w ON w.id=t.workspace_id
                 WHERE x.id=?1 AND x.state='starting'",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let input: CommandRunInput = serde_json::from_str(&raw)?;
            c.execute(
                "UPDATE command_executions SET state='running' WHERE id=?1 AND state='starting'",
                [id],
            )?;
            Ok((input, path))
        });
        let (input, workspace) = match prepared {
            Ok(value) => value,
            Err(_) => {
                return self.complete_command(
                    id,
                    "failed",
                    None,
                    "",
                    "",
                    Some("command target or intent unavailable before spawn"),
                )
            }
        };
        let cwd = match validate_command(&input.command, &workspace) {
            Ok(v) => v,
            Err(e) => {
                return self.complete_command(id, "failed", None, "", "", Some(&e.to_string()));
            }
        };
        let mut cmd = if let Some(shell) = &input.command.shell {
            #[cfg(unix)]
            let mut c = tokio::process::Command::new("/bin/sh");
            #[cfg(windows)]
            let mut c = tokio::process::Command::new("cmd.exe");
            #[cfg(unix)]
            c.arg("-c");
            #[cfg(windows)]
            c.arg("/C");
            c.arg(shell);
            c
        } else {
            let mut c = tokio::process::Command::new(&input.command.argv[0]);
            c.args(&input.command.argv[1..]);
            c
        };
        cmd.current_dir(cwd)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .envs(inherited_command_env())
            .envs(&input.command.env)
            .env("POCKYMOE_COMMAND_ID", id)
            // Hook scripts written before the rename read the old name.
            .env("REMOTE_CODEX_COMMAND_ID", id)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // No machine/managed credentials, CLI config or upstream keys are inherited.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.as_std_mut().process_group(0);
        }
        crate::child_process::hide_tokio(&mut cmd);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return self.complete_command(
                    id,
                    "failed",
                    None,
                    "",
                    "",
                    Some(&format!("spawn failed: {e}")),
                )
            }
        };
        let pid = child.id();
        let out = child.stdout.take().context("stdout unavailable")?;
        let err = child.stderr.take().context("stderr unavailable")?;
        let out = tokio::spawn(read_bounded(out));
        let err = tokio::spawn(read_bounded(err));
        let waited = tokio::time::timeout(
            std::time::Duration::from_secs(input.command.timeout_seconds),
            child.wait(),
        )
        .await;
        let (state, code, error) = match waited {
            Ok(Ok(s)) => (
                if s.success() { "completed" } else { "failed" },
                s.code(),
                None,
            ),
            Ok(Err(e)) => ("uncertain", None, Some(format!("wait failed: {e}"))),
            Err(_) => ("timedOut", None, Some("command timeout".into())),
        };
        // Clean up the owned process group, including descendants retaining pipes.
        if let Some(pid) = pid {
            kill_command_tree(pid).await;
        }
        if state == "timedOut" {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        let stdout = collect_output(out).await;
        let stderr = collect_output(err).await;
        self.complete_command(id, state, code, &stdout, &stderr, error.as_deref())
    }
    fn complete_command(
        &self,
        id: &str,
        state: &str,
        code: Option<i32>,
        out: &str,
        err: &str,
        error: Option<&str>,
    ) -> Result<()> {
        self.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            finish_command(&tx, id, state, code, out, err, error, &now_rfc3339())?;
            tx.commit()?;
            Ok(())
        })
    }
}
async fn read_bounded(mut input: impl tokio::io::AsyncRead + Unpin) -> String {
    let mut saved = vec![];
    let mut buf = [0u8; 8192];
    let mut truncated = false;
    loop {
        match input.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let take = n.min(65536usize.saturating_sub(saved.len()));
                saved.extend_from_slice(&buf[..take]);
                truncated |= take < n;
            }
        }
    }
    let mut out = String::from_utf8_lossy(&saved).into_owned();
    if truncated {
        out.push_str("\n[output truncated at 64 KiB]");
    }
    out
}
async fn collect_output(mut job: tokio::task::JoinHandle<String>) -> String {
    match tokio::time::timeout(std::time::Duration::from_secs(1), &mut job).await {
        Ok(Ok(s)) => s,
        _ => {
            job.abort();
            "[output stream did not close]".into()
        }
    }
}
pub(crate) async fn kill_command_tree(pid: u32) {
    #[cfg(unix)]
    {
        let _ = tokio::process::Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .env_clear()
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
    #[cfg(windows)]
    {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
}
#[allow(clippy::too_many_arguments)]
fn finish_command(
    c: &Connection,
    id: &str,
    state: &str,
    code: Option<i32>,
    out: &str,
    err: &str,
    error: Option<&str>,
    now: &str,
) -> Result<()> {
    let (thread,key,run,chain):(String,Option<String>,Option<String>,String)=c.query_row("SELECT thread_id,command_key,automation_run_id,ancestry_json FROM command_executions WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    let exit_error = if state == "failed" {
        Some(
            code.map(|code| format!("command exited with code {code}"))
                .unwrap_or_else(|| "command terminated without an exit status".into()),
        )
    } else {
        None
    };
    let error = error.or(exit_error.as_deref());
    let changed=c.execute("UPDATE command_executions SET state=?2,completed_at=?3,exit_code=?4,stdout=?5,stderr=?6,error=?7 WHERE id=?1 AND state IN ('starting','running')",params![id,state,now,code,out,err,error])?;
    if changed == 0 {
        return Ok(());
    }
    let workspace: Option<String> = c
        .query_row(
            "SELECT workspace_id FROM threads WHERE id=?1",
            [&thread],
            |r| r.get(0),
        )
        .optional()?;
    record_event(
        c,
        &format!("command:{id}:terminal"),
        &json!({"kind":"commandEnded","sourceThreadId":thread,"workspaceId":workspace,"commandId":id,"commandKey":key,"status":state,"exitCode":code,"ancestry":serde_json::from_str::<Value>(&chain)?}),
        now,
    )?;
    if let Some(run) = run {
        c.execute("UPDATE automation_runs SET state=?2,completed_at=?3,error=?4 WHERE id=?1 AND state='running'",params![run,state,now,error])?;
        c.execute("UPDATE automations SET error=?2,updated_at=?3 WHERE id=(SELECT automation_id FROM automation_runs WHERE id=?1)",params![run,error,now])?;
    }
    // Execution results, including script errors, are always passive.
    if workspace.is_some() {
        crate::interaction::inbox::store(
            c,
            &thread,
            &format!("command-result-{id}"),
            None,
            &format!(
                "Controlled command {id}: {state}. Exit code: {}.{}",
                code.map(|c| c.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                error.map(|e| format!(" {e}")).unwrap_or_default()
            ),
            now,
            crate::interaction::inbox::Envelope {
                subject: Some("Controlled command result"),
                kind: "result",
                in_reply_to: None,
                topic_key: None,
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "automation_tests.rs"]
pub(crate) mod tests;
