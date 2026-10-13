//! Caller-side peer routing and the durable sender-side outbox.
use crate::{peer_files, peer_link};
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Duration, Utc};
use pockymoe_runtime::Supervisor;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex, OnceLock, Weak},
};
use uuid::Uuid;

const OUTBOX: &str = "peer:outbox:";
const REMOTE_OPERATIONS: &[&str] = &[
    "info",
    "workspaces",
    "list",
    "status",
    "show",
    "transcript",
    "send",
    "backends",
    "models",
    "create",
];

#[derive(Debug, PartialEq)]
struct Device {
    id: String,
    local: bool,
}

fn local_device(state: &Supervisor, target: &str) -> bool {
    target.eq_ignore_ascii_case(&state.db.host_id)
        || peer_link::relay_identity(state).is_some_and(|identity| {
            target.eq_ignore_ascii_case(&identity.device_id)
                || target.to_lowercase() == identity.device_name.to_lowercase()
        })
}

fn directory_device(devices: &[Value], target: &str) -> Result<Device> {
    if let Some(device) = devices.iter().find(|d| d["deviceId"] == target) {
        return Ok(Device {
            id: target.into(),
            local: device["self"] == true,
        });
    }
    let matches = devices
        .iter()
        .filter(|d| {
            d["name"]
                .as_str()
                .is_some_and(|name| name.to_lowercase() == target.to_lowercase())
        })
        .collect::<Vec<_>>();
    ensure!(
        matches.len() <= 1,
        "device name is ambiguous: {target}; use its relay device ID"
    );
    let device = matches
        .first()
        .with_context(|| format!("device not found: {target}"))?;
    Ok(Device {
        id: device["deviceId"]
            .as_str()
            .context("directory returned no device ID")?
            .into(),
        local: device["self"] == true,
    })
}

async fn resolve_device(state: &Supervisor, target: &str) -> Result<Device> {
    ensure!(!target.trim().is_empty(), "deviceId is required");
    if target.eq_ignore_ascii_case(&state.db.host_id)
        || peer_link::relay_identity(state)
            .is_some_and(|identity| target.eq_ignore_ascii_case(&identity.device_id))
    {
        return Ok(Device {
            id: target.into(),
            local: true,
        });
    }
    // An explicit ID remains usable while the relay is disconnected, so sends can
    // be saved without requiring a live directory lookup first.
    if let Ok(id) = Uuid::parse_str(target) {
        return Ok(Device {
            id: id.to_string(),
            local: false,
        });
    }
    match peer_link::directory(state).await {
        Ok(devices) => directory_device(&devices, target),
        Err(error) if retryable(&error) && local_device(state, target) => Ok(Device {
            id: target.into(),
            local: true,
        }),
        Err(error) => Err(error),
    }
}

fn sender<'a>(caller: Option<&'a str>, input: &'a Value) -> Option<&'a str> {
    caller.or_else(|| input["fromThreadId"].as_str())
}

