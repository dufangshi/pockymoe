//! When loaded harnesses are released (docs/harness-lifecycle.md): which
//! threads pages display, the view, idle and limit rules, explicit release,
//! and the process monitor behind `GET /api/device/processes`.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Weak;
use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat, Utc};
use pockymoe_protocol::{
    HarnessBusyReason as Busy, HarnessLoadReason, HarnessProcessesDto, HarnessResourceSettingsDto,
    LoadedThreadDto, ReleaseThreadResultDto,
};
use tokio::sync::{broadcast, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use crate::actor::{LoadedSession, ReleaseReason, ReleaseScope, SessionLifecycle, TurnRunning};
use crate::harness_processes::{ProcessKey, ProcessSample};

/// After the last viewer leaves, how long a harness loaded only for viewing stays.
pub(crate) const VIEW_RELEASE_DELAY: Duration = Duration::from_secs(10);
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(60);
const PROCESSES_CACHE: Duration = Duration::from_secs(2);
/// A load the Supervisor cannot attribute to a thread (a fork being saved, a
/// deleted thread) is released once it is this old and settled.
const ORPHAN_GRACE: chrono::TimeDelta = chrono::TimeDelta::minutes(1);
/// Limits leave loads younger than this alone; the next check revisits them.
const RECENT_LOAD: Duration = Duration::from_secs(30);
/// Threads one connection may report as displayed.
pub const MAX_VIEWED_THREADS: usize = 64;
const MIN_MEMORY_LIMIT: u64 = 256 * 1024 * 1024;
/// How long explicit release waits for an interrupted turn or a running
/// operation (such as a capability load) on the same thread.
const RELEASE_WAIT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayChange {
    pub thread_id: String,
    pub displayed: bool,
}

/// Which threads each connected page shows (`thread.view`).
#[derive(Default)]
pub(crate) struct DisplayState {
    viewers: HashMap<u64, HashSet<String>>,
    counts: HashMap<String, usize>,
    /// When each thread last stopped being displayed by anyone.
    left_at: HashMap<String, DateTime<Utc>>,
}

impl DisplayState {
    /// Replace one viewer's set; returns the threads whose display changed.
    pub(crate) fn set(
        &mut self,
        viewer: u64,
        threads: HashSet<String>,
        now: DateTime<Utc>,
    ) -> Vec<DisplayChange> {
        let previous = self.viewers.remove(&viewer).unwrap_or_default();
        let mut changes = Vec::new();
        for thread in previous.difference(&threads) {
            let count = self.counts.entry(thread.clone()).or_default();
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.counts.remove(thread);
                self.left_at.insert(thread.clone(), now);
                changes.push(DisplayChange {
                    thread_id: thread.clone(),
                    displayed: false,
                });
            }
        }
        for thread in threads.difference(&previous) {
            let count = self.counts.entry(thread.clone()).or_default();
            *count += 1;
            if *count == 1 {
                changes.push(DisplayChange {
                    thread_id: thread.clone(),
                    displayed: true,
                });
            }
        }
        if !threads.is_empty() {
            self.viewers.insert(viewer, threads);
        }
        changes.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));
        changes
    }

    pub(crate) fn clear(&mut self, viewer: u64, now: DateTime<Utc>) -> Vec<DisplayChange> {
        self.set(viewer, HashSet::new(), now)
    }

    pub(crate) fn displayed(&self, thread: &str) -> bool {
        self.counts.contains_key(thread)
    }

    pub(crate) fn left_at(&self, thread: &str) -> Option<DateTime<Utc>> {
        self.left_at.get(thread).copied()
    }

    fn prune(&mut self, before: DateTime<Utc>) {
        self.left_at.retain(|_, left| *left >= before);
    }
}

/// Settings rejected for their values (HTTP 400), not for storage failures.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct InvalidSettings(pub String);

pub struct HarnessState {
    display: std::sync::Mutex<DisplayState>,
    /// Per process: its tree while no session on it had been prompted yet.
    baselines: std::sync::Mutex<HashMap<String, HashSet<ProcessKey>>>,
    display_changes: broadcast::Sender<DisplayChange>,
    next_viewer: AtomicU64,
    /// Readers are operations that use a thread's harness; release writes.
    gates: std::sync::Mutex<HashMap<String, Weak<RwLock<()>>>>,
    processes: tokio::sync::Mutex<Option<(Instant, HarnessProcessesDto)>>,
    started: AtomicBool,
    /// [`VIEW_RELEASE_DELAY`] and [`RECENT_LOAD`]; tests shorten them.
    view_delay_ms: AtomicU64,
    recent_load_ms: AtomicU64,
}

impl HarnessState {
    fn view_delay(&self) -> Duration {
        Duration::from_millis(self.view_delay_ms.load(Ordering::Relaxed))
    }

    fn recent_load(&self) -> Duration {
        Duration::from_millis(self.recent_load_ms.load(Ordering::Relaxed))
    }
}

impl Default for HarnessState {
    fn default() -> Self {
        Self {
            display: Default::default(),
            baselines: Default::default(),
            display_changes: broadcast::channel(256).0,
            next_viewer: AtomicU64::new(1),
            gates: Default::default(),
            processes: Default::default(),
            started: AtomicBool::new(false),
            view_delay_ms: AtomicU64::new(VIEW_RELEASE_DELAY.as_millis() as u64),
            recent_load_ms: AtomicU64::new(RECENT_LOAD.as_millis() as u64),
        }
    }
}

