//! Load records and the release action for loaded ACP sessions
//! (docs/harness-lifecycle.md). The policy deciding when to release lives in
//! the Supervisor; this module only reports state and ends processes.
use super::*;
use crate::actor::{LoadedSession, ReleaseScope, SessionLifecycle, TurnRunning};
use chrono::{DateTime, Utc};
use pockymoe_protocol::{HarnessBusyReason, HarnessLoadReason};

/// How long `session/close` and the SIGTERM grace period may each take.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const TERMINATE_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub(super) struct LoadRecord {
    pub thread_id: Option<String>,
    pub reason: HarnessLoadReason,
    pub requested_at: DateTime<Utc>,
    pub loaded_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub prompted: bool,
}

impl LoadRecord {
    /// For a session started inside `with_load_intent`; `Other` outside it.
    pub fn from_intent() -> Self {
        let now = Utc::now();
        let intent = crate::actor::load_intent();
        Self {
            thread_id: intent.as_ref().map(|intent| intent.thread_id.clone()),
            reason: intent
                .as_ref()
                .map_or(HarnessLoadReason::Other, |intent| intent.reason),
            requested_at: intent.map_or(now, |intent| intent.requested_at),
            loaded_at: now,
            last_activity_at: now,
            prompted: false,
        }
    }

    /// A fork served by its parent's process. Its thread row does not exist yet.
    pub fn fork() -> Self {
        Self {
            thread_id: None,
            reason: HarnessLoadReason::Fork,
            ..Self::from_intent()
        }
    }

    pub fn touch(&mut self) {
        self.last_activity_at = Utc::now();
    }

    pub fn prompt(&mut self) {
        self.touch();
        self.prompted = true;
    }
}

impl AcpRuntime {
    pub(super) fn announce_load(&self, session_id: &str, live: &LiveSession) {
        let _ = self.inner.load_events.send(SessionLifecycle::Loaded {
            session_id: session_id.into(),
            thread_id: live.load.thread_id.clone(),
            reason: live.load.reason,
            requested_at: live.load.requested_at,
        });
    }

    pub(super) fn announce_release(
        &self,
        session_id: &str,
        live: &LiveSession,
        reason: crate::actor::ReleaseReason,
    ) {
        let _ = self.inner.load_events.send(SessionLifecycle::Released {
            session_id: session_id.into(),
            thread_id: live.load.thread_id.clone(),
            reason,
        });
    }

    /// Approvals and answers belong to running turns.
    pub(super) async fn touch_active_sessions(&self) {
        for live in self.inner.sessions.lock().await.values_mut() {
            if live.active.is_some() {
                live.load.touch();
            }
        }
    }

    pub(super) async fn loaded(&self) -> Vec<LoadedSession> {
        let mut loaded: Vec<(LoadedSession, Option<String>)> = {
            let sessions = self.inner.sessions.lock().await;
            sessions
                .iter()
                .map(|(key, live)| {
                    let active_thread = live.active.as_ref().map(|turn| turn.thread_id.clone());
                    (
                        LoadedSession {
                            session_id: key.clone(),
                            thread_id: live.load.thread_id.clone(),
                            process_id: live.process.id.clone(),
                            root_pid: live.process.pid,
                            load_reason: live.load.reason,
                            requested_at: live.load.requested_at,
                            loaded_at: live.load.loaded_at,
                            last_activity_at: live.load.last_activity_at,
                            prompted: live.load.prompted,
                            busy: session_busy(
                                live,
                                self.inner.terminals.running(&live.process.id),
                            ),
                        },
                        active_thread.or_else(|| live.load.thread_id.clone()),
                    )
                })
                .collect()
        };
        let pending = self.inner.pending_dtos.lock().await;
        for (session, thread) in &mut loaded {
            if thread
                .as_ref()
                .and_then(|thread| pending.get(thread))
                .is_some_and(|requests| !requests.is_empty())
            {
                session.busy.push(HarnessBusyReason::PendingRequest);
                session.busy.sort_by_key(|reason| *reason as u8);
            }
        }
        loaded.into_iter().map(|(session, _)| session).collect()
    }

