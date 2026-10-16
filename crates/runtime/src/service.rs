pub(crate) mod automation;
mod automation_statistics;
mod child_delete;
mod claude_history;
mod claude_reply_phases;
mod generation;
mod harness_release;
mod inbox_reminder;
pub(crate) mod peer_work;
mod reliability;
mod search;
mod update;
mod watches;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Result};
use pockymoe_protocol::{
    now_rfc3339, truncate_title, AgentBackendDto, AgentCapabilitySnapshotDto, CreateThreadInput,
    CreateWorkspaceInput, ImportThreadCandidateDto, ImportThreadInput, ModelOptionDto, Provider,
    SendThreadPromptInput, ThreadActivityDto, ThreadDetailDto, ThreadDto, ThreadForkTurnOptionDto,
    ThreadGoalDto, ThreadHistoryItemDto, ThreadPendingSteerDto, ThreadTurnDto,
    ThreadWorkspaceFilePreviewDto, ThreadWorkspaceTreeNodeDto, UpdateWorkspaceSettingsInput,
    WorkspaceDto, WorkspaceSettingsDto,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::actor::{
    EventBus, GoalState, ImportSessionMeta, PromptImage, SessionSettings, SharedRuntime,
    StartSessionInput, StartTurnInput,
};
use crate::config::RuntimeConfig;
use crate::db::Database;
use crate::files;
use crate::history::{summarize_completed_turn, turn_activity, ActivityRow};
use crate::import_id::{
    bind_import_target, parse_session_ref, scoped_session_id, session_ids_match,
};
use crate::local_sessions::{find_local_session, list_local_sessions, LocalSessionHomes};
pub use harness_release::{DisplayChange, HarnessViewer, InvalidSettings, MAX_VIEWED_THREADS};
use pockymoe_protocol::HarnessLoadReason;

fn thread_session_settings(thread: &ThreadDto) -> SessionSettings {
    SessionSettings {
        model: thread.model.clone(),
        effort: thread.reasoning_effort.clone(),
        sandbox_mode: thread.sandbox_mode.clone(),
        collaboration_mode: Some(thread.collaboration_mode.clone()),
        approval_mode: Some(thread.approval_mode.clone()),
        performance_mode: thread.fast_mode.then_some(true),
    }
}

struct GoalSubmission {
    wire_prompt: String,
    goal: GoalState,
    token_budget: Option<Option<u64>>,
    accepted: tokio::sync::oneshot::Sender<()>,
}

const RESTART_INTERRUPTED_ERROR: &str =
    "Backend completion was not confirmed before the Supervisor connection ended. Reconnect to verify before continuing.";

fn goal_status_is_terminal(status: &str) -> bool {
    matches!(status, "complete" | "terminated")
}

struct LiveTurn {
    turn_id: String,
    cancel: CancellationToken,
}

struct PendingSteerRecord {
    id: String,
    turn_id: String,
    submitted_prompt: String,
    display_prompt: String,
    delivery: String,
}

fn sqlite_table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1",
            params![table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn upsert_legacy_thread_goal(
    conn: &Connection,
    local_thread_id: &str,
    goal: &ThreadGoalDto,
) -> Result<()> {
    if !sqlite_table_exists(conn, "thread_goals")? {
        return Ok(());
    }
    let Some(local_goal_id) = goal.local_goal_id.as_deref() else {
        return Ok(());
    };
    let provider_session_id = conn
        .query_row(
            "SELECT provider_session_id FROM threads WHERE id=?1",
            params![local_thread_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let Some(provider_session_id) = provider_session_id else {
        return Ok(());
    };

    if let Some(owner) = conn
        .query_row(
            "SELECT thread_id FROM thread_goals WHERE id=?1",
            params![local_goal_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        if owner != local_thread_id {
            bail!(
                "legacy goal id `{local_goal_id}` belongs to thread `{owner}`, not `{local_thread_id}`"
            );
        }
    }

    // A new current snapshot replaces all older active histories. Keeping the rows and marking
    // them terminated prevents an old goal from resurfacing after rolling back to Node.
    conn.execute(
        "UPDATE thread_goals
         SET status='terminated', completed_at=?1, updated_at=?1
         WHERE thread_id=?2 AND id<>?3
           AND status IN ('active','paused','budgetLimited')",
        params![goal.updated_at, local_thread_id, local_goal_id],
    )?;

    let token_budget = goal.token_budget.map(i64::try_from).transpose()?;
    let tokens_used = i64::try_from(goal.tokens_used)?;
    let time_used_seconds = i64::try_from(goal.time_used_seconds)?;
    let completed_at = if goal_status_is_terminal(&goal.status) {
        goal.completed_at
            .as_deref()
            .or(Some(goal.updated_at.as_str()))
    } else {
        None
    };
    let updated = conn.execute(
        "UPDATE thread_goals
         SET provider_session_id=?1, objective=?2, status=?3, token_budget=?4,
             tokens_used=?5, time_used_seconds=?6, started_at=?7,
             completed_at=?8, updated_at=?9
         WHERE id=?10 AND thread_id=?11",
        params![
            provider_session_id,
            goal.objective,
            goal.status,
            token_budget,
            tokens_used,
            time_used_seconds,
            goal.created_at,
            completed_at,
            goal.updated_at,
            local_goal_id,
            local_thread_id,
        ],
    )?;
    if updated == 0 {
        conn.execute(
            "INSERT INTO thread_goals(
               id,thread_id,provider_session_id,objective,status,token_budget,
               tokens_used,time_used_seconds,started_at,completed_at,created_at,updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                local_goal_id,
                local_thread_id,
                provider_session_id,
                goal.objective,
                goal.status,
                token_budget,
                tokens_used,
                time_used_seconds,
                goal.created_at,
                completed_at,
                goal.created_at,
                goal.updated_at,
            ],
        )?;
    }
    Ok(())
}

fn legacy_turn_metadata_has_display_prompt(conn: &Connection) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('thread_turn_metadata') WHERE name='display_prompt'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

#[allow(clippy::too_many_arguments)]
fn upsert_legacy_turn_metadata(
    conn: &Connection,
    thread_id: &str,
    turn_id: &str,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    display_prompt: Option<&str>,
    created_at: Option<&str>,
    updated_at: &str,
) -> Result<()> {
    if !sqlite_table_exists(conn, "thread_turn_metadata")? {
        return Ok(());
    }
    let existing_id = conn
        .query_row(
            "SELECT id FROM thread_turn_metadata WHERE thread_id=?1 AND turn_id=?2",
            params![thread_id, turn_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let has_display_prompt = legacy_turn_metadata_has_display_prompt(conn)?;
    if let Some(id) = existing_id {
        if has_display_prompt {
            conn.execute(
                "UPDATE thread_turn_metadata
                 SET model=COALESCE(?1,model), reasoning_effort=COALESCE(?2,reasoning_effort),
                     display_prompt=COALESCE(?3,display_prompt), updated_at=?4
                 WHERE id=?5",
                params![model, reasoning_effort, display_prompt, updated_at, id],
            )?;
        } else {
            conn.execute(
                "UPDATE thread_turn_metadata
                 SET model=COALESCE(?1,model), reasoning_effort=COALESCE(?2,reasoning_effort),
                     updated_at=?3 WHERE id=?4",
                params![model, reasoning_effort, updated_at, id],
            )?;
        }
        return Ok(());
    }

    let id = Uuid::new_v4().to_string();
    let created_at = created_at.unwrap_or(updated_at);
    if has_display_prompt {
        conn.execute(
            "INSERT INTO thread_turn_metadata(
               id, thread_id, turn_id, model, reasoning_effort,
               display_prompt, created_at, updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                id,
                thread_id,
                turn_id,
                model,
                reasoning_effort,
                display_prompt,
                created_at,
                updated_at
            ],
        )?;
    } else {
        conn.execute(
            "INSERT INTO thread_turn_metadata(
               id, thread_id, turn_id, model, reasoning_effort, created_at, updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                id,
                thread_id,
                turn_id,
                model,
                reasoning_effort,
                created_at,
                updated_at
            ],
        )?;
    }
    Ok(())
}

pub struct UploadedPromptAttachment {
    pub kind: String,
    pub original_name: String,
    pub placeholder: String,
    pub bytes: Vec<u8>,
}

pub struct Supervisor {
    automation_gate: tokio::sync::Mutex<()>,
    pub(crate) plugins: crate::plugins::PluginHost,
    pub upstream_gate: Arc<tokio::sync::Mutex<()>>,
    pub harness_install_gate: Arc<tokio::sync::Mutex<()>>,
    pub interaction: crate::interaction::InteractionState,
    pub relay_connected: std::sync::atomic::AtomicBool,
    pub started_at: String,
    pub started_instant: std::time::Instant,
    pub config: RuntimeConfig,
    pub host_capabilities: crate::host_mode::HostCapabilities,
    pub db: Database,
    pub bus: EventBus,
    runtimes: HashMap<Provider, SharedRuntime>,
    live: Mutex<HashMap<String, LiveTurn>>,
    pub harness_gates: std::sync::Mutex<HashMap<String, Arc<tokio::sync::RwLock<()>>>>,
    pub supervisor_update_status: std::sync::Mutex<Option<Value>>,
    pub update_lock: Arc<Mutex<()>>,
    update_draining: std::sync::atomic::AtomicBool,
    update_recovering: std::sync::Mutex<HashSet<String>>,
    pub maintenance_gate: Arc<tokio::sync::RwLock<()>>,
    pub management_jobs: std::sync::Mutex<HashMap<String, Value>>,
    steer_locks: Mutex<HashMap<String, std::sync::Weak<Mutex<()>>>>,
    pub(crate) local_session_homes: LocalSessionHomes,
    usage_history: crate::usage_history::UsageHistoryCache,
    generation: generation::GenerationCache,
    claude_history: claude_history::HistoryCache,
    claude_reply_phases: claude_reply_phases::ReplyPhaseCache,
    pub(crate) native_subagents:
        Arc<std::sync::Mutex<crate::native_subagents::NativeSubagentsCache>>,
    pub subscription_usage: crate::subscription::SubscriptionUsage,
    pub device_monitor: crate::device_metrics::DeviceMonitor,
    pub(crate) harness: harness_release::HarnessState,
    /// The optional local-mode passcode, cached from `kv`.
    pub local_access: std::sync::RwLock<Option<crate::local_access::LocalAccess>>,
}

fn harness_context_key(thread_id: &str) -> String {
    format!("harness-context:{thread_id}")
}

impl Supervisor {
    pub async fn invalidate_models(&self, id: &str) {
        for runtime in self.runtimes.values() {
            runtime.invalidate_models(id).await;
        }
    }
    pub fn harness_gate(&self, id: &str) -> Arc<tokio::sync::RwLock<()>> {
        self.harness_gates
            .lock()
            .unwrap()
            .entry(id.into())
            .or_default()
            .clone()
    }
    pub async fn restart_harness(&self, id: &str) -> Result<usize> {
        let mut count = 0;
        for runtime in self.runtimes.values() {
            count += runtime.restart(id).await?;
        }
        Ok(count)
    }

    pub fn new(config: RuntimeConfig, db: Database, runtimes: Vec<SharedRuntime>) -> Self {
        let map = runtimes
            .into_iter()
            .map(|runtime| (runtime.provider(), runtime))
            .collect();
        let local_access = crate::local_access::load(&db).unwrap_or_else(|error| {
            tracing::warn!(%error, "ignoring an unreadable local passcode");
            None
        });
        let supervisor = Self {
            local_access: std::sync::RwLock::new(local_access),
            automation_gate: Default::default(),
            plugins: Default::default(),
            upstream_gate: Default::default(),
            harness_install_gate: Default::default(),
            started_at: now_rfc3339(),
            started_instant: std::time::Instant::now(),
            interaction: Default::default(),
            relay_connected: Default::default(),
            host_capabilities: config.host_mode.capabilities(),
            config,
            db,
            bus: EventBus::new(),
            runtimes: map,
            live: Mutex::new(HashMap::new()),
            harness_gates: Default::default(),
            management_jobs: Default::default(),
            maintenance_gate: Default::default(),
            update_lock: Default::default(),
            supervisor_update_status: Default::default(),
            update_draining: Default::default(),
            update_recovering: Default::default(),
            steer_locks: Mutex::new(HashMap::new()),
            local_session_homes: LocalSessionHomes::from_env(),
            usage_history: Default::default(),
            generation: Default::default(),
            claude_history: Default::default(),
            claude_reply_phases: Default::default(),
            native_subagents: Default::default(),
            subscription_usage: Default::default(),
            device_monitor: Default::default(),
            harness: Default::default(),
        };
        if let Err(error) = supervisor.reconcile_stale_turns(None, false) {
            tracing::warn!(%error, "failed to reconcile stale turns at startup");
        }
        if let Err(error) = supervisor.recover_automation_commands() {
            tracing::warn!(%error, "automation command recovery failed");
        }
        supervisor
    }

    pub fn with_local_session_homes(mut self, homes: LocalSessionHomes) -> Self {
        self.local_session_homes = homes;
        self
    }

    pub fn spawn_live_item_persister(self: &Arc<Self>) {
        // Replace the hook on repeated registration, and avoid keeping the
        // supervisor alive through its own event bus.
        let supervisor = Arc::downgrade(self);
        self.bus.set_persister(Arc::new(move |event| {
            let Some(supervisor) = supervisor.upgrade() else {
                return Ok(());
            };
            match event.event_type.as_str() {
                "thread.turn.completed" => {
                    let Some(turn_id) = event.payload["turnId"].as_str() else {
                        return Ok(());
                    };
                    let status = event.payload["status"].as_str().unwrap_or("recovering");
                    supervisor.persist_turn_result(
                        &event.thread_id,
                        turn_id,
                        status,
                        event.payload["error"].as_str(),
                        &[],
                        &event.timestamp,
                    )
                }
                "runtime.usage.updated" => supervisor.persist_usage_event(event),
                "thread.context.delivered" => match event.payload["providerSessionId"].as_str() {
                    Some(session) => supervisor
                        .db
                        .set_kv(&harness_context_key(&event.thread_id), session),
                    None => Ok(()),
                },
                // The harness itself left plan mode (an approved plan review);
                // the next turn must not switch it back on.
                "thread.collaboration.updated" => supervisor.db.with(|conn| {
                    if let Some(mode @ ("default" | "plan")) =
                        event.payload["collaborationMode"].as_str()
                    {
                        conn.execute(
                            "UPDATE threads SET collaboration_mode=?1 WHERE id=?2",
                            params![mode, event.thread_id],
                        )?;
                    }
                    Ok(())
                }),
                "thread.harness.ready" => supervisor.db.with(|conn| {
                    if let (Some(turn), Some(instance)) = (
                        event.payload["turnId"].as_str(),
                        event.payload["instanceId"].as_str(),
                    ) {
                        conn.execute(
                            "INSERT OR REPLACE INTO kv(key,value) VALUES(?1,?2)",
                            params![format!("turn-process:{turn}"), instance],
                        )?;
                    }
                    Ok(())
                }),
                "thread.context.updated" => supervisor.db.with(|conn| {
                    if let Some(context) = event
                        .payload
                        .get("contextUsage")
                        .filter(|v| v["availability"] == "available")
                    {
                        conn.execute(
                            "UPDATE threads SET context_usage_json=?1 WHERE id=?2",
                            params![context.to_string(), event.thread_id],
                        )?;
                    }
                    Ok(())
                }),
                "thread.output.delta" => supervisor.append_history_delta(event),
                "thread.item.started" | "thread.item.completed" => {
                    let Some(turn_id) = event.payload.get("turnId").and_then(Value::as_str) else {
                        return Ok(());
                    };
                    let Some(item) = event.payload.get("item").cloned() else {
                        return Ok(());
                    };
                    let item = serde_json::from_value::<ThreadHistoryItemDto>(item)?;
                    supervisor.upsert_history_item(&event.thread_id, turn_id, &item)?;
                    crate::history::defer_tool_details(&mut event.payload);
                    Ok(())
                }
                _ => Ok(()),
            }?;
            supervisor.observe_generation_event(event)
        }));
    }

    fn append_history_delta(
        &self,
        event: &mut pockymoe_protocol::ThreadEventEnvelope,
    ) -> Result<()> {
        let (Some(turn_id), Some(item_id), Some(delta)) = (
            event.payload.get("turnId").and_then(Value::as_str),
            event.payload.get("itemId").and_then(Value::as_str),
            event.payload.get("delta").and_then(Value::as_str),
        ) else {
            return Ok(());
        };
        if delta.is_empty() {
            return Ok(());
        }
        let text = self.db.with(|conn| {
            // Ignore late output once the runtime has saved its complete snapshot;
            // appending it would duplicate text in the final reply.
            let running = conn
                .query_row(
                    "SELECT status='inProgress' FROM thread_turns WHERE thread_id=?1 AND id=?2",
                    params![event.thread_id, turn_id],
                    |row| row.get::<_, bool>(0),
                )
                .optional()?
                .unwrap_or(false);
            if !running {
                return Ok(None);
            }
            let existing: Option<String> = conn
                .query_row(
                    "SELECT item_json FROM thread_history_items
                     WHERE thread_id=?1 AND turn_id=?2 AND item_id=?3",
                    params![event.thread_id, turn_id, item_id],
                    |row| row.get(0),
                )
                .optional()?;
            let mut item = if let Some(raw) = existing {
                serde_json::from_str::<ThreadHistoryItemDto>(&raw)?
            } else {
                ThreadHistoryItemDto {
                    id: item_id.into(),
                    created_at: Some(
                        event
                            .payload
                            .get("createdAt")
                            .and_then(Value::as_str)
                            .unwrap_or(&event.timestamp)
                            .into(),
                    ),
                    kind: pockymoe_protocol::ThreadItemKind::AgentMessage,
                    text: String::new(),
                    preview_text: None,
                    detail_text: None,
                    status: Some("running".into()),
                    sequence: event.payload.get("sequence").and_then(Value::as_i64),
                    source_turn_id: Some(turn_id.into()),
                    artifact: None,
                    extra: Default::default(),
                }
            };
            if matches!(
                item.status.as_deref(),
                Some("completed" | "failed" | "interrupted")
            ) {
                return Ok(None);
            }
            item.text.push_str(delta);
            conn.execute(
                "INSERT INTO thread_history_items(id, thread_id, turn_id, item_id, item_json, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(thread_id, turn_id, item_id) DO UPDATE SET
                    item_json=excluded.item_json,
                    updated_at=excluded.updated_at",
                params![
                    Uuid::new_v4().to_string(), event.thread_id, turn_id, item_id,
                    serde_json::to_string(&item)?, item.created_at, event.timestamp,
                ],
            )?;
            Ok(Some(item.text))
        })?;
        if let Some(text) = text {
            // A refresh can read a snapshot before the corresponding websocket
            // event arrives. Sending the saved prefix lets the client merge that
            // event idempotently instead of appending the same delta twice.
            event.payload["text"] = Value::String(text);
        }
        Ok(())
    }

    fn upsert_history_item(
        &self,
        thread_id: &str,
        turn_id: &str,
        item: &ThreadHistoryItemDto,
    ) -> Result<()> {
        let now = item.created_at.clone().unwrap_or_else(now_rfc3339);
        self.db.with(|conn| {
            conn.execute(
                "INSERT INTO thread_history_items(id, thread_id, turn_id, item_id, item_json, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?6)
                 ON CONFLICT(thread_id, turn_id, item_id) DO UPDATE SET
                    item_json=excluded.item_json,
                    updated_at=excluded.updated_at
                 WHERE COALESCE(json_extract(thread_history_items.item_json, '$.status'), '')
                           NOT IN ('completed', 'failed', 'interrupted')
                    OR json_extract(excluded.item_json, '$.status')
                           IN ('completed', 'failed', 'interrupted')",
                params![
                    Uuid::new_v4().to_string(),
                    thread_id,
                    turn_id,
                    item.id,
                    serde_json::to_string(item)?,
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub(crate) fn persist_usage_event(
        &self,
        event: &pockymoe_protocol::ThreadEventEnvelope,
    ) -> Result<()> {
        let Some(turn_id) = event.payload.get("turnId").and_then(Value::as_str) else {
            return Ok(());
        };
        let Some(raw) = event.payload.get("usage") else {
            return Ok(());
        };
        let Some(mut usage) = crate::usage::normalize_usage(raw) else {
            return Ok(());
        };
        if let Some(context) = crate::usage::context_usage(raw, &event.timestamp) {
            self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadContextUpdated,
                thread_id: event.thread_id.clone(),
                timestamp: event.timestamp.clone(),
                payload: json!({"contextUsage":context}),
            });
        }
        let catalog = self.model_pricing();
        let payload = self.db.with(|conn| {
            let row = conn.query_row(
                "SELECT model, reasoning_effort, token_usage_json, pricing_model_key, pricing_tier_key FROM thread_turns WHERE thread_id=?1 AND id=?2",
                params![event.thread_id, turn_id],
                |row| Ok((row.get::<_, Option<String>>(0)?,row.get::<_, Option<String>>(1)?,row.get::<_, Option<String>>(2)?,row.get::<_, Option<String>>(3)?,row.get::<_, Option<String>>(4)?)),
            ).optional()?;
            let Some((model, effort, previous, pricing_model, tier)) = row else { return Ok(None); };
            let reported_model = raw.get("model").and_then(Value::as_str).filter(|model| !model.is_empty());
            let model = reported_model.map(str::to_string).or(model);
            let effort = raw.get("reasoningEffort").and_then(Value::as_str).map(str::to_string).or(effort);
            let pricing_model = reported_model.map(str::to_string).or(pricing_model);
            let tier = raw.get("pricingTierKey").and_then(Value::as_str).filter(|tier| matches!(*tier,"standard" | "fast")).map(str::to_string).or(tier);
            let previous = previous.and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
            if let Some(speed) = previous.as_ref().and_then(|v| v.get("generationSpeed")) {
                usage["generationSpeed"] = speed.clone();
            }
            // Local Codex rollouts provide the complete turn; do not replace them with
            // codex-acp's final-request-only PromptResponse.usage fallback.
            if previous.as_ref().and_then(|v| v.get("source")).and_then(Value::as_str) == Some("codexRollout")
                && raw.get("source").and_then(Value::as_str) != Some("codexRollout") { return Ok(None); }
            // Claude native assistant records give per-response snapshots during
            // tool work. A late ACP report must not replace that turn total with
            // only the final request's usage or mix accounting sources.
            if previous.as_ref().and_then(|v| v.get("source")).and_then(Value::as_str) == Some("claudeRollout")
                && raw.get("source").and_then(Value::as_str) != Some("claudeRollout") { return Ok(None); }
            if usage["cumulative"] == true {
                let baseline = usage.get("baselineTotal").and_then(crate::usage::Tokens::parse)
                    .or_else(|| previous.as_ref().and_then(|v| v.get("baselineTotal")).and_then(crate::usage::Tokens::parse))
                    .or_else(|| {
                        conn.query_row("SELECT token_usage_json FROM thread_turns WHERE thread_id=?1 AND id<>?2 AND token_usage_json IS NOT NULL ORDER BY ordinal DESC LIMIT 1",params![event.thread_id,turn_id],|row| row.get::<_,String>(0)).ok()
                            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                            .and_then(|v| v.get("cumulativeTotal").and_then(crate::usage::Tokens::parse))
                    }).unwrap_or_default();
                let total = crate::usage::Tokens::parse(&usage["total"]).unwrap_or_default();
                usage["cumulativeTotal"] = json!(total);
                usage["baselineTotal"] = json!(baseline);
                let turn_total = previous.as_ref().and_then(|previous| {
                    let previous_counter = crate::usage::Tokens::parse(&previous["cumulativeTotal"])?;
                    let previous_turn = crate::usage::Tokens::parse(&previous["total"])?;
                    Some(previous_turn.add(&total.cumulative_delta(&previous_counter, crate::usage::Tokens::parse(&usage["last"]).as_ref())))
                }).unwrap_or_else(|| total.cumulative_delta(&baseline, crate::usage::Tokens::parse(&usage["last"]).as_ref()));
                usage["total"] = json!(turn_total);
            }
            if usage["modelContextWindow"].is_null() {
                usage["modelContextWindow"] = previous.as_ref().and_then(|v| v.get("modelContextWindow")).cloned().unwrap_or(Value::Null);
            }
            if let Some(source) = raw.get("source") { usage["source"] = source.clone(); }
            let pricing_model = pricing_model.as_deref().or(model.as_deref());
            let price = crate::usage::estimate_price_with_catalog(&usage, pricing_model, tier.as_deref(), &catalog, Some(&event.timestamp));
            // Accumulate request prices, so crossing the long-context threshold later
            // does not retrospectively change the rates of earlier requests.
            let price = if usage.get("requests").is_some_and(|v| v.is_array()) {
                // Complete native request snapshots already include parent and
                // child prices. Adding a total delta would bill them again.
                price
            } else if let (Some(previous), Some(mut current_price)) = (previous.as_ref(), price.clone()) {
                if let (Some(previous_total), Some(current_total), Some(previous_price)) = (
                    previous.get("total").and_then(crate::usage::Tokens::parse),
                    usage.get("total").and_then(crate::usage::Tokens::parse),
                    previous.get("priceEstimate").filter(|v| v.is_object() && (v["ratesSignature"] == current_price["ratesSignature"] || v.get("ratesSignature").is_none())),
                ) {
                    if current_total.total_tokens >= previous_total.total_tokens {
                        let mut delta_usage = usage.clone();
                        delta_usage["total"] = json!(current_total.subtract(&previous_total));
                        if let Some(delta_price) = crate::usage::estimate_price_with_catalog(&delta_usage, pricing_model, tier.as_deref(), &catalog, Some(&event.timestamp)) {
                            for field in ["inputUsd","cachedInputUsd","cacheWriteInputUsd","outputUsd","totalUsd"] {
                                current_price[field] = json!(previous_price[field].as_f64().unwrap_or(0.0) + delta_price[field].as_f64().unwrap_or(0.0));
                            }
                        }
                    }
                }
                Some(current_price)
            } else { price };
            if let Some(price) = &price { usage["priceEstimate"] = price.clone(); }
            let stored = serde_json::to_string(&usage)?;
            conn.execute("UPDATE thread_turns SET token_usage_json=?1,model=COALESCE(?4,model),reasoning_effort=COALESCE(?5,reasoning_effort),pricing_model_key=COALESCE(?6,pricing_model_key),pricing_tier_key=COALESCE(?7,pricing_tier_key) WHERE thread_id=?2 AND id=?3",params![stored,event.thread_id,turn_id,model,effort,pricing_model,tier])?;
            if sqlite_table_exists(conn,"thread_turn_metadata")? {
                conn.execute("UPDATE thread_turn_metadata SET token_usage_json=?1 WHERE thread_id=?2 AND turn_id=?3",params![stored,event.thread_id,turn_id])?;
            }
            Ok(Some(json!({"turnId":turn_id,"model":model,"reasoningEffort":effort,"tokenUsage":crate::usage::public_usage(&usage),"priceEstimate":price})))
        })?;
        if let Some(payload) = payload {
            self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadTurnTokenUpdated,
                thread_id: event.thread_id.clone(),
                timestamp: event.timestamp.clone(),
                payload,
            });
        }
        Ok(())
    }

    pub fn runtime(&self, provider: Provider) -> Result<&SharedRuntime> {
        self.runtimes
            .get(&provider)
            .ok_or_else(|| anyhow!("{} is not enabled", provider.as_str()))
    }

    fn reconcile_stale_turns(
        &self,
        only_thread_id: Option<&str>,
        include_disconnected: bool,
    ) -> Result<usize> {
        let now = now_rfc3339();
        let turn_status = if only_thread_id.is_some() {
            "interrupted"
        } else {
            "recovering"
        };
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            let thread_ids = {
                let mut stmt = tx.prepare(
                    "SELECT t.id FROM threads t
                     WHERE (?1 IS NULL OR t.id=?1)
                       AND (?2=1 OR COALESCE(t.is_connected, 1)=1)
                       AND (
                         t.status IN ('running','recovering') OR EXISTS (
                           SELECT 1 FROM thread_turns tt
                           WHERE tt.thread_id=t.id AND tt.status IN ('inProgress','recovering')
                         )
                       )",
                )?;
                let ids = stmt
                    .query_map(
                        params![only_thread_id, include_disconnected as i64],
                        |row| row.get::<_, String>(0),
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                ids
            };
            for thread_id in &thread_ids {
                // Journal only the turns transitioned by this reconciliation;
                // historical interrupted turns must never become new hook events.
                let transitioned = {
                    let mut query = tx.prepare("SELECT id FROM thread_turns WHERE thread_id=?1 AND status IN ('inProgress','recovering')")?;
                    let rows = query.query_map([thread_id], |row| row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };
                if turn_status=="interrupted" {
                    for turn in &transitioned {automation::turn_ended(&tx,thread_id,turn,"interrupted",&now)?;}
                }
                tx.execute(
                    "UPDATE thread_turns
                     SET status=?4, error=COALESCE(error, ?1),
                         completed_at=CASE WHEN ?4='interrupted' THEN COALESCE(completed_at, ?2) ELSE NULL END
                     WHERE thread_id=?3 AND status IN ('inProgress','recovering')",
                    params![RESTART_INTERRUPTED_ERROR, now, thread_id, turn_status],
                )?;
                tx.execute(
                    "UPDATE threads
                     SET status=?4, last_error=?1, updated_at=?2,
                         last_turn_completed_at=CASE WHEN ?4='interrupted' THEN ?2 ELSE last_turn_completed_at END
                     WHERE id=?3",
                    params![RESTART_INTERRUPTED_ERROR, now, thread_id, turn_status],
                )?;
            }
            tx.commit()?;
            Ok(thread_ids.len())
        })
    }

    pub fn backends(&self) -> Vec<AgentBackendDto> {
        let mut list: Vec<_> = self.runtimes.values().map(|r| r.descriptor()).collect();
        list.sort_by_key(|b| match b.provider {
            Provider::Codex => 0,
            Provider::Claude => 1,
            Provider::Opencode => 2,
            Provider::Acp => 3,
        });
        list
    }

    pub fn default_provider(&self) -> Provider {
        self.backends()
            .into_iter()
            .find(|b| b.enabled && b.capabilities.sessions.resume && b.capabilities.turns.start)
            .map(|b| b.provider)
            .unwrap_or(Provider::Codex)
    }

    pub fn workspace_settings(&self) -> WorkspaceSettingsDto {
        let stored = self
            .db
            .get_kv("workspace_settings")
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str::<WorkspaceSettingsDto>(&raw).ok());
        let workspace_root = self.config.workspace_root.to_string_lossy().into_owned();
        WorkspaceSettingsDto {
            workspace_root: workspace_root.clone(),
            dev_home: stored
                .as_ref()
                .map(|settings| settings.dev_home.clone())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(workspace_root),
            default_backend: stored
                .as_ref()
                .map(|settings| settings.default_backend)
                .unwrap_or_else(|| self.default_provider()),
        }
    }

    pub fn update_workspace_settings(
        &self,
        input: UpdateWorkspaceSettingsInput,
    ) -> Result<WorkspaceSettingsDto> {
        let mut current = self.workspace_settings();
        if let Some(dev_home) = input.dev_home.filter(|value| !value.trim().is_empty()) {
            let path = PathBuf::from(dev_home.trim());
            if !path.is_dir() {
                bail!("dev home must be an existing directory");
            }
            current.dev_home = path.canonicalize()?.to_string_lossy().into();
        }
        if let Some(backend) = input.default_backend {
            current.default_backend = backend;
        }
        current.workspace_root = self.config.workspace_root.to_string_lossy().into();
        self.db
            .set_kv("workspace_settings", &serde_json::to_string(&current)?)?;
        Ok(current)
    }

    pub fn plugin_enabled(&self, plugin_id: &str, default_enabled: bool) -> bool {
        self.db
            .get_kv(&format!("plugin:{plugin_id}:enabled"))
            .ok()
            .flatten()
            .and_then(|value| value.parse::<bool>().ok())
            .unwrap_or(default_enabled)
    }

    pub fn set_plugin_enabled(&self, plugin_id: &str, enabled: bool) -> Result<()> {
        let plugin = self.get_plugin(plugin_id)?;
        ensure!(
            !enabled || plugin.available != Some(false),
            "plugin is unavailable"
        );
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![format!("plugin:{plugin_id}:enabled"), enabled.to_string()])?;
            if !enabled {
                tx.execute("UPDATE automation_runs SET state='cancelled',completed_at=?2,error='pluginDisabled' WHERE state='due' AND json_extract(definition_json,'$.action.kind')='pluginAction' AND json_extract(definition_json,'$.action.pluginId')=?1", params![plugin_id, now_rfc3339()])?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceDto>> {
        self.db.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, host_id, label, abs_path, is_favorite, created_at, last_opened_at FROM workspaces ORDER BY created_at DESC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(WorkspaceDto {
                    id: row.get(0)?,
                    host_id: row.get(1)?,
                    label: row.get(2)?,
                    abs_path: row.get(3)?,
                    is_favorite: row.get::<_, i64>(4)? != 0,
                    created_at: row.get(5)?,
                    last_opened_at: row.get(6)?,
                })
            })?;
            Ok(rows.filter_map(|r| r.ok()).collect())
        })
    }

    pub fn get_workspace(&self, id: &str) -> Result<WorkspaceDto> {
        self.list_workspaces()?
            .into_iter()
            .find(|w| w.id == id)
            .ok_or_else(|| anyhow!("workspace not found"))
    }

    pub fn create_workspace(&self, input: CreateWorkspaceInput) -> Result<WorkspaceDto> {
        std::fs::create_dir_all(&self.config.workspace_root)?;
        let settings = self.workspace_settings();
        let dev_home = PathBuf::from(&settings.dev_home);
        std::fs::create_dir_all(&dev_home)?;
        let abs_path = if let Some(git) = input.git_url.filter(|s| !s.is_empty()) {
            let dest = dev_home.join(infer_git_repo_name(&git));
            if dest.exists() {
                bail!("The Git clone target directory already exists.");
            }
            let mut command = std::process::Command::new("git");
            command.args(["clone", "--depth", "1", &git, &dest.to_string_lossy()]);
            crate::child_process::hide_std(&mut command);
            let status = command.status()?;
            if !status.success() {
                bail!("git clone failed");
            }
            dest
        } else {
            let requested = input
                .abs_path
                .ok_or_else(|| anyhow!("absPath is required"))?;
            let requested = requested.trim();
            if requested.is_empty() {
                bail!("absPath is required");
            }
            let target = if is_workspace_name(requested) {
                dev_home.join(requested)
            } else {
                PathBuf::from(requested)
            };
            if !target.exists() {
                let parent = target.parent().filter(|path| !path.as_os_str().is_empty());
                if let Some(parent) = parent {
                    if !parent.exists() {
                        bail!("The parent directory for the workspace path does not exist.");
                    }
                }
                std::fs::create_dir_all(&target)?;
            }
            if !target.is_dir() {
                bail!("workspace path is not a directory");
            }
            target
        };
        let abs_path = abs_path.canonicalize()?;
        let already = self
            .list_workspaces()?
            .into_iter()
            .any(|workspace| PathBuf::from(&workspace.abs_path) == abs_path);
        if already {
            bail!("This workspace has already been added.");
        }
        let id = Uuid::new_v4().to_string();
        let now = now_rfc3339();
        let label = input
            .label
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                abs_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "workspace".into())
            });
        self.db.with(|conn| {
            conn.execute(
                "INSERT INTO workspaces(id, host_id, label, abs_path, is_favorite, created_at, last_opened_at)
                 VALUES (?1,?2,?3,?4,0,?5,?5)",
                params![id, self.db.host_id, label, abs_path.to_string_lossy().to_string(), now],
            )?;
            Ok(())
        })?;
        self.get_workspace(&id)
    }

    pub fn update_workspace(&self, id: &str, label: &str) -> Result<WorkspaceDto> {
        self.db.with(|conn| {
            let n = conn.execute(
                "UPDATE workspaces SET label=?1 WHERE id=?2",
                params![label, id],
            )?;
            if n == 0 {
                bail!("workspace not found");
            }
            Ok(())
        })?;
        self.get_workspace(id)
    }

    pub fn set_favorite(&self, id: &str, favorite: bool) -> Result<WorkspaceDto> {
        self.db.with(|conn| {
            conn.execute(
                "UPDATE workspaces SET is_favorite=?1 WHERE id=?2",
                params![favorite as i64, id],
            )?;
            Ok(())
        })?;
        self.get_workspace(id)
    }

    pub fn open_workspace(&self, id: &str) -> Result<WorkspaceDto> {
        self.db.with(|conn| {
            conn.execute(
                "UPDATE workspaces SET last_opened_at=?1 WHERE id=?2",
                params![now_rfc3339(), id],
            )?;
            Ok(())
        })?;
        self.get_workspace(id)
    }

    pub fn delete_workspace(&self, id: &str) -> Result<()> {
        self.db.with(|conn| {
            conn.execute("DELETE FROM threads WHERE workspace_id=?1", params![id])?;
            conn.execute("DELETE FROM workspaces WHERE id=?1", params![id])?;
            Ok(())
        })
    }

    pub fn workspace_tree(&self, id: &str, rel: &str) -> Result<Vec<ThreadWorkspaceTreeNodeDto>> {
        let ws = self.get_workspace(id)?;
        files::list_tree(Path::new(&ws.abs_path), rel)
    }

    pub fn workspace_preview(&self, id: &str, rel: &str) -> Result<ThreadWorkspaceFilePreviewDto> {
        let ws = self.get_workspace(id)?;
        files::preview_file(Path::new(&ws.abs_path), rel, 64 * 1024)
    }

    pub fn workspace_read_bytes(&self, id: &str, rel: &str) -> Result<(PathBuf, Vec<u8>)> {
        let workspace = self.get_workspace(id)?;
        let (path, bytes) = files::read_bytes(Path::new(&workspace.abs_path), rel)?;
        if !path.is_file() {
            bail!("Workspace download path must point to a file.");
        }
        Ok((path, bytes))
    }

    pub fn workspace_download(&self, id: &str, rel: &str) -> Result<files::WorkspaceDownload> {
        let workspace = self.get_workspace(id)?;
        files::prepare_download(Path::new(&workspace.abs_path), rel)
    }

    pub fn workspace_write(&self, id: &str, rel: &str, content: &str) -> Result<()> {
        let ws = self.get_workspace(id)?;
        files::write_file(Path::new(&ws.abs_path), rel, content)
    }

    pub fn workspace_create_file(&self, id: &str, rel: &str) -> Result<()> {
        let _gate = crate::file_documents::mutation_guard();
        let workspace = self.get_workspace(id)?;
        files::create_empty_file(Path::new(&workspace.abs_path), rel)
    }

    pub fn workspace_write_bytes(
        &self,
        id: &str,
        rel: &str,
        content: &[u8],
    ) -> Result<(String, u64)> {
        let _gate = crate::file_documents::mutation_guard();
        let workspace = self.get_workspace(id)?;
        let root = PathBuf::from(&workspace.abs_path).canonicalize()?;
        let path = files::assert_within(&root, Path::new(rel))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        let relative = path
            .strip_prefix(&root)?
            .to_string_lossy()
            .replace('\\', "/");
        Ok((relative, u64::try_from(content.len()).unwrap_or(u64::MAX)))
    }

    /// Turn counts, running time (all time, inside `[since, now]` and per hour of
    /// it) and the latest prompt for every thread with turns. A turn without a
    /// completion time counts up to `now` only while it is in progress and its
    /// thread runs, so a turn left open by a crash or an import adds nothing.
    pub fn thread_activity(
        &self,
        since: &str,
        now: &str,
        hours: usize,
    ) -> Result<Vec<ThreadActivityDto>> {
        const HOUR: f64 = 3_600_000.0;
        const TOKENS: [&str; 6] = [
            "totalTokens",
            "inputTokens",
            "cachedInputTokens",
            "cacheWriteInputTokens",
            "outputTokens",
            "reasoningOutputTokens",
        ];
        const USD: [&str; 5] = [
            "totalUsd",
            "inputUsd",
            "cachedInputUsd",
            "cacheWriteInputUsd",
            "outputUsd",
        ];
        struct Sum {
            entry: ThreadActivityDto,
            hourly: Vec<f64>,
            total: f64,
            active: f64,
            tokens: [u64; 6],
            usd: [f64; 5],
            priced: bool,
        }
        // Read before the database lock: custom rates live in the database too.
        let catalog = self.model_pricing();
        self.db.with(|conn| {
            // Turn bounds in milliseconds after `since`; an unknown bound is NULL.
            let mut stmt = conn.prepare(
                "SELECT tt.thread_id,
                        (julianday(tt.started_at) - julianday(?1)) * 86400000,
                        (julianday(MIN(COALESCE(tt.completed_at,
                              CASE WHEN tt.status IN ('inProgress','recovering')
                                   AND t.status IN ('running','recovering') THEN ?2 END), ?2))
                         - julianday(?1)) * 86400000,
                        tt.token_usage_json, COALESCE(tt.pricing_model_key, tt.model),
                        tt.pricing_tier_key, tt.started_at
                 FROM thread_turns tt JOIN threads t ON t.id = tt.thread_id
                 ORDER BY tt.thread_id",
            )?;
            let mut rows = stmt.query(params![since, now])?;
            let span = hours as f64 * HOUR;
            let mut sums: Vec<Sum> = Vec::new();
            while let Some(row) = rows.next()? {
                let thread_id: String = row.get(0)?;
                if sums
                    .last()
                    .is_none_or(|sum| sum.entry.thread_id != thread_id)
                {
                    sums.push(Sum {
                        entry: ThreadActivityDto {
                            thread_id,
                            turns: 0,
                            active_ms: 0,
                            total_ms: 0,
                            hourly_ms: Vec::new(),
                            last_prompt: None,
                            token_usage: None,
                            price_estimate: None,
                            price_partial: false,
                        },
                        hourly: vec![0.0; hours],
                        total: 0.0,
                        active: 0.0,
                        tokens: [0; 6],
                        usd: [0.0; 5],
                        priced: false,
                    });
                }
                let sum = sums.last_mut().expect("pushed above");
                sum.entry.turns += 1;
                if let Some(usage) = row
                    .get::<_, Option<String>>(3)?
                    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                {
                    let public = crate::usage::public_usage(&usage);
                    let total = public.as_ref().map(|public| &public["total"]);
                    for (slot, key) in sum.tokens.iter_mut().zip(TOKENS) {
                        *slot += total.and_then(|total| total[key].as_u64()).unwrap_or(0);
                    }
                    let model = row.get::<_, Option<String>>(4)?;
                    let tier = row.get::<_, Option<String>>(5)?;
                    let at = row.get::<_, Option<String>>(6)?;
                    match crate::usage::estimate_price_with_catalog(
                        &usage,
                        model.as_deref(),
                        tier.as_deref(),
                        &catalog,
                        at.as_deref(),
                    ) {
                        Some(price) => {
                            sum.priced = true;
                            for (slot, key) in sum.usd.iter_mut().zip(USD) {
                                *slot += price[key].as_f64().unwrap_or(0.0);
                            }
                        }
                        None if total
                            .is_some_and(|total| total["totalTokens"].as_u64() > Some(0)) =>
                        {
                            sum.entry.price_partial = true;
                        }
                        None => {}
                    }
                }
                let (Some(start), Some(end)) =
                    (row.get::<_, Option<f64>>(1)?, row.get::<_, Option<f64>>(2)?)
                else {
                    continue;
                };
                if end <= start {
                    continue;
                }
                sum.total += end - start;
                let (from, to) = (start.max(0.0), end.min(span));
                if to <= from {
                    continue;
                }
                sum.active += to - from;
                let mut at = from;
                while at < to {
                    let hour = ((at / HOUR) as usize).min(hours - 1);
                    let next = ((hour + 1) as f64 * HOUR).min(to);
                    sum.hourly[hour] += next - at;
                    at = next.max(at + 1.0);
                }
            }
            let mut prompts = conn.prepare(
                "SELECT thread_id, display_prompt FROM (
                   SELECT thread_id, display_prompt,
                          ROW_NUMBER() OVER (PARTITION BY thread_id ORDER BY ordinal DESC) AS latest
                   FROM thread_turns
                   WHERE display_prompt IS NOT NULL AND TRIM(display_prompt) != '')
                 WHERE latest = 1",
            )?;
            let mut latest: std::collections::HashMap<String, String> = prompts
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(sums
                .into_iter()
                .map(|sum| {
                    let mut entry = sum.entry;
                    entry.total_ms = sum.total.round() as u64;
                    entry.active_ms = sum.active.round() as u64;
                    if sum.active > 0.0 {
                        entry.hourly_ms =
                            sum.hourly.into_iter().map(|ms| ms.round() as u64).collect();
                    }
                    if sum.tokens[0] > 0 {
                        entry.token_usage = Some(Value::Object(
                            TOKENS
                                .iter()
                                .zip(sum.tokens)
                                .map(|(key, value)| (key.to_string(), json!(value)))
                                .collect(),
                        ));
                    }
                    if sum.priced {
                        let mut price =
                            serde_json::Map::from_iter([("currency".to_string(), json!("USD"))]);
                        price.extend(
                            USD.iter()
                                .zip(sum.usd)
                                .map(|(key, value)| (key.to_string(), json!(value))),
                        );
                        entry.price_estimate = Some(Value::Object(price));
                    }
                    entry.last_prompt = latest.remove(&entry.thread_id).map(|prompt| {
                        prompt
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .chars()
                            .take(240)
                            .collect()
                    });
                    entry
                })
                .collect())
        })
    }

    /// `include_agent_threads` false returns only lineage roots - the threads a person
    /// started - so an agent fan-out cannot flood a workspace listing. Internal callers
    /// pass true because they resolve threads by id/session and must see every row.
    pub fn list_threads(
        &self,
        workspace_id: Option<&str>,
        include_agent_threads: bool,
    ) -> Result<Vec<ThreadDto>> {
        let mut rows = self.db.with(|conn| {
            let mut sql = String::from(
                "SELECT t.id, t.workspace_id, t.provider, t.agent_id, t.provider_session_id, t.source, t.title, t.model,
                        t.reasoning_effort, t.fast_mode, t.collaboration_mode, t.approval_mode, t.sandbox_mode, t.status,
                        t.summary_text, t.last_error, t.created_at, t.updated_at, t.last_turn_started_at, t.last_turn_completed_at,
                        t.is_pinned, t.context_usage_json, t.parent_thread_id, t.root_thread_id, t.lineage_depth,
                        (SELECT COUNT(*) FROM threads d WHERE d.root_thread_id = t.id),
                        t.agent_name, t.agent_role, t.worktree_path, t.closed_at
                 FROM threads t",
            );
            if workspace_id.is_some() {
                sql.push_str(" WHERE t.workspace_id=?1");
            }
            if !include_agent_threads {
                sql.push_str(if workspace_id.is_some() { " AND" } else { " WHERE" });
                sql.push_str(" t.parent_thread_id IS NULL");
            }
            sql.push_str(" ORDER BY t.updated_at DESC");
            let mut stmt = conn.prepare(&sql)?;
            let map_row = |row: &rusqlite::Row| -> rusqlite::Result<ThreadDto> {
                Ok(thread_from_row(row))
            };
            let rows = if let Some(ws) = workspace_id {
                stmt.query_map(params![ws], map_row)?
                    .filter_map(|r| r.ok())
                    .collect()
            } else {
                stmt.query_map([], map_row)?.filter_map(|r| r.ok()).collect()
            };
            Ok(rows)
        })?;
        for thread in &mut rows {
            self.apply_loaded_flag(thread);
        }
        Ok(rows)
    }

    pub fn get_thread(&self, id: &str) -> Result<ThreadDto> {
        let mut thread = self
            .list_threads(None, true)?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| anyhow!("thread not found"))?;
        if thread.context_usage.is_none() || thread.source == "local_codex_import" {
            let historical_context = self.db.with(|conn| {
                let raw: Option<String> = conn.query_row(
                    "SELECT token_usage_json FROM thread_turns WHERE thread_id=?1 AND token_usage_json IS NOT NULL ORDER BY ordinal DESC LIMIT 1",
                    params![id], |row| row.get(0)).optional()?;
                Ok(raw.and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .and_then(|usage| crate::usage::context_usage(&usage, &thread.updated_at)))
            })?;
            thread.context_usage = historical_context.or(thread.context_usage);
        }
        thread.active_turn_id = self.active_turn_id(id)?;
        if thread.active_turn_id.is_some() {
            thread.status = pockymoe_protocol::ThreadStatus::Running;
        }
        Ok(thread)
    }

    fn active_turn_id(&self, thread_id: &str) -> Result<Option<String>> {
        self.db.with(|conn| {
            let id: Option<String> = conn
                .query_row(
                    "SELECT id FROM thread_turns WHERE thread_id=?1 AND status='inProgress' ORDER BY ordinal DESC LIMIT 1",
                    params![thread_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(id)
        })
    }

    pub async fn create_thread(&self, input: CreateThreadInput) -> Result<ThreadDto> {
        self.create_thread_with(input, crate::interaction::AgentOptions::default())
            .await
    }

    pub async fn create_thread_with(
        &self,
        input: CreateThreadInput,
        options: crate::interaction::AgentOptions,
    ) -> Result<ThreadDto> {
        self.create_thread_inner(input, options, None).await
    }

    pub async fn create_thread_with_cli_identity(
        &self,
        input: CreateThreadInput,
        env: Vec<(String, String)>,
    ) -> Result<ThreadDto> {
        ensure!(
            input.parent_thread_id.is_none(),
            "identity requires a top-level thread"
        );
        self.create_thread_inner(
            input,
            crate::interaction::AgentOptions::default(),
            Some(env),
        )
        .await
    }

    async fn create_thread_inner(
        &self,
        input: CreateThreadInput,
        options: crate::interaction::AgentOptions,
        identity: Option<Vec<(String, String)>>,
    ) -> Result<ThreadDto> {
        let (approval_mode, sandbox_mode) = match input.approval_mode.as_str() {
            "yolo" => ("yolo", "danger-full-access"),
            "guarded" => ("guarded", "workspace-write"),
            _ => bail!("approvalMode must be yolo or guarded"),
        };
        let workspace = self.get_workspace(&input.workspace_id)?;
        // Refuse over-cap or misnamed delegates before a harness session or a
        // worktree exists, so a refusal leaves nothing behind.
        let lineage = self.resolve_lineage(input.parent_thread_id.as_deref())?;
        if let Some(name) = &options.name {
            self.ensure_agent_name_free(lineage.root.as_deref(), name)?;
        }
        let provider = input.provider.unwrap_or_else(|| self.default_provider());
        let _maintenance = self
            .maintenance_gate
            .try_read()
            .map_err(|_| anyhow!("conflict: Supervisor update in progress"))?;
        let gate = self.harness_gate(input.agent_id.as_deref().unwrap_or(
            if provider == Provider::Acp {
                "codex"
            } else {
                provider.as_str()
            },
        ));
        let _guard = gate
            .try_read()
            .map_err(|_| anyhow!("conflict: Harness maintenance in progress"))?;
        let runtime = self.runtime(provider)?;
        let id = Uuid::new_v4().to_string();
        let worktree = match &options.worktree {
            Some(branch) => {
                let slug = options.name.clone().unwrap_or_else(|| id[..8].to_string());
                Some(crate::interaction::create_worktree(
                    Path::new(&workspace.abs_path),
                    &slug,
                    branch.as_deref(),
                )?)
            }
            None => None,
        };
        let cwd = worktree
            .as_ref()
            .map(|w| w.path.to_string_lossy().into_owned())
            .unwrap_or_else(|| workspace.abs_path.clone());
        if let Some(env) = identity {
            self.stage_thread_cli_identity(&id, env);
        }
        let started = match self
            .with_load(
                &id,
                HarnessLoadReason::Create,
                runtime.start_session(StartSessionInput {
                    cwd,
                    agent_id: input.agent_id.clone(),
                    model: input.model.clone(),
                    reasoning_effort: input.reasoning_effort.clone(),
                    approval_mode: approval_mode.into(),
                    sandbox_mode: Some(sandbox_mode.into()),
                }),
            )
            .await
        {
            Ok(started) => started,
            Err(error) => {
                self.clear_thread_cli_identity(&id);
                if let Some(w) = &worktree {
                    let _ = crate::interaction::remove_worktree(
                        Path::new(&workspace.abs_path),
                        &w.path,
                    );
                }
                return Err(error);
            }
        };
        let now = now_rfc3339();
        let title = input
            .title
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "New thread".into());
        let saved = self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "INSERT INTO threads(id, workspace_id, provider, agent_id, provider_session_id, source, title, model,
                    reasoning_effort, collaboration_mode, approval_mode, sandbox_mode, status, created_at, updated_at,
                    parent_thread_id, root_thread_id, lineage_depth, agent_name, agent_role, worktree_path)
                 VALUES (?1,?2,?3,?4,?5,'supervisor',?6,?7,?8,'default',?9,?10,'idle',?11,?11,?12,?13,?14,?15,?16,?17)",
                params![
                    id,
                    input.workspace_id,
                    provider.as_str(),
                    input.agent_id,
                    started.provider_session_id,
                    title,
                    started.model.clone().unwrap_or(input.model.clone()),
                    started.reasoning_effort.clone(),
                    approval_mode,
                    sandbox_mode,
                    now,
                    lineage.parent,
                    lineage.root,
                    lineage.depth,
                    options.name,
                    options.role.as_ref().map(|r| &r.name),
                    worktree.as_ref().map(|w| w.path.to_string_lossy().into_owned()),
                ],
            )?;
            if let Some(role) = &options.role {
                crate::interaction::stage_role(&tx, &id, role)?;
            }
            tx.commit()?;
            Ok(())
        });
        if let Err(error) = saved {
            self.clear_thread_cli_identity(&id);
            let _ = runtime
                .release_loaded(
                    &started.provider_session_id,
                    crate::actor::ReleaseScope::Session,
                    None,
                )
                .await;
            return Err(error);
        }
        let thread = self.get_thread(&id)?;
        self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
            thread_id: id.clone(),
            timestamp: now_rfc3339(),
            payload: json!({"reason":"thread_created"}),
        });
        for parent in [
            thread.parent_thread_id.as_ref(),
            thread.root_thread_id.as_ref(),
        ]
        .into_iter()
        .flatten()
        .collect::<HashSet<_>>()
        {
            self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
                thread_id: parent.clone(),
                timestamp: now_rfc3339(),
                payload: json!({"reason":"children_changed"}),
            });
        }
        Ok(thread)
    }

    /// Where a thread's harness runs: its own worktree when it has one.
    pub fn session_cwd(&self, thread: &ThreadDto) -> Option<String> {
        thread.worktree_path.clone().or_else(|| {
            self.get_workspace(&thread.workspace_id)
                .ok()
                .map(|ws| ws.abs_path)
        })
    }

    pub async fn list_import_candidates(
        &self,
        provider: Option<Provider>,
        agent_id: Option<&str>,
    ) -> Result<Vec<ImportThreadCandidateDto>> {
        let provider = provider.unwrap_or_else(|| self.default_provider());
        if provider == Provider::Acp
            && agent_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
        {
            return Ok(Vec::new());
        }
        let agent = agent_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| crate::import_id::default_agent_for_provider(provider))
            .to_string();
        let mut sessions = list_local_sessions(&self.local_session_homes, &agent);
        if let Ok(runtime) = self.runtime(provider) {
            match runtime.list_import_sessions(Some(&agent)).await {
                Ok(listed) => sessions.extend(listed),
                Err(err) => tracing::warn!(error = %err, "import candidate listing failed"),
            }
        }
        let existing = self.list_threads(None, true).unwrap_or_default();
        let mut out = Vec::new();
        for session in sessions {
            if session.session_id.trim().is_empty() || !Path::new(&session.cwd).is_absolute() {
                continue;
            }
            if existing.iter().any(|thread| {
                thread
                    .provider_session_id
                    .as_deref()
                    .map(|stored| session_ids_match(stored, &session.session_id))
                    .unwrap_or(false)
            }) {
                continue;
            }
            if out.iter().any(|candidate: &ImportThreadCandidateDto| {
                session_ids_match(&candidate.session_id, &session.session_id)
            }) {
                continue;
            }
            out.push(ImportThreadCandidateDto {
                provider,
                agent_id: Some(session.agent_id.clone()),
                session_id: crate::import_id::raw_session_id(&session.session_id).to_string(),
                cwd: session.cwd,
                title: session.title,
                preview: session.preview,
                created_at: session.created_at,
                updated_at: session.updated_at,
                history_status: "unknown".into(),
            });
        }
        out.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.created_at.cmp(&left.created_at))
        });
        Ok(out)
    }

    pub async fn import_thread(&self, input: ImportThreadInput) -> Result<ThreadDetailDto> {
        let parsed = parse_session_ref(&input.session_id);
        if parsed.raw_id.is_empty() {
            bail!("Session id is required.");
        }
        let selected_provider = input.provider.unwrap_or_else(|| self.default_provider());
        let enabled: Vec<Provider> = self.runtimes.keys().copied().collect();
        let (provider, agent_id) = bind_import_target(
            selected_provider,
            input.agent_id.as_deref(),
            parsed.agent_id.as_deref(),
            &enabled,
        );
        if let Some(existing) = self.find_thread_by_session(&parsed.raw_id)? {
            return self
                .get_thread_detail_page(&existing.id, Some(3), None, true)
                .await;
        }

        let mut session = find_local_session(&self.local_session_homes, &agent_id, &parsed.raw_id);
        if session.is_none() {
            if let Ok(runtime) = self.runtime(provider) {
                session = runtime
                    .resolve_import_session(Some(&agent_id), &parsed.raw_id)
                    .await?;
            }
        }
        let Some(session) = session else {
            bail!("Session not found on this machine.");
        };
        if !Path::new(&session.cwd).is_absolute() {
            bail!("Imported session path must be absolute.");
        }

        let workspace = self.ensure_workspace_at(&session.cwd)?;
        let source = if agent_id == "codex" {
            "local_codex_import"
        } else {
            "local_provider_import"
        };
        let scoped = scoped_session_id(&agent_id, &parsed.raw_id);
        let now = now_rfc3339();
        let thread_id = Uuid::new_v4().to_string();
        let title = truncate_title(&session.title);
        let summary = session.preview.clone();
        let model = session
            .model
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "default".into());
        self.db.with(|conn| {
            conn.execute(
                "INSERT INTO threads(id, workspace_id, provider, agent_id, provider_session_id, source, title, model,
                    reasoning_effort, collaboration_mode, approval_mode, status, summary_text, created_at, updated_at, is_connected)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,NULL,'default','yolo','idle',?9,?10,?10,0)",
                params![
                    thread_id,
                    workspace.id,
                    provider.as_str(),
                    agent_id,
                    scoped,
                    source,
                    title,
                    model,
                    summary,
                    now
                ],
            )?;
            Ok(())
        })?;
        self.persist_imported_turns(&thread_id, &session)?;
        self.get_thread_detail_page(&thread_id, Some(3), None, true)
            .await
    }

    fn find_thread_by_session(&self, session_id: &str) -> Result<Option<ThreadDto>> {
        Ok(self.list_threads(None, true)?.into_iter().find(|thread| {
            thread
                .provider_session_id
                .as_deref()
                .map(|stored| session_ids_match(stored, session_id))
                .unwrap_or(false)
        }))
    }

    fn ensure_workspace_at(&self, abs_path: &str) -> Result<WorkspaceDto> {
        let path = PathBuf::from(abs_path);
        if !path.is_absolute() {
            bail!("Imported session path must be absolute.");
        }
        let resolved = if path.exists() {
            path.canonicalize()?
        } else {
            path
        };
        if let Some(existing) = self
            .list_workspaces()?
            .into_iter()
            .find(|workspace| PathBuf::from(&workspace.abs_path) == resolved)
        {
            return Ok(existing);
        }
        let id = Uuid::new_v4().to_string();
        let now = now_rfc3339();
        let label = resolved
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "workspace".into());
        let stored = resolved.to_string_lossy().into_owned();
        self.db.with(|conn| {
            conn.execute(
                "INSERT INTO workspaces(id, host_id, label, abs_path, is_favorite, created_at, last_opened_at)
                 VALUES (?1,?2,?3,?4,0,?5,?5)",
                params![id, self.db.host_id, label, stored, now],
            )?;
            Ok(())
        })?;
        self.get_workspace(&id)
    }

    fn persist_imported_turns(&self, thread_id: &str, session: &ImportSessionMeta) -> Result<()> {
        if session.turns.is_empty() {
            return Ok(());
        }
        let now = now_rfc3339();
        self.db.with(|conn| {
            let transaction = conn.unchecked_transaction()?;
            for (index, turn) in session.turns.iter().enumerate() {
                let turn_id = if turn.id.trim().is_empty() {
                    format!("imported-{thread_id}-{}", index + 1)
                } else {
                    turn.id.clone()
                };
                let display_prompt = turn
                    .items
                    .iter()
                    .find(|item| item.kind == "userMessage")
                    .map(|item| item.text.as_str());
                let token_usage_json = turn
                    .token_usage
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?;
                transaction.execute(
                    "INSERT INTO thread_turns(
                       id, thread_id, status, error, model, reasoning_effort,
                       token_usage_json, display_prompt, started_at, ordinal
                     ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        turn_id,
                        thread_id,
                        if turn.status.as_str().trim().is_empty() {
                            "completed".to_string()
                        } else {
                            turn.status.to_string()
                        },
                        turn.error.clone(),
                        turn.model.clone(),
                        turn.reasoning_effort.clone(),
                        token_usage_json,
                        display_prompt,
                        turn.started_at.clone(),
                        (index as i64) + 1
                    ],
                )?;
                upsert_legacy_turn_metadata(
                    &transaction,
                    thread_id,
                    &turn_id,
                    turn.model.as_deref(),
                    turn.reasoning_effort.as_deref(),
                    display_prompt,
                    turn.started_at.as_deref(),
                    &now,
                )?;
                for item in &turn.items {
                    transaction.execute(
                        "INSERT INTO thread_history_items(id, thread_id, turn_id, item_id, item_json, created_at, updated_at)
                         VALUES (?1,?2,?3,?4,?5,?6,?6)
                         ON CONFLICT(thread_id, turn_id, item_id) DO UPDATE SET
                            item_json=excluded.item_json,
                            updated_at=excluded.updated_at",
                        params![
                            Uuid::new_v4().to_string(),
                            thread_id,
                            turn_id,
                            item.id,
                            serde_json::to_string(item)?,
                            item.created_at.clone().unwrap_or_else(|| now.clone())
                        ],
                    )?;
                }
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn ensure_prompt_allowed(&self, thread: &ThreadDto) -> Result<()> {
        ensure!(
            !self.integration_thread_paused(&thread.id),
            "conflict: integration binding is detached"
        );
        self.ensure_not_updating()?;
        if thread.source != "local_codex_import" && thread.source != "local_provider_import" {
            return Ok(());
        }
        let loaded = thread
            .provider_session_id
            .as_deref()
            .and_then(|session| {
                self.runtime(thread.provider)
                    .ok()
                    .map(|runtime| runtime.session_loaded(session))
            })
            .unwrap_or(false);
        if loaded {
            Ok(())
        } else {
            bail!("Resume / Connect this imported session before sending a new prompt.")
        }
    }

    fn apply_loaded_flag(&self, thread: &mut ThreadDto) {
        if thread.source != "local_codex_import" && thread.source != "local_provider_import" {
            return;
        }
        thread.is_loaded = thread
            .provider_session_id
            .as_deref()
            .and_then(|session| {
                self.runtime(thread.provider)
                    .ok()
                    .map(|runtime| runtime.session_loaded(session))
            })
            .unwrap_or(false);
    }

    fn stored_goal(&self, thread_id: &str) -> Result<Option<ThreadGoalDto>> {
        Ok(self
            .db
            .get_kv(&format!("thread_goal:{thread_id}"))?
            .and_then(|raw| serde_json::from_str(&raw).ok()))
    }

    fn save_goal(&self, thread_id: &str, goal: &ThreadGoalDto) -> Result<()> {
        let serialized = serde_json::to_string(goal)?;
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            upsert_legacy_thread_goal(&tx, thread_id, goal)?;
            tx.execute(
                "INSERT INTO kv(key,value) VALUES(?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![format!("thread_goal:{thread_id}"), serialized],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    fn delete_stored_goal(&self, thread_id: &str) -> Result<()> {
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            if sqlite_table_exists(&tx, "thread_goals")? {
                let now = now_rfc3339();
                tx.execute(
                    "UPDATE thread_goals
                     SET status='terminated', completed_at=?1, updated_at=?1
                     WHERE thread_id=?2
                       AND status IN ('active','paused','budgetLimited')",
                    params![now, thread_id],
                )?;
            }
            tx.execute(
                "DELETE FROM kv WHERE key=?1",
                params![format!("thread_goal:{thread_id}")],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    fn goal_snapshot(
        &self,
        thread_id: &str,
        runtime_goal: Option<GoalState>,
    ) -> Result<Option<ThreadGoalDto>> {
        let stored = self.stored_goal(thread_id)?;
        let Some(runtime_goal) = runtime_goal else {
            return Ok(stored);
        };
        let now = now_rfc3339();
        let mut goal = if let Some(existing) =
            stored.filter(|goal| goal.objective == runtime_goal.objective)
        {
            existing
        } else {
            ThreadGoalDto {
                thread_id: thread_id.into(),
                local_goal_id: Some(Uuid::new_v4().to_string()),
                objective: runtime_goal.objective.clone(),
                status: runtime_goal.status.clone(),
                token_budget: None,
                tokens_used: runtime_goal.tokens_used.into(),
                time_used_seconds: runtime_goal.time_used_seconds.into(),
                created_at: now.clone(),
                updated_at: now.clone(),
                completed_at: None,
            }
        };
        let changed = goal.status != runtime_goal.status
            || goal.tokens_used != u64::from(runtime_goal.tokens_used)
            || goal.time_used_seconds != u64::from(runtime_goal.time_used_seconds);
        goal.status = runtime_goal.status;
        goal.tokens_used = runtime_goal.tokens_used.into();
        goal.time_used_seconds = runtime_goal.time_used_seconds.into();
        if changed {
            goal.updated_at = now;
        }
        goal.completed_at = goal_status_is_terminal(&goal.status).then(|| goal.updated_at.clone());
        self.save_goal(thread_id, &goal)?;
        Ok(Some(goal))
    }

    fn updated_goal_snapshot(
        &self,
        thread_id: &str,
        runtime_goal: GoalState,
        requested_status: Option<&str>,
        token_budget: Option<Option<u64>>,
    ) -> Result<ThreadGoalDto> {
        let now = now_rfc3339();
        let stored = self.stored_goal(thread_id)?;
        let mut goal = if let Some(existing) =
            stored.filter(|goal| goal.objective == runtime_goal.objective)
        {
            existing
        } else {
            ThreadGoalDto {
                thread_id: thread_id.into(),
                local_goal_id: Some(Uuid::new_v4().to_string()),
                objective: runtime_goal.objective.clone(),
                status: "active".into(),
                token_budget: None,
                tokens_used: 0,
                time_used_seconds: 0,
                created_at: now.clone(),
                updated_at: now.clone(),
                completed_at: None,
            }
        };
        goal.objective = runtime_goal.objective;
        goal.status = requested_status.unwrap_or(&runtime_goal.status).into();
        if let Some(token_budget) = token_budget {
            goal.token_budget = token_budget;
        }
        goal.tokens_used = runtime_goal.tokens_used.into();
        goal.time_used_seconds = runtime_goal.time_used_seconds.into();
        goal.updated_at = now.clone();
        goal.completed_at = goal_status_is_terminal(&goal.status).then_some(now);
        self.save_goal(thread_id, &goal)?;
        Ok(goal)
    }

    /// Delivery checks must not read/serialize command history or scan usage logs.
    pub async fn thread_delivery(&self, id: &str) -> Result<Value> {
        let detail = self.thread_action_detail(id).await?;
        let accepted: Vec<String> = self.db.with(|conn| {
            let mut statement = conn.prepare("SELECT item_id FROM thread_history_items WHERE thread_id=?1 AND item_id LIKE 'steer:%'")?;
            let rows = statement.query_map(params![id], |row| row.get::<_, String>(0))?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })?;
        let mut response = serde_json::to_value(detail)?;
        response["acceptedSteerIds"] = json!(accepted
            .into_iter()
            .map(|id| id.trim_start_matches("steer:").to_string())
            .collect::<Vec<_>>());
        Ok(response)
    }

    async fn thread_action_detail(&self, id: &str) -> Result<ThreadDetailDto> {
        let thread = self.get_thread(id)?;
        let workspace = self.get_workspace(&thread.workspace_id)?;
        let pending_requests = if let Ok(runtime) = self.runtime(thread.provider) {
            runtime.pending_requests(id).await
        } else {
            vec![]
        };
        let active_subagents = if let (Ok(runtime), Some(session_id)) = (
            self.runtime(thread.provider),
            thread.provider_session_id.as_deref(),
        ) {
            runtime.active_subagents(session_id).await
        } else {
            vec![]
        };
        Ok(ThreadDetailDto {
            thread,
            workspace_path_status: if Path::new(&workspace.abs_path).exists() {
                "present"
            } else {
                "missing"
            }
            .into(),
            workspace,
            // Empty is an incremental response: clients keep already loaded turns.
            turns: vec![],
            total_turn_count: None,
            pending_requests,
            pending_steers: self.load_steers(id)?,
            active_subagents,
            activity_notes: None,
            goal: self
                .stored_goal(id)?
                .and_then(|goal| serde_json::to_value(goal).ok()),
        })
    }

    pub async fn get_thread_detail(&self, id: &str, limit: Option<u32>) -> Result<ThreadDetailDto> {
        self.get_thread_detail_page(id, limit, None, false).await
    }

    pub async fn get_thread_detail_view(
        &self,
        id: &str,
        limit: Option<u32>,
        summary_only: bool,
    ) -> Result<ThreadDetailDto> {
        self.get_thread_detail_page(id, limit, None, summary_only)
            .await
    }

    pub async fn get_thread_detail_page(
        &self,
        id: &str,
        limit: Option<u32>,
        before_turn_id: Option<&str>,
        summary_only: bool,
    ) -> Result<ThreadDetailDto> {
        self.get_thread_detail_page_with(id, limit, before_turn_id, summary_only, false)
            .await
    }

    /// `narration` keeps every reply with text in summary turns, so a transcript
    /// can show the agent's progress between folded runs of work.
    pub async fn get_thread_detail_page_with(
        &self,
        id: &str,
        limit: Option<u32>,
        before_turn_id: Option<&str>,
        summary_only: bool,
        narration: bool,
    ) -> Result<ThreadDetailDto> {
        self.sync_claude_scheduled_history(id).await?;
        self.observe_execution(id).await?;
        let thread = self.get_thread(id)?;
        let workspace = self.get_workspace(&thread.workspace_id)?;
        let (mut turns, total) = if summary_only {
            self.load_turn_summaries(
                id,
                Some(limit.unwrap_or(3)),
                before_turn_id,
                thread.source == "local_codex_import",
                narration,
            )?
        } else {
            self.load_turns(id, limit, before_turn_id)?
        };
        self.hydrate_missing_harness_usage(&thread, &mut turns)
            .await?;
        self.hydrate_legacy_claude_reply_phases(&thread, &mut turns)
            .await;
        if !summary_only && thread.source == "local_codex_import" {
            normalize_imported_turns(&mut turns);
        }
        let pending_steers = self.load_steers(id)?;
        let present = Path::new(&workspace.abs_path).exists();
        let runtime = self.runtime(thread.provider).ok().cloned();
        let pending_requests = if let Some(runtime) = &runtime {
            runtime.pending_requests(id).await
        } else {
            vec![]
        };
        let active_subagents = if let (Some(runtime), Some(session_id)) =
            (runtime.as_ref(), thread.provider_session_id.as_deref())
        {
            runtime.active_subagents(session_id).await
        } else {
            vec![]
        };
        let goal = if summary_only && thread.status != "running" {
            None
        } else {
            let runtime_goal = if let (Some(runtime), Some(session)) =
                (runtime.as_ref(), thread.provider_session_id.as_deref())
            {
                runtime.get_goal(session).await.ok().flatten()
            } else {
                None
            };
            self.goal_snapshot(id, runtime_goal)?
                .and_then(|goal| serde_json::to_value(goal).ok())
        };
        let thread = self.get_thread(id)?;
        Ok(ThreadDetailDto {
            thread,
            workspace,
            workspace_path_status: if present { "present" } else { "missing" }.into(),
            turns,
            total_turn_count: Some(total),
            pending_requests,
            pending_steers,
            active_subagents,
            activity_notes: Some(vec![]),
            goal,
        })
    }

    async fn hydrate_missing_harness_usage(
        &self,
        thread: &ThreadDto,
        turns: &mut [ThreadTurnDto],
    ) -> Result<()> {
        let grok = thread.agent_id.as_deref() == Some("grok");
        if (!grok
            && thread.provider != Provider::Codex
            && thread.agent_id.as_deref() != Some("codex"))
            || !turns
                .iter()
                .any(|turn| turn.status != "inProgress" && usage_needs_hydration(turn))
        {
            return Ok(());
        }
        let Some(session_id) = thread.provider_session_id.as_deref() else {
            return Ok(());
        };
        let history = if grok {
            self.usage_history
                .get_grok(&self.local_session_homes.grok_home, session_id)
                .await
        } else {
            self.usage_history
                .get(&self.local_session_homes.codex_home, session_id)
                .await
        };
        let Some(history) = history else {
            return Ok(());
        };
        let timestamp = |value: Option<&str>| {
            value
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.timestamp_millis())
        };
        for turn in turns
            .iter_mut()
            .filter(|turn| turn.status != "inProgress" && usage_needs_hydration(turn))
        {
            let source = history
                .iter()
                .find(|source| source.id == turn.id)
                .or_else(|| {
                    let start = timestamp(turn.started_at.as_deref())?;
                    let end = timestamp(turn.completed_at.as_deref())?;
                    let prompt = turn
                        .items
                        .iter()
                        .find(|item| item.kind == "userMessage")
                        .map(|item| item.text.trim());
                    let mut candidates = history.iter().filter(|source| {
                        timestamp(source.started_at.as_deref())
                            .is_some_and(|t| t >= start.saturating_sub(1000) && t <= end)
                            && prompt.is_some_and(|prompt| {
                                source.items.iter().any(|item| {
                                    item.kind == "userMessage"
                                        && usage_prompt_key(&item.text) == usage_prompt_key(prompt)
                                })
                            })
                    });
                    let source = candidates.next()?;
                    candidates.next().is_none().then_some(source)
                });
            let Some(source) = source.filter(|source| source.token_usage.is_some()) else {
                continue;
            };
            let usage = source.token_usage.as_ref().unwrap();
            turn.model = source.model.clone().or(turn.model.take());
            turn.reasoning_effort = source
                .reasoning_effort
                .clone()
                .or(turn.reasoning_effort.take());
            turn.price_estimate = if grok {
                crate::usage::estimate_price_with_catalog(
                    usage,
                    turn.model.as_deref(),
                    None,
                    &self.model_pricing(),
                    turn.started_at.as_deref(),
                )
            } else {
                source.price_estimate.clone()
            };
            turn.token_usage = crate::usage::public_usage(usage);
            self.db.with(|conn| {
                conn.execute("UPDATE thread_turns SET token_usage_json=?1,model=COALESCE(?2,model),reasoning_effort=COALESCE(?3,reasoning_effort) WHERE id=?4 AND thread_id=?5 AND (token_usage_json IS NULL OR (json_extract(token_usage_json,'$.total.totalTokens')<json_extract(token_usage_json,'$.last.totalTokens')))",params![serde_json::to_string(usage)?,turn.model,turn.reasoning_effort,turn.id,thread.id])?;
                Ok(())
            })?;
        }
        Ok(())
    }

    fn load_turns(
        &self,
        thread_id: &str,
        limit: Option<u32>,
        before_turn_id: Option<&str>,
    ) -> Result<(Vec<ThreadTurnDto>, u32)> {
        let (mut turns, total) = self.load_turns_meta_page(thread_id, limit, before_turn_id)?;
        for turn in &mut turns {
            turn.items = self.load_items_for_turn(thread_id, &turn.id)?;
        }
        Ok((turns, total))
    }

    fn load_items_for_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<Vec<ThreadHistoryItemDto>> {
        self.db.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT item_json FROM thread_history_items
                 WHERE thread_id=?1 AND turn_id=?2
                 ORDER BY created_at ASC, rowid ASC",
            )?;
            let mut items = stmt
                .query_map(params![thread_id, turn_id], |row| row.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .filter_map(|raw| serde_json::from_str::<ThreadHistoryItemDto>(&raw).ok())
                .map(|mut item| {
                    crate::history::normalize_legacy_file_change(&mut item);
                    item
                })
                .collect::<Vec<_>>();
            peer_work::annotate(conn, thread_id, turn_id, &mut items, true)?;
            Ok(items)
        })
    }

    fn load_turn_summaries(
        &self,
        thread_id: &str,
        limit: Option<u32>,
        before_turn_id: Option<&str>,
        normalize_imported: bool,
        narration: bool,
    ) -> Result<(Vec<ThreadTurnDto>, u32)> {
        let (mut turns, total_turn_count) =
            self.load_turns_meta_page(thread_id, limit, before_turn_id)?;
        for turn in &mut turns {
            let (items, total_item_count, activity) =
                self.load_turn_conversation(thread_id, &turn.id)?;
            let conversation_item_count = items.len();
            turn.items = items;
            if normalize_imported {
                normalize_imported_turns(std::slice::from_mut(turn));
            }
            let removed_item_count = conversation_item_count.saturating_sub(turn.items.len());
            let summarized = summarize_completed_turn(turn.clone(), narration);
            let deferred = total_item_count
                .saturating_sub(removed_item_count)
                .saturating_sub(summarized.items.len());
            turn.items = summarized.items;
            if deferred > 0 {
                turn.has_deferred_items = Some(true);
                turn.deferred_item_count = Some(deferred as u32);
                turn.activity = Some(activity);
            }
        }
        Ok((turns, total_turn_count))
    }

    fn load_turn_conversation(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<(Vec<ThreadHistoryItemDto>, usize, Value)> {
        self.db.with(|conn| {
            // One ordered pass over the turn: the item total and what a folded
            // summary reports (docs/architecture.md, History payloads).
            let mut stmt = conn.prepare(
                "SELECT item_id, json_extract(item_json, '$.kind'),
                   lower(COALESCE(json_extract(item_json, '$.status'), '')),
                   json_extract(item_json, '$.origin') IN ('nativeBackgroundWait', 'nativeTaskNotification')
                     OR json_extract(item_json, '$.kind') = 'userMessage'
                     OR (json_extract(item_json, '$.kind') = 'agentMessage' AND length(trim(COALESCE(json_extract(item_json, '$.text'), ''))) > 0),
                   CASE WHEN json_extract(item_json, '$.kind') = 'fileChange' THEN json_extract(item_json, '$.text') END,
                   COALESCE(json_extract(item_json, '$.changedFiles'), 0),
                   COALESCE(json_extract(item_json, '$.addedLines'), 0),
                   COALESCE(json_extract(item_json, '$.removedLines'), 0),
                   COALESCE(json_extract(item_json, '$.createdAt'), created_at),
                   COALESCE(json_extract(item_json, '$.updatedAt'), json_extract(item_json, '$.createdAt'), created_at)
                 FROM thread_history_items WHERE thread_id=?1 AND turn_id=?2
                 ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = stmt
                .query_map(params![thread_id, turn_id], |row| {
                    Ok(ActivityRow {
                        item_id: row.get(0)?,
                        kind: row.get(1)?,
                        status: row.get(2)?,
                        anchor: row.get::<_, Option<bool>>(3)?.unwrap_or(false),
                        paths: row.get(4)?,
                        changed_files: row.get(5)?,
                        added: row.get(6)?,
                        removed: row.get(7)?,
                        started_at: row.get(8)?,
                        ended_at: row.get(9)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            let total_item_count = rows.len();
            let activity = turn_activity(&rows);
            let mut stmt = conn.prepare(
                "SELECT item_json FROM thread_history_items
                 WHERE thread_id=?1 AND turn_id=?2
                   AND (json_extract(item_json, '$.kind') IN ('userMessage', 'agentMessage')
                     OR json_extract(item_json, '$.origin') IN ('nativeBackgroundWait', 'nativeTaskNotification'))
                 ORDER BY created_at ASC, rowid ASC",
            )?;
            let mut items = stmt
                .query_map(params![thread_id, turn_id], |row| row.get::<_, String>(0))?
                .filter_map(|row| row.ok())
                .filter_map(|raw| serde_json::from_str::<ThreadHistoryItemDto>(&raw).ok())
                .collect::<Vec<_>>();
            peer_work::annotate(conn, thread_id, turn_id, &mut items, false)?;
            Ok((
                items,
                total_item_count,
                Value::Object(activity),
            ))
        })
    }

    fn load_turns_meta(&self, thread_id: &str) -> Result<Vec<ThreadTurnDto>> {
        self.load_turns_meta_page(thread_id, None, None)
            .map(|(turns, _)| turns)
    }

    fn load_turns_meta_page(
        &self,
        thread_id: &str,
        limit: Option<u32>,
        before_turn_id: Option<&str>,
    ) -> Result<(Vec<ThreadTurnDto>, u32)> {
        let catalog = self.model_pricing();
        self.db.with(|conn| {
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM thread_turns WHERE thread_id=?1",
                params![thread_id],
                |row| row.get(0),
            )?;
            let before_ordinal = before_turn_id
                .map(|turn_id| {
                    conn.query_row(
                        "SELECT ordinal FROM thread_turns WHERE thread_id=?1 AND id=?2",
                        params![thread_id, turn_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                })
                .transpose()?
                .flatten();
            let page_limit = limit
                .map(i64::from)
                .or_else(|| before_turn_id.map(|_| 10))
                .unwrap_or(i64::MAX);
            let mut stmt = conn.prepare(
                "SELECT id, status, error, model, reasoning_effort, token_usage_json, started_at, completed_at, pricing_model_key, pricing_tier_key
                 FROM thread_turns
                 WHERE thread_id=?1 AND (?2 IS NULL OR ordinal < ?2)
                 ORDER BY ordinal DESC, rowid DESC LIMIT ?3",
            )?;
            let mut turns = stmt
                .query_map(params![thread_id, before_ordinal, page_limit], |row| {
                    let stored_usage = row.get::<_, Option<String>>(5)?.and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
                    let model = row.get::<_, Option<String>>(3)?;
                    let pricing_model = row.get::<_, Option<String>>(8)?.or(model.clone());
                    let tier = row.get::<_, Option<String>>(9)?;
                    let price_estimate = stored_usage.as_ref().and_then(|usage| crate::usage::estimate_price_with_catalog(usage, pricing_model.as_deref(), tier.as_deref(), &catalog, row.get::<_, String>(6).ok().as_deref()));
                    Ok(ThreadTurnDto {
                        id: row.get(0)?,
                        started_at: row.get(6)?,
                        completed_at: row.get(7)?,
                        status: row.get::<_, String>(1)?.into(),
                        error: row.get(2)?,
                        model: row.get(3)?,
                        reasoning_effort: row.get(4)?,
                        token_usage: stored_usage.as_ref().and_then(crate::usage::public_usage).or(stored_usage),
                        price_estimate,
                        has_deferred_items: None,
                        deferred_item_count: None,
                        activity: None,
                        items: Vec::new(),
                    })
                })?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>();
            turns.reverse();
            Ok((turns, u32::try_from(total).unwrap_or(u32::MAX)))
        })
    }

    pub async fn get_thread_turn_detail(&self, id: &str, turn_id: &str) -> Result<ThreadTurnDto> {
        let mut turns = self.load_turns_meta(id)?;
        let mut turn = turns
            .iter_mut()
            .find(|turn| turn.id == turn_id)
            .ok_or_else(|| anyhow!("turn not found"))?
            .clone();
        turn.items = self.load_items_for_turn(id, turn_id)?;
        let thread = self.get_thread(id)?;
        if thread.source == "local_codex_import" {
            normalize_imported_turns(std::slice::from_mut(&mut turn));
        }
        self.hydrate_missing_harness_usage(&thread, std::slice::from_mut(&mut turn))
            .await?;
        self.hydrate_legacy_claude_reply_phases(&thread, std::slice::from_mut(&mut turn))
            .await;
        turn.has_deferred_items = Some(false);
        turn.deferred_item_count = Some(0);
        Ok(turn)
    }

    pub fn get_history_item_detail(&self, id: &str, item_id: &str) -> Result<serde_json::Value> {
        let raw: Option<String> = self.db.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT item_json FROM thread_history_items WHERE thread_id=?1 AND item_id=?2 LIMIT 1",
                    params![id, item_id],
                    |row| row.get(0),
                )
                .optional()?)
        })?;
        let Some(raw) = raw else {
            bail!("history item not found");
        };
        let mut item: ThreadHistoryItemDto = serde_json::from_str(&raw)?;
        crate::history::normalize_legacy_file_change(&mut item);
        Ok(json!({
            "id": item.id,
            "kind": item.kind,
            "title": item.preview_text.unwrap_or_else(|| item.text.clone()),
            "text": item.detail_text.unwrap_or(item.text),
        }))
    }

    pub fn thread_image(&self, id: &str, rel: &str) -> Result<(Vec<u8>, &'static str)> {
        let thread = self.get_thread(id)?;
        let workspace = self.get_workspace(&thread.workspace_id)?;
        files::read_thread_image(Path::new(&workspace.abs_path), id, rel)
    }

    /// Validate and open a thread image without reading it, with the same
    /// checks as [`Self::thread_image`].
    pub fn open_thread_image(&self, id: &str, rel: &str) -> Result<files::ThreadImageFile> {
        let thread = self.get_thread(id)?;
        let workspace = self.get_workspace(&thread.workspace_id)?;
        files::open_thread_image(Path::new(&workspace.abs_path), id, rel)
    }

    pub fn thread_thumbnail_cache_dir(&self) -> PathBuf {
        crate::thread_images::cache_dir(&self.config.database_url)
    }

    fn load_steers(&self, thread_id: &str) -> Result<Vec<ThreadPendingSteerDto>> {
        self.db.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, client_request_id, turn_id, display_prompt, delivery, created_at
                 FROM thread_pending_steers WHERE thread_id=?1 AND delivery!='update-resume' ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map(params![thread_id], |row| {
                Ok(ThreadPendingSteerDto {
                    id: row.get(0)?,
                    client_request_id: row.get(1)?,
                    turn_id: row.get(2)?,
                    prompt: row.get(3)?,
                    delivery: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?;
            Ok(rows.filter_map(|r| r.ok()).collect())
        })
    }

    pub async fn prompt(
        &self,
        thread_id: &str,
        input: SendThreadPromptInput,
    ) -> Result<ThreadDetailDto> {
        self.accept_prompt(thread_id, &input).await?;
        self.drain_steers(thread_id).await?;
        self.thread_action_detail(thread_id).await
    }

    fn ensure_not_updating(&self) -> Result<()> {
        if self
            .update_draining
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            bail!("conflict: Supervisor is updating; running threads will resume automatically");
        }
        Ok(())
    }

    async fn run_turn(
        &self,
        thread: ThreadDto,
        prompt: String,
        model: Option<String>,
        effort: Option<String>,
        images: Vec<PromptImage>,
        pending_steer_id: Option<&str>,
        goal_submission: Option<GoalSubmission>,
    ) -> Result<()> {
        let id = thread.id.clone();
        self.with_cli_context(
            &id,
            self.run_turn_inner(
                thread,
                prompt,
                model,
                effort,
                images,
                pending_steer_id,
                goal_submission,
            ),
        )
        .await
    }

    async fn run_turn_inner(
        &self,
        thread: ThreadDto,
        prompt: String,
        model: Option<String>,
        effort: Option<String>,
        images: Vec<PromptImage>,
        pending_steer_id: Option<&str>,
        goal_submission: Option<GoalSubmission>,
    ) -> Result<()> {
        ensure!(
            !self.integration_thread_paused(&thread.id),
            "conflict: integration binding is detached"
        );
        // Released only after the turn; the queue drain below takes its own.
        let harness = self.hold_harness(&thread.id).await;
        self.ensure_not_updating()?;
        // A release before the gate may have forgotten an unsaved session.
        let current = self.get_thread(&thread.id)?;
        if current.status == "recovering" {
            bail!("conflict: Reconnect to confirm backend state before starting another turn");
        }
        let thread = ThreadDto {
            provider_session_id: current.provider_session_id,
            ..thread
        };
        let provider = thread.provider;
        let _maintenance = self
            .maintenance_gate
            .try_read()
            .map_err(|_| anyhow!("conflict: Supervisor update in progress"))?;
        let gate = self.harness_gate(thread.agent_id.as_deref().unwrap_or(
            if provider == Provider::Acp {
                "codex"
            } else {
                provider.as_str()
            },
        ));
        let _guard = gate
            .try_read()
            .map_err(|_| anyhow!("conflict: Harness maintenance in progress"))?;
        let runtime = self.runtime(provider)?.clone();
        let session_id = self.ensure_thread_session(&thread).await?;
        if !runtime.session_loaded(&session_id) {
            let cwd = self.session_cwd(&thread);
            // Callers that queue or automate the turn say so with their own intent.
            let reason = crate::actor::load_intent()
                .filter(|intent| intent.thread_id == thread.id)
                .map_or(HarnessLoadReason::Turn, |intent| intent.reason);
            self.with_load(
                &thread.id,
                reason,
                runtime.resume_session(
                    &session_id,
                    cwd.as_deref(),
                    thread_session_settings(&thread),
                ),
            )
            .await?;
            runtime
                .apply_session_settings(
                    session_id.as_str(),
                    SessionSettings {
                        model: thread.model.clone(),
                        effort: thread.reasoning_effort.clone(),
                        sandbox_mode: thread.sandbox_mode.clone(),
                        collaboration_mode: Some(thread.collaboration_mode.clone()),
                        approval_mode: Some(thread.approval_mode.clone()),
                        performance_mode: thread.fast_mode.then_some(true),
                    },
                )
                .await?;
        }
        if let Some(submission) = &goal_submission {
            runtime
                .stage_goal(&session_id, submission.goal.clone())
                .await?;
        }
        let turn_id = Uuid::new_v4().to_string();
        let now = now_rfc3339();
        let turn_model = model.clone().or_else(|| thread.model.clone());
        let turn_reasoning_effort = effort.clone().or_else(|| thread.reasoning_effort.clone());
        let title = if thread.title == "New thread" {
            Some(truncate_title(&prompt))
        } else {
            None
        };
        let mut live_turns = self.live.lock().await;
        self.ensure_not_updating()?;
        if live_turns.contains_key(&thread.id) {
            bail!("conflict: This thread is already running");
        }
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            let conn = &tx;
            if let Some(id) = pending_steer_id {
                crate::interaction::bind_notification(conn, id, &turn_id)?;
                automation::bind_prompt(conn, id, &turn_id, &now)?;
                if conn.execute("DELETE FROM thread_pending_steers WHERE id=?1 AND thread_id=?2", params![id, thread.id])? != 1 {
                    bail!("conflict: Queued prompt was already consumed or cancelled");
                }
            }
            let ordinal: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(ordinal),0)+1 FROM thread_turns WHERE thread_id=?1",
                    params![thread.id],
                    |row| row.get(0),
                )?;
            conn.execute(
                "INSERT INTO thread_turns(
                   id, thread_id, status, model, reasoning_effort,
                   display_prompt, started_at, ordinal, pricing_model_key, pricing_tier_key
                 ) VALUES (?1,?2,'inProgress',?3,?4,?5,?6,?7,?3,?8)",
                params![
                    turn_id,
                    thread.id,
                    turn_model,
                    turn_reasoning_effort,
                    prompt,
                    now,
                    ordinal,
                    if thread.fast_mode { "fast" } else { "standard" }
                ],
            )?;
            let user_item = ThreadHistoryItemDto {
                id: format!("{turn_id}:user"),
                created_at: Some(now.clone()),
                kind: pockymoe_protocol::ThreadItemKind::UserMessage,
                text: prompt.clone(),
                preview_text: None,
                detail_text: None,
                status: None,
                sequence: None,
                source_turn_id: Some(turn_id.clone()),
                artifact: None,
                extra: Default::default(),
            };
            conn.execute(
                "INSERT INTO thread_history_items(id, thread_id, turn_id, item_id, item_json, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?6)",
                params![
                    Uuid::new_v4().to_string(),
                    thread.id,
                    turn_id,
                    user_item.id,
                    serde_json::to_string(&user_item)?,
                    now
                ],
            )?;
            conn.execute(
                "UPDATE threads SET status='running', last_error=NULL, updated_at=?1, last_turn_started_at=?1, title=COALESCE(?2,title), closed_at=NULL,
                 agent_name=CASE WHEN closed_at IS NOT NULL AND EXISTS(SELECT 1 FROM threads o WHERE o.root_thread_id=threads.root_thread_id AND o.agent_name=threads.agent_name AND o.closed_at IS NULL AND o.id<>threads.id) THEN NULL ELSE agent_name END
                 WHERE id=?3",
                params![now, title, thread.id],
            )?;
            upsert_legacy_turn_metadata(
                conn,
                &thread.id,
                &turn_id,
                turn_model.as_deref(),
                turn_reasoning_effort.as_deref(),
                Some(&prompt),
                Some(&now),
                &now,
            )?;
            tx.commit()?;
            Ok(())
        })?;
        self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
            thread_id: thread.id.clone(),
            timestamp: now.clone(),
            payload: json!({ "status": "running", "turnId": turn_id, "title": title }),
        });
        let cancel = CancellationToken::new();
        live_turns.insert(
            thread.id.clone(),
            LiveTurn {
                turn_id: turn_id.clone(),
                cancel: cancel.clone(),
            },
        );
        drop(live_turns);
        let prompt = if let Some(submission) = goal_submission {
            self.updated_goal_snapshot(
                &thread.id,
                submission.goal,
                Some("active"),
                submission.token_budget,
            )?;
            let _ = submission.accepted.send(());
            submission.wire_prompt
        } else {
            prompt
        };
        // The inbox is passive by design: mail never interrupts a turn, and the
        // receiver is expected to poll. Nothing surfaced it though, so a peer's
        // result could sit unread while the collaboration silently stalled. Announce
        // it at the start of a turn - the one moment the agent is about to think
        // anyway - and say it is passive so it does not abandon work mid-task.
        // A role template applies once, to whichever prompt first starts work.
        let prompt = match crate::interaction::take_role(self, &thread.id) {
            Some(role) => format!("{role}\n\n{prompt}"),
            None => prompt,
        };
        let prompt = match self.pending_mail_notice(&thread.id) {
            Some(notice) => format!("{notice}\n\n{prompt}"),
            None => prompt,
        };
        let context_delivered = self.harness_context_delivered(&thread.id, &session_id);
        let bus = self.bus.for_turn(&thread.id, &turn_id);
        let result = runtime
            .start_turn(
                StartTurnInput {
                    provider_session_id: session_id,
                    prompt,
                    model,
                    reasoning_effort: effort,
                    sandbox_mode: thread.sandbox_mode.clone(),
                    collaboration_mode: Some(thread.collaboration_mode.clone()),
                    approval_mode: Some(thread.approval_mode.clone()),
                    performance_mode: thread.fast_mode.then_some(true),
                    thread_id: thread.id.clone(),
                    turn_id: turn_id.clone(),
                    hidden: false,
                    images,
                    title: Some(title.clone().unwrap_or_else(|| thread.title.clone())),
                    context_delivered,
                },
                bus.clone(),
                cancel.clone(),
            )
            .await;
        bus.close_turn();
        let mut live = self.live.lock().await;
        let completed_at = now_rfc3339();
        let result_failed = result.is_err();
        let uncertain = result
            .as_ref()
            .err()
            .is_some_and(|err| err.is::<crate::actor::ExecutionUncertain>());
        match result {
            Ok(items) => {
                let interrupted = cancel.is_cancelled();
                let status = if interrupted {
                    "interrupted"
                } else {
                    "completed"
                };
                self.persist_turn_result(
                    &thread.id,
                    &turn_id,
                    status,
                    None,
                    &items,
                    &completed_at,
                )?;
            }
            Err(err) => {
                let error = err.to_string();
                self.persist_turn_result(
                    &thread.id,
                    &turn_id,
                    if cancel.is_cancelled() {
                        "interrupted"
                    } else if uncertain {
                        "recovering"
                    } else {
                        "failed"
                    },
                    if cancel.is_cancelled() {
                        None
                    } else {
                        Some(error.as_str())
                    },
                    &[],
                    &completed_at,
                )?;
            }
        }
        live.remove(&thread.id);
        drop(live);
        let thread_status = if cancel.is_cancelled() {
            "interrupted"
        } else if uncertain {
            "recovering"
        } else if result_failed {
            "failed"
        } else {
            "idle"
        };
        self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
            thread_id: thread.id.clone(),
            timestamp: completed_at.clone(),
            payload: json!({
                "status": thread_status,
                "turnId": turn_id,
                "completedAt": completed_at
            }),
        });
        drop(harness);
        self.drain_steers(&thread.id).await?;
        Ok(())
    }

    fn persist_turn_result(
        &self,
        thread_id: &str,
        turn_id: &str,
        status: &str,
        error: Option<&str>,
        items: &[ThreadHistoryItemDto],
        now: &str,
    ) -> Result<()> {
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            let conn = &tx;
            conn.execute(
                "UPDATE thread_turns SET status=?1, error=?2, completed_at=CASE WHEN ?1='recovering' THEN NULL ELSE ?3 END WHERE id=?4",
                params![status, error, now, turn_id],
            )?;
            for item in items {
                conn.execute(
                    "INSERT INTO thread_history_items(id, thread_id, turn_id, item_id, item_json, created_at, updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?6)
                     ON CONFLICT(thread_id, turn_id, item_id) DO UPDATE SET
                        item_json=excluded.item_json,
                        updated_at=excluded.updated_at",
                    params![
                        Uuid::new_v4().to_string(),
                        thread_id,
                        turn_id,
                        item.id,
                        serde_json::to_string(item)?,
                        now
                    ],
                )?;
            }
            let thread_status = if status == "completed" { "idle" } else { status };
            conn.execute(
                "UPDATE threads SET status=?1, last_error=?2, updated_at=?3, last_turn_completed_at=?3 WHERE id=?4",
                params![thread_status, error, now, thread_id],
            )?;
            let (model, reasoning_effort, display_prompt, started_at): (
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
            ) = conn.query_row(
                "SELECT model, reasoning_effort, display_prompt, started_at
                 FROM thread_turns WHERE id=?1",
                params![turn_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            upsert_legacy_turn_metadata(
                conn,
                thread_id,
                turn_id,
                model.as_deref(),
                reasoning_effort.as_deref(),
                display_prompt.as_deref(),
                started_at.as_deref(),
                now,
            )?;
            conn.execute(
                "DELETE FROM thread_pending_steers
                 WHERE thread_id=?1 AND turn_id=?2 AND delivery='steer'",
                params![thread_id, turn_id],
            )?;
            crate::interaction::finish_notification(conn, thread_id, turn_id, status, now)?;
            if matches!(status, "completed" | "failed" | "interrupted") {
                // Plugin configuration errors must not roll back the agent's completed turn.
                if let Err(error) = self.plugins.sync_turn_automations(conn, thread_id, now) {
                    tracing::warn!(%error, "could not configure plugin turn hooks");
                }
            }
            automation::turn_ended(conn, thread_id, turn_id, status, now)?;
            if self.config.relay_server_url.is_some() {
                crate::relay_notifications::record(conn, thread_id, turn_id, status, now)?;
            }
            tx.commit()?;
            Ok(())
        })?;
        self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
            thread_id: thread_id.into(),
            timestamp: now.into(),
            payload: json!({ "status": if status == "completed" { "idle" } else { status }, "turnId": turn_id }),
        });
        Ok(())
    }

    pub(crate) async fn drain_steers(&self, thread_id: &str) -> Result<()> {
        let lock = {
            let mut locks = self.steer_locks.lock().await;
            let key = format!("inbox:{thread_id}");
            let lock = locks
                .get(&key)
                .and_then(std::sync::Weak::upgrade)
                .unwrap_or_else(|| Arc::new(Mutex::new(())));
            locks.insert(key, Arc::downgrade(&lock));
            lock
        };
        let Ok(_delivery) = lock.try_lock() else {
            return Ok(());
        };
        loop {
            if self
                .update_draining
                .load(std::sync::atomic::Ordering::SeqCst)
                || self.integration_thread_paused(thread_id)
                || self.live.lock().await.contains_key(thread_id)
            {
                return Ok(());
            }
            let thread = self.get_thread(thread_id)?;
            if matches!(thread.status.as_str(), "running" | "recovering") {
                return Ok(());
            }
            let next = self.db.with(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT id,submitted_prompt,payload_json,turn_id FROM thread_pending_steers
                 WHERE thread_id=?1 AND delivery='continuation' ORDER BY created_at,rowid LIMIT 1",
                        params![thread_id],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, String>(1)?,
                                r.get::<_, Option<String>>(2)?,
                                r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                            ))
                        },
                    )
                    .optional()?)
            })?;
            let Some((id, prompt, payload, queued_behind)) = next else {
                return Ok(());
            };
            let load_reason = self.db.with(|conn| {
                let automated = conn
                    .query_row(
                        "SELECT 1 FROM automation_runs WHERE pending_id=?1",
                        params![id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                // A prompt submitted while the thread was idle is a turn; one that
                // waited behind a turn, or came from another thread, was queued.
                Ok(if automated {
                    HarnessLoadReason::Automation
                } else if payload.is_some() && queued_behind.is_empty() {
                    HarnessLoadReason::Turn
                } else {
                    HarnessLoadReason::Queue
                })
            })?;
            let allowed = self.db.with(|c| {
                let tx = c.unchecked_transaction()?;
                let allowed = automation::pending_allowed(&tx, &id, &now_rfc3339())?;
                tx.commit()?;
                Ok(allowed)
            })?;
            if !allowed {
                continue;
            }
            let input = payload
                .map(|raw| serde_json::from_str::<SendThreadPromptInput>(&raw))
                .transpose()?;
            let model = input.as_ref().and_then(|i| i.model.clone());
            let effort = input.as_ref().and_then(|i| i.reasoning_effort.clone());
            let images = input
                .map(|i| {
                    i.images
                        .into_iter()
                        .map(|i| PromptImage {
                            mime_type: i.mime_type,
                            data: i.data,
                        })
                        .collect()
                })
                .unwrap_or_default();
            Box::pin(crate::actor::with_load_intent(
                thread_id,
                load_reason,
                self.run_turn(thread, prompt, model, effort, images, Some(&id), None),
            ))
            .await?;
        }
    }

    fn find_pending_steer(
        &self,
        thread_id: &str,
        pending_steer_id: &str,
    ) -> Result<Option<PendingSteerRecord>> {
        self.db.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT id, turn_id, submitted_prompt, delivery, display_prompt
                     FROM thread_pending_steers WHERE thread_id=?1 AND id=?2",
                    params![thread_id, pending_steer_id],
                    |row| {
                        Ok(PendingSteerRecord {
                            id: row.get(0)?,
                            turn_id: row.get(1)?,
                            submitted_prompt: row.get(2)?,
                            delivery: row.get(3)?,
                            display_prompt: row.get(4)?,
                        })
                    },
                )
                .optional()?)
        })
    }

    pub async fn cancel_pending_steer(
        &self,
        thread_id: &str,
        pending_steer_id: &str,
    ) -> Result<ThreadDetailDto> {
        self.get_thread(thread_id)?;
        let removed = self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            let removed = tx.execute(
                "DELETE FROM thread_pending_steers WHERE thread_id=?1 AND id=?2",
                params![thread_id, pending_steer_id],
            )?;
            if removed > 0 {
                tx.execute(
                    "DELETE FROM kv WHERE key IN (?1,?2)",
                    params![
                        format!("cli:notify:pending:{pending_steer_id}"),
                        format!("cli:peer-work:pending:{pending_steer_id}")
                    ],
                )?;
            }
            tx.commit()?;
            Ok(removed)
        })?;
        if removed == 0 {
            bail!("Pending queued prompt was not found.");
        }
        self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
            thread_id: thread_id.into(),
            timestamp: now_rfc3339(),
            payload: json!({ "reason": "pending_steer_updated" }),
        });
        self.thread_action_detail(thread_id).await
    }

    pub async fn steer_pending_prompt(
        &self,
        thread_id: &str,
        pending_steer_id: &str,
    ) -> Result<ThreadDetailDto> {
        let lock = {
            let mut locks = self.steer_locks.lock().await;
            locks.retain(|_, lock| lock.strong_count() > 0);
            let key = format!("{thread_id}:{pending_steer_id}");
            let lock = locks
                .get(&key)
                .and_then(std::sync::Weak::upgrade)
                .unwrap_or_else(|| Arc::new(Mutex::new(())));
            locks.insert(key, Arc::downgrade(&lock));
            lock
        };
        let _delivery_guard = lock.lock().await;
        let delivered = self.db.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT 1 FROM thread_history_items WHERE thread_id=?1 AND item_id=?2 LIMIT 1",
                    params![thread_id, format!("steer:{pending_steer_id}")],
                    |_| Ok(()),
                )
                .optional()?
                .is_some())
        })?;
        if delivered {
            return self.thread_action_detail(thread_id).await;
        }
        let thread = self.get_thread(thread_id)?;
        let pending = self
            .find_pending_steer(thread_id, pending_steer_id)?
            .ok_or_else(|| anyhow!("Pending queued prompt was not found."))?;
        if !["continuation", "cli-steer"].contains(&pending.delivery.as_str()) {
            bail!("conflict: This prompt has already been steered.");
        }
        let active_turn_id = thread
            .active_turn_id
            .as_deref()
            .filter(|_| thread.status == "running")
            .ok_or_else(|| {
                anyhow!("conflict: The active turn finished before this prompt could be steered.")
            })?;
        if pending.delivery == "cli-steer"
            && !pending.turn_id.is_empty()
            && pending.turn_id != active_turn_id
        {
            bail!("conflict: The targeted turn ended; this message is held and cannot steer a different turn.");
        }
        let provider_session_id = thread
            .provider_session_id
            .as_deref()
            .ok_or_else(|| anyhow!("thread has no provider session"))?;
        let runtime = self.runtime(thread.provider)?;
        if !runtime
            .negotiated_caps(thread.agent_id.as_deref())
            .turns
            .steer
        {
            bail!("conflict: This backend does not support steering an active turn.");
        }
        let submitted_at = now_rfc3339();
        runtime
            .send_input(
                provider_session_id,
                active_turn_id,
                &pending.submitted_prompt,
            )
            .await?;

        // The harness acknowledgement does not echo a user-message event.
        // Move the delivered prompt into durable history before removing its
        // temporary queue bubble, including when the turn already completed.
        let now = now_rfc3339();
        let item = ThreadHistoryItemDto {
            id: format!("steer:{}", pending.id),
            kind: pockymoe_protocol::ThreadItemKind::UserMessage,
            text: pending.display_prompt.clone(),
            created_at: Some(submitted_at.clone()),
            source_turn_id: Some(active_turn_id.into()),
            preview_text: None,
            detail_text: None,
            status: None,
            sequence: None,
            artifact: None,
            extra: Default::default(),
        };
        self.db.with(|conn| {
            let tx=conn.unchecked_transaction()?;
            let conn=&tx;
            conn.execute(
                "INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(thread_id,turn_id,item_id) DO NOTHING",
                params![Uuid::new_v4().to_string(), thread_id, active_turn_id, item.id,
                    serde_json::to_string(&item)?, submitted_at, now],
            )?;
            conn.execute(
                "UPDATE thread_pending_steers SET delivery='steer',turn_id=?1,updated_at=?2 WHERE thread_id=?3 AND id=?4",
                params![active_turn_id, now, thread_id, pending.id],
            )?;
            crate::interaction::bind_notification(conn, &pending.id, &active_turn_id)?;
            tx.commit()?;
            Ok(())
        })?;
        self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
            thread_id: thread_id.into(),
            timestamp: now,
            payload: json!({
                "reason": "pending_steer_updated",
                "turnId": active_turn_id,
                "previousTurnId": pending.turn_id
            }),
        });
        self.thread_action_detail(thread_id).await
    }

    pub async fn interrupt(&self, thread_id: &str) -> Result<ThreadDetailDto> {
        // Serialize user stops with the update's snapshot and cancellation.
        let live_turns = self.live.lock().await;
        // An explicit user stop cancels an update's automatic continuation too.
        self.db.with(|conn| {
            conn.execute(
                "DELETE FROM thread_pending_steers WHERE thread_id=?1 AND delivery='update-resume'",
                params![thread_id],
            )?;
            Ok(())
        })?;
        self.pause_thread_prompt_automations(thread_id, "userStop")?;
        let interrupted_turn_id = self.active_turn_id(thread_id)?;
        let had_live_turn = {
            if let Some(live) = live_turns.get(thread_id) {
                live.cancel.cancel();
                true
            } else {
                false
            }
        };
        drop(live_turns);
        if !had_live_turn {
            if let Ok(thread) = self.get_thread(thread_id) {
                if let Some(session) = thread.provider_session_id {
                    let _ = self
                        .runtime(thread.provider)?
                        .interrupt(&session, thread.active_turn_id.as_deref().unwrap_or(""))
                        .await;
                }
            }

            let reconciled = self.reconcile_stale_turns(Some(thread_id), true)?;
            if reconciled > 0 {
                self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
                    event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
                    thread_id: thread_id.into(),
                    timestamp: now_rfc3339(),
                    payload: json!({ "status": "interrupted" }),
                });
            }
            return self.get_thread_detail_view(thread_id, Some(3), true).await;
        }
        for _ in 0..100 {
            if self.active_turn_id(thread_id)? != interrupted_turn_id {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        // The live turn owns cancellation and queue draining. Do not reconcile
        // a slow cancellation (or its newly started continuation) as stale.
        self.get_thread_detail_view(thread_id, Some(3), true).await
    }

    pub fn rename_thread(&self, id: &str, title: &str) -> Result<ThreadDto> {
        self.db.with(|conn| {
            conn.execute(
                "UPDATE threads SET title=?1, updated_at=?2 WHERE id=?3",
                params![title, now_rfc3339(), id],
            )?;
            Ok(())
        })?;
        self.get_thread(id)
    }

    pub fn delete_thread(&self, id: &str) -> Result<()> {
        self.db.with(|conn| {
            conn.execute(
                "DELETE FROM thread_history_items WHERE thread_id=?1",
                params![id],
            )?;
            conn.execute("DELETE FROM thread_turns WHERE thread_id=?1", params![id])?;
            conn.execute(
                "DELETE FROM thread_pending_steers WHERE thread_id=?1",
                params![id],
            )?;
            conn.execute("DELETE FROM threads WHERE id=?1", params![id])?;
            Ok(())
        })
    }

    pub async fn update_settings(
        &self,
        id: &str,
        model: Option<String>,
        effort: Option<String>,
        fast: Option<bool>,
        collab: Option<String>,
        sandbox: Option<String>,
    ) -> Result<ThreadDto> {
        let thread = self.get_thread(id)?;
        let approval = sandbox.as_deref().map(|mode| {
            if mode == "danger-full-access" {
                "yolo".to_string()
            } else {
                "guarded".to_string()
            }
        });
        let _harness = self.hold_harness(id).await;
        if let Some(session) = thread.provider_session_id.as_deref() {
            let runtime = self.runtime(thread.provider)?;
            if fast == Some(true)
                && !runtime
                    .negotiated_caps(thread.agent_id.as_deref())
                    .controls
                    .performance_mode
            {
                bail!("This backend does not support Fast mode.");
            }
            self.with_cli_context(
                id,
                runtime.apply_session_settings(
                    session,
                    SessionSettings {
                        model: model.clone(),
                        effort: effort.clone(),
                        sandbox_mode: sandbox.clone(),
                        collaboration_mode: collab.clone(),
                        approval_mode: approval.clone(),
                        performance_mode: fast,
                    },
                ),
            )
            .await?;
        }
        self.db.with(|conn| {
            if let Some(model) = model {
                conn.execute(
                    "UPDATE threads SET model=?1 WHERE id=?2",
                    params![model, id],
                )?;
            }
            if let Some(effort) = effort {
                conn.execute(
                    "UPDATE threads SET reasoning_effort=?1 WHERE id=?2",
                    params![effort, id],
                )?;
            }
            if let Some(fast) = fast {
                conn.execute(
                    "UPDATE threads SET fast_mode=?1 WHERE id=?2",
                    params![fast as i64, id],
                )?;
            }
            if let Some(collab) = collab {
                conn.execute(
                    "UPDATE threads SET collaboration_mode=?1 WHERE id=?2",
                    params![collab, id],
                )?;
            }
            if let Some(sandbox) = sandbox {
                conn.execute(
                    "UPDATE threads SET sandbox_mode=?1, approval_mode=?2 WHERE id=?3",
                    params![sandbox, approval, id],
                )?;
            }
            conn.execute(
                "UPDATE threads SET updated_at=?1 WHERE id=?2",
                params![now_rfc3339(), id],
            )?;
            Ok(())
        })?;
        self.get_thread(id)
    }

    pub fn list_fork_turn_options(&self, id: &str) -> Result<Vec<ThreadForkTurnOptionDto>> {
        self.get_thread(id)?;
        Ok(self
            .load_turns_meta(id)?
            .into_iter()
            .enumerate()
            .map(|(index, turn)| ThreadForkTurnOptionDto {
                turn_id: turn.id,
                turn_index: u32::try_from(index + 1).unwrap_or(u32::MAX),
                started_at: turn.started_at,
                status: turn.status,
            })
            .collect())
    }

    pub async fn fork_thread(&self, id: &str) -> Result<ThreadDto> {
        self.fork_thread_at(id, "latest", None)
            .await
            .map(|(thread, _, _)| thread)
    }

    pub async fn fork_thread_at(
        &self,
        id: &str,
        mode: &str,
        requested_turn_id: Option<&str>,
    ) -> Result<(ThreadDto, Option<String>, Option<u32>)> {
        if !matches!(mode, "latest" | "turn") {
            bail!("mode must be latest or turn");
        }
        let detail = self.get_thread_detail(id, None).await?;
        if detail.thread.status == "running" {
            bail!("conflict: Cannot fork a thread while it is still running.");
        }
        let turn_options = self.list_fork_turn_options(id)?;
        let selected = if mode == "turn" {
            let requested_turn_id = requested_turn_id
                .filter(|turn_id| !turn_id.trim().is_empty())
                .ok_or_else(|| anyhow!("turnId is required when mode is turn"))?;
            turn_options
                .iter()
                .find(|turn| turn.turn_id == requested_turn_id)
                .ok_or_else(|| anyhow!("The selected fork turn was not found."))?
        } else if let Some(latest) = turn_options.last() {
            latest
        } else {
            return self.fork_empty_thread(&detail).await;
        };
        let runtime = self.runtime(detail.thread.provider)?;
        let session = detail
            .thread
            .provider_session_id
            .as_deref()
            .ok_or_else(|| anyhow!("thread has no provider session"))?;
        // Until the fork is saved, its session shares the source's process.
        let _harness = self.hold_harness(&detail.thread.id).await;
        self.with_load(
            &detail.thread.id,
            HarnessLoadReason::Fork,
            runtime.resume_session(
                session,
                Some(&detail.workspace.abs_path),
                thread_session_settings(&detail.thread),
            ),
        )
        .await?;
        let caps = runtime.negotiated_caps(detail.thread.agent_id.as_deref());
        if !caps.branching.fork {
            bail!("this harness does not support session/fork");
        }
        let session = detail
            .thread
            .provider_session_id
            .clone()
            .ok_or_else(|| anyhow!("thread has no provider session"))?;
        let rollback_count = u32::try_from(turn_options.len())
            .unwrap_or(u32::MAX)
            .saturating_sub(selected.turn_index);
        let forked = runtime.fork_session_at(&session, rollback_count).await?;
        let saved = self.persist_forked_thread(
            &detail,
            forked,
            usize::try_from(selected.turn_index).unwrap_or(usize::MAX),
            Some(selected.turn_id.clone()),
            Some(selected.turn_index),
        )?;
        self.announce_fork_load(&saved.0).await;
        Ok(saved)
    }

    async fn fork_empty_thread(
        &self,
        detail: &ThreadDetailDto,
    ) -> Result<(ThreadDto, Option<String>, Option<u32>)> {
        let runtime = self.runtime(detail.thread.provider)?;
        let _harness = self.hold_harness(&detail.thread.id).await;
        let session = self
            .ensure_thread_session(&self.get_thread(&detail.thread.id)?)
            .await?;
        self.with_load(
            &detail.thread.id,
            HarnessLoadReason::Fork,
            runtime.resume_session(
                &session,
                Some(&detail.workspace.abs_path),
                thread_session_settings(&detail.thread),
            ),
        )
        .await?;
        let caps = runtime.negotiated_caps(detail.thread.agent_id.as_deref());
        if !caps.branching.fork {
            bail!("this harness does not support session/fork");
        }
        let forked = runtime.fork_session(&session).await?;
        let saved = self.persist_forked_thread(detail, forked, 0, None, None)?;
        self.announce_fork_load(&saved.0).await;
        Ok(saved)
    }

    /// A Claude or Grok fork is served by its source's process until reloaded.
    async fn announce_fork_load(&self, fork: &ThreadDto) {
        let (Some(session), Ok(runtime)) = (&fork.provider_session_id, self.runtime(fork.provider))
        else {
            return;
        };
        if runtime
            .loaded_sessions()
            .await
            .iter()
            .any(|loaded| session_ids_match(&loaded.session_id, session))
        {
            self.emit_loaded(&fork.id, HarnessLoadReason::Fork);
        }
    }

    fn persist_forked_thread(
        &self,
        detail: &ThreadDetailDto,
        forked: crate::actor::StartSessionResult,
        turn_count: usize,
        selected_turn_id: Option<String>,
        selected_turn_index: Option<u32>,
    ) -> Result<(ThreadDto, Option<String>, Option<u32>)> {
        let new_id = Uuid::new_v4().to_string();
        let now = now_rfc3339();
        let title = format!("{} / fork", detail.thread.title);
        self.db.with(|conn| {
            conn.execute(
                "INSERT INTO threads(id, workspace_id, provider, agent_id, provider_session_id, source, title, model,
                    reasoning_effort, collaboration_mode, approval_mode, status, created_at, updated_at, sandbox_mode, fast_mode)
                 VALUES (?1,?2,?3,?4,?5,'supervisor',?6,?7,?8,?11,?9,'idle',?10,?10,?12,?13)",
                params![
                    new_id,
                    detail.thread.workspace_id,
                    detail.thread.provider.as_str(),
                    detail.thread.agent_id,
                    forked.provider_session_id,
                    title,
                    detail.thread.model,
                    detail.thread.reasoning_effort,
                    detail.thread.approval_mode,
                    now,
                    detail.thread.collaboration_mode,
                    detail.thread.sandbox_mode,
                    detail.thread.fast_mode
                ],
            )?;
            for (ordinal, turn) in detail.turns.iter().take(turn_count).enumerate() {
                let new_turn_id = Uuid::new_v4().to_string();
                let display_prompt = turn
                    .items
                    .iter()
                    .find(|item| item.kind == "userMessage")
                    .map(|item| item.text.as_str());
                let token_usage_json = turn
                    .token_usage
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?;
                conn.execute(
                    "INSERT INTO thread_turns(
                       id, thread_id, status, error, model, reasoning_effort,
                       token_usage_json, display_prompt, started_at, ordinal
                     ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        new_turn_id,
                        new_id,
                        turn.status.as_str(),
                        turn.error,
                        turn.model,
                        turn.reasoning_effort,
                        token_usage_json,
                        display_prompt,
                        turn.started_at,
                        ordinal as i64 + 1
                    ],
                )?;
                upsert_legacy_turn_metadata(
                    conn,
                    &new_id,
                    &new_turn_id,
                    turn.model.as_deref(),
                    turn.reasoning_effort.as_deref(),
                    display_prompt,
                    turn.started_at.as_deref(),
                    &now,
                )?;
                for item in &turn.items {
                    let mut copied = item.clone();
                    copied.source_turn_id = Some(new_turn_id.clone());
                    conn.execute(
                        "INSERT INTO thread_history_items(id, thread_id, turn_id, item_id, item_json, created_at, updated_at)
                         VALUES (?1,?2,?3,?4,?5,?6,?6)",
                        params![
                            Uuid::new_v4().to_string(),
                            new_id,
                            new_turn_id,
                            copied.id,
                            serde_json::to_string(&copied)?,
                            now
                        ],
                    )?;
                }
            }
            Ok(())
        })?;
        Ok((
            self.get_thread(&new_id)?,
            selected_turn_id,
            selected_turn_index,
        ))
    }

    pub async fn compact_thread(&self, id: &str) -> Result<ThreadDto> {
        let _harness = self.hold_harness(id).await;
        let thread = self.get_thread(id)?;
        let session = thread
            .provider_session_id
            .clone()
            .ok_or_else(|| anyhow!("thread has no provider session"))?;
        self.runtime(thread.provider)?
            .compact_session(&session, id, self.bus.clone())
            .await?;
        // Compaction can drop the context; the next turn delivers it again.
        self.db.set_kv(&harness_context_key(id), "")?;
        self.get_thread(id)
    }

    /// Whether this harness session already holds the Pockymoe context.
    /// A restarted or resumed process replays its history, so this survives
    /// processes; a new session ID (new thread session, fork) starts over.
    fn harness_context_delivered(&self, thread_id: &str, session_id: &str) -> bool {
        self.db
            .get_kv(&harness_context_key(thread_id))
            .ok()
            .flatten()
            .is_some_and(|delivered| delivered == session_id)
    }

    pub async fn resume_thread(self: &Arc<Self>, id: &str) -> Result<ThreadDetailDto> {
        let state = self.clone();
        let id = id.to_string();
        // Reconnection and accepted work belong to the Supervisor. A browser or
        // relay request timing out must not drop an owned backend operation.
        tokio::spawn(async move {
            state.reconnect_thread(&id).await?;
            let detail = state.get_thread_detail_view(&id, Some(3), true).await?;
            let maintenance_pending = state.db.with(|conn| Ok(conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM thread_pending_steers WHERE thread_id=?1 AND delivery='update-resume')",
                [&id], |r| r.get::<_, bool>(0),
            )?))?;
            if maintenance_pending {
                state.spawn_update_recovery_for(Some(&id));
            } else {
                state.dispatch_inbox(id);
            }
            Ok(detail)
        })
        .await?
    }

    async fn reconnect_thread(&self, id: &str) -> Result<()> {
        let harness = self.hold_harness(id).await;
        let thread = self.get_thread(id)?;
        let cwd = self.session_cwd(&thread);
        if let Some(session) = &thread.provider_session_id {
            let runtime = self.runtime(thread.provider)?;
            self.with_load(
                &thread.id,
                HarnessLoadReason::Resume,
                runtime.resume_session(session, cwd.as_deref(), thread_session_settings(&thread)),
            )
            .await?;
            runtime
                .apply_session_settings(
                    session,
                    SessionSettings {
                        model: thread.model.clone(),
                        effort: thread.reasoning_effort.clone(),
                        sandbox_mode: thread.sandbox_mode.clone(),
                        collaboration_mode: Some(thread.collaboration_mode.clone()),
                        approval_mode: Some(thread.approval_mode.clone()),
                        performance_mode: thread.fast_mode.then_some(true),
                    },
                )
                .await?;
        }
        drop(harness);
        self.settle_reconnected_execution(id).await
    }

    pub fn prepare_prompt_attachments(
        &self,
        thread_id: &str,
        prompt: &str,
        attachments: Vec<UploadedPromptAttachment>,
    ) -> Result<String> {
        if attachments.is_empty() {
            return Ok(prompt.to_string());
        }
        let thread = self.get_thread(thread_id)?;
        let workspace = self.get_workspace(&thread.workspace_id)?;
        let dir = PathBuf::from(&workspace.abs_path)
            .join(".temp")
            .join("threads")
            .join(thread_id);
        std::fs::create_dir_all(&dir)?;
        let mut rewritten = prompt.to_string();
        for attachment in attachments {
            if !rewritten.contains(&attachment.placeholder) {
                bail!(
                    "Prompt is missing attachment placeholder {}.",
                    attachment.placeholder
                );
            }
            let safe = sanitize_file_name(&attachment.original_name);
            std::fs::write(dir.join(&safe), &attachment.bytes)?;
            let rel = format!("./.temp/threads/{thread_id}/{safe}");
            let token = if attachment.kind == "photo" {
                format!("[PHOTO {rel}]")
            } else {
                format!("[FILE {rel}]")
            };
            rewritten = rewritten.replace(&attachment.placeholder, &token);
        }
        Ok(rewritten)
    }

    /// Goal set/resume in codex-acp can execute a complete model turn. Admit it
    /// like a prompt, returning after durable history rather than model completion.
    pub async fn submit_thread_goal(
        self: &Arc<Self>,
        id: &str,
        objective: Option<String>,
        status: Option<String>,
        token_budget: Option<Option<u64>>,
    ) -> Result<Value> {
        if objective.as_deref().is_some_and(|s| s.trim().is_empty()) {
            bail!("objective must not be empty");
        }
        if token_budget == Some(Some(0)) {
            bail!("tokenBudget must be positive");
        }
        if status.as_deref().is_some_and(|s| {
            !matches!(
                s,
                "active" | "paused" | "budgetLimited" | "complete" | "terminated"
            )
        }) {
            bail!("invalid goal status");
        }
        if objective.is_none() && status.as_deref() != Some("active") {
            return self
                .thread_goal(id, objective, status, token_budget, false)
                .await;
        }
        let harness = self.hold_harness(id).await;
        let thread = self.get_thread(id)?;
        let runtime = self.runtime(thread.provider)?;
        let session = self.ensure_thread_session(&thread).await?;
        let session = session.as_str();
        if !runtime.session_loaded(session) {
            let cwd = self.session_cwd(&thread);
            self.with_load(
                &thread.id,
                HarnessLoadReason::Turn,
                runtime.resume_session(session, cwd.as_deref(), thread_session_settings(&thread)),
            )
            .await?;
        }
        let argument = objective.as_deref().unwrap_or("resume").trim();
        let Some(wire_prompt) = runtime.goal_prompt(session, argument).await? else {
            return self
                .thread_goal(id, objective, status, token_budget, false)
                .await;
        };
        if objective.is_some()
            && (matches!(
                argument.to_ascii_lowercase().as_str(),
                "pause" | "resume" | "clear"
            ) || argument.chars().count() > 4000)
        {
            bail!("Use a descriptive goal objective of at most 4000 characters");
        }
        self.ensure_prompt_allowed(&thread)?;
        if thread.status == "running" {
            bail!("conflict: Pause or interrupt the current turn before starting a goal");
        }
        let objective = objective
            .or_else(|| self.stored_goal(id).ok().flatten().map(|g| g.objective))
            .ok_or_else(|| anyhow!("No goal to resume"))?;
        let (accepted, acknowledgement) = tokio::sync::oneshot::channel();
        let submission = GoalSubmission {
            wire_prompt,
            goal: GoalState {
                objective: objective.clone(),
                status: "active".into(),
                tokens_used: 0,
                time_used_seconds: 0,
            },
            token_budget,
            accepted,
        };
        let background = self.clone();
        drop(harness);
        let task = tokio::spawn(async move {
            background
                .run_turn(
                    thread,
                    objective,
                    None,
                    None,
                    Vec::new(),
                    None,
                    Some(submission),
                )
                .await
        });
        match tokio::time::timeout(std::time::Duration::from_secs(20), acknowledgement).await {
            Ok(Ok(())) => Ok(json!({"goal": self.stored_goal(id)?})),
            Ok(Err(_)) => {
                task.await.map_err(|e| anyhow!(e))??;
                bail!("Goal was not accepted")
            }
            Err(_) => {
                // The acknowledgement only waits for preflight, not the model.
                task.abort();
                let _ = task.await;
                bail!("Goal startup timed out; retry after reconnecting the thread")
            }
        }
    }

    pub async fn thread_goal(
        &self,
        id: &str,
        objective: Option<String>,
        status: Option<String>,
        token_budget: Option<Option<u64>>,
        clear: bool,
    ) -> Result<serde_json::Value> {
        let _harness = self.hold_harness(id).await;
        let thread = self.get_thread(id)?;
        // None until the first prompt when an unsaved first session was released.
        let session = thread.provider_session_id.clone();
        let runtime = self.runtime(thread.provider)?;
        if clear {
            if let Some(session) = &session {
                runtime
                    .set_goal(session, None, Some("clear".into()))
                    .await?;
            }
            self.delete_stored_goal(id)?;
            return Ok(json!({ "cleared": true, "goalHistory": [] }));
        }
        if objective
            .as_deref()
            .is_some_and(|objective| objective.trim().is_empty())
        {
            bail!("objective must not be empty");
        }
        if status.as_deref().is_some_and(|status| {
            !matches!(
                status,
                "active" | "paused" | "budgetLimited" | "complete" | "terminated"
            )
        }) {
            bail!("invalid goal status");
        }
        if token_budget == Some(Some(0)) {
            bail!("tokenBudget must be positive");
        }

        let stored = self.stored_goal(id)?;
        let updates_goal = objective.is_some() || status.is_some() || token_budget.is_some();
        let runtime_goal = match &session {
            Some(session) if objective.is_some() || status.is_some() => {
                runtime.set_goal(session, objective, status.clone()).await?
            }
            Some(session) => runtime.get_goal(session).await?,
            None if objective.is_some() || status.is_some() => {
                bail!("conflict: Send the thread's first prompt before changing its goal")
            }
            None => None,
        };
        let runtime_goal = runtime_goal.or_else(|| {
            stored.as_ref().map(|goal| GoalState {
                objective: goal.objective.clone(),
                status: goal.status.clone(),
                tokens_used: u32::try_from(goal.tokens_used).unwrap_or(u32::MAX),
                time_used_seconds: u32::try_from(goal.time_used_seconds).unwrap_or(u32::MAX),
            })
        });
        let goal = if let Some(runtime_goal) = runtime_goal {
            if updates_goal {
                Some(self.updated_goal_snapshot(
                    id,
                    runtime_goal,
                    status.as_deref(),
                    token_budget,
                )?)
            } else {
                self.goal_snapshot(id, Some(runtime_goal))?
            }
        } else {
            None
        };
        Ok(json!({ "goal": goal }))
    }

    pub async fn respond_request(
        &self,
        id: &str,
        request_id: &str,
        allow: bool,
        answer: Option<&str>,
    ) -> Result<ThreadDetailDto> {
        let thread = self.get_thread(id)?;
        ensure!(
            self.runtime(thread.provider)?
                .pending_requests(id)
                .await
                .iter()
                .any(|request| request.id == request_id),
            "request not found in this thread"
        );
        self.runtime(thread.provider)?
            .respond_permission(request_id, allow, answer)
            .await?;
        self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadRequestResolved,
            thread_id: id.into(),
            timestamp: now_rfc3339(),
            payload: json!({ "requestId": request_id }),
        });
        self.get_thread_detail_view(id, Some(3), true).await
    }

    pub fn active_turn_count(&self) -> u32 {
        self.list_threads(None, true)
            .unwrap_or_default()
            .iter()
            .filter(|t| t.status == "running")
            .count() as u32
    }

    pub async fn list_models(
        &self,
        provider: Provider,
        agent_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<Vec<ModelOptionDto>> {
        let models = self.runtime(provider)?.list_models(agent_id, cwd).await?;
        let mut names: Value = self
            .db
            .get_kv("model_display_names")?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(json!({}));
        for model in &models {
            names[&model.id] = json!(model.display_name);
            names[&model.model] = json!(model.display_name);
        }
        self.db.set_kv("model_display_names", &names.to_string())?;
        Ok(models)
    }

    pub async fn list_agents(&self, provider: Provider) -> Result<Vec<ModelOptionDto>> {
        self.runtime(provider)?.list_agents().await
    }

    pub async fn capabilities(
        &self,
        provider: Provider,
        agent_id: Option<&str>,
    ) -> Result<AgentCapabilitySnapshotDto> {
        self.runtime(provider)?.capabilities(agent_id).await
    }

    /// With `load` false, a thread without a harness gets the provider's
    /// capabilities and no process is started.
    pub async fn thread_capabilities(
        &self,
        id: &str,
        load: bool,
    ) -> Result<AgentCapabilitySnapshotDto> {
        // Read after the gate: a release may just have forgotten the session.
        let _harness = if load {
            Some(self.hold_harness(id).await)
        } else {
            None
        };
        let thread = self.get_thread(id)?;
        let runtime = self.runtime(thread.provider)?;
        let Some(session) = thread.provider_session_id.as_deref() else {
            return runtime.capabilities(thread.agent_id.as_deref()).await;
        };
        if !load {
            return runtime
                .session_capabilities(thread.agent_id.as_deref(), session)
                .await;
        }
        let cwd = self.session_cwd(&thread);
        self.with_load(
            &thread.id,
            HarnessLoadReason::View,
            runtime.resume_session(session, cwd.as_deref(), thread_session_settings(&thread)),
        )
        .await?;
        runtime
            .session_capabilities(thread.agent_id.as_deref(), session)
            .await
    }

    pub async fn harness_catalog(
        &self,
        provider: Provider,
        agent_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<serde_json::Value> {
        self.runtime(provider)?.harness_catalog(agent_id, cwd).await
    }

    /// Typed harness-panel action for the thread's live session.
    pub async fn thread_harness_action(
        &self,
        id: &str,
        action: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let _harness = self.hold_harness(id).await;
        let thread = self.get_thread(id)?;
        let runtime = self.runtime(thread.provider)?;
        let session = self.ensure_thread_session(&thread).await?;
        let session = session.as_str();
        let cwd = self.session_cwd(&thread);
        self.with_load(
            &thread.id,
            HarnessLoadReason::View,
            runtime.resume_session(session, cwd.as_deref(), thread_session_settings(&thread)),
        )
        .await?;
        runtime.harness_action(session, action).await
    }

    pub async fn install(
        &self,
        provider: Provider,
        agent_id: Option<&str>,
    ) -> Result<AgentBackendDto> {
        self.runtime(provider)?.install(agent_id).await
    }
}

fn normalize_imported_turns(turns: &mut [ThreadTurnDto]) {
    for turn in turns {
        let mut seen_messages = HashSet::new();
        turn.items.retain_mut(|item| {
            if item.kind == "userMessage" {
                let Some(text) = crate::local_sessions::sanitize_codex_user_text(&item.text) else {
                    return false;
                };
                item.text = text;
            }
            if let Some(key) = crate::local_sessions::message_key(item) {
                return seen_messages.insert(key);
            }
            true
        });
        for (sequence, item) in turn.items.iter_mut().enumerate() {
            item.sequence = Some(sequence as i64);
        }
    }
}

fn is_workspace_name(value: &str) -> bool {
    !Path::new(value).is_absolute()
        && value != "."
        && value != ".."
        && value.chars().enumerate().all(|(index, ch)| {
            if index == 0 {
                ch.is_ascii_alphanumeric()
            } else {
                ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
            }
        })
        && (1..=128).contains(&value.len())
}

fn infer_git_repo_name(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let name = trimmed
        .rsplit(['/', ':'])
        .find(|part| !part.is_empty())
        .unwrap_or("repo");
    if name.is_empty() {
        "repo".into()
    } else {
        name.into()
    }
}

fn sanitize_file_name(name: &str) -> String {
    let basename = std::path::Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "attachment".into());
    let extension = Path::new(&basename)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| {
            value
                .chars()
                .filter(|ch| ch.is_ascii_alphanumeric())
                .take(15)
                .collect::<String>()
        })
        .filter(|value| !value.is_empty());
    let stem = Path::new(&basename)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment");
    let cleaned: String = stem
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let stem = cleaned.trim_matches('-');
    let stem = if stem.is_empty() { "attachment" } else { stem };
    let suffix = &Uuid::new_v4().to_string()[..8];
    match extension {
        Some(extension) => format!("{stem}-{suffix}.{extension}"),
        None => format!("{stem}-{suffix}"),
    }
}

