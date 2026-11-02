//! Content-free ActivityKit updates. Titles remain in the phone's local cache.
use super::*;

pub(super) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS relay_live_activity_tokens (
      subscription_id TEXT PRIMARY KEY REFERENCES relay_push_subscriptions(id) ON DELETE CASCADE,
      token TEXT NOT NULL, updated_at INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS relay_live_activity_devices (
      device_id TEXT PRIMARY KEY REFERENCES relay_devices(id) ON DELETE CASCADE,
      snapshot TEXT NOT NULL, updated_at INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS relay_live_activity_outbox (
      subscription_id TEXT PRIMARY KEY REFERENCES relay_live_activity_tokens(subscription_id) ON DELETE CASCADE,
      payload TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL DEFAULT 0);")?;
    Ok(())
}

pub(super) fn register(conn: &Connection, id: &str, token: &str) -> Result<()> {
    anyhow::ensure!(
        super::apns::valid_token(token),
        "Invalid Live Activity token"
    );
    conn.execute("INSERT INTO relay_live_activity_tokens VALUES (?1,?2,?3) ON CONFLICT(subscription_id) DO UPDATE SET token=excluded.token,updated_at=excluded.updated_at", params![id,token,chrono::Utc::now().timestamp_millis()])?;
    let user: String = conn.query_row(
        "SELECT user_id FROM relay_push_subscriptions WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    let payload = payload(conn, &user)?;
    conn.execute("INSERT INTO relay_live_activity_outbox(subscription_id,payload) VALUES (?1,?2) ON CONFLICT(subscription_id) DO UPDATE SET payload=excluded.payload,attempts=0,next_at=0",params![id,payload.to_string()])?;
    Ok(())
}

// Rebuild each row from an allowlist: untrusted extra fields can never reach APNs.
fn sanitized(rows: &Value) -> Result<Value> {
    let rows = rows
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid activity snapshot"))?;
    anyhow::ensure!(rows.len() <= 100_000, "Activity snapshot too large");
    let mut clean = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let id = row["threadId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing thread ID"))?;
        Uuid::parse_str(id)?;
        anyhow::ensure!(seen.insert(id), "Duplicate thread ID");
        let state = row["state"].as_str().unwrap_or("");
        anyhow::ensure!(
            [
                "running",
                "needsInput",
                "failed",
                "completed",
                "interrupted"
            ]
            .contains(&state),
            "Invalid activity state"
        );
        let since = row["since"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing timestamp"))?;
        let at = chrono::DateTime::parse_from_rfc3339(since)?.timestamp_millis();
        anyhow::ensure!(
            at <= chrono::Utc::now().timestamp_millis() + 60_000,
            "Future activity"
        );
        let mut item = json!({"threadId":id,"state":state,"since":chrono::DateTime::parse_from_rfc3339(since)?.to_rfc3339()});
        if let Some(running_since) = row.get("runningSince") {
            let running_since = chrono::DateTime::parse_from_rfc3339(
                running_since
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Invalid running timestamp"))?,
            )?;
            anyhow::ensure!(
                running_since.timestamp_millis() <= chrono::Utc::now().timestamp_millis() + 60_000,
                "Future activity"
            );
            item["runningSince"] = json!(running_since.to_rfc3339());
        }
        clean.push(item);
    }
    Ok(json!(clean))
}

pub(super) fn replace(conn: &Connection, device: &str, rows: &Value) -> Result<()> {
    let clean = sanitized(rows)?;
    let tx = conn.unchecked_transaction()?;
    let user: String = tx.query_row(
        "SELECT owner_user_id FROM relay_devices WHERE id=?1",
        [device],
        |r| r.get(0),
    )?;
    let previous: Option<String> = tx
        .query_row(
            "SELECT snapshot FROM relay_live_activity_devices WHERE device_id=?1",
            [device],
            |r| r.get(0),
        )
        .optional()?;
    tx.execute("INSERT INTO relay_live_activity_devices VALUES (?1,?2,?3) ON CONFLICT(device_id) DO UPDATE SET snapshot=excluded.snapshot,updated_at=excluded.updated_at", params![device,clean.to_string(),chrono::Utc::now().timestamp_millis()])?;
    if previous.as_deref() != Some(&clean.to_string()) {
        let payload = payload(&tx, &user)?;
        tx.execute("INSERT INTO relay_live_activity_outbox(subscription_id,payload) SELECT t.subscription_id,?2 FROM relay_live_activity_tokens t JOIN relay_push_subscriptions p ON p.id=t.subscription_id WHERE p.user_id=?1 ON CONFLICT(subscription_id) DO UPDATE SET payload=excluded.payload,attempts=0,next_at=0", params![user,payload.to_string()])?;
    }
    tx.commit()?;
    Ok(())
}

fn payload(conn: &Connection, user: &str) -> Result<Value> {
    payload_at(conn, user, chrono::Utc::now())
}
fn payload_at(conn: &Connection, user: &str, now: chrono::DateTime<chrono::Utc>) -> Result<Value> {
    let mut stmt = conn.prepare("SELECT a.device_id,a.snapshot FROM relay_live_activity_devices a JOIN relay_devices d ON d.id=a.device_id WHERE d.owner_user_id=?1 AND a.updated_at>?2 ORDER BY a.device_id")?;
    let snapshots = stmt
        .query_map(params![user, now.timestamp_millis() - 86_400_000], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut threads = Vec::new();
    let mut earliest_running: Option<f64> = None;
    let mut counts = json!({"running":0,"needsInput":0,"failed":0,"completed":0,"interrupted":0});
    for (device, snapshot) in snapshots {
        for row in serde_json::from_str::<Vec<Value>>(&snapshot)? {
            let state = row["state"].as_str().unwrap_or("");
            let since = chrono::DateTime::parse_from_rfc3339(row["since"].as_str().unwrap_or(""))?;
            if !["running", "needsInput"].contains(&state)
                && !row["runningSince"].is_string()
                && since < now - chrono::Duration::minutes(15)
            {
                continue;
            }
            counts[state] = json!(counts[state].as_u64().unwrap_or(0) + 1);
            // A failed/waiting agent can share a conversation with a running
            // thread. Display priority must never end an activity prematurely.
            if state != "running" && row["runningSince"].is_string() {
                counts["running"] = json!(counts["running"].as_u64().unwrap_or(0) + 1);
            }
            let started = row["runningSince"]
                .as_str()
                .unwrap_or(row["since"].as_str().unwrap_or(""));
            if state == "running" || row["runningSince"].is_string() {
                let started = swift_date(
                    chrono::DateTime::parse_from_rfc3339(started)?.with_timezone(&chrono::Utc),
                );
                earliest_running = Some(earliest_running.map_or(started, |old| old.min(started)));
            }
            threads.push(json!({"key":format!("{device}:{}",row["threadId"].as_str().unwrap_or("")),"state":state,"startedAt":swift_date(chrono::DateTime::parse_from_rfc3339(started)?.with_timezone(&chrono::Utc))}));
        }
    }
    // APNs has a 4 KiB limit. Counts include every conversation; the compact
    // system surface only needs the first rows. IDs/times remain content-free.
    let priority = |row: &Value| match row["state"].as_str() {
        Some("needsInput") => 0,
        Some("failed") => 1,
        Some("running") => 2,
        _ => 3,
    };
    threads.sort_by(|a, b| {
        priority(a)
            .cmp(&priority(b))
            .then_with(|| {
                a["startedAt"]
                    .as_f64()
                    .partial_cmp(&b["startedAt"].as_f64())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a["key"].as_str().cmp(&b["key"].as_str()))
    });
    let total_count = threads.len();
    let earliest_start = earliest_running.unwrap_or_else(|| {
        threads
            .first()
            .and_then(|row| row["startedAt"].as_f64())
            .unwrap_or_else(|| swift_date(now))
    });
    threads.truncate(3);
    let ended = counts["running"] == 0 && counts["needsInput"] == 0;
    // ActivityKit owns the system surface: a static running ring and system
    // timer replace looping SVG animation (ui-principles §6 exception).
    let mut aps = json!({"timestamp":now.timestamp(),"event":if ended {"end"} else {"update"},"content-state":{"rows":threads,"totalCount":total_count,"waitingCount":counts["needsInput"],"failedCount":counts["failed"],"runningCount":counts["running"],"completedCount":if ended {json!(total_count)} else {counts["completed"].clone()},"endedAt":if ended {json!(swift_date(now))} else {Value::Null},"earliestStart":earliest_start}});
    if ended {
        aps["dismissal-date"] = json!(now.timestamp() + 900);
    }
    Ok(json!({"aps":aps}))
}

// ActivityKit requires default Codable strategies for content-state. Foundation
// Date's default encoding is seconds since 2001-01-01 (not Unix); APNs envelope
// timestamp/dismissal-date remain Unix seconds. See the shared JSON fixtures.
fn swift_date(date: chrono::DateTime<chrono::Utc>) -> f64 {
    date.timestamp_millis() as f64 / 1000.0 - 978_307_200.0
}

pub(super) async fn deliver_batch(state: &AppState, http: &reqwest::Client) -> Result<()> {
    let now = chrono::Utc::now().timestamp_millis();
    let jobs = {
        let conn = state.store.conn.lock().await;
        conn.execute(
            "DELETE FROM relay_live_activity_devices WHERE updated_at<?1",
            [now - 86_400_000],
        )?;
        let mut stmt = conn.prepare("SELECT o.subscription_id,p.subscription,t.token,o.payload,o.attempts FROM relay_live_activity_outbox o JOIN relay_live_activity_tokens t ON t.subscription_id=o.subscription_id JOIN relay_push_subscriptions p ON p.id=t.subscription_id JOIN relay_auth_sessions s ON s.token_hash=p.session_hash JOIN relay_users u ON u.id=p.user_id WHERE s.expires_at>?1 AND u.enabled=1 AND o.next_at<=?1 LIMIT 20")?;
        let rows = stmt
            .query_map([now], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, u32>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (id, raw, token, payload, attempts) in jobs {
        let mut sub: apns::Subscription = serde_json::from_str(&raw)?;
        sub.device_token = token.clone();
        let result = apns::deliver_live_activity(http, &sub, &payload).await;
        let conn = state.store.conn.lock().await;
        match result {
            Ok(status) if status == StatusCode::GONE || status == StatusCode::NOT_FOUND => {
                conn.execute(
                    "DELETE FROM relay_live_activity_tokens WHERE subscription_id=?1 AND token=?2",
                    params![id, token],
                )?;
            }
            Ok(status) if status.is_success() => {
                conn.execute("DELETE FROM relay_live_activity_outbox WHERE subscription_id=?1 AND payload=?2 AND EXISTS(SELECT 1 FROM relay_live_activity_tokens WHERE subscription_id=?1 AND token=?3)",params![id,payload,token])?;
            }
            _ => {
                conn.execute("UPDATE relay_live_activity_outbox SET attempts=attempts+1,next_at=?3 WHERE subscription_id=?1 AND payload=?2",params![id,payload,now+15_000*(1i64<<attempts.min(8))])?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_activity_payload_matches_ios_shared_fixtures() -> Result<()> {
        let (state, dir) = crate::tests::test_app_state("ios-activity-fixture");
        let conn = state.store.conn.try_lock().unwrap();
        let device = "00000000-0000-4000-8000-000000000001";
        conn.execute(
            "INSERT INTO relay_users VALUES ('u','u','u','user',1,NULL,'now','salt','hash')",
            [],
        )?;
        conn.execute(
            "INSERT INTO relay_devices VALUES (?1,'u',?1,NULL,?1,?1,'now')",
            [device],
        )?;
        for raw in [
            include_str!("../test-fixtures/live-activity-update.json"),
            include_str!("../test-fixtures/live-activity-end.json"),
        ] {
            let expected: Value = serde_json::from_str(raw)?;
            let now =
                chrono::DateTime::from_timestamp(expected["aps"]["timestamp"].as_i64().unwrap(), 0)
                    .unwrap();
            let rows=expected["aps"]["content-state"]["rows"].as_array().unwrap().iter().map(|row| {
                let timestamp=row["startedAt"].as_f64().unwrap()+978_307_200.0;
                json!({"threadId":row["key"].as_str().unwrap().split(':').nth(1).unwrap(),"state":row["state"],"since":chrono::DateTime::from_timestamp(timestamp as i64,0).unwrap().to_rfc3339()})
            }).collect::<Vec<_>>();
            conn.execute("INSERT INTO relay_live_activity_devices VALUES (?1,?2,?3) ON CONFLICT(device_id) DO UPDATE SET snapshot=excluded.snapshot,updated_at=excluded.updated_at",params![device,json!(rows).to_string(),now.timestamp_millis()])?;
            assert_eq!(payload_at(&conn, "u", now)?, expected);
        }
        drop(conn);
        drop(state);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
    #[test]
    fn live_activity_privacy_account_isolation_rotation_and_additive_schema() -> Result<()> {
        let (state, dir) = crate::tests::test_app_state("live-activity");
        let conn = state.store.conn.try_lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        for user in ["owner", "other"] {
            conn.execute(
                "INSERT INTO relay_users VALUES (?1,?1,?1,'user',1,NULL,'now','salt','hash')",
                [user],
            )?;
            let session = create_session(&conn, "secret", user)?;
            conn.execute("INSERT INTO relay_push_subscriptions VALUES (?1,?1,?2,?3,?4)",params![user,security::token_hash(&session),json!({"nativePlatform":"apns","deviceToken":"ab".repeat(32),"relayOrigin":"https://relay.test"}).to_string(),now])?;
            conn.execute(
                "INSERT INTO relay_devices VALUES (?1,?1,?1,NULL,?1,?1,'now')",
                [user],
            )?;
            register(&conn, user, &"ab".repeat(32))?;
        }
        ensure_schema(&conn)?;
        ensure_schema(&conn)?;
        conn.execute("DELETE FROM relay_live_activity_outbox", [])?;
        let id = Uuid::new_v4().to_string();
        let rows = json!([{"threadId":id,"state":"needsInput","since":now_rfc3339(),"title":"SECRET TITLE","deviceName":"SECRET DEVICE","message":"SECRET MESSAGE"}]);
        replace(&conn, "owner", &rows)?;
        let queued: String = conn.query_row(
            "SELECT payload FROM relay_live_activity_outbox WHERE subscription_id='owner'",
            [],
            |r| r.get(0),
        )?;
        assert!(!queued.contains("SECRET"));
        assert_eq!(
            conn.query_row("SELECT count(*) FROM relay_live_activity_outbox", [], |r| r
                .get::<_, i64>(0))?,
            1
        );
        let p: Value = serde_json::from_str(&queued)?;
        assert_eq!(p["aps"]["content-state"]["waitingCount"], 1);
        assert_eq!(p["aps"]["event"], "update");
        assert_eq!(
            p["aps"]["content-state"]["rows"][0],
            json!({"key":format!("owner:{id}"),"state":"needsInput","startedAt":swift_date(chrono::DateTime::parse_from_rfc3339(rows[0]["since"].as_str().unwrap())?.with_timezone(&chrono::Utc))})
        );
        assert!(register(&conn, "owner", "invalid").is_err());
        register(&conn, "owner", &"cd".repeat(32))?;
        assert_eq!(
            conn.query_row(
                "SELECT token FROM relay_live_activity_tokens WHERE subscription_id='owner'",
                [],
                |r| r.get::<_, String>(0)
            )?,
            "cd".repeat(32)
        );
        replace(&conn, "owner", &json!([]))?;
        let end = payload(&conn, "owner")?;
        assert_eq!(end["aps"]["event"], "end");
        assert_eq!(
            end["aps"]["dismissal-date"].as_i64().unwrap()
                - end["aps"]["timestamp"].as_i64().unwrap(),
            900
        );
        // Invalid snapshots leave the last good snapshot/outbox untouched.
        assert!(replace(
            &conn,
            "owner",
            &json!([{"threadId":"bad","state":"running","since":"bad"}])
        )
        .is_err());
        conn.execute("DELETE FROM relay_auth_sessions WHERE user_id='owner'", [])?;
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM relay_live_activity_tokens WHERE subscription_id='owner'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        drop(conn);
        drop(state);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
    #[test]
    fn live_activity_counts_all_rows_but_fits_apns_and_ends_only_without_waiting() -> Result<()> {
        let (state, dir) = crate::tests::test_app_state("live-activity-size");
        let conn = state.store.conn.try_lock().unwrap();
        conn.execute(
            "INSERT INTO relay_users VALUES ('u','u','u','user',1,NULL,'now','salt','hash')",
            [],
        )?;
        conn.execute(
            "INSERT INTO relay_devices VALUES ('d','u','d',NULL,'d','d','now')",
            [],
        )?;
        let rows = (0..250).map(|_|json!({"threadId":Uuid::new_v4().to_string(),"state":"running","since":now_rfc3339(),"runningSince":now_rfc3339()})).collect::<Vec<_>>();
        replace(&conn, "d", &json!(rows))?;
        let p = payload(&conn, "u")?;
        assert_eq!(p["aps"]["content-state"]["runningCount"], 250);
        assert_eq!(
            p["aps"]["content-state"]["rows"].as_array().unwrap().len(),
            3
        );
        assert!(p.to_string().len() <= 4096);
        assert_eq!(p["aps"]["event"], "update");
        assert!(sanitized(&json!([rows[0], rows[0]])).is_err());
        replace(
            &conn,
            "d",
            &json!([{"threadId":Uuid::new_v4().to_string(),"state":"failed","since":(chrono::Utc::now()-chrono::Duration::hours(1)).to_rfc3339(),"runningSince":now_rfc3339()}]),
        )?;
        let p = payload(&conn, "u")?;
        assert_eq!(p["aps"]["event"], "update");
        assert_eq!(p["aps"]["content-state"]["failedCount"], 1);
        assert_eq!(p["aps"]["content-state"]["runningCount"], 1);
        drop(conn);
        drop(state);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
