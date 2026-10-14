use super::*;
use crate::actor::ExecutionState;
use sha2::{Digest, Sha256};

impl Supervisor {
    /// Acknowledge receipt only after the complete submission has committed. The
    /// inbox also covers preflight failures and a crash before a turn is started.
    pub async fn accept_prompt(&self, id: &str, input: &SendThreadPromptInput) -> Result<()> {
        let thread = self.get_thread(id)?;
        self.ensure_prompt_allowed(&thread)?;
        self.observe_execution(id).await?;
        let raw = serde_json::to_string(input)?;
        let fingerprint = hex::encode(Sha256::digest(raw.as_bytes()));
        let pending_id = Uuid::new_v4().to_string();
        let request_key = input
            .client_request_id
            .as_deref()
            .map(|request| format!("prompt-receipt:{id}:{request}"));
        if input
            .client_request_id
            .as_ref()
            .is_some_and(|id| id.len() > 200)
        {
            bail!("clientRequestId is too long");
        }
        let now = now_rfc3339();
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            if let Some(key) = &request_key {
                if let Some(previous) = tx.query_row("SELECT value FROM kv WHERE key=?1", [key], |r| r.get::<_,String>(0)).optional()? {
                    if previous != fingerprint { bail!("conflict: clientRequestId was already used for another prompt"); }
                    return Ok(());
                }
                tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2)", params![key,fingerprint])?;
            }
            tx.execute(
                "INSERT INTO thread_pending_steers(id,thread_id,turn_id,client_request_id,display_prompt,submitted_prompt,delivery,created_at,updated_at,payload_json)
                 VALUES(?1,?2,?3,?4,?5,?5,'continuation',?6,?6,?7)",
                params![pending_id,id,thread.active_turn_id.as_deref().unwrap_or(""),input.client_request_id,input.prompt,now,raw],
            )?;
            tx.commit()?;
            Ok(())
        })?;
        self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated, thread_id: id.into(), timestamp: now,
            payload: json!({"reason":"pending_steer_updated","pendingSteers":self.load_steers(id)?}),
        });
        Ok(())
    }

    pub fn dispatch_inbox(self: &Arc<Self>, id: String) {
        let state = self.clone();
        tokio::spawn(async move {
            if let Err(error) = state.drain_steers(&id).await {
                // The durable row survives. Do not silently drop it or retry a
                // potentially delivered prompt; expose the failure to the user.
                let message = format!("Message saved, but delivery could not start: {error}");
                let _ = state.db.with(|conn| {
                    conn.execute(
                        "UPDATE threads SET last_error=?1 WHERE id=?2",
                        params![message, id],
                    )?;
                    Ok(())
                });
                state.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
                    event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
                    thread_id: id,
                    timestamp: now_rfc3339(),
                    payload: json!({"lastError":message}),
                });
            }
        });
    }

    pub async fn observe_execution(&self, id: &str) -> Result<()> {
        // Serialize with turn admission and completion. A late observation must
        // never overwrite a newer turn, even when it targets the same session.
        let live = self.live.lock().await;
        let Some(owned) = live.get(id) else {
            return Ok(());
        };
        let thread = self.get_thread(id)?;
        let Some(session) = thread.provider_session_id.as_deref() else {
            return Ok(());
        };
        let observed = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            self.runtime(thread.provider)?.execution_state(session),
        )
        .await
        .unwrap_or(ExecutionState::Unknown);
        let status = match observed {
            ExecutionState::Running { turn_id } if turn_id == owned.turn_id => "running",
            ExecutionState::Unknown => "recovering",
            // Idle while admission/settlement is finishing is not a completion
            // event. Only the prompt response is allowed to finish a turn.
            _ => return Ok(()),
        };
        let turn_status = if status == "running" {
            "inProgress"
        } else {
            "recovering"
        };
        let error = (status == "recovering").then_some(
            "Backend status is unconfirmed. Messages are saved and will wait for reconnection.",
        );
        if thread.status == status {
            return Ok(());
        }
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            tx.execute("UPDATE thread_turns SET status=?1,error=?2,completed_at=NULL WHERE id=?3 AND thread_id=?4", params![turn_status,error,owned.turn_id,id])?;
            tx.execute("UPDATE threads SET status=?1,last_error=?2,updated_at=?3 WHERE id=?4",params![status,error,now_rfc3339(),id])?;
            tx.commit()?;
            Ok(())
        })?;
        self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
            thread_id: id.into(),
            timestamp: now_rfc3339(),
            payload: json!({"status":status,"turnId":owned.turn_id,"lastError":error}),
        });
        Ok(())
    }

    pub fn spawn_inbox_recovery(self: &Arc<Self>) {
        if self
            .update_draining
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let ids = self.db.with(|conn| {
            let mut stmt = conn.prepare("SELECT DISTINCT p.thread_id FROM thread_pending_steers p JOIN threads t ON t.id=p.thread_id WHERE p.delivery='continuation' AND t.status NOT IN ('running','recovering')")?;
            let ids = stmt.query_map([], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(ids)
        });
        if let Ok(ids) = ids {
            for id in ids {
                self.dispatch_inbox(id);
            }
        }
    }

    pub fn spawn_execution_observer(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                tick.tick().await;
                let Some(state) = weak.upgrade() else {
                    break;
                };
                let ids: Vec<_> = state.live.lock().await.keys().cloned().collect();
                for id in ids {
                    if let Err(error) = state.observe_execution(&id).await {
                        tracing::warn!(%id,%error,"backend state reconciliation failed");
                    }
                }
                if let Err(error) = state.refresh_generation_speeds() {
                    tracing::warn!(%error, "generation speed refresh failed");
                }
                state.observe_claude_scheduled_history().await;
            }
        });
    }

    /// Only an explicit reconnect can settle an unconfirmed turn without its
    /// original completion response. Reconnecting does not resend that prompt.
    pub(super) async fn settle_reconnected_execution(&self, id: &str) -> Result<()> {
        let live = self.live.lock().await;
        if live.contains_key(id) {
            return Ok(());
        }
        let thread = self.get_thread(id)?;
        if thread.status != "recovering" {
            return Ok(());
        }
        let Some(session) = thread.provider_session_id.as_deref() else {
            return Ok(());
        };
        if self
            .runtime(thread.provider)?
            .execution_state(session)
            .await
            != ExecutionState::Idle
        {
            return Ok(());
        }
        self.reconcile_stale_turns(Some(id), true)?;
        // The old turn remains interrupted, but the current connection has now
        // been verified. Do not keep asking the user to reconnect successfully.
        self.db.with(|conn| {
            conn.execute("UPDATE threads SET last_error=NULL WHERE id=?1", [id])?;
            Ok(())
        })?;
        self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
            event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
            thread_id: id.into(),
            timestamp: now_rfc3339(),
            payload: json!({"status":"interrupted","lastError":null}),
        });
        Ok(())
    }
}