/// How deep an agent-spawned chain may go. A root is 0, so this permits a thread
/// to delegate, and its delegate to delegate once more. Bounding it keeps a
/// misbehaving agent from opening an unbounded chain of live harness sessions.
pub const MAX_LINEAGE_DEPTH: i64 = 3;

/// Unfinished agent threads permitted beneath one lineage root. Matches the
/// concurrent-subagent limit Claude Code documents, and like that limit it is a hard
/// refusal rather than a queue: backpressure belongs with the agent deciding to
/// delegate, not in a scheduler holding half-started harness sessions open.
pub const MAX_OPEN_AGENT_THREADS: i64 = 20;

pub(crate) struct Lineage {
    pub parent: Option<String>,
    pub root: Option<String>,
    pub depth: i64,
}

impl Supervisor {
    /// Resolves a new thread's place in the tree from its parent. The root is copied
    /// from the parent (or *is* the parent, when the parent is itself a root), so the
    /// write costs one lookup and grouping never needs a recursive walk.
    pub(crate) fn resolve_lineage(&self, parent: Option<&str>) -> Result<Lineage> {
        let Some(parent) = parent.filter(|value| !value.trim().is_empty()) else {
            return Ok(Lineage {
                parent: None,
                root: None,
                depth: 0,
            });
        };
        let (root, depth): (Option<String>, i64) = self.db.with(|conn| {
            conn.query_row(
                "SELECT root_thread_id, lineage_depth FROM threads WHERE id=?1",
                params![parent],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| anyhow!("parent thread not found"))
        })?;
        let depth = depth + 1;
        anyhow::ensure!(
            depth <= MAX_LINEAGE_DEPTH,
            "agent thread nesting is limited to {MAX_LINEAGE_DEPTH} levels; \
             delegate from the root thread instead of chaining deeper"
        );
        // A root stores NULL for its own root, so descendants of a root anchor on
        // the parent id itself.
        let root = root.unwrap_or_else(|| parent.to_string());
        let open = self.open_agent_threads(&root)?;
        anyhow::ensure!(
            open < MAX_OPEN_AGENT_THREADS,
            "this thread already has {open} unfinished agent threads, which is the \
             limit of {MAX_OPEN_AGENT_THREADS}. Do not retry: creating another will \
             fail the same way. Close delegates whose work you have collected with \
             `pockymoe thread close NAME`, wait for running ones, or reuse an idle \
             peer instead of creating more."
        );
        Ok(Lineage {
            root: Some(root),
            parent: Some(parent.to_string()),
            depth,
        })
    }