/// One page connection's displayed threads; dropping it clears them.
pub struct HarnessViewer {
    id: u64,
    state: Weak<Supervisor>,
}

impl HarnessViewer {
    /// The complete set the page displays now (not a delta).
    pub fn set(&self, threads: impl IntoIterator<Item = String>) {
        if let Some(state) = self.state.upgrade() {
            let threads = threads
                .into_iter()
                .filter(|id| !id.is_empty() && id.len() <= 128)
                .take(MAX_VIEWED_THREADS)
                .collect();
            let changes = state
                .harness
                .display
                .lock()
                .unwrap()
                .set(self.id, threads, Utc::now());
            state.publish_display(changes);
        }
    }
}

impl Drop for HarnessViewer {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            let changes = state
                .harness
                .display
                .lock()
                .unwrap()
                .clear(self.id, Utc::now());
            state.publish_display(changes);
        }
    }
}

/// A loaded session with everything the release rules look at.
#[derive(Debug, Clone)]
pub(crate) struct Member {
    pub provider: Provider,
    pub session: LoadedSession,
    /// None for a load not saved as a thread yet, or a deleted thread.
    pub thread_id: Option<String>,
    pub displayed: bool,
    pub left_at: Option<DateTime<Utc>>,
    pub busy: Vec<Busy>,
    /// An operation such as a turn admission or capability load holds it.
    pub pinned: bool,
    /// Memory of the process tree, shared by every member of a group.
    pub bytes: Option<u64>,
}

impl Member {
    fn settled(&self) -> bool {
        !self.displayed && !self.pinned && self.busy.is_empty()
    }

    /// Idle time counts from the last activity or the last viewer leaving.
    fn idle_since(&self) -> DateTime<Utc> {
        self.left_at.map_or(self.session.last_activity_at, |left| {
            left.max(self.session.last_activity_at)
        })
    }

    /// Loaded for viewing, never prompted, and its viewer left meanwhile.
    fn view_due(&self, now: DateTime<Utc>, delay: Duration) -> bool {
        self.session.load_reason == HarnessLoadReason::View
            && !self.session.prompted
            && self.left_at.is_some_and(|left| {
                left >= self.session.requested_at
                    && now - left >= chrono::TimeDelta::from_std(delay).unwrap_or_default()
            })
    }

    fn idle_due(&self, now: DateTime<Utc>, minutes: Option<u32>) -> bool {
        minutes.is_some_and(|minutes| {
            now - self.idle_since() >= chrono::TimeDelta::minutes(i64::from(minutes))
        })
    }

    fn orphan_due(&self, now: DateTime<Utc>) -> bool {
        self.thread_id.is_none() && now - self.session.loaded_at >= ORPHAN_GRACE
    }
}

/// Sessions grouped by the process they share (a fork before its reload).
pub(crate) fn groups(members: Vec<Member>) -> Vec<Vec<Member>> {
    let mut groups: Vec<Vec<Member>> = Vec::new();
    for member in members {
        match groups.iter_mut().find(|group| {
            group[0].provider == member.provider
                && group[0].session.process_id == member.session.process_id
        }) {
            Some(group) => group.push(member),
            None => groups.push(vec![member]),
        }
    }
    groups
}

/// A shared process is released only when every session on it may go.
pub(crate) fn release_due(
    group: &[Member],
    now: DateTime<Utc>,
    idle_minutes: Option<u32>,
    view_delay: Duration,
) -> Option<ReleaseReason> {
    if group.is_empty() || !group.iter().all(Member::settled) {
        return None;
    }
    let view = |m: &Member| m.view_due(now, view_delay);
    if group.iter().any(view) && group.iter().all(|m| view(m) || m.orphan_due(now)) {
        return Some(ReleaseReason::View);
    }
    group
        .iter()
        .all(|m| view(m) || m.idle_due(now, idle_minutes) || m.orphan_due(now))
        .then_some(ReleaseReason::Idle)
}

/// Least recently used settled groups to release until both limits hold.
/// `exclude` (the load that triggered the check) and loads younger than
/// `recent` stay, so the user's current action is never undone.
pub(crate) fn limit_victims(
    groups: &[Vec<Member>],
    settings: &HarnessResourceSettingsDto,
    harness_bytes: Option<u64>,
    exclude: Option<&str>,
    now: DateTime<Utc>,
    recent: Duration,
) -> Vec<usize> {
    let recent = chrono::TimeDelta::from_std(recent).unwrap_or_default();
    let mut loaded: usize = groups.iter().map(Vec::len).sum();
    let mut bytes = harness_bytes;
    let over = |loaded: usize, bytes: Option<u64>| {
        settings
            .max_loaded_threads
            .is_some_and(|max| loaded > max as usize)
            || settings
                .max_harness_memory_bytes
                .zip(bytes)
                .is_some_and(|(max, bytes)| bytes > max)
    };
    let mut candidates: Vec<usize> = (0..groups.len())
        .filter(|index| {
            let group = &groups[*index];
            group.iter().all(Member::settled)
                && !group.iter().any(|m| {
                    Some(m.session.session_id.as_str()) == exclude
                        || now - m.session.loaded_at < recent
                })
        })
        .collect();
    candidates.sort_by_key(|index| groups[*index].iter().map(Member::idle_since).max());
    let mut victims = Vec::new();
    for index in candidates {
        if !over(loaded, bytes) {
            break;
        }
        loaded -= groups[index].len();
        bytes = bytes.map(|total| total.saturating_sub(groups[index][0].bytes.unwrap_or(0)));
        victims.push(index);
    }
    victims
}

