//! Successful durable snapshots, compared before taking the SQLite lock.
use super::*;

#[derive(Default)]
pub(super) struct Snapshots {
    activity: Option<Value>,
    last_seen: Option<std::time::Instant>,
    renewed: Option<std::time::Instant>,
    upstreams: Option<Value>,
    lineage: Option<Value>,
}

impl RelayStore {
    // Activity snapshots expire after a day. Renew online devices in the
    // background every 12 hours, independently of heartbeat persistence.
    pub(super) async fn renew_activity(&self) -> Result<()> {
        let mut cache = self.heartbeats.lock().await;
        for (device, snapshot) in cache.iter_mut() {
            if snapshot.activity.is_some()
                && snapshot
                    .last_seen
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(90))
                && snapshot
                    .renewed
                    .is_some_and(|t| t.elapsed() >= Duration::from_secs(12 * 3600))
            {
                live_activity::replace(
                    &*self.conn.lock().await,
                    device,
                    snapshot.activity.as_ref().unwrap(),
                )?;
                snapshot.renewed = Some(std::time::Instant::now());
            }
        }
        Ok(())
    }

    pub(super) async fn persist_heartbeat(&self, device: &str, frame: &Value) {
        let mut cache = self.heartbeats.lock().await;
        // Prune disconnected entries under pressure; never evict active devices.
        // A returning evicted device simply performs a cold write.
        if cache.len() >= 1024 && !cache.contains_key(device) {
            cache.retain(|_, s| {
                s.last_seen
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(90))
            });
        }
        let snapshots = cache.entry(device.to_owned()).or_default();
        snapshots.last_seen = Some(std::time::Instant::now());
        for (key, previous, persist) in [
            (
                "threadActivity",
                &mut snapshots.activity,
                live_activity::replace as fn(&Connection, &str, &Value) -> Result<()>,
            ),
            (
                "upstreamConnections",
                &mut snapshots.upstreams,
                upstream_catalog::replace,
            ),
            (
                "threadLineage",
                &mut snapshots.lineage,
                thread_groups::replace,
            ),
        ] {
            let value = &frame[key];
            if !value.is_array() || previous.as_ref() == Some(value) {
                continue;
            }
            let conn = self.conn.lock().await;
            match persist(&conn, device, value) {
                Ok(()) => {
                    *previous = Some(value.clone());
                    if key == "threadActivity" {
                        snapshots.renewed = Some(std::time::Instant::now());
                    }
                }
                Err(error) => tracing::warn!(%error, snapshot=key, "invalid device snapshot"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn rejected_snapshot_is_retried_and_activity_expiry_is_renewed_off_heartbeat() {
        let (state, dir) = crate::tests::test_app_state("heartbeat-retry");
        let frame = json!({"threadActivity":[]});
        state.store.persist_heartbeat("device", &frame).await; // no owner: must fail
        assert!(state.store.heartbeats.lock().await["device"]
            .activity
            .is_none());
        {
            let c = state.store.conn.lock().await;
            c.execute(
                "INSERT INTO relay_users VALUES ('u','u','u','user',1,NULL,'now','salt','hash')",
                [],
            )
            .unwrap();
            c.execute("INSERT INTO relay_devices VALUES ('device','u','device',NULL,'hash','preview','now')", []).unwrap();
        }
        state.store.persist_heartbeat("device", &frame).await;
        assert!(state.store.heartbeats.lock().await["device"]
            .activity
            .is_some());
        state
            .store
            .conn
            .lock()
            .await
            .execute("DELETE FROM relay_live_activity_devices", [])
            .unwrap();
        state
            .store
            .heartbeats
            .lock()
            .await
            .get_mut("device")
            .unwrap()
            .renewed = Some(std::time::Instant::now() - Duration::from_secs(13 * 3600));
        state.store.renew_activity().await.unwrap();
        assert_eq!(
            state
                .store
                .conn
                .lock()
                .await
                .query_row(
                    "SELECT COUNT(*) FROM relay_live_activity_devices",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        state.store.conn.take_sample();
        state.store.persist_heartbeat("device", &frame).await;
        state.store.renew_activity().await.unwrap();
        assert_eq!(state.store.conn.take_sample().acquired, 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn unchanged_heartbeat_never_takes_sqlite_lock() {
        let (state, dir) = crate::tests::test_app_state("heartbeat-cache");
        {
            let c = state.store.conn.lock().await;
            c.execute(
                "INSERT INTO relay_users VALUES ('u','u','u','user',1,NULL,'now','salt','hash')",
                [],
            )
            .unwrap();
            c.execute("INSERT INTO relay_devices VALUES ('device','u','device',NULL,'hash','preview','now')", []).unwrap();
        }
        let frame = json!({"threadLineage":[{"id":Uuid::new_v4().to_string()}],"upstreamConnections":[],"threadActivity":[]});
        state.store.persist_heartbeat("device", &frame).await;
        state.store.conn.take_sample();
        let before = state.store.conn.lock().await.total_changes();
        state.store.conn.take_sample();
        for _ in 0..100 {
            state.store.persist_heartbeat("device", &frame).await;
        }
        assert_eq!(
            state.store.conn.take_sample().acquired,
            0,
            "no connection lock means no write transaction"
        );
        assert_eq!(state.store.conn.lock().await.total_changes(), before);
        // Missing fields preserve the last snapshot; an explicit empty list clears it.
        state.store.persist_heartbeat("device", &json!({})).await;
        state
            .store
            .persist_heartbeat("device", &json!({"threadLineage":[]}))
            .await;
        assert_eq!(state.store.conn.take_sample().acquired, 2); // inspection + changed lineage
        std::fs::remove_dir_all(dir).unwrap();
    }
}
