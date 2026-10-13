//! Loaded harness processes, their memory, and the release policy.
//!
//! `GET /api/device/processes` returns [`HarnessProcessesDto`];
//! `GET`/`PUT /api/harness-resources/settings` read and replace
//! [`HarnessResourceSettingsDto`]. Both are owner/device-scoped, like
//! `/api/device/metrics`, because they expose every thread's title.

use serde::{Deserialize, Serialize};

/// Release policy. `None` disables the corresponding rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessResourceSettingsDto {
    /// Release an undisplayed, settled thread after this many idle minutes.
    pub idle_release_minutes: Option<u32>,
    /// Release least recently used releasable threads above this count.
    pub max_loaded_threads: Option<u32>,
    /// Release least recently used releasable threads above this many bytes
    /// of harness memory (same measure as [`HarnessProcessesDto::metric`]).
    pub max_harness_memory_bytes: Option<u64>,
}

impl Default for HarnessResourceSettingsDto {
    fn default() -> Self {
        Self {
            idle_release_minutes: Some(10),
            max_loaded_threads: None,
            max_harness_memory_bytes: None,
        }
    }
}

/// How process memory was measured on this platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessMemoryMetric {
    /// Linux proportional set size: shared pages divided among sharers.
    Pss,
    /// macOS physical footprint, as shown by Activity Monitor.
    PhysFootprint,
    /// Windows private working set.
    PrivateWorkingSet,
    /// Resident set size; shared pages are counted in every process.
    Rss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HarnessProcessRole {
    /// The ACP adapter we spawned (codex-acp, claude-agent-acp, grok agent).
    Adapter,
    /// The native agent binary (Claude, codex app-server).
    Agent,
    /// Launchers and our own bridge shims between adapter and agent.
    Bridge,
    /// MCP servers started by the harness.
    Mcp,
    /// Commands the agent started with its tools.
    Tool,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessProcessDto {
    pub pid: u32,
    pub role: HarnessProcessRole,
    /// Executable basename plus a distinguishing argument; never the full
    /// command line, which may contain secrets.
    pub name: String,
    pub bytes: Option<u64>,
}

/// Why a thread's harness was loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HarnessLoadReason {
    /// A page opened the thread; released when the last viewer leaves
    /// unless a prompt was sent meanwhile.
    View,
    Create,
    Turn,
    Queue,
    Automation,
    Resume,
    Fork,
    Other,
}

/// Something that keeps a loaded thread from being released automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HarnessBusyReason {
    Turn,
    PendingRequest,
    QueuedInput,
    BackgroundTask,
    Subagent,
    Goal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedThreadDto {
    pub thread_id: String,
    pub title: String,
    pub workspace_id: String,
    pub provider: String,
    pub agent_id: Option<String>,
    pub load_reason: HarnessLoadReason,
    pub loaded_at: String,
    pub last_activity_at: String,
    /// Some frontend currently displays this thread.
    pub displayed: bool,
    /// Empty when the thread is releasable.
    pub busy: Vec<HarnessBusyReason>,
    /// Other threads served by the same process (a fork before its reload).
    pub shares_process_with: Vec<String>,
    pub root_pid: Option<u32>,
    /// Sum of `processes`; null when the platform could not measure it.
    pub bytes: Option<u64>,
    pub processes: Vec<HarnessProcessDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessProcessesDto {
    pub sampled_at: String,
    pub metric: ProcessMemoryMetric,
    pub supervisor_pid: u32,
    pub supervisor_bytes: Option<u64>,
    /// Sum over loaded threads (each process counted once).
    pub harness_bytes: Option<u64>,
    /// Supervisor plus every descendant process.
    pub total_bytes: Option<u64>,
    pub settings: HarnessResourceSettingsDto,
    pub over_thread_limit: bool,
    pub over_memory_limit: bool,
    pub threads: Vec<LoadedThreadDto>,
    /// Descendants not attributed to a loaded thread (probes, shells).
    pub other: Vec<HarnessProcessDto>,
}

/// `POST /api/threads/{id}/release` body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseThreadInput {
    /// Interrupt a running turn first instead of refusing with 409.
    #[serde(default)]
    pub interrupt: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseThreadResultDto {
    pub thread_id: String,
    /// False when nothing was loaded.
    pub released: bool,
    /// Threads whose shared process was released together with this one.
    pub also_released: Vec<String>,
}