pub(crate) fn validate_settings(settings: &HarnessResourceSettingsDto) -> Result<()> {
    let invalid = |message: &str| Err(InvalidSettings(message.into()).into());
    if settings
        .idle_release_minutes
        .is_some_and(|minutes| !(1..=1440).contains(&minutes))
    {
        return invalid("idleReleaseMinutes must be between 1 and 1440, or null");
    }
    if settings
        .max_loaded_threads
        .is_some_and(|count| !(1..=200).contains(&count))
    {
        return invalid("maxLoadedThreads must be between 1 and 200, or null");
    }
    if settings
        .max_harness_memory_bytes
        .is_some_and(|bytes| bytes < MIN_MEMORY_LIMIT)
    {
        return invalid("maxHarnessMemoryBytes must be at least 256 MiB, or null");
    }
    if settings
        .max_harness_memory_bytes
        .is_some_and(|bytes| i64::try_from(bytes).is_err())
    {
        return invalid("maxHarnessMemoryBytes is too large");
    }
    Ok(())
}

/// Native subagents still running, or a transcript scan still in progress.
pub(crate) fn native_activity(listing: &Value) -> bool {
    listing["refreshing"] == true
        || listing["agents"]
            .as_array()
            .is_some_and(|agents| agents.iter().any(|agent| agent["status"] == "running"))
}

/// Harness memory for the memory limit, counting each process once. None when
/// the sample is partly RSS on a PSS platform: it overstates memory, and
/// releasing on it would end threads that are under the limit.
pub(crate) fn limit_bytes(
    sample: &ProcessSample,
    roots: impl IntoIterator<Item = u32>,
) -> Option<u64> {
    if sample.degraded {
        return None;
    }
    let pids: HashSet<u32> = roots
        .into_iter()
        .flat_map(|root| sample.descendants(root))
        .collect();
    sample.total(pids)
}

