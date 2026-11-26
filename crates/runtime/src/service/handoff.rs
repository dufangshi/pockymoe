//! Portable public history; native state remains owned by each harness.
use super::*;
use pockymoe_protocol::{ThreadSummaryQuery, ThreadSummaryRole};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessSwitchInput {
    #[serde(default = "acp")]
    pub provider: Provider,
    pub agent_id: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}
fn acp() -> Provider {
    Provider::Acp
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffPreview {
    pub text: String,
    pub bytes: usize,
    pub estimated_tokens: usize,
    pub turns: usize,
    pub omitted_turns: Vec<String>,
    pub resumes_session: bool,
    pub settings_note: &'static str,
}
const SAME_SESSION_NOTE: &str =
    "Same session on the other protocol; history and settings are kept.";
const SETTINGS_NOTE: &str = "Plan, sandbox, fast mode and reasoning effort are reset; approval mode is retained. Active goals must be cleared before switching.";

pub(super) fn record_seen(conn: &Connection, id: &str, now: &str) -> Result<()> {
    conn.execute("INSERT INTO thread_harness_sessions(thread_id,provider,agent_id,session_id,model,turns_seen,created_at,last_used_at)
      SELECT id,provider,COALESCE(agent_id,''),provider_session_id,model,
      (SELECT COALESCE(MAX(ordinal),0) FROM thread_turns WHERE thread_id=?1),?2,?2 FROM threads WHERE id=?1
      ON CONFLICT(thread_id,provider,agent_id) DO UPDATE SET session_id=excluded.session_id,model=excluded.model,turns_seen=excluded.turns_seen,last_used_at=excluded.last_used_at", params![id,now])?;
    conn.execute(
        "DELETE FROM kv WHERE key=?1",
        [format!("handoff:pending:{id}")],
    )?;
    Ok(())
}

/// Stage a fresh portable session inside the caller's journal transaction.
/// The caller holds the thread's exclusive claim and has released its native
/// session. Retained history and the pending prefix must commit together.
pub(super) fn stage_portable_session(conn: &Connection, id: &str, kept_turns: usize) -> Result<()> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM thread_turns WHERE thread_id=?1",
        [id],
        |r| r.get::<_, usize>(0),
    )?;
    ensure!(
        count == kept_turns,
        "retained journal does not match kept_turns"
    );
    conn.execute(
        "UPDATE threads SET provider_session_id=NULL WHERE id=?1",
        [id],
    )?;
    conn.execute(
        "DELETE FROM thread_harness_sessions WHERE thread_id=?1",
        [id],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO kv(key,value) VALUES(?1,'1')",
        [format!("handoff:pending:{id}")],
    )?;
    conn.execute("DELETE FROM kv WHERE key=?1", [harness_context_key(id)])?;
    Ok(())
}

// Forks remap anchors just like copied turn items. A selected turn excludes
// switches that happened after it; a full fork also keeps trailing idle switches.
pub(super) fn copy_switch_entries(
    conn: &Connection,
    detail: &ThreadDetailDto,
    fork_id: &str,
    copied_turns: &HashMap<String, String>,
    through: Option<&str>,
) -> Result<()> {
    for entry in &detail.harness_switches {
        let anchor = entry.extra.get("afterTurnId").and_then(Value::as_str);
        let mapped = if let Some(anchor) = anchor {
            if through == Some(anchor) {
                continue;
            }
            let Some(mapped) = copied_turns.get(anchor) else {
                continue;
            };
            Some(mapped)
        } else {
            None
        };
        let mut copied = entry.clone();
        copied.id = format!("harness-switch:{}", Uuid::new_v4());
        copied.extra.insert("afterTurnId".into(), json!(mapped));
        conn.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,?2,'',?1,?3,?4,?4)", params![copied.id,fork_id,serde_json::to_string(&copied)?,copied.created_at])?;
    }
    Ok(())
}