/// `None` leaves the operation to the local handler.
pub(crate) async fn intercept(
    state: &Arc<Supervisor>,
    caller: Option<&str>,
    input: &Value,
) -> Result<Option<Value>> {
    crate::host_policy::require_command(state, input)?;
    let operation = input["operation"].as_str().unwrap_or("");
    match operation {
        "devices" => {
            return Ok(Some(json!({
                "devices": peer_link::directory(state).await?,
                "peerAccess": state.peer_access_enabled(),
            })))
        }
        "peerAccess" => {
            if let Some(enabled) = input.get("enabled") {
                ensure!(
                    caller.is_none(),
                    "forbidden: changing peer access requires a machine credential"
                );
                return Ok(Some(state.set_peer_access(
                    enabled.as_bool().context("enabled must be true or false")?,
                )?));
            }
            return Ok(Some(json!({"enabled":state.peer_access_enabled()})));
        }
        "peerTrust" => {
            ensure!(
                caller.is_none(),
                "forbidden: resetting device trust requires a machine credential"
            );
            ensure!(input["reset"] == true, "device trust requires --reset");
            let device = resolve_device(
                state,
                input["deviceId"].as_str().context("deviceId is required")?,
            )
            .await?;
            ensure!(!device.local, "device trust applies to other devices");
            peer_link::reset_pin(state, &device.id)?;
            return Ok(Some(json!({"deviceId":device.id,"reset":true})));
        }
        "outbox" => return Ok(Some(json!(outbox_records(state)?))),
        _ => {}
    }
    let Some(target) = input["deviceId"].as_str() else {
        ensure!(
            !matches!(operation, "fsList" | "fsGet"),
            "deviceId is required"
        );
        ensure!(
            input.get("attachments").is_none(),
            "attachments require a target on another device"
        );
        return Ok(None);
    };
    let device = resolve_device(state, target).await?;
    let mut request = input.clone();
    request
        .as_object_mut()
        .context("CLI input must be an object")?
        .remove("deviceId");
    if device.local {
        ensure!(
            !matches!(operation, "fsList" | "fsGet"),
            "use the local filesystem for this device"
        );
        ensure!(
            input.get("attachments").is_none(),
            "use local file paths for this device"
        );
        if operation == "workspaces" {
            let workspaces = state.list_workspaces()?.iter().map(|workspace| json!({"id":workspace.id,"name":workspace.label,"absPath":workspace.abs_path})).collect::<Vec<_>>();
            return Ok(Some(json!(workspaces)));
        }
        return Ok(Some(
            crate::interaction::run(state, request, caller.map(str::to_owned)).await?,
        ));
    }
    ensure!(
        REMOTE_OPERATIONS.contains(&operation) || matches!(operation, "fsList" | "fsGet"),
        "{operation} is not available across devices"
    );
    ensure!(
        state.peer_access_enabled(),
        "peer_access_disabled: enable this device with pockymoe device access on"
    );
    let from = sender(caller, input);
    match operation {
        "fsList" | "fsGet" => {
            let workspace = input["workspaceId"]
                .as_str()
                .context("workspaceId is required")?;
            let path = input["path"].as_str().unwrap_or(".");
            return Ok(Some(if operation == "fsList" {
                peer_files::fs_list(state, &device.id, workspace, path).await?
            } else {
                peer_files::fs_get(
                    state,
                    &device.id,
                    workspace,
                    path,
                    input["out"].as_str().map(PathBuf::from),
                    from,
                )
                .await?
            }));
        }
        "send" | "create" => {
            if let Some(from) = from {
                state.get_thread(from)?;
            }
            request["fromThreadId"] = json!(from);
        }
        _ => {}
    }
    if operation == "send" {
        return Ok(Some(send(state, device.id, request).await?));
    }
    ensure!(
        input.get("attachments").is_none(),
        "attachments are only supported by send"
    );
    if operation == "create" {
        ensure!(
            request["workspaceId"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "cross-device create requires workspaceId"
        );
    }
    Ok(Some(
        peer_link::request_json(state, &device.id, "/api/peer/cli", &request).await?,
    ))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutboxRecord {
    id: String,
    target_device_id: String,
    request: Value,
    #[serde(default)]
    attachments: Vec<PathBuf>,
    created_at: DateTime<Utc>,
    attempts: u64,
    next_attempt_at: DateTime<Utc>,
    last_error: Option<String>,
    expires_at: DateTime<Utc>,
}

impl OutboxRecord {
    fn new(target: String, request: Value) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            target_device_id: target,
            request,
            attachments: Vec::new(),
            created_at: now,
            attempts: 0,
            next_attempt_at: now + retry_delay(0),
            last_error: None,
            expires_at: now + Duration::days(7),
        }
    }
    fn key(&self) -> String {
        format!("{OUTBOX}{}", self.id)
    }
    fn save(&self, state: &Supervisor) -> Result<()> {
        state.db.set_kv(&self.key(), &serde_json::to_string(self)?)
    }
}

fn staging_dir(state: &Supervisor, id: &str) -> Result<PathBuf> {
    let id = Uuid::parse_str(id)?;
    let data = state.config.database_url.parent().unwrap_or(Path::new("."));
    let data = if data.is_absolute() {
        data.to_path_buf()
    } else {
        std::env::current_dir()?.join(data)
    };
    Ok(data.join("peer-outbox").join(id.to_string()))
}

fn cleanup_staging(state: &Supervisor, id: &str) {
    if let Ok(path) = staging_dir(state, id) {
        if let Err(error) = std::fs::remove_dir_all(&path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(outbox_id = id, %error, "cannot remove staged peer attachments");
            }
        }
    }
}

fn retryable(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<peer_link::PeerError>()
        .is_some_and(peer_link::PeerError::retryable)
}

fn retry_delay(attempts: u64) -> Duration {
    Duration::seconds(match attempts {
        0 => 5,
        1 => 15,
        2 => 60,
        3 => 300,
        _ => 900,
    })
}

