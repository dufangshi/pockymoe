//! Persist the runtime's actual session identity after an explicit load. An
//! environment generation changes this identity while keeping its native state.
use super::*;

impl Supervisor {
    pub(crate) async fn resume_thread_session(
        &self,
        thread: &ThreadDto,
        session: &str,
        cwd: Option<&str>,
        reason: HarnessLoadReason,
    ) -> Result<String> {
        let runtime = self.runtime(thread.provider)?;
        let resumed = self
            .with_load(
                &thread.id,
                reason,
                runtime.resume_session(session, cwd, thread_session_settings(thread)),
            )
            .await?;
        ensure!(
            !resumed.provider_session_id.is_empty(),
            "runtime returned an empty session identity"
        );
        if let Some(context) = self.execution_context(&thread.id)? {
            context.checked()?;
            let expected = format!("env/{}/{}/", context.environment_id, context.generation);
            ensure!(
                resumed
                    .provider_session_id
                    .split_once("::")
                    .is_some_and(|(_, raw)| raw.starts_with(&expected)),
                "resumed session does not belong to the current environment generation"
            );
        }
        if resumed.provider_session_id != session {
            self.db.with(|connection| {
                let changed = connection.execute(
                    "UPDATE threads SET provider_session_id=?1 WHERE id=?2 AND provider_session_id=?3",
                    params![resumed.provider_session_id, thread.id, session],
                )?;
                if changed == 0 {
                    let current: Option<String> = connection.query_row(
                        "SELECT provider_session_id FROM threads WHERE id=?1", [&thread.id], |row| row.get(0),
                    )?;
                    ensure!(current.as_deref() == Some(resumed.provider_session_id.as_str()),
                        "conflict: thread session changed during resume");
                }
                Ok(())
            })?;
            self.bus.emit(pockymoe_protocol::ThreadEventEnvelope {
                event_type: pockymoe_protocol::ThreadEventType::ThreadUpdated,
                thread_id: thread.id.clone(),
                timestamp: now_rfc3339(),
                payload: json!({"providerSessionId":resumed.provider_session_id}),
            });
        }
        Ok(resumed.provider_session_id)
    }
}
