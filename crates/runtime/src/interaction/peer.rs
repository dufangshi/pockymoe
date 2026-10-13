//! Device opt-in and server-supplied remote senders; see docs/cross-device-peer.zh.md.
use super::SendInput;
use crate::Supervisor;
use anyhow::Result;
use chrono::{DateTime, Duration, SecondsFormat};
use pockymoe_protocol::now_rfc3339;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

const SETTINGS: &str = "peer:settings";

/// Built by the peer HTTP handler, never deserialized from local CLI input.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSender {
    pub device_id: String,
    pub device_name: String,
    pub thread_id: Option<String>,
}

impl Supervisor {
    pub fn send_to_thread_from_peer(
        &self,
        id: &str,
        mut input: SendInput,
        sender: RemoteSender,
    ) -> Result<Value> {
        input.from_thread_id = sender.thread_id.clone();
        self.send_to_thread_inner(id, input, Some(sender))
    }

    pub fn peer_access_enabled(&self) -> bool {
        self.db
            .get_kv(SETTINGS)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .is_some_and(|settings| settings["enabled"] == true)
    }

    pub fn set_peer_access(&self, enabled: bool) -> Result<Value> {
        let settings = json!({"enabled": enabled, "updatedAt": now_rfc3339()});
        self.db.set_kv(SETTINGS, &settings.to_string())?;
        Ok(settings)
    }
}