async fn send(state: &Supervisor, target: String, mut request: Value) -> Result<Value> {
    let body: pockymoe_runtime::interaction::SendInput = serde_json::from_value(request.clone())?;
    ensure!(
        request["threadId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "threadId is required"
    );
    ensure!(
        !body.text.trim().is_empty() && body.text.len() <= 256 * 1024,
        "text must be nonempty and at most 256 KiB"
    );
    ensure!(
        ["inbox", "direct", "queue", "steer"].contains(&body.delivery.as_str()),
        "delivery must be inbox, direct, queue or steer"
    );
    ensure!(
        body.notify_delivery == "inbox",
        "notifyDelivery must be inbox"
    );
    ensure!(
        !body.notify_on_complete || (body.delivery != "inbox" && body.from_thread_id.is_some()),
        "notifyOnComplete requires executable delivery and a caller thread"
    );
    ensure!(
        body.client_request_id
            .as_ref()
            .is_none_or(|key| !key.is_empty() && key.len() <= 128),
        "invalid clientRequestId"
    );
    ensure!(
        body.subject
            .as_ref()
            .is_none_or(|subject| !subject.trim().is_empty() && subject.chars().count() <= 120),
        "subject must be nonempty and at most 120 characters"
    );
    ensure!(
        body.kind
            .as_deref()
            .is_none_or(|kind| pockymoe_protocol::MESSAGE_KINDS.contains(&kind)),
        "invalid message kind"
    );
    ensure!(
        body.in_reply_to
            .as_ref()
            .is_none_or(|id| !id.trim().is_empty()),
        "inReplyTo must be a message id"
    );
    request["delivery"] = json!(body.delivery);
    let paths = request
        .as_object_mut()
        .unwrap()
        .remove("attachments")
        .map(|value| -> Result<Vec<PathBuf>> {
            let paths = value
                .as_array()
                .context("attachments must be an array of local paths")?;
            ensure!(paths.len() <= 20, "at most 20 attachments are allowed");
            paths
                .iter()
                .map(|path| {
                    Ok(PathBuf::from(
                        path.as_str().context("attachment must be a local path")?,
                    ))
                })
                .collect()
        })
        .transpose()?
        .unwrap_or_default();
    let mut record = OutboxRecord::new(target, request);
    if !record.request["clientRequestId"].is_string() {
        // Use one deduplication key even when a timeout hides remote acceptance.
        record.request["clientRequestId"] = json!(format!("outbox-{}", record.id));
    }
    if !paths.is_empty() {
        let dir = staging_dir(state, &record.id)?;
        match peer_files::stage(&paths, &dir) {
            Ok(files) => record.attachments = files,
            Err(error) => {
                cleanup_staging(state, &record.id);
                return Err(error);
            }
        }
    }
    match deliver(state, &mut record).await {
        Ok(mut receipt) => {
            cleanup_staging(state, &record.id);
            // The sender learns where its files landed on the target.
            if record.request["attachments"].is_array() {
                receipt["attachments"] = record.request["attachments"].clone();
            }
            Ok(receipt)
        }
        Err(error)
            if matches!(
                record.request["delivery"].as_str().unwrap_or("inbox"),
                "inbox" | "queue"
            ) && retryable(&error) =>
        {
            record.last_error = Some(error.to_string());
            record.next_attempt_at = Utc::now() + retry_delay(0);
            if let Err(error) = record.save(state) {
                cleanup_staging(state, &record.id);
                return Err(error);
            }
            Ok(
                json!({"delivery":"outboxed","outboxId":record.id,"targetDeviceId":record.target_device_id,"threadId":record.request["threadId"],"requestedDelivery":record.request["delivery"].as_str().unwrap_or("inbox"),"clientRequestId":record.request["clientRequestId"],"acceptedAt":record.created_at}),
            )
        }
        Err(error) => {
            cleanup_staging(state, &record.id);
            Err(error)
        }
    }
}

async fn deliver(state: &Supervisor, record: &mut OutboxRecord) -> Result<Value> {
    if !record.attachments.is_empty() && !record.request["attachments"].is_array() {
        let thread = record.request["threadId"]
            .as_str()
            .context("threadId is required")?;
        let uploaded =
            peer_files::upload(state, &record.target_device_id, thread, &record.attachments)
                .await?;
        let paths = uploaded
            .iter()
            .map(|file| {
                file["path"]
                    .as_str()
                    .map(|path| format!("- {path}"))
                    .context("upload returned no attachment path")
            })
            .collect::<Result<Vec<_>>>()?;
        let text = record.request["text"]
            .as_str()
            .context("text is required")?;
        record.request["text"] = json!(format!("{text}\n\nAttachments:\n{}", paths.join("\n")));
        record.request["attachments"] = json!(uploaded);
        // Preserve committed paths across uncertain acknowledgements; uploading
        // again would change the content under the same deduplication key.
        if state.db.get_kv(&record.key())?.is_some() {
            record.save(state)?;
        }
    }
    peer_link::request_json(
        state,
        &record.target_device_id,
        "/api/peer/cli",
        &record.request,
    )
    .await
}

fn outbox_records(state: &Supervisor) -> Result<Vec<OutboxRecord>> {
    state.db.with(|conn| {
        let mut statement = conn.prepare("SELECT value FROM kv WHERE key GLOB 'peer:outbox:*' ORDER BY json_extract(value,'$.createdAt'),key")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
        rows.iter().map(|raw| Ok(serde_json::from_str(raw)?)).collect()
    })
}

fn remove_record(state: &Supervisor, record: &OutboxRecord, failure: Option<&str>) -> Result<()> {
    if let Some(reason) = failure {
        if let Some(from) = record.request["fromThreadId"]
            .as_str()
            .filter(|from| state.get_thread(from).is_ok())
        {
            state.send_to_thread(
                from,
                pockymoe_runtime::interaction::SendInput {
                    text: format!(
                        "Cross-device message {} to {}/{} could not be delivered: {reason}",
                        record.id,
                        record.target_device_id,
                        record.request["threadId"].as_str().unwrap_or(""),
                        reason = reason.chars().take(4000).collect::<String>()
                    ),
                    delivery: "inbox".into(),
                    kind: Some("status".into()),
                    subject: Some("Cross-device delivery failed".into()),
                    client_request_id: Some(format!("outbox-failed-{}", record.id)),
                    notify_delivery: "inbox".into(),
                    from_thread_id: None,
                    notify_on_complete: false,
                    in_reply_to: None,
                    interrupt_reason: None,
                    topic_key: None,
                },
            )?;
        }
    }
    state.db.with(|conn| {
        conn.execute("DELETE FROM kv WHERE key=?1", [record.key()])?;
        Ok(())
    })?;
    cleanup_staging(state, &record.id);
    Ok(())
}

type DeliveryFuture<'a> = Pin<Box<dyn Future<Output = Result<Value>> + Send + 'a>>;

async fn process_outbox(
    state: &Supervisor,
    now: DateTime<Utc>,
    mut deliver: impl for<'a> FnMut(&'a Supervisor, &'a mut OutboxRecord) -> DeliveryFuture<'a>,
) -> Result<()> {
    for mut record in outbox_records(state)? {
        let attempt_at = now.max(Utc::now());
        if record.expires_at <= attempt_at {
            remove_record(state, &record, Some("message expired after 7 days"))?;
            continue;
        }
        if record.next_attempt_at > attempt_at || !state.peer_access_enabled() {
            continue;
        }
        match deliver(state, &mut record).await {
            Ok(_) => remove_record(state, &record, None)?,
            Err(error) if retryable(&error) => {
                record.attempts = record.attempts.saturating_add(1);
                record.next_attempt_at = now.max(Utc::now()) + retry_delay(record.attempts);
                record.last_error = Some(error.to_string());
                record.save(state)?;
            }
            Err(error) => remove_record(state, &record, Some(&error.to_string()))?,
        }
    }
    Ok(())
}

struct Worker {
    state: Weak<Supervisor>,
    task: tokio::task::JoinHandle<()>,
}
fn workers() -> &'static Mutex<HashMap<PathBuf, Worker>> {
    static WORKERS: OnceLock<Mutex<HashMap<PathBuf, Worker>>> = OnceLock::new();
    WORKERS.get_or_init(Default::default)
}