    /// A one-line notice when peer mail is waiting, or None. Returns None unless the
    /// CLI is configured, because without credentials the agent cannot act on it and
    /// the line would be noise. Deliberately states that the mail is passive: an agent
    /// nudged every turn about work it is reasonably deferring might otherwise
    /// acknowledge messages just to silence the reminder, which loses them.
    pub fn pending_mail_notice(&self, thread: &str) -> Option<String> {
        if self.interaction.context.read().ok()?.is_none() {
            return None;
        }
        let count = self.inbox_unread_count(thread).ok()?;
        if count == 0 {
            return None;
        }
        // Name what is waiting. A bare count cannot tell an agent whether looking now
        // is worth interrupting itself; "1 question: missing API key" can.
        let digest = self.inbox_unread_digest(thread, 5).unwrap_or_default();
        let listed = digest
            .iter()
            .map(|(kind, subject)| format!("  - {kind}: {subject}"))
            .collect::<Vec<_>>()
            .join("\n");
        let more = count.saturating_sub(digest.len() as i64);
        let tail = if more > 0 {
            format!("\n  - ...and {more} more")
        } else {
            String::new()
        };
        let waiting_question = digest.iter().any(|(kind, _)| kind == "question");
        let urgency = if waiting_question {
            " One is a question, so a peer is waiting on you."
        } else {
            ""
        };
        Some(format!(
            "[remoteCodex: {count} unread peer message{}.{urgency} Passive - handle at a natural \
             checkpoint with `pockymoe inbox`, and acknowledge only what you have actually \
             dealt with.\n{listed}{tail}]",
            if count == 1 { "" } else { "s" }
        ))
    }

