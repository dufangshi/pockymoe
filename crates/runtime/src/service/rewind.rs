use super::*;
use anyhow::Context;
impl Supervisor {
    pub async fn rewind_thread(&self, id: &str, input: &Value) -> Result<Value> {
        let _claim = self
            .try_claim(&[id.into()])
            .context("conflict: thread is busy")?;
        let thread = self.get_thread(id)?;
        ensure!(
            !matches!(thread.status.as_str(), "running" | "recovering"),
            "conflict: thread is running"
        );
        let queued: i64 = self.db.with(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM thread_pending_steers WHERE thread_id=?1",
                [id],
                |r| r.get(0),
            )?)
        })?;
        ensure!(
            queued == 0 || input["dropQueued"] == true,
            "conflict: queued prompts exist; dropQueued is required"
        );
        let turn = input["turnId"].as_str().context("turnId required")?;
        let detail = self.get_thread_detail(id, None).await?;
        let options = self.list_fork_turn_options(id)?;
        let selected = options
            .iter()
            .position(|t| t.turn_id == turn)
            .context("turn not found")?;
        let count = u32::try_from(options.len() - selected)?;
        let prompt=self.db.with(|c|{
            let text:Option<String>=c.query_row("SELECT display_prompt FROM thread_turns WHERE id=?1 AND thread_id=?2",params![turn,id],|r|r.get(0))?;
            let item:Option<String>=c.query_row("SELECT item_json FROM thread_history_items WHERE thread_id=?1 AND turn_id=?2 AND json_extract(item_json,'$.kind')='userMessage' ORDER BY created_at LIMIT 1",params![id,turn],|r|r.get(0)).optional()?;
            let item:Value=item.and_then(|s|serde_json::from_str(&s).ok()).unwrap_or_default();
            let text=text.unwrap_or_default();
            let mut attachments=item["attachments"].as_array().cloned().unwrap_or_default();
            for (prefix,kind) in [("[PHOTO ","photo"),("[FILE ","file")] {
                let mut rest=text.as_str();
                while let Some(i)=rest.find(prefix) {rest=&rest[i+prefix.len()..];let Some(end)=rest.find(']') else {break;};let path=&rest[..end];if path.starts_with(&format!("./.temp/threads/{id}/")){attachments.push(json!({"path":path,"originalName":std::path::Path::new(path).file_name().and_then(|s|s.to_str()).unwrap_or("attachment"),"kind":kind,"placeholder":""}));}rest=&rest[end+1..];}
            }
            Ok(json!({"text":text,"attachments":attachments}))
        })?;
        let runtime = self.runtime(thread.provider)?;

        let session = thread
            .provider_session_id
            .as_deref()
            .context("unsupported_rewind: no session")?;
        let session = self
            .resume_thread_session(
                &thread,
                session,
                Some(&detail.workspace.abs_path),
                HarnessLoadReason::Fork,
            )
            .await?;
        let rollback = runtime.rewind_session(&session, count).await;
        if let Err(error) = &rollback {
            ensure!(error.to_string().contains("unsupported_rewind"), "{error}");
        }
        let (history, replacement) = if rollback.is_ok() {
            ("rollback", None)
        } else {
            let reply = selected
                .checked_sub(1)
                .and_then(|i| fork_reply(&detail.turns, &options[i].turn_id));
            let fork = runtime
                .fork_session_at(
                    &session,
                    &crate::actor::ForkPoint {
                        rollback_count: count,
                        reply,
                    },
                )
                .await
                .map_err(|e| anyhow!("unsupported_rewind: {e}"))?;
            ("fork", Some(fork.session.provider_session_id))
        };
        if replacement.is_some() {
            runtime
                .release_loaded(&session, crate::actor::ReleaseScope::Process, None)
                .await?;
        }
        let restored =
            if input["restoreFiles"] == true {
                Some(self.checkpoint_restore_inner(
                id,
                &json!({"turnId":turn,"phase":"start","workingCopyId":input["workingCopyId"]}),
                false,
                true,
            )
            .await?)
            } else {
                None
            };
        let saved=self.db.with(|c|{
            let tx=c.unchecked_transaction()?;
            let ordinal:i64=tx.query_row("SELECT ordinal FROM thread_turns WHERE id=?1 AND thread_id=?2",params![turn,id],|r|r.get(0))?;
            // Rewind owns conversation rows only. Automation receipts, activity
            // notes, fork lineage and accounting remain as evidence of real work.
            tx.execute("INSERT OR IGNORE INTO rewound_turn_accounting SELECT id,thread_id,status,started_at,completed_at,model,token_usage_json,pricing_model_key,pricing_tier_key,origin,history_copy_of FROM thread_turns WHERE thread_id=?1 AND ordinal>=?2",params![id,ordinal])?;
            for table in ["thread_history_items","thread_turn_metadata","search_documents"] {
                let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",[table],|r|r.get(0))?;
                if exists {tx.execute(&format!("DELETE FROM {table} WHERE thread_id=?1 AND turn_id IN (SELECT id FROM thread_turns WHERE thread_id=?1 AND ordinal>=?2)"),params![id,ordinal])?;}
            }
            // search_documents_delete maintains its external-content FTS index.
            tx.execute("DELETE FROM thread_turns WHERE thread_id=?1 AND ordinal>=?2",params![id,ordinal])?;
            tx.execute("DELETE FROM thread_pending_steers WHERE thread_id=?1",[id])?;
            tx.execute("UPDATE threads SET provider_session_id=COALESCE(?2,provider_session_id),status='idle',last_error=NULL,updated_at=?3 WHERE id=?1",params![id,replacement,now_rfc3339()])?;
            tx.commit()?;Ok(())
        });
        if let Err(error) = saved {
            if let Some(undo) = restored.as_ref().and_then(|v| v["undoId"].as_str()) {
                let parts: Vec<_> = undo.split(':').collect();
                if parts.len() == 3 {
                    self.checkpoint_restore_inner(id,&json!({"turnId":parts[0],"workingCopyId":parts[1],"phase":format!("undo-{}",parts[2])}),false,true).await.context(format!("rewind failed ({error}); could not undo file restore"))?;
                }
            }
            return Err(error);
        }
        if replacement.is_some() {
            self.announce_fork_load(&self.get_thread(id)?).await;
        }
        self.git_changed(id, "historyRewound");
        Ok(
            json!({"removedTurns":count,"prompt":prompt,"history":history,"undoId":restored.as_ref().and_then(|v|v["undoId"].as_str())}),
        )
    }
}