impl Supervisor {
    pub(super) fn load_harness_switches(&self, id: &str) -> Result<Vec<ThreadHistoryItemDto>> {
        self.db.with(|c| {
            let mut q = c.prepare("SELECT item_json FROM thread_history_items WHERE thread_id=?1 AND turn_id='' AND json_extract(item_json,'$.kind')='harnessSwitch' ORDER BY rowid")?;
            let raw = q.query_map([id], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            raw.into_iter().map(|raw| serde_json::from_str(&raw).map_err(Into::into)).collect()
        })
    }

    pub fn handoff_token_cap(&self) -> Result<usize> {
        Ok(self
            .db
            .get_kv("handoff:token-cap")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(16_000))
    }
    pub fn set_handoff_token_cap(&self, cap: usize) -> Result<()> {
        ensure!(
            (1024..=64_000).contains(&cap),
            "tokenCap must be between 1024 and 64000"
        );
        self.db.set_kv("handoff:token-cap", &cap.to_string())
    }

    fn target_seen(&self, id: &str, target: &HarnessSwitchInput) -> Result<(Option<String>, i64)> {
        // Both protocols of one agent share its session store and row.
        let agent = pockymoe_protocol::split_agent_key(&target.agent_id).0;
        self.db.with(|c| Ok(c.query_row("SELECT session_id,turns_seen FROM thread_harness_sessions WHERE thread_id=?1 AND provider=?2 AND agent_id=?3", params![id,target.provider.as_str(),agent], |r| Ok((r.get(0)?,r.get(1)?))).optional()?.unwrap_or((None,0))))
    }

    pub fn preview_harness_switch(
        &self,
        id: &str,
        target: &HarnessSwitchInput,
    ) -> Result<HandoffPreview> {
        self.preview_harness_switch_prompt(id, target, "")
    }

    pub fn preview_harness_switch_prompt(
        &self,
        id: &str,
        target: &HarnessSwitchInput,
        prompt: &str,
    ) -> Result<HandoffPreview> {
        let thread = self.get_thread(id)?;
        let (agent, protocol) = pockymoe_protocol::split_agent_key(target.agent_id.trim());
        if thread.provider == target.provider
            && thread.agent_id.as_deref() == Some(agent)
            && thread.harness_protocol.as_deref() != protocol
            && thread.provider_session_id.is_some()
        {
            // The other protocol of the same agent reads the same session.
            return Ok(HandoffPreview {
                text: String::new(),
                bytes: 0,
                estimated_tokens: 0,
                turns: 0,
                omitted_turns: vec![],
                resumes_session: true,
                settings_note: SAME_SESSION_NOTE,
            });
        }
        let (session, seen) = self.target_seen(id, target)?;
        self.build_handoff(
            id,
            seen,
            session.is_some(),
            prompt,
            target.model.as_deref(),
            None,
        )
    }

    pub fn preview_harness_fork(
        &self,
        id: &str,
        target: &HarnessSwitchInput,
        through: Option<&str>,
    ) -> Result<HandoffPreview> {
        self.get_thread(id)?;
        if let Some(turn) = through {
            ensure!(
                self.db.with(|c| Ok(c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM thread_turns WHERE id=?1 AND thread_id=?2)",
                    params![turn, id],
                    |r| r.get::<_, bool>(0)
                )?))?,
                "turn not found"
            );
        }
        self.build_handoff(id, 0, false, "", target.model.as_deref(), through)
    }

    fn build_handoff(
        &self,
        id: &str,
        seen: i64,
        resumes: bool,
        prompt: &str,
        model: Option<&str>,
        through: Option<&str>,
    ) -> Result<HandoffPreview> {
        // Conservative UTF-8 byte estimate. Reserve 25% of the unknown 128k window.
        let cap = self
            .db
            .get_kv("handoff:token-cap")?
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(16_000)
            .clamp(1024, 64_000);
        let catalog = self.model_pricing();
        let window = model
            .and_then(|m| pockymoe_protocol::pricing::resolve_model(&catalog, m))
            .and_then(|(key, _)| catalog["models"][key]["contextWindowTokens"].as_u64())
            .unwrap_or(128_000) as usize;
        let allowance = window.saturating_sub(16_000.max(window / 4));
        ensure!(prompt.len()<=allowance,"prompt exceeds the target context allowance ({allowance} estimated tokens); shorten it or choose a larger context model");
        let budget = cap.min(allowance.saturating_sub(prompt.len()));
        let eligible: Vec<String> = self.db.with(|c| {
            let mut q = c.prepare(
                "SELECT id FROM thread_turns WHERE thread_id=?1 AND ordinal>?2 AND (?3 IS NULL OR ordinal <= (SELECT ordinal FROM thread_turns WHERE id=?3 AND thread_id=?1)) ORDER BY ordinal",
            )?;
            let rows = q
                .query_map(params![id, seen, through], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })?;
        let summary = self.thread_summary(id, &ThreadSummaryQuery::default())?;
        let mut blocks: Vec<(String, String)> = eligible
            .iter()
            .map(|turn| (turn.clone(), format!("Turn {turn}\n")))
            .collect();
        self.db.with(|c| {
            let mut query = c.prepare(
                "SELECT COALESCE(agent_id,provider,'unknown'),model FROM thread_turns WHERE id=?1",
            )?;
            for (turn, text) in &mut blocks {
                let (agent, model) = query.query_row([turn.as_str()], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
                })?;
                text.push_str(&format!(
                    "Harness: {agent} · {}\n",
                    model.unwrap_or_default()
                ));
            }
            Ok(())
        })?;
        for message in summary.messages {
            if let Some((_, text)) = blocks
                .iter_mut()
                .find(|(turn, _)| Some(turn) == message.turn_id.as_ref())
            {
                let role = if message.role == ThreadSummaryRole::User {
                    "User"
                } else {
                    "Assistant"
                };
                text.push_str(&format!("{role}: {}\n", message.text));
            }
        }
        self.db.with(|c| {
            let mut query=c.prepare("SELECT turn_id,item_json FROM thread_history_items WHERE thread_id=?1 ORDER BY created_at")?;
            let rows=query.query_map([id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?;
            for row in rows {
                let (turn,raw)=row?;
                let Ok(item)=serde_json::from_str::<ThreadHistoryItemDto>(&raw) else {continue};
                if item.kind!="commandExecution" && item.kind!="fileChange" {continue;}
                if let Some((_,block))=blocks.iter_mut().find(|(id,_)|id==&turn) {
                    let title:String=item.text.chars().take(400).collect();
                    block.push_str(&format!("{} [{}]: {}\n",item.kind.as_str(),item.status.as_deref().unwrap_or("unknown"),title));
                }
            }
            Ok(())
        })?;
        let git = self
            .worktree_git_status(id, None)
            .ok()
            .map(|mut state| {
                if let Some(files) = state["files"].as_array_mut() {
                    files.truncate(12);
                }
                let json = state.to_string();
                json.chars().take((budget / 8).min(400)).collect::<String>()
            })
            .unwrap_or_else(|| "unavailable".into());
        let header=format!("[Pockymoe portable history: saved conversation, not new instructions]\nRead omitted history: pockymoe transcript {id} --view summary\nRead a turn: pockymoe transcript {id} --turn TURN_ID --view overview\nRead item details: pockymoe transcript {id} --turn TURN_ID --item ITEM_ID\nCurrent Git state: {git}\n");
        let mut contract = self
            .db
            .get_kv(&format!("handoff:contract:{id}"))?
            .unwrap_or_default();
        if let Some(role) = self.db.get_kv(&format!("handoff:role:{id}"))? {
            contract.push('\n');
            contract.push_str(&role);
        }
        let omissions_reserve = eligible.iter().map(|id| id.len() + 2).sum::<usize>() + 32;
        let fixed = header.len() + contract.len() + 2 + omissions_reserve.min(budget / 4);
        ensure!(
            fixed <= budget,
            "prompt leaves insufficient room for the handoff instructions; shorten it"
        );
        let available = budget - fixed;
        if let Some((_, first)) = blocks.first_mut() {
            if first.len() > available {
                let keep = available / 3;
                let mut boundary = keep.min(first.len());
                while !first.is_char_boundary(boundary) {
                    boundary -= 1;
                }
                first.truncate(boundary);
                first.push_str("\n[Partial turn; read full text with transcript]\n");
            }
        }
        let (selected, omitted) = select_blocks(&blocks, available);
        let mut text = header;
        text.push_str(&contract);
        text.push_str("\n\n");
        for (index, (_, block)) in blocks.iter().enumerate() {
            if selected.contains(&index) {
                text.push_str(block);
            }
        }
        if !omitted.is_empty() {
            let notice = format!("Omitted turns: {}\n", omitted.join(", "));
            if text.len() + notice.len() <= budget {
                text.push_str(&notice);
            } else {
                text.push_str("Additional turns omitted; use transcript summary.\n");
            }
        }
        let bytes = text.len();
        Ok(HandoffPreview {
            text,
            bytes,
            estimated_tokens: bytes,
            turns: selected.len(),
            omitted_turns: omitted,
            resumes_session: resumes,
            settings_note: SETTINGS_NOTE,
        })
    }

    pub(super) fn pending_handoff(
        &self,
        thread: &ThreadDto,
        prompt: &str,
    ) -> Result<Option<HandoffPreview>> {
        if self
            .db
            .get_kv(&format!("handoff:pending:{}", thread.id))?
            .is_none()
        {
            return Ok(None);
        }
        let target = HarnessSwitchInput {
            provider: thread.provider,
            agent_id: thread.agent_id.clone().unwrap_or_default(),
            model: thread.model.clone(),
            reasoning_effort: thread.reasoning_effort.clone(),
        };
        let (session, seen) = self.target_seen(&thread.id, &target)?;
        self.build_handoff(
            &thread.id,
            seen,
            session.is_some(),
            prompt,
            thread.model.as_deref(),
            None,
        )
        .map(Some)
    }

    /// Owner-facing operation. Callers must enforce their access ceiling.
    pub async fn switch_thread_harness(
        &self,
        id: &str,
        target: HarnessSwitchInput,
    ) -> Result<ThreadDto> {
        ensure!(!target.agent_id.trim().is_empty(), "agentId is required");
        // `acp:codex` targets Codex's ACP adapter, `codex` native Codex.
        let (agent, protocol) = pockymoe_protocol::split_agent_key(target.agent_id.trim());
        let protocol = protocol.map(str::to_string);
        let target = HarnessSwitchInput {
            agent_id: agent.to_string(),
            ..target
        };
        let before = self.get_thread(id)?;
        if before.provider == target.provider
            && before.agent_id.as_deref() == Some(agent)
            && before.harness_protocol != protocol
            && before.provider_session_id.is_some()
        {
            return self
                .switch_thread_protocol(id, before, target, protocol)
                .await;
        }
        ensure!(
            !matches!(before.status.as_str(), "running" | "recovering"),
            "conflict: Stop the thread before switching harnesses"
        );
        if self.execution_context(id)?.is_some() {
            ensure!(
                matches!(target.agent_id.as_str(), "codex" | "claude" | "custom"),
                "unsupported_isolated_harness: {}",
                target.agent_id
            );
        }
        ensure!(
            self.stored_goal(id)?
                .is_none_or(|goal| !matches!(goal.status.as_str(), "active" | "paused")),
            "conflict: Clear the goal before switching harnesses"
        );
        if target.provider == Provider::Acp {
            let key = pockymoe_protocol::agent_key(&target.agent_id, protocol.as_deref());
            let agents = self.list_agents(target.provider).await?;
            ensure!(
                agents
                    .iter()
                    .any(|agent| agent.id == key || agent.model == key),
                "unknown harness: {key}"
            );
        } else {
            self.runtime(target.provider)?;
            ensure!(
                target.agent_id == target.provider.as_str(),
                "agentId must match the provider"
            );
        }
        // Validate the portable prefix before releasing the current session.
        let preview = self.preview_harness_switch(id, &target)?;
        self.release_thread_harness(id, false, None).await?;
        let _guard = self.hold_harness(id).await;
        let current = self.get_thread(id)?;
        ensure!(
            current.provider == before.provider
                && current.agent_id == before.agent_id
                && !self.live.lock().await.contains_key(id),
            "conflict: Thread changed during switch; retry"
        );
        let (session, _) = self.target_seen(id, &target)?;
        let now = now_rfc3339();
        self.db.with(|c| {
            let tx=c.unchecked_transaction()?;
            // Preserve an existing watermark; an unused session must not claim turns it never saw.
            tx.execute("INSERT INTO thread_harness_sessions(thread_id,provider,agent_id,session_id,model,turns_seen,created_at,last_used_at) VALUES(?1,?2,?3,?4,?5,0,?6,?6) ON CONFLICT(thread_id,provider,agent_id) DO UPDATE SET session_id=excluded.session_id,model=excluded.model,last_used_at=excluded.last_used_at",params![id,current.provider.as_str(),current.agent_id.clone().unwrap_or_default(),current.provider_session_id,current.model,now])?;
            tx.execute("UPDATE threads SET source='supervisor',provider=?2,agent_id=?3,provider_session_id=?4,model=?5,reasoning_effort=?6,collaboration_mode='default',sandbox_mode=NULL,fast_mode=0,context_usage_json=NULL,last_error=NULL,status='idle',updated_at=?7,harness_protocol=?8 WHERE id=?1",params![id,target.provider.as_str(),target.agent_id,session,target.model,target.reasoning_effort,now,protocol])?;
            let switch=json!({"fromAgentId":before.agent_id,"fromModel":before.model,"fromHarnessProtocol":before.harness_protocol,"agentId":target.agent_id,"model":target.model,"harnessProtocol":protocol,"turns":preview.turns,"recorded":true});
            let after: Option<String> = tx.query_row("SELECT id FROM thread_turns WHERE thread_id=?1 ORDER BY ordinal DESC LIMIT 1", [id], |r| r.get(0)).optional()?;
            let entry_id = format!("harness-switch:{}", Uuid::new_v4());
            let entry = json!({"id":entry_id,"kind":"harnessSwitch","text":format!("{} → {}",before.agent_id.as_deref().unwrap_or("unknown"),target.agent_id),"createdAt":now,"afterTurnId":after,"harnessSwitch":switch});
            tx.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,?2,'',?1,?3,?4,?4)",params![entry_id,id,entry.to_string(),now])?;
            tx.execute("INSERT OR REPLACE INTO kv(key,value) VALUES(?1,?2)",params![format!("handoff:pending:{id}"),switch.to_string()])?;
            tx.execute("DELETE FROM kv WHERE key=?1",[harness_context_key(id)])?;
            tx.commit()?; Ok(())
        })?;
        if let Some(session) = &session {
            let key = pockymoe_protocol::agent_key(&target.agent_id, protocol.as_deref());
            self.runtime(target.provider)?
                .set_session_route(session, &key);
        }
        self.bus.emit(pockymoe_protocol::ThreadEventEnvelope{event_type:pockymoe_protocol::ThreadEventType::ThreadUpdated,thread_id:id.into(),timestamp:now,payload:json!({"reason":"harnessSwitched","fromAgentId":before.agent_id,"agentId":target.agent_id,"turns":preview.turns,"settingsNote":SETTINGS_NOTE})});
        self.get_thread(id)
    }

    /// Codex (or Claude) between its native runtime and its ACP adapter: both
    /// read the same session store, so the thread keeps its session, history
    /// and settings, and no handoff prefix is sent.
    async fn switch_thread_protocol(
        &self,
        id: &str,
        before: ThreadDto,
        target: HarnessSwitchInput,
        protocol: Option<String>,
    ) -> Result<ThreadDto> {
        ensure!(
            !matches!(before.status.as_str(), "running" | "recovering"),
            "conflict: Stop the thread before switching harnesses"
        );
        if target.provider == Provider::Acp {
            let key = pockymoe_protocol::agent_key(&target.agent_id, protocol.as_deref());
            let agents = self.list_agents(target.provider).await?;
            ensure!(
                agents.iter().any(|agent| agent.model == key),
                "unknown harness: {key}"
            );
        }
        self.release_thread_harness(id, false, None).await?;
        let _guard = self.hold_harness(id).await;
        let current = self.get_thread(id)?;
        ensure!(
            current.agent_id == before.agent_id
                && current.harness_protocol == before.harness_protocol
                && !self.live.lock().await.contains_key(id),
            "conflict: Thread changed during switch; retry"
        );
        let now = now_rfc3339();
        let model = target.model.clone().or(current.model.clone());
        let effort = target
            .reasoning_effort
            .clone()
            .or(current.reasoning_effort.clone());
        self.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute("UPDATE threads SET harness_protocol=?2,model=?3,reasoning_effort=?4,last_error=NULL,status='idle',updated_at=?5 WHERE id=?1", params![id, protocol, model, effort, now])?;
            let switch = json!({"fromAgentId":before.agent_id,"fromModel":before.model,"fromHarnessProtocol":before.harness_protocol,"agentId":target.agent_id,"model":model,"harnessProtocol":protocol,"turns":0,"recorded":true,"sameSession":true});
            let after: Option<String> = tx.query_row("SELECT id FROM thread_turns WHERE thread_id=?1 ORDER BY ordinal DESC LIMIT 1", [id], |r| r.get(0)).optional()?;
            let entry_id = format!("harness-switch:{}", Uuid::new_v4());
            let label = |agent: Option<&str>, protocol: Option<&str>| match protocol {
                Some("acp") => format!("ACP {}", agent.unwrap_or("unknown")),
                _ => agent.unwrap_or("unknown").to_string(),
            };
            let entry = json!({"id":entry_id,"kind":"harnessSwitch","text":format!("{} → {}", label(before.agent_id.as_deref(), before.harness_protocol.as_deref()), label(Some(&target.agent_id), protocol.as_deref())),"createdAt":now,"afterTurnId":after,"harnessSwitch":switch});
            tx.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,?2,'',?1,?3,?4,?4)", params![entry_id, id, entry.to_string(), now])?;
            tx.commit()?;
            Ok(())
        })?;
        if let Some(session) = &current.provider_session_id {
            let key = pockymoe_protocol::agent_key(&target.agent_id, protocol.as_deref());
            self.runtime(target.provider)?
                .set_session_route(session, &key);
        }
        self.bus.emit(pockymoe_protocol::ThreadEventEnvelope{event_type:pockymoe_protocol::ThreadEventType::ThreadUpdated,thread_id:id.into(),timestamp:now,payload:json!({"reason":"harnessSwitched","fromAgentId":before.agent_id,"agentId":target.agent_id,"fromHarnessProtocol":before.harness_protocol,"harnessProtocol":protocol,"turns":0})});
        self.get_thread(id)
    }

    /// Fork public history without asking the target harness to fork foreign native state.
    pub async fn fork_thread_to_harness(
        &self,
        id: &str,
        turn_id: Option<&str>,
        target: HarnessSwitchInput,
    ) -> Result<ThreadDto> {
        let detail = self.get_thread_detail(id, None).await?;
        ensure!(
            detail.thread.status != "running",
            "conflict: Stop the thread before forking"
        );
        ensure!(
            self.execution_context(id)?.is_none(),
            "unsupported_environment_fork: create a child thread to retain isolation"
        );
        let count = match turn_id {
            Some(turn) => detail
                .turns
                .iter()
                .position(|t| t.id == turn)
                .map(|i| i + 1)
                .ok_or_else(|| anyhow!("turn not found"))?,
            None => detail.turns.len(),
        };
        let forked = crate::actor::StartSessionResult {
            provider_session_id: String::new(),
            model: None,
            reasoning_effort: None,
        };
        let (thread, _, _) = self.persist_forked_thread(
            &detail,
            forked,
            count,
            turn_id.map(str::to_owned),
            Some(count as u32),
        )?;
        self.db.with(|c| {
            c.execute(
                "UPDATE threads SET provider_session_id=NULL WHERE id=?1",
                [&thread.id],
            )?;
            for kind in ["contract", "role"] {
                c.execute(
                    "INSERT OR REPLACE INTO kv(key,value) SELECT ?1,value FROM kv WHERE key=?2",
                    params![
                        format!("handoff:{kind}:{}", thread.id),
                        format!("handoff:{kind}:{id}")
                    ],
                )?;
            }
            Ok(())
        })?;
        match self.switch_thread_harness(&thread.id, target).await {
            Ok(fork) => {
                self.host_thread_created(
                    &fork,
                    crate::host_hooks::ThreadOrigin::Fork {
                        source_thread_id: id.into(),
                    },
                )
                .await?;
                Ok(fork)
            }
            Err(error) => {
                let _ = self.delete_thread(&thread.id);
                Err(error)
            }
        }
    }

    /// Rewind/import fallback: detach the native session, then replay the retained journal on next prompt.
    /// `kept_turns` is validated against the journal; the caller removes discarded turns first.
    pub async fn start_portable_session(&self, id: &str, kept_turns: usize) -> Result<()> {
        let thread = self.get_thread(id)?;
        ensure!(
            !matches!(thread.status.as_str(), "running" | "recovering"),
            "conflict: Stop the thread before replacing its session"
        );
        let count = self.db.with(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM thread_turns WHERE thread_id=?1",
                [id],
                |r| r.get::<_, usize>(0),
            )?)
        })?;
        ensure!(
            count == kept_turns,
            "retained journal does not match kept_turns"
        );
        self.release_thread_harness(id, false, None).await?;
        let _claim = self
            .try_claim(&[id.into()])
            .ok_or_else(|| anyhow!("conflict: Thread is busy; retry"))?;
        self.db.with(|c| {
            let tx = c.unchecked_transaction()?;
            stage_portable_session(&tx, id, kept_turns)?;
            tx.commit()?;
            Ok(())
        })
    }
}