/// Weak ownership lets temporary supervisors be dropped normally.
pub(crate) fn start_outbox_worker(state: &Arc<Supervisor>) {
    if !state.host_capabilities.device_peer {
        return;
    }
    let mut workers = workers().lock().unwrap();
    workers.retain(|_, worker| worker.state.strong_count() > 0 && !worker.task.is_finished());
    let key = state.config.database_url.clone();
    if workers.contains_key(&key) {
        return;
    }
    let weak = Arc::downgrade(state);
    let worker_state = weak.clone();
    let task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let Some(state) = worker_state.upgrade() else {
                break;
            };
            if let Err(error) = process_outbox(&state, Utc::now(), |state, record| {
                Box::pin(deliver(state, record))
            })
            .await
            {
                tracing::warn!(%error, "peer outbox delivery failed");
            }
        }
    });
    workers.insert(key, Worker { state: weak, task });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Mode, Provider};
    use pockymoe_runtime::{
        actor::SharedRuntime, config::RuntimeConfig, db::Database, fake::FakeRuntime,
        local_sessions::LocalSessionHomes,
    };
    use tempfile::TempDir;

    fn test_state() -> (TempDir, Arc<Supervisor>) {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            host_mode: Default::default(),
            mode: Mode::Local,
            host: "127.0.0.1".into(),
            port: 0,
            workspace_root: dir.path().join("workspaces"),
            database_url: dir.path().join("supervisor.sqlite"),
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
        };
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let db = Database::open(&config.database_url).unwrap();
        let runtime = Arc::new(FakeRuntime::new(Provider::Codex)) as SharedRuntime;
        let state = Supervisor::new(config, db, vec![runtime]).with_local_session_homes(
            LocalSessionHomes {
                codex_home: dir.path().join("codex-home"),
                grok_home: dir.path().join("grok-home"),
                claude_home: dir.path().join("claude-home"),
            },
        );
        (dir, Arc::new(state))
    }

    async fn caller_thread(state: &Supervisor) -> String {
        let workspace = state
            .create_workspace(CreateWorkspaceInput {
                abs_path: Some(state.config.workspace_root.to_string_lossy().into()),
                git_url: None,
                label: None,
            })
            .unwrap();
        state
            .create_thread(CreateThreadInput {
                workspace_id: workspace.id,
                title: None,
                provider: Some(Provider::Codex),
                agent_id: None,
                model: "ios-e2e-stream".into(),
                reasoning_effort: None,
                approval_mode: "yolo".into(),
                parent_thread_id: None,
            })
            .await
            .unwrap()
            .id
    }

    #[test]
    fn device_names_require_unique_case_insensitive_matches() {
        let first = Uuid::new_v4().to_string();
        let second = Uuid::new_v4().to_string();
        let devices = vec![
            json!({"deviceId":first,"name":"Treer","self":false}),
            json!({"deviceId":second,"name":"Desktop","self":true}),
        ];
        assert_eq!(
            directory_device(&devices, "tREeR").unwrap(),
            Device {
                id: first.clone(),
                local: false
            }
        );
        assert_eq!(directory_device(&devices, &first).unwrap().id, first);
        assert!(directory_device(&devices, "desktop").unwrap().local);
        assert!(directory_device(&devices, "missing")
            .unwrap_err()
            .to_string()
            .contains("not found"));
        let mut ambiguous = devices;
        ambiguous.push(json!({"deviceId":Uuid::new_v4(),"name":"TREER","self":false}));
        assert!(directory_device(&ambiguous, "treer")
            .unwrap_err()
            .to_string()
            .contains("ambiguous"));
        assert_eq!(directory_device(&ambiguous, &first).unwrap().id, first);
    }

    #[tokio::test]
    async fn intercept_keeps_local_calls_and_restricts_remote_operations() {
        let (_dir, state) = test_state();
        assert!(intercept(&state, None, &json!({"operation":"info"}))
            .await
            .unwrap()
            .is_none());
        let info = intercept(
            &state,
            None,
            &json!({"operation":"info","deviceId":state.db.host_id}),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(info["deviceId"], state.db.host_id);
        assert!(info.get("relayDeviceId").is_some());
        assert!(info.get("deviceName").is_some());
        assert!(intercept(
            &state,
            None,
            &json!({"operation":"fsList","deviceId":state.db.host_id,"workspaceId":"ws"})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("local filesystem"));
        let remote = Uuid::new_v4().to_string();
        assert!(
            intercept(&state, None, &json!({"operation":"list","deviceId":remote}))
                .await
                .unwrap_err()
                .to_string()
                .contains("peer_access_disabled")
        );
        state.set_peer_access(true).unwrap();
        for operation in [
            "info",
            "workspaces",
            "list",
            "status",
            "show",
            "transcript",
            "backends",
            "models",
            "create",
        ] {
            let error = intercept(&state, None, &json!({"operation":operation,"deviceId":remote,"workspaceId":"ws","threadId":Uuid::new_v4()})).await.unwrap_err();
            assert!(
                matches!(
                    error.downcast_ref::<peer_link::PeerError>(),
                    Some(peer_link::PeerError::RelayUnavailable)
                ),
                "{operation}: {error}"
            );
        }
        for operation in [
            "wait",
            "wake",
            "tree",
            "taskAdd",
            "close",
            "delete",
            "inbox",
            "inboxWait",
            "roles",
            "resolve",
        ] {
            let error = intercept(
                &state,
                None,
                &json!({"operation":operation,"deviceId":remote}),
            )
            .await
            .unwrap_err();
            assert!(
                error.to_string().contains("not available across devices"),
                "{operation}: {error}"
            );
        }
        assert!(outbox_records(&state).unwrap().is_empty());
    }

    #[tokio::test]
    async fn intercept_requires_machine_credentials_for_access_and_trust() {
        let (_dir, state) = test_state();
        assert_eq!(
            intercept(&state, Some("caller"), &json!({"operation":"peerAccess"}))
                .await
                .unwrap()
                .unwrap()["enabled"],
            false
        );
        for input in [
            json!({"operation":"peerAccess","enabled":true}),
            json!({"operation":"peerTrust","deviceId":Uuid::new_v4(),"reset":true}),
        ] {
            assert!(intercept(&state, Some("caller"), &input)
                .await
                .unwrap_err()
                .to_string()
                .contains("machine credential"));
        }
        assert!(!state.peer_access_enabled());
        intercept(
            &state,
            None,
            &json!({"operation":"peerAccess","enabled":true}),
        )
        .await
        .unwrap();
        assert!(state.peer_access_enabled());
        assert!(intercept(
            &state,
            None,
            &json!({"operation":"peerAccess","enabled":"yes"})
        )
        .await
        .is_err());
        assert!(intercept(
            &state,
            None,
            &json!({"operation":"peerTrust","deviceId":Uuid::new_v4()})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("--reset"));
    }

    #[tokio::test]
    async fn intercept_outboxes_only_retryable_inbox_and_queue_and_binds_sender() {
        let (_dir, state) = test_state();
        state.set_peer_access(true).unwrap();
        let from = caller_thread(&state).await;
        let remote = Uuid::new_v4().to_string();
        let thread = Uuid::new_v4().to_string();
        for delivery in ["inbox", "queue"] {
            let receipt = intercept(&state, Some(&from), &json!({"operation":"send","deviceId":remote,"threadId":thread,"text":"hello","delivery":delivery,"fromThreadId":"spoofed"})).await.unwrap().unwrap();
            assert_eq!(receipt["delivery"], "outboxed");
            let raw = state
                .db
                .get_kv(&format!(
                    "{OUTBOX}{}",
                    receipt["outboxId"].as_str().unwrap()
                ))
                .unwrap()
                .unwrap();
            let record: OutboxRecord = serde_json::from_str(&raw).unwrap();
            assert_eq!(record.request["fromThreadId"], from);
            assert!(record.request.get("deviceId").is_none());
            assert_eq!(
                record.request["clientRequestId"],
                format!("outbox-{}", record.id)
            );
            assert!(record.next_attempt_at - record.created_at >= Duration::seconds(5));
            assert_eq!(record.expires_at - record.created_at, Duration::days(7));
            assert_eq!(record.attempts, 0);
        }
        for delivery in ["direct", "steer"] {
            let error = intercept(&state, None, &json!({"operation":"send","deviceId":remote,"threadId":thread,"text":"hello","delivery":delivery})).await.unwrap_err();
            assert!(retryable(&error));
        }
        let receipt = intercept(&state, None, &json!({"operation":"send","deviceId":remote,"threadId":thread,"text":"default inbox","clientRequestId":"user-key"})).await.unwrap().unwrap();
        assert_eq!(receipt["clientRequestId"], "user-key");
        assert_eq!(receipt["requestedDelivery"], "inbox");
        assert_eq!(
            intercept(&state, None, &json!({"operation":"outbox"}))
                .await
                .unwrap()
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert!(intercept(&state, None, &json!({"operation":"send","deviceId":remote,"threadId":thread,"text":"hello","notifyOnComplete":true})).await.is_err());
        assert_eq!(outbox_records(&state).unwrap().len(), 3);
        assert!(!retryable(
            &peer_link::PeerError::IdentityChanged(remote.clone()).into()
        ));
        assert!(!retryable(
            &peer_link::PeerError::Remote {
                status: 403,
                code: "peer_access_disabled".into(),
                message: "disabled".into()
            }
            .into()
        ));
        assert!(!retryable(&anyhow::anyhow!("local staging failure")));
    }

    #[tokio::test]
    async fn intercept_validates_attachments_and_remote_file_targets() {
        let (_dir, state) = test_state();
        state.set_peer_access(true).unwrap();
        let remote = Uuid::new_v4().to_string();
        for attachments in [json!(vec!["file"; 21]), json!([42]), json!("file")] {
            assert!(intercept(&state, None, &json!({"operation":"send","deviceId":remote,"threadId":Uuid::new_v4(),"text":"files","attachments":attachments})).await.is_err());
        }
        for operation in ["fsList", "fsGet"] {
            assert!(intercept(&state, None, &json!({"operation":operation}))
                .await
                .unwrap_err()
                .to_string()
                .contains("deviceId"));
            assert!(intercept(
                &state,
                None,
                &json!({"operation":operation,"deviceId":remote})
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("workspaceId"));
        }
        assert!(outbox_records(&state).unwrap().is_empty());
    }

    #[tokio::test]
    async fn outbox_schedules_due_records_and_backs_off_before_removing_success() {
        let (_dir, state) = test_state();
        state.set_peer_access(true).unwrap();
        let mut record = OutboxRecord::new(
            Uuid::new_v4().to_string(),
            json!({"operation":"send","threadId":Uuid::new_v4(),"text":"hello","delivery":"queue","clientRequestId":"stable"}),
        );
        let mut now = Utc::now() + Duration::days(1);
        record.created_at = now;
        record.expires_at = now + Duration::days(7);
        record.next_attempt_at = now + Duration::seconds(5);
        let dir = staging_dir(&state, &record.id).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file"), "staged").unwrap();
        record.save(&state).unwrap();
        process_outbox(&state, now, |_, _| panic!("not due yet"))
            .await
            .unwrap();
        for delay in [15, 60, 300, 900, 900] {
            now = outbox_records(&state).unwrap()[0].next_attempt_at;
            process_outbox(&state, now, |_, _| {
                Box::pin(async { Err(peer_link::PeerError::Offline("remote".into()).into()) })
            })
            .await
            .unwrap();
            record = outbox_records(&state).unwrap().remove(0);
            assert_eq!(record.next_attempt_at - now, Duration::seconds(delay));
            assert_eq!(record.request["clientRequestId"], "stable");
            assert!(record.last_error.as_deref().unwrap().contains("offline"));
        }
        assert_eq!(record.attempts, 5);
        state.set_peer_access(false).unwrap();
        process_outbox(&state, record.next_attempt_at, |_, _| {
            panic!("access disabled")
        })
        .await
        .unwrap();
        assert_eq!(outbox_records(&state).unwrap()[0].attempts, 5);
        state.set_peer_access(true).unwrap();
        process_outbox(&state, record.next_attempt_at, |_, _| {
            Box::pin(async { Ok(json!({"delivery":"queued"})) })
        })
        .await
        .unwrap();
        assert!(outbox_records(&state).unwrap().is_empty());
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn outbox_expiry_and_permanent_errors_mail_passive_failure_and_clean_files() {
        let (_dir, state) = test_state();
        let from = caller_thread(&state).await;
        let now = Utc::now();
        let mut expired = OutboxRecord::new(
            Uuid::new_v4().to_string(),
            json!({"operation":"send","threadId":Uuid::new_v4(),"text":"hello","delivery":"inbox","fromThreadId":from}),
        );
        expired.created_at = now - Duration::days(7);
        expired.expires_at = now;
        expired.next_attempt_at = now + Duration::hours(1);
        let dir = staging_dir(&state, &expired.id).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file"), "staged").unwrap();
        expired.save(&state).unwrap();
        process_outbox(&state, now, |_, _| panic!("expired records never send"))
            .await
            .unwrap();
        assert!(!dir.exists());
        let mut permanent = OutboxRecord::new(Uuid::new_v4().to_string(), expired.request.clone());
        permanent.next_attempt_at = now;
        permanent.save(&state).unwrap();
        state.set_peer_access(true).unwrap();
        process_outbox(&state, now, |_, _| {
            Box::pin(async { Err(peer_link::PeerError::IdentityChanged("remote".into()).into()) })
        })
        .await
        .unwrap();
        assert!(outbox_records(&state).unwrap().is_empty());
        let inbox = state.inbox_list(&from, &json!({})).unwrap();
        let messages = inbox["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|m| m["kind"] == "status"));
        assert!(messages
            .iter()
            .any(|m| m["preview"].as_str().unwrap().contains("expired")));
        assert!(messages
            .iter()
            .any(|m| m["preview"].as_str().unwrap().contains("identity changed")));
        let pending: i64 = state
            .db
            .with(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM thread_pending_steers", [], |r| {
                        r.get(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(pending, 0);
        let mut deleted_sender = permanent;
        deleted_sender.request["fromThreadId"] = json!(Uuid::new_v4());
        deleted_sender.save(&state).unwrap();
        process_outbox(&state, now, |_, _| {
            Box::pin(async { Err(anyhow::anyhow!("permanent local failure")) })
        })
        .await
        .unwrap();
        assert!(outbox_records(&state).unwrap().is_empty());
    }

    #[tokio::test]
    async fn outbox_reuses_committed_attachment_paths_after_lost_acknowledgement() {
        let (_dir, state) = test_state();
        // Opted in with no tunnel: the send fails as retryable, after the upload step.
        state.set_peer_access(true).unwrap();
        let mut record = OutboxRecord::new(
            Uuid::new_v4().to_string(),
            json!({"operation":"send","threadId":Uuid::new_v4(),"text":"hello\n\nAttachments:\n- /remote/incoming/file","attachments":[{"path":"/remote/incoming/file"}],"clientRequestId":"stable"}),
        );
        record
            .attachments
            .push(staging_dir(&state, &record.id).unwrap().join("file"));
        let original = record.request.clone();
        let error = deliver(&state, &mut record).await.unwrap_err();
        assert!(
            retryable(&error),
            "should reach send without uploading again: {error}"
        );
        assert_eq!(record.request, original);
    }

    #[tokio::test]
    async fn host_mode_does_not_start_outbox_worker() {
        let (_dir, mut state) = test_state();
        let state_mut = Arc::get_mut(&mut state).unwrap();
        state_mut.config.host_mode = pockymoe_runtime::host_mode::HostMode::Hosted;
        state_mut.host_capabilities = state_mut.config.host_mode.capabilities();
        start_outbox_worker(&state);
        assert!(!workers()
            .lock()
            .unwrap()
            .contains_key(&state.config.database_url));
    }

    #[tokio::test]
    async fn outbox_worker_is_idempotent_per_state_and_does_not_retain_it() {
        let (_dir, state) = test_state();
        let (_other_dir, other) = test_state();
        start_outbox_worker(&state);
        let key = state.config.database_url.clone();
        let task = workers().lock().unwrap()[&key].task.id();
        start_outbox_worker(&state);
        assert_eq!(workers().lock().unwrap()[&key].task.id(), task);
        start_outbox_worker(&other);
        assert_ne!(
            workers().lock().unwrap()[&other.config.database_url]
                .task
                .id(),
            task
        );
        let weak = Arc::downgrade(&state);
        drop(state);
        assert!(weak.upgrade().is_none());
    }
    #[tokio::test]
    async fn strict_peers_send_read_files_and_deliver_offline_outbox_with_fresh_signatures() {
        use crate::{
            auth::PeerCaller, owner_trust::tests::bound_peer, peer_link::tests::connected,
            tunnel::forward_local,
        };
        use std::sync::atomic::{AtomicBool, Ordering};
        let (_a_dir, a) = test_state();
        let (_b_dir, b) = test_state();
        for (state, id) in [(&a, "caller"), (&b, "target")] {
            state.set_peer_access(true).unwrap();
            bound_peer(state, id);
        }
        let a_thread = caller_thread(&a).await;
        let b_thread = caller_thread(&b).await;
        let b_workspace = b.get_thread(&b_thread).unwrap().workspace_id;
        let a_workspace = a.get_thread(&a_thread).unwrap().workspace_id;
        std::fs::write(b.config.workspace_root.join("peer.txt"), b"from target").unwrap();
        std::fs::write(a.config.workspace_root.join("peer.txt"), b"from caller").unwrap();
        let online = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(Mutex::new(Vec::new()));
        let (a_link, mut a_rx) = connected(&a, "caller");
        let (b_link, mut b_rx) = connected(&b, "target");
        let task_a = {
            let b = b.clone();
            let online = online.clone();
            let frames = frames.clone();
            tokio::spawn(async move {
                while let Some(frame) = a_rx.recv().await {
                    assert_eq!(frame["targetDeviceId"], "target");
                    frames.lock().unwrap().push(frame["payload"].clone());
                    let response = if online.load(Ordering::SeqCst) {
                        forward_local(
                            &b,
                            frame["payload"].clone(),
                            Some(PeerCaller {
                                device_id: "caller".into(),
                                device_name: "Caller".into(),
                                user_id: "owner".into(),
                            }),
                        )
                        .await
                    } else {
                        json!({"statusCode":503,"headers":{},"body":"offline"})
                    };
                    a_link.receive(&json!({"type":"peer.response","requestId":frame["requestId"],"payload":response}));
                }
            })
        };
        let task_b = {
            let a = a.clone();
            tokio::spawn(async move {
                while let Some(frame) = b_rx.recv().await {
                    assert_eq!(frame["targetDeviceId"], "caller");
                    let response = forward_local(
                        &a,
                        frame["payload"].clone(),
                        Some(PeerCaller {
                            device_id: "target".into(),
                            device_name: "Target".into(),
                            user_id: "owner".into(),
                        }),
                    )
                    .await;
                    b_link.receive(&json!({"type":"peer.response","requestId":frame["requestId"],"payload":response}));
                }
            })
        };
        let body = json!({"operation":"send","threadId":b_thread,"text":"offline hello","delivery":"inbox","fromThreadId":a_thread,"kind":"task","subject":"Review"});
        let receipt = send(&a, "target".into(), body).await.unwrap();
        assert_eq!(receipt["delivery"], "outboxed");
        let pending = outbox_records(&a).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(!serde_json::to_string(&pending[0])
            .unwrap()
            .contains("x-rcd-peer-signature"));
        online.store(true, Ordering::SeqCst);
        process_outbox(&a, Utc::now() + Duration::minutes(1), |state, record| {
            Box::pin(deliver(state, record))
        })
        .await
        .unwrap();
        assert!(outbox_records(&a).unwrap().is_empty());
        assert_eq!(b.inbox_unread_count(&b_thread).unwrap(), 1);
        let sent: Vec<Value> = frames
            .lock()
            .unwrap()
            .iter()
            .filter(|f| f["path"] == "/api/peer/cli")
            .cloned()
            .collect();
        assert_eq!(sent.len(), 2); // registration-required response, then freshly signed retry
        assert_ne!(
            sent[0]["headers"]["x-rcd-request"],
            sent[1]["headers"]["x-rcd-request"]
        );
        assert_ne!(
            sent[0]["headers"]["x-rcd-peer-signature"],
            sent[1]["headers"]["x-rcd-peer-signature"]
        );
        assert_eq!(sent[1]["headers"]["x-rcd-peer-device"], "caller");
        for (source, target, thread, workspace, expected) in [
            (
                &a,
                "target",
                &b_thread,
                &b_workspace,
                b"from target".as_slice(),
            ),
            (
                &b,
                "caller",
                &a_thread,
                &a_workspace,
                b"from caller".as_slice(),
            ),
        ] {
            let receipt = peer_link::request_json(source,target,"/api/peer/cli",&json!({"operation":"send","threadId":thread,"text":"online hello","delivery":"inbox","kind":"task","subject":"Review"})).await.unwrap();
            assert_eq!(receipt["delivery"], "inbox");
            let file = peer_link::request(
                source,
                target,
                "GET",
                &format!("/api/peer/files/read?workspaceId={workspace}&path=peer.txt"),
                None,
                vec![],
            )
            .await
            .unwrap();
            assert_eq!(file.status, 206);
            assert_eq!(file.body, expected);
        }
        task_a.abort();
        task_b.abort();
        let _ = task_a.await;
        let _ = task_b.await;
    }
}
