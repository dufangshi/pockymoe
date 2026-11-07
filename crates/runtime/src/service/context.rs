use super::*;

impl Supervisor {
    pub fn context_inspection_settings(&self) -> Value {
        self.context_inspection.settings()
    }
    pub fn update_context_inspection_settings(&self, enabled: bool) -> Result<Value> {
        crate::context_inspection::save_settings(&self.db, enabled)?;
        self.context_inspection.set_enabled(enabled);
        Ok(self.context_inspection.settings())
    }
    pub async fn thread_context_inspection(
        &self,
        id: &str,
        detail: &str,
        refresh: bool,
    ) -> Result<Value> {
        let thread = self.get_thread(id)?;
        if self.environments.for_thread(id)?.is_some() {
            return Ok(
                json!({"enabled": self.context_inspection.enabled(), "supported": false,
                "availability": "unavailable", "snapshot": null, "reason": "unsupportedEnvironmentContextInspection"}),
            );
        }
        let provider = thread
            .agent_id
            .as_deref()
            .unwrap_or(thread.provider.as_str());
        let supported = matches!(provider, "claude" | "codex" | "deepseek");
        let enabled = self.context_inspection.enabled();
        if !enabled || !supported {
            return Ok(
                json!({"enabled":enabled,"supported":supported,"availability":if !enabled{"disabled"}else{"unavailable"},"snapshot":null,"reason":if !enabled{"disabled"}else{"unsupportedHarness"}}),
            );
        }
        let session = thread.provider_session_id.as_deref().unwrap_or("");
        let mut snapshot = self
            .context_inspection
            .cached(session)
            .or_else(|| {
                thread
                    .context_usage
                    .as_ref()
                    .and_then(|v| v.get("details").cloned())
            })
            .filter(crate::context_inspection::usable_snapshot);
        let mut reason = None;
        if refresh || snapshot.is_none() {
            match self
                .runtime(thread.provider)?
                .inspect_context(session, detail)
                .await
            {
                Ok(Some(fresh)) => {
                    // Codex inspect_context returns a trace cache, not a new
                    // observation. Re-emitting it on panel open can overwrite
                    // a newer native reading (e.g. after a delta/compaction).
                    if provider != "codex" {
                        crate::context_inspection::emit(
                            &self.context_inspection,
                            session,
                            id,
                            &self.bus,
                            fresh.clone(),
                        );
                    }
                    snapshot = Some(fresh);
                }
                Ok(None) => reason = Some("waitingForNextRequest"),
                Err(_) => reason = Some("inspectionUnavailable"),
            }
        }
        if let Some(snapshot) = snapshot.as_mut() {
            if thread.context_usage.as_ref().is_some_and(|base| {
                base["updatedAt"]
                    .as_str()
                    .zip(snapshot["updatedAt"].as_str())
                    .is_some_and(|(base_at, snapshot_at)| base_at > snapshot_at)
                    && (base["tokensInContextWindow"] != snapshot["usedTokens"]
                        || base["modelContextWindow"] != snapshot["windowTokens"])
            }) {
                snapshot["stale"] = json!(true);
            }
            if reason.is_some() {
                snapshot["stale"] = json!(true);
            }
            // Full request refresh must not silently call a cached summary a full reading.
            if provider == "claude"
                && detail == "full"
                && snapshot["source"] != "claudeFull"
                && refresh
            {
                reason = Some("fullInspectionUnavailable");
                snapshot["stale"] = json!(true);
            }
        }
        Ok(
            json!({"enabled":enabled,"supported":true,"availability":if snapshot.is_some(){"available"}else{"unavailable"},"snapshot":snapshot,"reason":reason}),
        )
    }
}