    /// Descendants of `root` that still hold a slot. Counting *unfinished* rather than
    /// only *running* threads is deliberate: a freshly created thread sits at `idle`
    /// until it is prompted, so a running-only cap would not stop an agent opening
    /// hundreds of threads in a burst - the exact runaway this bound exists to prevent.
    /// A finished turn returns a thread to `idle`, which still counts: the delegate
    /// stays addressable until someone closes it, and closing is what frees the slot.
    pub(crate) fn open_agent_threads(&self, root: &str) -> Result<i64> {
        self.db.with(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM threads
                 WHERE root_thread_id=?1 AND closed_at IS NULL
                   AND COALESCE(status,'idle') NOT IN ('completed','failed','interrupted')",
                params![root],
                |row| row.get(0),
            )?)
        })
    }
}

fn thread_from_row(row: &rusqlite::Row<'_>) -> ThreadDto {
    let provider: String = row.get(2).unwrap_or_else(|_| "codex".into());
    let provider = match provider.as_str() {
        "claude" => Provider::Claude,
        "opencode" => Provider::Opencode,
        "acp" => Provider::Acp,
        _ => Provider::Codex,
    };
    ThreadDto {
        id: row.get(0).unwrap_or_default(),
        workspace_id: row.get(1).unwrap_or_default(),
        provider,
        agent_id: row.get(3).unwrap_or(None),
        provider_session_id: row.get(4).unwrap_or(None),
        source: row.get(5).unwrap_or_else(|_| "supervisor".into()),
        title: row.get(6).unwrap_or_default(),
        model: row.get(7).unwrap_or(None),
        reasoning_effort: row.get(8).unwrap_or(None),
        fast_mode: row.get::<_, i64>(9).unwrap_or(0) != 0,
        collaboration_mode: row.get(10).unwrap_or_else(|_| "default".into()),
        approval_mode: row.get(11).unwrap_or_else(|_| "guarded".into()),
        sandbox_mode: row.get(12).unwrap_or(None),
        status: row
            .get::<_, Option<String>>(13)
            .ok()
            .flatten()
            .unwrap_or_else(|| "idle".into())
            .into(),
        summary_text: row.get(14).unwrap_or(None),
        last_error: row.get(15).unwrap_or(None),
        active_turn_id: None,
        is_loaded: true,
        is_pinned: row.get::<_, i64>(20).unwrap_or(0) != 0,
        created_at: row.get(16).unwrap_or_default(),
        updated_at: row.get(17).unwrap_or_default(),
        last_turn_started_at: row.get(18).unwrap_or(None),
        last_turn_completed_at: row.get(19).unwrap_or(None),
        context_usage: row
            .get::<_, Option<String>>(21)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok()),
        parent_thread_id: row.get(22).unwrap_or(None),
        root_thread_id: row.get(23).unwrap_or(None),
        lineage_depth: row.get(24).unwrap_or(0),
        descendant_count: row.get::<_, i64>(25).ok().filter(|count| *count > 0),
        agent_name: row.get(26).unwrap_or(None),
        agent_role: row.get(27).unwrap_or(None),
        worktree_path: row.get(28).unwrap_or(None),
        closed_at: row.get(29).unwrap_or(None),
    }
}

