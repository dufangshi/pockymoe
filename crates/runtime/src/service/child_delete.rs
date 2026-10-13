use super::*;
use crate::interaction::finish_notification;
use anyhow::ensure;
use pockymoe_protocol::ThreadEventEnvelope;

impl Supervisor {
    /// Delete one direct child only. Admission/completion and the DB transaction
    /// serialize the check with new work; no subtree or force-delete variant.
    pub async fn delete_child_thread(&self, parent: &str, id: &str) -> Result<Value> {
        Uuid::parse_str(parent)?;
        Uuid::parse_str(id)?;
        let live = self.live.lock().await;
        ensure!(
            !live.contains_key(id),
            "conflict: child is still running; wait for it to finish"
        );
        self.db.with(|conn| validate(conn, parent, id))?;
        let thread = self.get_thread(id)?;
        if let Some(session) = thread.provider_session_id.as_deref() {
            let runtime = self.runtime(thread.provider)?;
            let loaded = runtime
                .loaded_sessions()
                .await
                .iter()
                .any(|loaded| session_ids_match(&loaded.session_id, session));
            runtime.release_session(session).await?;
            if loaded {
                self.emit_released(id, crate::actor::ReleaseReason::Deleted);
            }
        }
        let now = now_rfc3339();
        let root = self.db.with(|conn| {
            let tx = conn.unchecked_transaction()?;
            // A send may have queued work while the harness was shutting down.
            // Recheck inside the delete transaction and preserve that work.
            let root = validate(&tx, parent, id)?;
            // Deliver any already-terminal completion before deleting its transcript.
            // Parent mailbox results remain available after helper cleanup.
            let turns = {
                let mut stmt =
                    tx.prepare("SELECT id,status FROM thread_turns WHERE thread_id=?1")?;
                let rows = stmt
                    .query_map([id], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                rows
            };
            for (turn, status) in turns {
                finish_notification(&tx, id, &turn, &status, &now)?;
                tx.execute(
                    "DELETE FROM kv WHERE key=?1",
                    [format!("turn-process:{turn}")],
                )?;
            }
            tx.execute(
                "DELETE FROM kv WHERE key GLOB ?1 OR key GLOB ?2 OR key GLOB ?3",
                params![
                    format!("cli:inbox:{id}:*"),
                    format!("cli:request:{id}:*"),
                    format!("prompt-receipt:{id}:*")
                ],
            )?;
            tx.execute("DELETE FROM thread_history_items WHERE thread_id=?1", [id])?;
            tx.execute("DELETE FROM thread_turns WHERE thread_id=?1", [id])?;
            tx.execute("DELETE FROM threads WHERE id=?1", [id])?;
            tx.commit()?;
            Ok(root)
        })?;
        self.revoke_cli_thread_token(id);
        self.claude_history.forget(id);
        drop(live);
        for thread_id in [Some(parent), root.as_deref()]
            .into_iter()
            .flatten()
            .collect::<HashSet<_>>()
        {
            self.bus.emit(ThreadEventEnvelope {
                event_type: "thread.updated".into(),
                thread_id: thread_id.into(),
                timestamp: now.clone(),
                payload: json!({"reason":"child_deleted","deletedThreadId":id}),
            });
        }
        Ok(json!({"threadId":id,"parentThreadId":parent,"deleted":true}))
    }
}

fn validate(conn: &Connection, parent: &str, id: &str) -> Result<Option<String>> {
    let parent_exists = conn
        .query_row("SELECT 1 FROM threads WHERE id=?1", [parent], |_| Ok(()))
        .optional()?
        .is_some();
    ensure!(parent_exists, "parent thread not found");
    let (owner, status, root): (Option<String>, String, Option<String>) = conn
        .query_row(
            "SELECT parent_thread_id,status,root_thread_id FROM threads WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("child thread not found"))?;
    ensure!(
        owner.as_deref() == Some(parent),
        "forbidden: only your own direct child may be deleted"
    );
    ensure!(
        matches!(
            status.as_str(),
            "idle" | "completed" | "failed" | "interrupted"
        ),
        "conflict: child is running or recovering; wait for confirmed completion"
    );
    let pending: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM thread_pending_steers WHERE thread_id=?1) OR EXISTS(SELECT 1 FROM thread_turns WHERE thread_id=?1 AND status IN ('inProgress','recovering'))", [id], |r| r.get(0))?;
    ensure!(
        !pending,
        "conflict: child has active or queued work; finish or cancel it first"
    );
    let descendants: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM threads WHERE parent_thread_id=?1)",
        [id],
        |r| r.get(0),
    )?;
    ensure!(
        !descendants,
        "conflict: child still owns child threads; clean them up from their own parent first"
    );
    Ok(root)
}