pub(super) fn completion_outbox(
    conn: &Connection,
    device: &str,
    target: &str,
    from: &str,
    text: &str,
    subject: &str,
    now: &str,
) -> Result<()> {
    let id = Uuid::new_v4().to_string();
    let expires = (DateTime::parse_from_rfc3339(now)? + Duration::days(7))
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let record = json!({
        "id": id,
        "targetDeviceId": device,
        "request": {
            "operation": "send", "threadId": target, "text": text,
            "delivery": "inbox", "kind": "result", "subject": subject,
            "inReplyTo": null, "fromThreadId": from, "notifyOnComplete": false,
            "clientRequestId": format!("outbox-{id}"),
        },
        "attachments": [], "createdAt": now, "attempts": 0,
        "nextAttemptAt": now, "lastError": null, "expiresAt": expires,
    });
    conn.execute(
        "INSERT INTO kv(key,value) VALUES(?1,?2)",
        params![format!("peer:outbox:{id}"), record.to_string()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fake::FakeRuntime, local_sessions::LocalSessionHomes, Database, RuntimeConfig};
    use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Mode, Provider};
    use std::sync::Arc;

    fn setup() -> (tempfile::TempDir, Supervisor) {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            mode: Mode::Local,
            host: "127.0.0.1".into(),
            port: 0,
            workspace_root: dir.path().into(),
            database_url: dir.path().join("test.sqlite"),
            app_name: "test".into(),
            app_version: "test".into(),
            environment: "test".into(),
            auth_required: false,
            admin_username: None,
            admin_password: None,
            session_secret: None,
            relay_server_url: None,
            web_origin: None,
            relay_agent_token: None,
            enabled_providers: vec![Provider::Codex],
            acp_command: None,
            acp_startup_timeout_ms: 1000,
            fake_runtime: true,
            supervisor_restart_strategy: Default::default(),
        };
        let db = Database::open(&config.database_url).unwrap();
        let state = Supervisor::new(
            config,
            db,
            vec![Arc::new(FakeRuntime::new(Provider::Codex))],
        )
        .with_local_session_homes(LocalSessionHomes {
            codex_home: dir.path().join("codex"),
            grok_home: dir.path().join("grok"),
            claude_home: dir.path().join("claude"),
        });
        state
            .create_workspace(CreateWorkspaceInput {
                abs_path: Some(dir.path().to_string_lossy().into()),
                git_url: None,
                label: Some("test".into()),
            })
            .unwrap();
        (dir, state)
    }

    async fn thread(state: &Supervisor) -> String {
        state
            .create_thread(CreateThreadInput {
                workspace_id: state.list_workspaces().unwrap()[0].id.clone(),
                title: None,
                provider: Some(Provider::Codex),
                agent_id: None,
                model: "ios-e2e-stream".into(),
                reasoning_effort: None,
                approval_mode: "guarded".into(),
                parent_thread_id: None,
            })
            .await
            .unwrap()
            .id
    }

    fn sender(device: &str, thread: Option<&str>) -> RemoteSender {
        RemoteSender {
            device_id: device.into(),
            device_name: "Peer laptop".into(),
            thread_id: thread.map(str::to_owned),
        }
    }

    fn input(delivery: &str) -> SendInput {
        let mut input = json!({
            "text": "hello peer", "delivery": delivery, "subject": "Review", "kind": "task",
            "inReplyTo": "prior-message", "clientRequestId": "shared-key",
        });
        // The peer policy applies to remote senders too.
        if matches!(delivery, "direct" | "steer") {
            input["interruptReason"] = json!("the remote task depends on this now");
        }
        serde_json::from_value(input).unwrap()
    }

    #[tokio::test]
    async fn peer_remote_mail_has_reply_route_and_json_cannot_supply_identity() {
        let (_dir, state) = setup();
        let target = thread(&state).await;
        let forged: SendInput = serde_json::from_value(json!({
            "text": "hello peer", "fromThreadId": "foreign-thread",
            "fromDeviceId": "forged", "fromDeviceName": "forged", "replyTo": "forged/thread",
            "remoteSender": {"deviceId": "forged", "deviceName": "forged", "threadId": "foreign-thread"},
        })).unwrap();
        assert!(state.send_to_thread(&target, forged.clone()).is_err());
        let receipt = state
            .send_to_thread_from_peer(&target, forged, sender("device-a", Some("foreign-thread")))
            .unwrap();
        let message = state
            .inbox_read(&target, &json!({"messageId": receipt["messageId"]}))
            .unwrap();
        assert_eq!(message["fromThreadId"], "foreign-thread");
        assert_eq!(message["fromDeviceId"], "device-a");
        assert_eq!(message["fromDeviceName"], "Peer laptop");
        assert_eq!(message["replyTo"], "device-a/foreign-thread");
        assert_eq!(message["text"], "hello peer");
        assert_eq!(
            state.interaction_status(&target).await.unwrap()["queuedCount"],
            0
        );
        let list = state.inbox_list(&target, &json!({})).unwrap();
        assert_eq!(list["messages"][0]["replyTo"], message["replyTo"]);
        state
            .inbox_ack(&target, &json!({"messageIds": [receipt["messageId"]]}))
            .unwrap();
        assert_eq!(
            state
                .inbox_read(&target, &json!({"messageId": receipt["messageId"]}))
                .unwrap()["replyTo"],
            message["replyTo"]
        );

        let local: SendInput = serde_json::from_value(json!({
            "text": "local JSON", "fromDeviceId": "forged", "fromDeviceName": "forged",
            "remoteSender": {"deviceId": "forged"},
        }))
        .unwrap();
        let local_receipt = state.send_to_thread(&target, local).unwrap();
        let local_message = state
            .inbox_read(&target, &json!({"messageId": local_receipt["messageId"]}))
            .unwrap();
        assert!(local_message.get("fromDeviceId").is_none());
        let machine_receipt = state
            .send_to_thread_from_peer(&target, input("inbox"), sender("device-a", None))
            .unwrap();
        let machine_message = state
            .inbox_read(&target, &json!({"messageId": machine_receipt["messageId"]}))
            .unwrap();
        assert_eq!(machine_message["fromDeviceId"], "device-a");
        assert!(machine_message["replyTo"].is_null());
    }

    #[tokio::test]
    async fn peer_remote_dedup_is_scoped_to_device_thread_and_local_sender() {
        let (_dir, state) = setup();
        let target = thread(&state).await;
        let from = thread(&state).await;
        let mut body = input("inbox");
        body.from_thread_id = Some(from.clone());
        let local = state.send_to_thread(&target, body.clone()).unwrap();
        let first = state
            .send_to_thread_from_peer(&target, body.clone(), sender("device-a", Some(&from)))
            .unwrap();
        let other_device = state
            .send_to_thread_from_peer(&target, body.clone(), sender("device-b", Some(&from)))
            .unwrap();
        let other_thread = state
            .send_to_thread_from_peer(
                &target,
                body.clone(),
                sender("device-a", Some("other-thread")),
            )
            .unwrap();
        for other in [&local, &other_device, &other_thread] {
            assert_ne!(first["messageId"], other["messageId"]);
        }
        assert_eq!(
            state
                .send_to_thread_from_peer(&target, body.clone(), sender("device-a", Some(&from)))
                .unwrap(),
            first
        );
        assert_eq!(state.inbox_unread_count(&target).unwrap(), 4);
        assert!(state
            .db
            .get_kv(&format!(
                "cli:request:{target}:peer:device-a:{from}:shared-key"
            ))
            .unwrap()
            .is_some());
        body.text = "different content".into();
        assert!(state
            .send_to_thread_from_peer(&target, body, sender("device-a", Some(&from)))
            .unwrap_err()
            .to_string()
            .starts_with("conflict:"));
        assert_eq!(state.inbox_unread_count(&target).unwrap(), 4);
    }

    #[tokio::test]
    async fn peer_remote_prompt_marks_source_for_queue_direct_and_steer() {
        let (_dir, state) = setup();
        for delivery in ["queue", "direct", "steer"] {
            let target = thread(&state).await;
            if delivery == "steer" {
                state.db.with(|conn| {
                    conn.execute("UPDATE threads SET status='running' WHERE id=?1", [&target])?;
                    conn.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES ('active-turn',?1,'inProgress',1)", [&target])?;
                    Ok(())
                }).unwrap();
            }
            let receipt = state
                .send_to_thread_from_peer(
                    &target,
                    input(delivery),
                    sender("device-a", Some("foreign-thread")),
                )
                .unwrap();
            assert_eq!(
                receipt["delivery"],
                if delivery == "steer" {
                    "steer"
                } else {
                    "queued"
                }
            );
            let prompt: (String, String) = state
                .db
                .with(|conn| {
                    Ok(conn.query_row(
                "SELECT display_prompt,submitted_prompt FROM thread_pending_steers WHERE id=?1",
                [receipt["pendingSteerId"].as_str().unwrap()], |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
                })
                .unwrap();
            let reason = if delivery == "queue" {
                ""
            } else {
                "\nImmediate handling needed: the remote task depends on this now"
            };
            let expected = format!("[Pockymoe task from device-a/foreign-thread (device \"Peer laptop\") | Review]\nIn reply to message prior-message{reason}\nhello peer");
            assert_eq!(prompt.0, expected);
            assert_eq!(prompt.1, expected);
        }
    }

    #[tokio::test]
    async fn peer_remote_notifications_write_outbox_once_for_each_terminal_status() {
        let (_dir, state) = setup();
        let target = thread(&state).await;
        let local_from = thread(&state).await;
        for (index, status) in ["completed", "failed", "interrupted"].iter().enumerate() {
            let mut body = input("queue");
            body.notify_on_complete = true;
            body.client_request_id = Some(format!("completion-{index}"));
            let remote = state
                .send_to_thread_from_peer(
                    &target,
                    body.clone(),
                    sender("device-a", Some(&local_from)),
                )
                .unwrap();
            body.from_thread_id = Some(local_from.clone());
            let local = state.send_to_thread(&target, body).unwrap();
            let turn = Uuid::new_v4().to_string();
            let now = "2026-10-05T00:00:00.000Z";
            let subscription: Value = serde_json::from_str(
                &state
                    .db
                    .get_kv(&format!(
                        "cli:notify:pending:{}",
                        remote["pendingSteerId"].as_str().unwrap()
                    ))
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(subscription["deviceId"], "device-a");
            assert_eq!(subscription["deviceName"], "Peer laptop");
            state.db.with(|conn| {
                conn.execute("INSERT INTO thread_turns(id,thread_id,status,ordinal) VALUES (?1,?2,?3,?4)", params![turn,target,status,index as i64])?;
                conn.execute("INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES (?1,?2,?1,'closing',?3,?4,?4)", params![turn,target,json!({"kind":"agentMessage","text":"字".repeat(4100)}).to_string(),now])?;
                super::super::bind_notification(conn, remote["pendingSteerId"].as_str().unwrap(), &turn)?;
                super::super::bind_notification(conn, local["pendingSteerId"].as_str().unwrap(), &turn)?;
                super::super::finish_notification(conn, &target, &turn, status, now)?;
                super::super::finish_notification(conn, &target, &turn, status, now)?;
                Ok(())
            }).unwrap();
        }
        assert_eq!(state.inbox_unread_count(&local_from).unwrap(), 3);
        let records: Vec<Value> = state
            .db
            .with(|conn| {
                let mut stmt =
                    conn.prepare("SELECT value FROM kv WHERE key GLOB 'peer:outbox:*'")?;
                let raw = stmt
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(raw
                    .iter()
                    .map(|s| serde_json::from_str(s).unwrap())
                    .collect())
            })
            .unwrap();
        assert_eq!(records.len(), 3);
        let local_mail = state.inbox_list(&local_from, &json!({})).unwrap();
        for record in records {
            assert_eq!(record["targetDeviceId"], "device-a");
            assert_eq!(record["request"]["threadId"], local_from);
            assert_eq!(record["request"]["fromThreadId"], target);
            assert_eq!(record["request"]["operation"], "send");
            assert_eq!(record["request"]["kind"], "result");
            assert_eq!(record["request"]["delivery"], "inbox");
            assert_eq!(record["request"]["notifyOnComplete"], false);
            assert_eq!(
                record["request"]["clientRequestId"],
                format!("outbox-{}", record["id"].as_str().unwrap())
            );
            assert_eq!(record["attachments"], json!([]));
            assert_eq!(record["attempts"], 0);
            assert_eq!(record["createdAt"], record["nextAttemptAt"]);
            assert!(record["lastError"].is_null());
            assert_eq!(record["expiresAt"], "2026-10-12T00:00:00.000Z");
            // The caller is on another device, so the transcript hint is qualified.
            assert!(record["request"]["text"]
                .as_str()
                .unwrap()
                .contains(&format!("pockymoe transcript DEVICE/{target} --turn")));
            let matching = local_mail["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["subject"] == record["request"]["subject"])
                .unwrap();
            let message = state
                .inbox_read(&local_from, &json!({"messageId": matching["id"]}))
                .unwrap();
            // Identical to the local notification apart from the qualified hint.
            let remote = record["request"]["text"].as_str().unwrap();
            assert_eq!(
                remote
                    .replace(&format!("DEVICE/{target}"), &target)
                    .replace(" (DEVICE is this message's fromDeviceId)", ""),
                message["text"].as_str().unwrap()
            );
            assert_eq!(
                message["text"]
                    .as_str()
                    .unwrap()
                    .chars()
                    .filter(|c| *c == '字')
                    .count(),
                4000
            );
            assert!(state
                .db
                .get_kv(&format!("peer:outbox:{}", record["id"].as_str().unwrap()))
                .unwrap()
                .is_some());
        }
    }
}