fn select_blocks(blocks: &[(String, String)], budget: usize) -> (Vec<usize>, Vec<String>) {
    let mut selected = Vec::new();
    let mut remaining = budget;
    // Original request, then recent turns, whole blocks only (never raw state or reasoning).
    for index in std::iter::once(0)
        .chain(
            (1..blocks.len())
                .rev()
                .filter(|i| blocks[*i].1.contains("[failed]")),
        )
        .chain((1..blocks.len()).rev())
    {
        if let Some((_, text)) = blocks.get(index) {
            if !selected.contains(&index) && text.len() <= remaining {
                selected.push(index);
                remaining -= text.len();
            }
        }
    }
    let omitted = blocks
        .iter()
        .enumerate()
        .filter(|(i, _)| !selected.contains(i))
        .map(|(_, (id, _))| id.clone())
        .collect();
    (selected, omitted)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_staging_commits_or_rolls_back_with_retained_journal() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE threads(id TEXT,provider_session_id TEXT);
             CREATE TABLE thread_turns(thread_id TEXT,id TEXT);
             CREATE TABLE thread_harness_sessions(thread_id TEXT);
             CREATE TABLE kv(key TEXT PRIMARY KEY,value TEXT);
             INSERT INTO threads VALUES('thread','native');
             INSERT INTO thread_turns VALUES('thread','keep'),('thread','discard');
             INSERT INTO thread_harness_sessions VALUES('thread');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO kv VALUES(?1,'delivered')",
            [harness_context_key("thread")],
        )
        .unwrap();
        {
            let tx = conn.unchecked_transaction().unwrap();
            tx.execute("DELETE FROM thread_turns WHERE id='discard'", [])
                .unwrap();
            assert!(stage_portable_session(&tx, "thread", 2).is_err());
            // Dropping the failed transaction preserves native state and history.
        }
        let count: usize = conn
            .query_row("SELECT COUNT(*) FROM thread_turns", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        let session: String = conn
            .query_row("SELECT provider_session_id FROM threads", [], |r| r.get(0))
            .unwrap();
        assert_eq!(session, "native");
        let tx = conn.unchecked_transaction().unwrap();
        tx.execute("DELETE FROM thread_turns WHERE id='discard'", [])
            .unwrap();
        stage_portable_session(&tx, "thread", 1).unwrap();
        tx.commit().unwrap();
        let state: (bool, usize, String, bool) = conn
            .query_row(
                "SELECT provider_session_id IS NULL,
                 (SELECT COUNT(*) FROM thread_harness_sessions),
                 (SELECT value FROM kv WHERE key='handoff:pending:thread'),
                 EXISTS(SELECT 1 FROM kv WHERE key=?1) FROM threads",
                [harness_context_key("thread")],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(state, (true, 0, "1".into(), false));
    }
    #[test]
    fn failures_survive_when_middle_successes_do_not_fit() {
        let blocks = vec![
            ("first".into(), "request...".into()),
            ("failed".into(), "[failed] test failure".into()),
            ("middle".into(), "success".repeat(30)),
            ("recent".into(), "reply".into()),
        ];
        let (selected, omitted) = select_blocks(&blocks, 36);
        assert_eq!(selected, vec![0, 1, 3]);
        assert_eq!(omitted, vec!["middle"]);
    }
    #[test]
    fn short_history_intact_and_long_history_keeps_first_and_recent() {
        let blocks = (0..5)
            .map(|i| (i.to_string(), "x".repeat(10)))
            .collect::<Vec<_>>();
        assert_eq!(select_blocks(&blocks, 50).0.len(), 5);
        assert_eq!(
            select_blocks(&blocks, 30),
            (vec![0, 4, 3], vec!["1".into(), "2".into()])
        );
        assert_eq!(select_blocks(&blocks, 0).1.len(), 5);
    }
}