fn rfc3339(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn add_busy(busy: &mut Vec<Busy>, reason: Busy) {
    if !busy.contains(&reason) {
        busy.push(reason);
    }
}

impl Supervisor {
    pub fn harness_viewer(self: &Arc<Self>) -> HarnessViewer {
        HarnessViewer {
            id: self.harness.next_viewer.fetch_add(1, Ordering::Relaxed),
            state: Arc::downgrade(self),
        }
    }

    /// Whether some page currently displays the thread.
    pub fn thread_displayed(&self, thread_id: &str) -> bool {
        self.harness.display.lock().unwrap().displayed(thread_id)
    }

    pub fn subscribe_display(&self) -> broadcast::Receiver<DisplayChange> {
        self.harness.display_changes.subscribe()
    }

    fn publish_display(&self, changes: Vec<DisplayChange>) {
        for change in changes {
            let _ = self.harness.display_changes.send(change);
        }
    }

    /// Run a harness load for `thread_id` with its CLI identity and reason.
    pub(crate) async fn with_load<T>(
        &self,
        thread_id: &str,
        reason: HarnessLoadReason,
        future: impl std::future::Future<Output = T>,
    ) -> T {
        self.with_cli_context(
            thread_id,
            crate::actor::with_load_intent(thread_id, reason, future),
        )
        .await
    }

    fn thread_gate(&self, thread_id: &str) -> Arc<RwLock<()>> {
        let mut gates = self.harness.gates.lock().unwrap();
        if gates.len() > 256 {
            gates.retain(|_, gate| gate.strong_count() > 0);
        }
        if let Some(gate) = gates.get(thread_id).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(RwLock::new(()));
        gates.insert(thread_id.into(), Arc::downgrade(&gate));
        gate
    }

    /// Held by operations that load or use a thread's harness, so a release
    /// never ends a process between their check and their request.
    pub(crate) async fn hold_harness(&self, thread_id: &str) -> OwnedRwLockReadGuard<()> {
        self.thread_gate(thread_id).read_owned().await
    }

    fn harness_pinned(&self, thread_id: &str, claimed: &HashSet<String>) -> bool {
        !claimed.contains(thread_id) && self.thread_gate(thread_id).try_write_owned().is_err()
    }

    fn try_claim(&self, threads: &[String]) -> Option<Vec<OwnedRwLockWriteGuard<()>>> {
        threads
            .iter()
            .map(|id| self.thread_gate(id).try_write_owned().ok())
            .collect()
    }

    /// Polls instead of queueing a writer, which would block nested readers.
    async fn claim(
        &self,
        threads: &[String],
        wait: Duration,
    ) -> Option<Vec<OwnedRwLockWriteGuard<()>>> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let Some(guards) = self.try_claim(threads) {
                return Some(guards);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn any_turn_running(&self, threads: &[String]) -> bool {
        let live = self.live.lock().await;
        threads.iter().any(|thread| live.contains_key(thread))
    }

    async fn wait_for_turn_end(&self, thread_id: &str, wait: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + wait;
        while self.live.lock().await.contains_key(thread_id) {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        true
    }

    pub fn harness_resource_settings(&self) -> Result<HarnessResourceSettingsDto> {
        self.db.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT idle_release_minutes, max_loaded_threads, max_harness_memory_bytes
                     FROM harness_resource_settings WHERE id=1",
                    [],
                    |row| {
                        Ok(HarnessResourceSettingsDto {
                            idle_release_minutes: row.get(0)?,
                            max_loaded_threads: row.get(1)?,
                            max_harness_memory_bytes: row
                                .get::<_, Option<i64>>(2)?
                                .and_then(|bytes| u64::try_from(bytes).ok()),
                        })
                    },
                )
                .optional()?
                .unwrap_or_default())
        })
    }

    /// Replace the policy, then apply it at once.
    pub fn update_harness_resource_settings(
        self: &Arc<Self>,
        settings: HarnessResourceSettingsDto,
    ) -> Result<HarnessResourceSettingsDto> {
        validate_settings(&settings)?;
        let bytes = settings
            .max_harness_memory_bytes
            .and_then(|bytes| i64::try_from(bytes).ok());
        self.db.with(|conn| {
            conn.execute(
                "INSERT INTO harness_resource_settings(id, idle_release_minutes, max_loaded_threads, max_harness_memory_bytes, updated_at)
                 VALUES (1, ?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET idle_release_minutes=excluded.idle_release_minutes,
                   max_loaded_threads=excluded.max_loaded_threads,
                   max_harness_memory_bytes=excluded.max_harness_memory_bytes,
                   updated_at=excluded.updated_at",
                params![
                    settings.idle_release_minutes,
                    settings.max_loaded_threads,
                    bytes,
                    now_rfc3339()
                ],
            )?;
            Ok(())
        })?;
        if let Ok(mut cached) = self.harness.processes.try_lock() {
            *cached = None;
        }
        let state = self.clone();
        tokio::spawn(async move {
            if let Err(error) = state.harness_tick().await {
                tracing::warn!(%error, "applying harness resource settings failed");
            }
        });
        self.harness_resource_settings()
    }

    fn threads_by_session(&self) -> Result<HashMap<String, ThreadDto>> {
        Ok(self
            .list_threads(None, true)?
            .into_iter()
            .filter_map(|thread| {
                let session = thread.provider_session_id.clone()?;
                Some((
                    crate::import_id::raw_session_id(&session).to_string(),
                    thread,
                ))
            })
            .collect())
    }

    fn session_thread(
        threads: &HashMap<String, ThreadDto>,
        session: &LoadedSession,
    ) -> Option<ThreadDto> {
        threads
            .get(crate::import_id::raw_session_id(&session.session_id))
            .cloned()
    }

    async fn native_subagent_running(&self, thread: &ThreadDto) -> bool {
        let native = matches!(thread.provider, Provider::Codex | Provider::Claude)
            || matches!(thread.agent_id.as_deref(), Some("codex" | "claude"));
        if !native {
            return false;
        }
        self.native_subagents(&thread.id, None)
            .await
            .is_ok_and(|listing| native_activity(&listing))
    }

    /// The tree a process had before any of its sessions was prompted: the
    /// runtime's snapshot at the first prompt plus the Supervisor's own.
    fn baseline(&self, session: &LoadedSession) -> HashSet<ProcessKey> {
        let mut baseline: HashSet<ProcessKey> = session.baseline.iter().copied().collect();
        if let Some(own) = self
            .harness
            .baselines
            .lock()
            .unwrap()
            .get(&session.process_id)
        {
            baseline.extend(own.iter().copied());
        }
        baseline
    }

    /// Every loaded session with its busy reasons, display and memory.
    pub(crate) async fn harness_members(
        &self,
        memory: bool,
        claimed: &HashSet<String>,
    ) -> Result<(
        Vec<Member>,
        Option<ProcessSample>,
        HashMap<String, ThreadDto>,
    )> {
        let mut sessions = Vec::new();
        for (provider, runtime) in &self.runtimes {
            for session in runtime.loaded_sessions().await {
                sessions.push((*provider, runtime.clone(), session));
            }
        }
        if sessions.is_empty() && !memory {
            return Ok((Vec::new(), None, HashMap::new()));
        }
        let by_session = self.threads_by_session()?;
        let live: HashSet<String> = self.live.lock().await.keys().cloned().collect();
        let queued: HashSet<String> = self.db.with(|conn| {
            // Held CLI steers never deliver on their own; everything else will.
            let mut stmt = conn.prepare(
                "SELECT DISTINCT thread_id FROM thread_pending_steers WHERE delivery<>'cli-steer'",
            )?;
            let ids = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(ids)
        })?;
        let sample = tokio::task::spawn_blocking(move || ProcessSample::collect(memory))
            .await
            .ok();
        if let Some(sample) = &sample {
            // Before any prompt the agent cannot have started a command.
            let mut baselines = self.harness.baselines.lock().unwrap();
            for (_, _, session) in &sessions {
                let untouched = sessions
                    .iter()
                    .filter(|(_, _, other)| other.process_id == session.process_id)
                    .all(|(_, _, other)| !other.prompted && !other.busy.contains(&Busy::Turn));
                if let (true, Some(root)) = (untouched, session.root_pid) {
                    baselines
                        .entry(session.process_id.clone())
                        .or_default()
                        .extend(sample.tree_keys(&[root]));
                }
            }
            baselines
                .retain(|process, _| sessions.iter().any(|(_, _, s)| &s.process_id == process));
        }
        let mut members = Vec::new();
        for (provider, runtime, session) in sessions {
            let thread = Self::session_thread(&by_session, &session);
            let mut busy = session.busy.clone();
            if let Some(thread) = &thread {
                if live.contains(&thread.id)
                    || matches!(thread.status.as_str(), "running" | "recovering")
                {
                    add_busy(&mut busy, Busy::Turn);
                }
                if !runtime.pending_requests(&thread.id).await.is_empty() {
                    add_busy(&mut busy, Busy::PendingRequest);
                }
                if queued.contains(&thread.id) {
                    add_busy(&mut busy, Busy::QueuedInput);
                }
            }
            let root = session.root_pid;
            // Commands the agent started (a dev server, a build) would die with it.
            if let (Some(sample), Some(root)) = (&sample, root) {
                if sample.runs_tools(root, &self.baseline(&session)) {
                    add_busy(&mut busy, Busy::BackgroundTask);
                }
            }
            // These read native transcripts, so only when nothing else holds it.
            if busy.is_empty() {
                if let Some(thread) = &thread {
                    if self.native_subagent_running(thread).await {
                        add_busy(&mut busy, Busy::Subagent);
                    } else if self.claude_schedules_pending(thread).await {
                        add_busy(&mut busy, Busy::BackgroundTask);
                    }
                }
            }
            busy.sort_by_key(|reason| *reason as u8);
            let thread_id = thread.as_ref().map(|thread| thread.id.clone());
            let (displayed, left_at) = thread_id
                .as_deref()
                .map(|id| {
                    let display = self.harness.display.lock().unwrap();
                    (display.displayed(id), display.left_at(id))
                })
                .unwrap_or_default();
            let pinned = thread_id
                .as_deref()
                .is_some_and(|id| self.harness_pinned(id, claimed));
            let bytes = match (&sample, root) {
                (Some(sample), Some(root)) => sample.total(sample.descendants(root)),
                _ => None,
            };
            members.push(Member {
                provider,
                session,
                thread_id,
                displayed,
                left_at,
                busy,
                pinned,
                bytes,
            });
        }
        let threads = by_session
            .into_values()
            .map(|thread| (thread.id.clone(), thread))
            .collect();
        Ok((members, sample, threads))
    }

    fn emit_runtime_event(&self, thread_id: &str, event_type: &str, payload: Value) {
        self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
            event_type: event_type.into(),
            thread_id: thread_id.into(),
            timestamp: now_rfc3339(),
            payload,
        });
    }

    pub(crate) fn emit_released(&self, thread_id: &str, reason: ReleaseReason) {
        self.emit_runtime_event(
            thread_id,
            "thread.runtime.released",
            json!({ "reason": reason }),
        );
    }

    pub(crate) fn emit_loaded(&self, thread_id: &str, reason: HarnessLoadReason) {
        self.emit_runtime_event(
            thread_id,
            "thread.runtime.loaded",
            json!({ "loadReason": reason }),
        );
    }

    async fn forget_processes(&self) {
        *self.harness.processes.lock().await = None;
    }

    /// Release one process group for `reason` if `still_due` holds once the
    /// threads are claimed and their state read again.
    async fn release_group(
        &self,
        group: &[Member],
        reason: ReleaseReason,
        still_due: impl Fn(&[Member]) -> bool,
    ) -> Result<bool> {
        let threads: Vec<String> = group.iter().filter_map(|m| m.thread_id.clone()).collect();
        let Some(_guards) = self.try_claim(&threads) else {
            return Ok(false);
        };
        let claimed: HashSet<String> = threads.iter().cloned().collect();
        let (members, _, _) = self.harness_members(false, &claimed).await?;
        let first = &group[0];
        let current: Vec<Member> = members
            .into_iter()
            .filter(|m| {
                m.provider == first.provider && m.session.process_id == first.session.process_id
            })
            .collect();
        if current.is_empty() || !still_due(&current) {
            return Ok(false);
        }
        let runtime = self.runtime(first.provider)?.clone();
        let released = match runtime
            .release_loaded(
                &current[0].session.session_id,
                ReleaseScope::Process,
                Some(&first.session.process_id),
            )
            .await
        {
            Ok(released) => released,
            Err(error)
                if error.is::<TurnRunning>() || error.to_string().starts_with("conflict:") =>
            {
                return Ok(false)
            }
            Err(error) => return Err(error),
        };
        self.forget_released(&released);
        for session in &released {
            if let Some(thread) = current
                .iter()
                .find(|m| m.session.session_id == session.session_id)
                .and_then(|m| m.thread_id.as_deref())
            {
                tracing::info!(thread_id = thread, ?reason, "released harness");
                self.emit_released(thread, reason);
            }
        }
        self.forget_processes().await;
        Ok(!released.is_empty())
    }

    /// The minute check: idle and view rules, abandoned loads, then limits.
    pub(crate) async fn harness_tick(&self) -> Result<()> {
        if self.update_draining.load(Ordering::SeqCst) {
            return Ok(());
        }
        let settings = self.harness_resource_settings()?;
        let (members, _, _) = self.harness_members(false, &HashSet::new()).await?;
        let now = Utc::now();
        let (idle, delay) = (settings.idle_release_minutes, self.harness.view_delay());
        for group in groups(members) {
            if let Some(reason) = release_due(&group, now, idle, delay) {
                if let Err(error) = self
                    .release_group(&group, reason, |current| {
                        release_due(current, Utc::now(), idle, delay).is_some()
                    })
                    .await
                {
                    tracing::warn!(%error, ?reason, "harness release failed");
                }
            }
        }
        self.harness
            .display
            .lock()
            .unwrap()
            .prune(now - chrono::TimeDelta::days(1));
        self.enforce_harness_limits(None).await
    }

    async fn harness_view_check(&self, thread_id: &str) -> Result<()> {
        let settings = self.harness_resource_settings()?;
        let (members, _, _) = self.harness_members(false, &HashSet::new()).await?;
        let (idle, delay) = (settings.idle_release_minutes, self.harness.view_delay());
        for group in groups(members) {
            if !group
                .iter()
                .any(|m| m.thread_id.as_deref() == Some(thread_id))
            {
                continue;
            }
            if let Some(reason) = release_due(&group, Utc::now(), idle, delay) {
                self.release_group(&group, reason, |current| {
                    release_due(current, Utc::now(), idle, delay).is_some()
                })
                .await?;
            }
        }
        Ok(())
    }

    /// Release least recently used settled harnesses above either limit.
    /// `exclude` is the session whose load triggered the check.
    pub(crate) async fn enforce_harness_limits(&self, exclude: Option<&str>) -> Result<()> {
        let settings = self.harness_resource_settings()?;
        if settings.max_loaded_threads.is_none() && settings.max_harness_memory_bytes.is_none() {
            return Ok(());
        }
        let memory = settings.max_harness_memory_bytes.is_some();
        let (members, sample, _) = self.harness_members(memory, &HashSet::new()).await?;
        let harness_bytes = sample.as_ref().filter(|_| memory).and_then(|sample| {
            limit_bytes(sample, members.iter().filter_map(|m| m.session.root_pid))
        });
        let groups = groups(members);
        let recent = self.harness.recent_load();
        for index in limit_victims(
            &groups,
            &settings,
            harness_bytes,
            exclude,
            Utc::now(),
            recent,
        ) {
            if let Err(error) = self
                .release_group(&groups[index], ReleaseReason::Limit, |current| {
                    current.iter().all(Member::settled)
                })
                .await
            {
                tracing::warn!(%error, "harness release over the limit failed");
            }
        }
        Ok(())
    }

    async fn on_harness_lifecycle(self: &Arc<Self>, event: SessionLifecycle) {
        match event {
            SessionLifecycle::Loaded {
                session_id,
                thread_id,
                reason,
                requested_at,
            } => {
                let thread = thread_id.or_else(|| {
                    self.threads_by_session().ok().and_then(|threads| {
                        threads
                            .get(crate::import_id::raw_session_id(&session_id))
                            .map(|thread| thread.id.clone())
                    })
                });
                if let Some(thread) = &thread {
                    self.emit_loaded(thread, reason);
                    let left = {
                        let display = self.harness.display.lock().unwrap();
                        (!display.displayed(thread))
                            .then(|| display.left_at(thread))
                            .flatten()
                    };
                    // The last viewer left while this view load was starting.
                    if reason == HarnessLoadReason::View
                        && left.is_some_and(|left| left >= requested_at)
                    {
                        self.schedule_view_check(thread.clone());
                    }
                }
                self.forget_processes().await;
                if let Err(error) = self.enforce_harness_limits(Some(&session_id)).await {
                    tracing::warn!(%error, "harness limit check failed");
                }
            }
            SessionLifecycle::Released {
                session_id,
                thread_id,
                reason,
                load_reason,
                prompted,
            } => {
                if let Err(error) =
                    self.forget_unsaved_sessions([(session_id.as_str(), load_reason, prompted)])
                {
                    tracing::warn!(%error, "could not forget an unsaved harness session");
                }
                let thread = thread_id.or_else(|| {
                    self.threads_by_session().ok().and_then(|threads| {
                        threads
                            .get(crate::import_id::raw_session_id(&session_id))
                            .map(|thread| thread.id.clone())
                    })
                });
                if let Some(thread) = thread {
                    self.emit_released(&thread, reason);
                }
                self.forget_processes().await;
            }
        }
    }

    fn schedule_view_check(self: &Arc<Self>, thread_id: String) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let delay = match weak.upgrade() {
                Some(state) => state.harness.view_delay(),
                None => return,
            };
            tokio::time::sleep(delay + Duration::from_millis(200)).await;
            if let Some(state) = weak.upgrade() {
                if let Err(error) = state.harness_view_check(&thread_id).await {
                    tracing::warn!(%thread_id, %error, "view release check failed");
                }
            }
        });
    }

    /// Start the release rules: load events, display changes and the minute check.
    pub fn spawn_harness_release(self: &Arc<Self>) {
        if self.harness.started.swap(true, Ordering::SeqCst) {
            return;
        }
        for runtime in self.runtimes.values() {
            let Some(mut events) = runtime.lifecycle() else {
                continue;
            };
            let weak = Arc::downgrade(self);
            tokio::spawn(async move {
                loop {
                    let event = match events.recv().await {
                        Ok(event) => event,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    let Some(state) = weak.upgrade() else {
                        break;
                    };
                    state.on_harness_lifecycle(event).await;
                }
            });
        }
        let mut display = self.subscribe_display();
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let change = match display.recv().await {
                    Ok(change) => change,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let Some(state) = weak.upgrade() else {
                    break;
                };
                if !change.displayed {
                    state.schedule_view_check(change.thread_id);
                }
            }
        });
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(IDLE_CHECK_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            loop {
                tick.tick().await;
                let Some(state) = weak.upgrade() else {
                    break;
                };
                if let Err(error) = state.harness_tick().await {
                    tracing::warn!(%error, "harness idle check failed");
                }
            }
        });
    }

    /// The thread's harness session, starting a new one with the thread's saved
    /// settings when it has none (its unsaved first session was released).
    pub(crate) async fn ensure_thread_session(&self, thread: &ThreadDto) -> Result<String> {
        if let Some(session) = &thread.provider_session_id {
            return Ok(session.clone());
        }
        let runtime = self.runtime(thread.provider)?.clone();
        let cwd = self
            .session_cwd(thread)
            .ok_or_else(|| anyhow!("workspace not found"))?;
        let started = self
            .with_load(
                &thread.id,
                HarnessLoadReason::Create,
                runtime.start_session(crate::actor::StartSessionInput {
                    cwd,
                    agent_id: thread.agent_id.clone(),
                    model: thread.model.clone().unwrap_or_else(|| "default".into()),
                    reasoning_effort: thread.reasoning_effort.clone(),
                    approval_mode: thread.approval_mode.clone(),
                    sandbox_mode: thread.sandbox_mode.clone(),
                }),
            )
            .await?;
        let saved = self.db.with(|conn| {
            Ok(conn.execute(
                "UPDATE threads SET provider_session_id=?1 WHERE id=?2 AND provider_session_id IS NULL",
                params![started.provider_session_id, thread.id],
            )?)
        })?;
        if saved == 0 {
            // Another request gave the thread a session first; use that one.
            let _ = runtime
                .release_loaded(&started.provider_session_id, ReleaseScope::Process, None)
                .await;
            return self
                .get_thread(&thread.id)?
                .provider_session_id
                .ok_or_else(|| anyhow!("thread has no provider session"));
        }
        Ok(started.provider_session_id)
    }

    /// Claude and Codex save a session with its first message. One created by
    /// this process and never prompted exists only in that process, so once it
    /// is released it can never be resumed: forget it, and the thread's first
    /// prompt starts a new one. Forks, imports and threads with turns keep theirs.
    pub(crate) fn forget_unsaved_sessions<'a>(
        &self,
        released: impl IntoIterator<Item = (&'a str, HarnessLoadReason, bool)>,
    ) -> Result<Vec<String>> {
        let unsaved: Vec<&str> = released
            .into_iter()
            .filter(|(_, reason, prompted)| *reason == HarnessLoadReason::Create && !*prompted)
            .map(|(session, _, _)| session)
            .collect();
        if unsaved.is_empty() {
            return Ok(Vec::new());
        }
        let threads = self.threads_by_session()?;
        let mut forgotten = Vec::new();
        for session in unsaved {
            let Some(thread) = threads.get(crate::import_id::raw_session_id(session)) else {
                continue;
            };
            let cleared = self.db.with(|conn| {
                Ok(conn.execute(
                    "UPDATE threads SET provider_session_id=NULL
                     WHERE id=?1 AND provider_session_id=?2
                       AND source NOT IN ('local_codex_import','local_provider_import')
                       AND NOT EXISTS(SELECT 1 FROM thread_turns WHERE thread_id=?1)",
                    params![thread.id, thread.provider_session_id],
                )?)
            })?;
            if cleared > 0 {
                forgotten.push(thread.id.clone());
            }
        }
        Ok(forgotten)
    }

    fn forget_released(&self, released: &[LoadedSession]) {
        if let Err(error) = self.forget_unsaved_sessions(
            released
                .iter()
                .map(|s| (s.session_id.as_str(), s.load_reason, s.prompted)),
        ) {
            tracing::warn!(%error, "could not forget unsaved harness sessions");
        }
    }

    /// `POST /api/threads/{id}/release`. `may_release` limits a shared caller
    /// to threads it controls; None is the owner.
    pub async fn release_thread_harness(
        &self,
        id: &str,
        interrupt: bool,
        may_release: Option<&(dyn Fn(&str) -> bool + Send + Sync)>,
    ) -> Result<ReleaseThreadResultDto> {
        let thread = self.get_thread(id)?;
        let unloaded = ReleaseThreadResultDto {
            thread_id: id.into(),
            released: false,
            also_released: Vec::new(),
        };
        let Some(session) = thread.provider_session_id.clone() else {
            return Ok(unloaded);
        };
        let runtime = self.runtime(thread.provider)?.clone();
        let loaded = runtime.loaded_sessions().await;
        let Some(target) = loaded
            .iter()
            .find(|s| session_ids_match(&s.session_id, &session))
            .cloned()
        else {
            return Ok(unloaded);
        };
        let threads = self.threads_by_session()?;
        let mut mates: Vec<String> = loaded
            .iter()
            .filter(|s| s.process_id == target.process_id)
            .filter_map(|s| Self::session_thread(&threads, s).map(|thread| thread.id))
            .collect();
        if !mates.iter().any(|mate| mate == id) {
            mates.push(id.into());
        }
        let sessions_on_process = loaded
            .iter()
            .filter(|s| s.process_id == target.process_id)
            .count();
        if let Some(may_release) = may_release {
            // A fork on the same process may be outside this caller's access.
            if mates.len() < sessions_on_process || !mates.iter().all(|mate| may_release(mate)) {
                bail!(
                    "conflict: This harness also serves a thread you cannot release; ask the owner"
                );
            }
        }
        if interrupt && self.live.lock().await.contains_key(id) {
            self.interrupt(id).await?;
            if !self.wait_for_turn_end(id, RELEASE_WAIT).await {
                return Err(TurnRunning.into());
            }
        }
        // Wait out operations such as a capability load, not a new turn.
        let deadline = tokio::time::Instant::now() + RELEASE_WAIT;
        let _guards = loop {
            if self.any_turn_running(&mates).await {
                return Err(TurnRunning.into());
            }
            if let Some(guards) = self.try_claim(&mates) {
                if self.any_turn_running(&mates).await {
                    return Err(TurnRunning.into());
                }
                break guards;
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("conflict: The harness is busy; retry shortly");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let released = runtime
            .release_loaded(
                &target.session_id,
                ReleaseScope::Process,
                Some(&target.process_id),
            )
            .await?;
        self.forget_released(&released);
        let mut released_threads = Vec::new();
        for session in &released {
            if let Some(thread) = Self::session_thread(&threads, session) {
                self.emit_released(&thread.id, ReleaseReason::Explicit);
                released_threads.push(thread.id);
            }
        }
        self.forget_processes().await;
        Ok(ReleaseThreadResultDto {
            thread_id: id.into(),
            released: !released.is_empty(),
            also_released: released_threads.into_iter().filter(|t| t != id).collect(),
        })
    }

    /// Before deleting or closing: stop a running turn, then release only this
    /// thread's session (a process shared with another thread stays).
    async fn release_for_removal(&self, thread: &ThreadDto, reason: ReleaseReason) {
        let Some(session) = thread.provider_session_id.as_deref() else {
            return;
        };
        let Ok(runtime) = self.runtime(thread.provider).cloned() else {
            return;
        };
        if let Some(turn) = self.live.lock().await.get(&thread.id) {
            turn.cancel.cancel();
        }
        self.wait_for_turn_end(&thread.id, RELEASE_WAIT).await;
        let _guards = self
            .claim(std::slice::from_ref(&thread.id), RELEASE_WAIT)
            .await;
        match runtime
            .release_loaded(session, ReleaseScope::Session, None)
            .await
        {
            Ok(released) if !released.is_empty() => {
                self.forget_released(&released);
                self.emit_released(&thread.id, reason);
                self.forget_processes().await;
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(thread_id = %thread.id, %error, "harness was not released; the idle check retries")
            }
        }
    }

    /// Delete a top-level thread and end its harness.
    pub async fn delete_thread_releasing(&self, id: &str) -> Result<()> {
        self.host_before_delete(id)?;
        if let Ok(thread) = self.get_thread(id) {
            self.release_for_removal(&thread, ReleaseReason::Deleted)
                .await;
        }
        self.delete_thread_record(id)
    }

    /// Close a delegate and end its harness.
    pub async fn close_agent_thread_releasing(
        &self,
        caller: Option<&str>,
        id: &str,
        drop_worktree: bool,
    ) -> Result<Value> {
        let closed = self.close_agent_thread(caller, id, drop_worktree)?;
        if let Ok(thread) = self.get_thread(id) {
            self.release_for_removal(&thread, ReleaseReason::Closed)
                .await;
        }
        Ok(closed)
    }

    /// `GET /api/device/processes`, sampled on request and cached briefly.
    pub async fn harness_processes(&self) -> Result<HarnessProcessesDto> {
        {
            let cached = self.harness.processes.lock().await;
            if let Some((at, value)) = cached.as_ref() {
                if at.elapsed() < PROCESSES_CACHE {
                    return Ok(value.clone());
                }
            }
        }
        let settings = self.harness_resource_settings()?;
        let (members, sample, threads) = self.harness_members(true, &HashSet::new()).await?;
        let sample = sample.ok_or_else(|| anyhow!("process table unavailable"))?;
        let mut attributed = HashSet::new();
        let mut loaded = Vec::new();
        for member in &members {
            let (Some(thread_id), Some(thread)) = (
                member.thread_id.as_ref(),
                member.thread_id.as_ref().and_then(|id| threads.get(id)),
            ) else {
                continue;
            };
            let processes = member
                .session
                .root_pid
                .map(|root| sample.harness_tree(root, &self.baseline(&member.session)))
                .unwrap_or_default();
            attributed.extend(processes.iter().map(|process| process.pid));
            let bytes = (!processes.is_empty())
                .then(|| {
                    processes
                        .iter()
                        .try_fold(0u64, |sum, process| Some(sum + process.bytes?))
                })
                .flatten();
            loaded.push(LoadedThreadDto {
                thread_id: thread_id.clone(),
                title: thread.title.clone(),
                workspace_id: thread.workspace_id.clone(),
                provider: thread.provider.as_str().into(),
                agent_id: thread.agent_id.clone(),
                load_reason: member.session.load_reason,
                loaded_at: rfc3339(member.session.loaded_at),
                last_activity_at: rfc3339(member.idle_since()),
                displayed: member.displayed,
                busy: member.busy.clone(),
                shares_process_with: members
                    .iter()
                    .filter(|other| {
                        other.provider == member.provider
                            && other.session.process_id == member.session.process_id
                            && other.thread_id != member.thread_id
                    })
                    .filter_map(|other| other.thread_id.clone())
                    .collect(),
                root_pid: member.session.root_pid,
                bytes,
                processes,
            });
        }
        loaded.sort_by(|a, b| b.last_activity_at.cmp(&a.last_activity_at));
        let harness_bytes = sample.total(attributed.iter().copied());
        let value = HarnessProcessesDto {
            sampled_at: now_rfc3339(),
            metric: sample.metric,
            supervisor_pid: sample.supervisor_pid,
            supervisor_bytes: sample.bytes(sample.supervisor_pid),
            harness_bytes,
            total_bytes: sample.total(sample.descendants(sample.supervisor_pid)),
            over_thread_limit: settings
                .max_loaded_threads
                .is_some_and(|max| loaded.len() > max as usize),
            over_memory_limit: settings
                .max_harness_memory_bytes
                .zip(harness_bytes)
                .is_some_and(|(max, bytes)| bytes > max),
            settings,
            threads: loaded,
            other: sample.unattributed(&attributed),
        };
        *self.harness.processes.lock().await = Some((Instant::now(), value.clone()));
        Ok(value)
    }
}

#[cfg(test)]
#[path = "harness_release_tests.rs"]
mod harness_release_tests;