// Native Codex stores image blocks separately while Pockymoe keeps PHOTO
// placeholders in display text. Compare their textual prompt within the same
// turn time window, still requiring exactly one match.
fn usage_prompt_key(text: &str) -> String {
    let mut remaining = text;
    let mut result = String::new();
    while let Some(start) = remaining.find("[PHOTO ") {
        result.push_str(&remaining[..start]);
        let Some(end) = remaining[start..].find(']') else {
            result.push_str(&remaining[start..]);
            remaining = "";
            break;
        };
        remaining = &remaining[start + end + 1..];
    }
    result.push_str(remaining);
    result.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn usage_needs_hydration(turn: &ThreadTurnDto) -> bool {
    match &turn.token_usage {
        None => true,
        Some(usage) => {
            usage["total"]["totalTokens"].as_u64().unwrap_or_default()
                < usage["last"]["totalTokens"].as_u64().unwrap_or_default()
        }
    }
}

pub fn bootstrap_runtimes(config: &RuntimeConfig) -> Vec<SharedRuntime> {
    use crate::acp::AcpRuntime;
    use crate::fake::FakeRuntime;
    let mut out: Vec<SharedRuntime> = Vec::new();
    if config.fake_runtime {
        for provider in [
            Provider::Codex,
            Provider::Claude,
            Provider::Opencode,
            Provider::Acp,
        ] {
            if config.enabled_providers.contains(&provider) {
                out.push(Arc::new(FakeRuntime::new(provider)));
            }
        }
        return out;
    }
    for provider in &config.enabled_providers {
        match provider {
            Provider::Acp => out.push(Arc::new(
                AcpRuntime::catalog(config.acp_command.clone(), config.acp_startup_timeout_ms)
                    .with_upstreams(crate::upstreams::directory(&config.database_url)),
            )),
            Provider::Codex => out.push(Arc::new(
                AcpRuntime::bound(Provider::Codex, "codex", config.acp_startup_timeout_ms)
                    .with_upstreams(crate::upstreams::directory(&config.database_url)),
            )),
            Provider::Claude => out.push(Arc::new(
                AcpRuntime::bound(Provider::Claude, "claude", config.acp_startup_timeout_ms)
                    .with_upstreams(crate::upstreams::directory(&config.database_url)),
            )),
            Provider::Opencode => out.push(Arc::new(
                AcpRuntime::bound(
                    Provider::Opencode,
                    "opencode",
                    config.acp_startup_timeout_ms,
                )
                .with_upstreams(crate::upstreams::directory(&config.database_url)),
            )),
        }
    }
    out
}
