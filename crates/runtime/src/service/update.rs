use super::*;
use std::sync::atomic::Ordering;
use tokio::sync::OwnedRwLockWriteGuard;

const RESUME_PROMPT: &str = "The Supervisor was restarted for device maintenance, interrupting this task. Continue the previous task from the existing session and workspace state. First check whether interrupted commands completed or left partial changes; do not repeat completed side effects. Preserve the user's scope, budget, queued instructions, and permission settings.";

impl Supervisor {
    /// Persist the restart intent before cancellation. Ordinary crashes and user interrupts
    /// never create these markers, so startup cannot accidentally restart unrelated work.
    pub async fn prepare_update_restart(&self) -> Result<OwnedRwLockWriteGuard<()>> {
        self.update_draining.store(true, Ordering::SeqCst);
        let live = self.live.lock().await;
        self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            for (thread_id, turn) in live.iter() {
                if turn.cancel.is_cancelled() { continue; }
                tx.execute(
                    "INSERT OR IGNORE INTO thread_pending_steers(id,thread_id,turn_id,display_prompt,submitted_prompt,delivery,created_at,updated_at)
                     SELECT 'update-resume:'||id,thread_id,id,?1,?1,'update-resume',?2,?2
                     FROM thread_turns WHERE thread_id=?3 AND status IN ('inProgress','recovering')",
                    params![RESUME_PROMPT, now_rfc3339(), thread_id],
                )?;
            }
            tx.commit()?;
            Ok(())
        })?;
        for turn in live.values() {
            turn.cancel.cancel();
        }
        drop(live);
        // run_turn holds a read guard through result persistence. Taking the write
        // guard proves that cancellation and history writes have finished.
        tokio::time::timeout(
            std::time::Duration::from_secs(45),
            self.maintenance_gate.clone().write_owned(),
        )
        .await
        .map_err(|_| anyhow!("Timed out pausing threads for Supervisor update"))
    }

    /// Startup must not consume recovery markers until the independent worker has
    /// verified this process and its relay connection (it may still roll back).
    pub fn defer_update_recovery(&self) -> bool {
        let pending = self.db.with(|conn| Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM thread_pending_steers WHERE delivery='update-resume')", [], |r| r.get::<_,bool>(0))?)).unwrap_or(false);
        if pending {
            self.update_draining.store(true, Ordering::SeqCst);
        }
        pending
    }

    pub fn finish_update_attempt(self: &Arc<Self>) {
        self.update_draining.store(false, Ordering::SeqCst);
        self.spawn_update_recovery();
    }

    pub fn spawn_update_recovery(self: &Arc<Self>) {
        self.spawn_update_recovery_for(None);
    }

    pub(super) fn spawn_update_recovery_for(self: &Arc<Self>, only_thread_id: Option<&str>) {
        let records = self.db.with(|conn| {
            let mut stmt = conn.prepare("SELECT id,thread_id,turn_id,submitted_prompt FROM thread_pending_steers WHERE delivery='update-resume' AND (?1 IS NULL OR thread_id=?1) ORDER BY created_at")?;
            let rows = stmt.query_map(params![only_thread_id], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        });
        let records = match records {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!(%error,"cannot read update recovery journal");
                return;
            }
        };
        for (id, thread_id, source_turn, prompt) in records {
            if !self
                .update_recovering
                .lock()
                .unwrap()
                .insert(thread_id.clone())
            {
                continue;
            }
            let state = self.clone();
            tokio::spawn(async move {
                let result = async {
                    // A failed pause can return before a slow harness finishes cancellation.
                    // Keep the durable intent until that turn has actually settled.
                    while state.live.lock().await.contains_key(&thread_id) {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    let exists = state.db.with(|conn| Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM thread_pending_steers WHERE id=?1)",params![id],|r| r.get::<_,bool>(0))?))?;
                    if !exists { return Ok(()); } // A user stop wins over automatic recovery.
                    // A crash between journaling and cancellation settlement leaves
                    // a recovering turn. Verify its backend before consuming the intent.
                    let latest_id: Option<String> = state.db.with(|conn| Ok(conn.query_row(
                        "SELECT id FROM thread_turns WHERE thread_id=?1 ORDER BY ordinal DESC LIMIT 1",
                        params![thread_id], |row| row.get(0)).optional()?))?;
                    if latest_id.as_deref() == Some(source_turn.as_str()) && state.get_thread(&thread_id)?.status == "recovering" {
                        state.reconnect_thread(&thread_id).await?;
                        if state.get_thread(&thread_id)?.status == "recovering" {
                            bail!("Backend status is still unconfirmed; reconnect before retrying recovery");
                        }
                    }
                    let thread = state.get_thread(&thread_id)?;
                    let latest: Option<(String,String)> = state.db.with(|conn| Ok(conn.query_row(
                        "SELECT id,status FROM thread_turns WHERE thread_id=?1 ORDER BY ordinal DESC LIMIT 1",
                        params![thread_id], |row| Ok((row.get(0)?,row.get(1)?))).optional()?))?;
                    if latest == Some((source_turn, "interrupted".into())) && thread.status == "interrupted" {
                        // Insertion of the new turn and removal of this marker happen in
                        // the same transaction inside run_turn; recovery is consumed once.
                        crate::actor::with_load_intent(
                            &thread_id,
                            pockymoe_protocol::HarnessLoadReason::Resume,
                            state.run_turn(thread, prompt, None, None, Vec::new(), Some(&id), None),
                        )
                        .await?;
                    } else {
                        state.db.with(|conn| {conn.execute("DELETE FROM thread_pending_steers WHERE id=?1",params![id])?;Ok(())})?;
                        if thread.status != "running" {state.drain_steers(&thread_id).await?;}
                    }
                    Ok::<_, anyhow::Error>(())
                }.await;
                if let Err(error) = result {
                    tracing::error!(%thread_id,%error,"update thread recovery failed");
                    let _ = state.db.with(|conn| {
                        // Preserve unconsumed recovery intent across transient
                        // connection failures. User stop still removes it explicitly.
                        if !conn.query_row(
                            "SELECT EXISTS(SELECT 1 FROM thread_pending_steers WHERE id=?1)",
                            params![id],
                            |r| r.get::<_, bool>(0),
                        )? {
                            return Ok(());
                        }
                        conn.execute(
                            "UPDATE threads SET last_error=?1 WHERE id=?2",
                            params![
                                format!("Automatic recovery after update failed: {error}"),
                                thread_id
                            ],
                        )?;
                        Ok(())
                    });
                }
                state.update_recovering.lock().unwrap().remove(&thread_id);
            });
        }
    }
}