    /// Remove the sessions from the table under the lifecycle lock, so no load
    /// of the same thread can race a Codex writer that is still exiting.
    pub(super) async fn release(
        &self,
        session_id: &str,
        scope: ReleaseScope,
    ) -> Result<Vec<LoadedSession>> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let before = self.loaded().await;
        let mut sessions = self.inner.sessions.lock().await;
        let Some(key) = sessions
            .iter()
            .find(|(key, live)| {
                session_ids_match(key, session_id)
                    || session_ids_match(&live.session_id, session_id)
            })
            .map(|(key, _)| key.clone())
        else {
            return Ok(Vec::new());
        };
        let process = sessions[&key].process.clone();
        let sharing: Vec<String> = sessions
            .iter()
            .filter(|(_, live)| Arc::ptr_eq(&live.process, &process))
            .map(|(key, _)| key.clone())
            .collect();
        let keys = match scope {
            ReleaseScope::Process => sharing.clone(),
            ReleaseScope::Session => vec![key.clone()],
        };
        let mut guards = Vec::new();
        for key in &keys {
            let live = &sessions[key];
            if live.active.is_some() {
                return Err(TurnRunning.into());
            }
            guards.push(
                live.operation
                    .clone()
                    .try_lock_owned()
                    .map_err(|_| anyhow!("conflict: The session is busy."))?,
            );
        }
        let stop_process = keys.len() == sharing.len();
        let removed: Vec<(String, LiveSession)> = keys
            .iter()
            .filter_map(|key| sessions.remove(key).map(|live| (key.clone(), live)))
            .collect();
        drop(sessions);
        for (_, live) in &removed {
            if live.negotiated.close {
                // codex-acp unsubscribes the native thread, which drops its writer lock.
                let closed = process
                    .request_with_timeout(
                        "session/close",
                        json!({ "sessionId": live.session_id }),
                        Some(CLOSE_TIMEOUT),
                    )
                    .await;
                if let Err(error) = closed {
                    tracing::debug!(%error, session = %live.session_id, "ACP session/close failed");
                }
            }
        }
        if stop_process {
            process.terminate(TERMINATE_GRACE).await?;
            self.inner.terminals.release_owner(&process.id);
        }
        let mut commands = self.inner.commands.lock().await;
        for (_, live) in &removed {
            commands.remove(&(process.id.clone(), live.session_id.clone()));
        }
        drop(commands);
        drop(guards);
        let removed: Vec<_> = removed.into_iter().map(|(key, _)| key).collect();
        Ok(before
            .into_iter()
            .filter(|session| removed.contains(&session.session_id))
            .collect())
    }
}

/// Busy reasons visible on the session itself. Pending requests are added by
/// the caller from the request table.
pub(super) fn session_busy(live: &LiveSession, running_terminals: usize) -> Vec<HarnessBusyReason> {
    let mut busy = Vec::new();
    if let Some(turn) = &live.active {
        busy.push(HarnessBusyReason::Turn);
        if turn.pending_prompts > 0 {
            busy.push(HarnessBusyReason::QueuedInput);
        }
    }
    if live
        .active
        .as_ref()
        .is_some_and(|turn| turn.background_owned)
        || live.claude_lifecycle.holds_background_work()
        || live.dsh_running
        || running_terminals > 0
    {
        busy.push(HarnessBusyReason::BackgroundTask);
    }
    if !live.active_subagents.is_empty() {
        busy.push(HarnessBusyReason::Subagent);
    }
    let dsh_goal = live
        .dsh_projections
        .get("goal")
        .and_then(super::super::deepseek::goal_state)
        .is_some_and(|goal| goal.status == "active");
    if dsh_goal
        || live
            .goal
            .as_ref()
            .is_some_and(|goal| goal.status == "active")
    {
        busy.push(HarnessBusyReason::Goal);
    }
    busy
}
