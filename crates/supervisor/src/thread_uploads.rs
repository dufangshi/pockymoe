//! Prompt attachments too large for one request, sent in chunks up to 1 GiB.
//! The Relay only forwards each chunk, so a file never collects there.
//! Contract: docs/prompt-attachments.md.
use crate::{
    http::{err, map_err, ApiErr},
    peer_files::{blocking, data_dir},
};
use anyhow::Result;
use axum::{
    body::to_bytes,
    extract::{Path as UrlPath, Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use pockymoe_runtime::{files, PromptAttachmentDir, Supervisor};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};
use uuid::Uuid;

const CHUNK_SIZE: usize = 4 * 1024 * 1024;
const MAX_SIZE: u64 = 1024 * 1024 * 1024;
/// A larger photo is linked as a file: `[PHOTO]` is read whole into the prompt.
const MAX_PHOTO_SIZE: u64 = 20 * 1024 * 1024;
/// Room left on the device after the file, so an upload cannot fill the disk.
const FREE_SPACE_MARGIN: u64 = 256 * 1024 * 1024;
const MAX_OPEN_UPLOADS: usize = 32;
const UPLOAD_TTL_MS: i64 = 24 * 60 * 60 * 1000;
const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60 * 60);
static LOCKS: OnceLock<Mutex<HashMap<String, Weak<Mutex<()>>>>> = OnceLock::new();

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Upload {
    thread_id: String,
    file_name: String,
    kind: String,
    size: u64,
    created_at: i64,
    /// Set by `complete`, which a client that lost the answer may repeat.
    #[serde(default)]
    token: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct BeginInput {
    name: String,
    size: u64,
    kind: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct OffsetQuery {
    offset: u64,
}

fn records(state: &Supervisor) -> PathBuf {
    data_dir(state).join("thread-uploads")
}

fn record_path(state: &Supervisor, id: &str) -> PathBuf {
    records(state).join(format!("{id}.json"))
}

fn write_record(state: &Supervisor, id: &str, upload: &Upload) -> Result<()> {
    let path = record_path(state, id);
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    fs::write(&temporary, serde_json::to_vec(upload)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

fn not_found() -> ApiErr {
    err(
        StatusCode::NOT_FOUND,
        "upload_not_found",
        "This upload no longer exists",
    )
}

/// A record only answers for the thread it was started in.
fn load(state: &Supervisor, thread: &str, id: &str) -> Result<Upload, ApiErr> {
    let id = Uuid::parse_str(id).map_err(|_| not_found())?.to_string();
    let bytes = fs::read(record_path(state, &id)).map_err(|_| not_found())?;
    let upload: Upload = serde_json::from_slice(&bytes).map_err(|_| not_found())?;
    if upload.thread_id != thread {
        return Err(not_found());
    }
    Ok(upload)
}

/// Retries of one chunk wait for the write before it instead of racing it.
fn lock_for(id: &str) -> Arc<Mutex<()>> {
    let mut locks = LOCKS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    let slot = locks.entry(id.to_owned()).or_default();
    let lock = slot.upgrade().unwrap_or_default();
    *slot = Arc::downgrade(&lock);
    lock
}

fn part_path(target: &PromptAttachmentDir, id: &str) -> PathBuf {
    target.dir.join(".uploads").join(format!("{id}.part"))
}

fn append(target: &PromptAttachmentDir, path: &Path, offset: u64, bytes: &[u8]) -> Result<u64> {
    match &target.context {
        Some(context) => {
            use base64::Engine;
            let answer = context.request(&json!({
                "op": "appendChunk",
                "root": target.root,
                "path": path,
                "offset": offset,
                "bytes": base64::engine::general_purpose::STANDARD.encode(bytes),
            }))?;
            answer["received"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("environment did not report the upload size"))
        }
        None => files::append_chunk(&target.root, path, offset, bytes),
    }
}

fn received(target: &PromptAttachmentDir, id: &str) -> Result<u64> {
    let part = part_path(target, id);
    match &target.context {
        Some(context) => {
            let answer = context.request(&json!({"op":"stat","root":target.root,"path":part}))?;
            Ok(answer["size"].as_u64().unwrap_or(0))
        }
        None => Ok(fs::metadata(part).map(|meta| meta.len()).unwrap_or(0)),
    }
}

fn remove_part(target: &PromptAttachmentDir, id: &str) -> Result<()> {
    let part = part_path(target, id);
    match &target.context {
        Some(context) => context
            .request(&json!({"op":"delete","root":target.root,"path":part}))
            .map(|_| ()),
        None => match fs::remove_file(part) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        },
    }
}

fn move_into_place(target: &PromptAttachmentDir, id: &str, file_name: &str) -> Result<()> {
    let part = part_path(target, id);
    let destination = target.dir.join(file_name);
    match &target.context {
        Some(context) => context
            .request(&json!({"op":"move","root":target.root,"path":part,"to":destination}))
            .map(|_| ()),
        None => {
            let destination = files::assert_within(&target.root, &destination)?;
            fs::rename(part, destination)?;
            Ok(())
        }
    }
}

fn out_of_space() -> ApiErr {
    err(
        StatusCode::INSUFFICIENT_STORAGE,
        "insufficient_storage",
        "The device does not have enough free disk space for this file",
    )
}

fn storage_err(error: anyhow::Error) -> ApiErr {
    let full = error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|io| io.kind() == std::io::ErrorKind::StorageFull);
    if full {
        out_of_space()
    } else {
        map_err(error)
    }
}

/// Removes what an unfinished or finished upload left behind; the finished file stays.
fn sweep(state: &Supervisor) -> Result<usize> {
    let dir = records(state);
    fs::create_dir_all(&dir)?;
    let now = Utc::now().timestamp_millis();
    let mut open = 0;
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let upload = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Upload>(&bytes).ok());
        let Some(upload) = upload else {
            let _ = fs::remove_file(&path);
            continue;
        };
        if now - upload.created_at < UPLOAD_TTL_MS {
            open += usize::from(upload.token.is_none());
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default();
        // A thread or workspace that is gone took its directory with it.
        if upload.token.is_none() {
            if let Ok(target) = state.prompt_attachment_dir(&upload.thread_id) {
                let _ = remove_part(&target, id);
            }
        }
        let _ = fs::remove_file(&path);
    }
    Ok(open)
}

/// Keeps expired partial files from waiting for the next upload to be noticed.
pub(crate) fn spawn_sweeper(state: Arc<Supervisor>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        loop {
            interval.tick().await;
            let state = state.clone();
            let result = tokio::task::spawn_blocking(move || sweep(&state)).await;
            if let Ok(Err(error)) = result {
                tracing::warn!(%error, "prompt attachment upload sweep failed");
            }
        }
    });
}

pub(crate) async fn begin(
    State(state): State<Arc<Supervisor>>,
    UrlPath(thread): UrlPath<String>,
    Json(input): Json<BeginInput>,
) -> Result<Json<Value>, ApiErr> {
    if input.size > MAX_SIZE {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            "attachment_too_large",
            "Each attachment must be 1 GB or smaller.",
        ));
    }
    blocking(move || {
        let open = sweep(&state).map_err(map_err)?;
        if open >= MAX_OPEN_UPLOADS {
            return Err(err(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_uploads",
                "Too many unfinished uploads on this device. Try again later.",
            ));
        }
        let target = state.prompt_attachment_dir(&thread).map_err(map_err)?;
        target.ensure().map_err(map_err)?;
        if target.context.is_none()
            && files::available_space(&target.dir)
                .is_some_and(|free| free < input.size.saturating_add(FREE_SPACE_MARGIN))
        {
            return Err(out_of_space());
        }
        let id = Uuid::new_v4().to_string();
        append(&target, &part_path(&target, &id), 0, &[]).map_err(storage_err)?;
        let kind = input.kind.as_deref().unwrap_or("file");
        let upload = Upload {
            thread_id: thread,
            file_name: pockymoe_runtime::prompt_attachment_file_name(&input.name),
            kind: if kind == "photo" { "photo" } else { "file" }.to_owned(),
            size: input.size,
            created_at: Utc::now().timestamp_millis(),
            token: None,
        };
        write_record(&state, &id, &upload).map_err(map_err)?;
        Ok(Json(json!({
            "uploadId": id,
            "received": 0,
            "size": upload.size,
            "chunkSize": CHUNK_SIZE,
        })))
    })
    .await
}

pub(crate) async fn status(
    State(state): State<Arc<Supervisor>>,
    UrlPath((thread, id)): UrlPath<(String, String)>,
) -> Result<Json<Value>, ApiErr> {
    blocking(move || {
        let upload = load(&state, &thread, &id)?;
        let now = if upload.token.is_some() {
            upload.size
        } else {
            let target = state.prompt_attachment_dir(&thread).map_err(map_err)?;
            received(&target, &id).map_err(map_err)?
        };
        Ok(Json(json!({
            "received": now,
            "size": upload.size,
            "token": upload.token,
        })))
    })
    .await
}

pub(crate) async fn chunk(
    State(state): State<Arc<Supervisor>>,
    UrlPath((thread, id)): UrlPath<(String, String)>,
    Query(query): Query<OffsetQuery>,
    request: Request,
) -> Result<Response, ApiErr> {
    let bytes = to_bytes(request.into_body(), CHUNK_SIZE)
        .await
        .map_err(|_| {
            err(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "An upload chunk is limited to 4 MiB",
            )
        })?;
    blocking(move || {
        let lock = lock_for(&id);
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let upload = load(&state, &thread, &id)?;
        let end = query
            .offset
            .checked_add(bytes.len() as u64)
            .filter(|end| *end <= upload.size)
            .ok_or_else(|| {
                err(
                    StatusCode::BAD_REQUEST,
                    "upload_size_mismatch",
                    "Chunk exceeds the declared file size",
                )
            })?;
        let target = state.prompt_attachment_dir(&thread).map_err(map_err)?;
        let now = if upload.token.is_some() {
            upload.size
        } else {
            append(&target, &part_path(&target, &id), query.offset, &bytes).map_err(storage_err)?
        };
        // A repeated chunk whose first copy landed reports the same end: nothing to redo.
        if now != end {
            return Ok((
                StatusCode::CONFLICT,
                Json(json!({
                    "code": "offset_conflict",
                    "message": "Upload offset does not match",
                    "received": now,
                })),
            )
                .into_response());
        }
        Ok(Json(json!({"received": now})).into_response())
    })
    .await
}

pub(crate) async fn complete(
    State(state): State<Arc<Supervisor>>,
    UrlPath((thread, id)): UrlPath<(String, String)>,
) -> Result<Response, ApiErr> {
    blocking(move || {
        let lock = lock_for(&id);
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut upload = load(&state, &thread, &id)?;
        if let Some(token) = &upload.token {
            return Ok(Json(json!({"token": token, "size": upload.size})).into_response());
        }
        let target = state.prompt_attachment_dir(&thread).map_err(map_err)?;
        let now = received(&target, &id).map_err(map_err)?;
        if now != upload.size {
            return Ok((
                StatusCode::CONFLICT,
                Json(json!({
                    "code": "offset_conflict",
                    "message": "The file has not been fully received",
                    "received": now,
                })),
            )
                .into_response());
        }
        move_into_place(&target, &id, &upload.file_name).map_err(map_err)?;
        let path = format!("./.temp/threads/{thread}/{}", upload.file_name);
        let token = if upload.kind == "photo" && upload.size <= MAX_PHOTO_SIZE {
            format!("[PHOTO {path}]")
        } else {
            format!("[FILE {path}]")
        };
        upload.token = Some(token.clone());
        write_record(&state, &id, &upload).map_err(map_err)?;
        Ok(Json(json!({"token": token, "size": upload.size})).into_response())
    })
    .await
}

pub(crate) async fn cancel(
    State(state): State<Arc<Supervisor>>,
    UrlPath((thread, id)): UrlPath<(String, String)>,
) -> Result<Json<Value>, ApiErr> {
    blocking(move || {
        let lock = lock_for(&id);
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let upload = match load(&state, &thread, &id) {
            Ok(upload) => upload,
            // Cancelling twice is not an error.
            Err(_) => return Ok(Json(json!({"cancelled": true}))),
        };
        if upload.token.is_none() {
            let target = state.prompt_attachment_dir(&thread).map_err(map_err)?;
            remove_part(&target, &id).map_err(map_err)?;
        }
        let _ = fs::remove_file(record_path(&state, &id));
        Ok(Json(json!({"cancelled": true})))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{header, Request as HttpRequest},
        routing::{post, put},
        Router,
    };
    use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Mode, Provider};
    use pockymoe_runtime::{
        fake::FakeRuntime, local_sessions::LocalSessionHomes, Database, RuntimeConfig,
    };
    use tower::ServiceExt;

    struct Fixture {
        _dir: tempfile::TempDir,
        state: Arc<Supervisor>,
        root: PathBuf,
        thread: String,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("workspace");
            fs::create_dir(&root).unwrap();
            let config = RuntimeConfig {
                treer_enabled: false,
                host_mode: Default::default(),
                mode: Mode::Local,
                host: "127.0.0.1".into(),
                port: 0,
                workspace_root: root.clone(),
                database_url: dir.path().join("state.sqlite"),
                app_name: "thread-uploads-test".into(),
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
            let state = Arc::new(
                Supervisor::new(
                    config,
                    db,
                    vec![Arc::new(FakeRuntime::new(Provider::Codex))],
                )
                .with_local_session_homes(LocalSessionHomes {
                    codex_home: dir.path().join("codex"),
                    grok_home: dir.path().join("grok"),
                    claude_home: dir.path().join("claude"),
                }),
            );
            let workspace = state
                .create_workspace(CreateWorkspaceInput {
                    abs_path: Some(root.to_string_lossy().into()),
                    git_url: None,
                    label: None,
                })
                .unwrap()
                .id;
            let thread = state
                .create_thread(CreateThreadInput {
                    workspace_id: workspace,
                    title: Some("upload".into()),
                    provider: Some(Provider::Codex),
                    agent_id: None,
                    model: "fake".into(),
                    reasoning_effort: None,
                    approval_mode: "yolo".into(),
                    parent_thread_id: None,
                })
                .await
                .unwrap()
                .id;
            Self {
                _dir: dir,
                state,
                root,
                thread,
            }
        }

        async fn call(&self, method: &str, path: &str, bytes: Vec<u8>) -> (u16, Value) {
            let router = Router::new()
                .route("/api/threads/{thread}/uploads", post(begin))
                .route(
                    "/api/threads/{thread}/uploads/{upload}",
                    put(chunk).get(status).delete(cancel),
                )
                .route(
                    "/api/threads/{thread}/uploads/{upload}/complete",
                    post(complete),
                )
                .with_state(self.state.clone());
            let request = HttpRequest::builder()
                .method(method)
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(bytes))
                .unwrap();
            let response = router.oneshot(request).await.unwrap();
            let status = response.status().as_u16();
            let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
        }

        async fn start(&self, name: &str, size: u64, kind: &str) -> (u16, Value) {
            let input = json!({"name":name,"size":size,"kind":kind});
            self.call(
                "POST",
                &format!("/api/threads/{}/uploads", self.thread),
                serde_json::to_vec(&input).unwrap(),
            )
            .await
        }

        fn path(&self, id: &str) -> String {
            format!("/api/threads/{}/uploads/{id}", self.thread)
        }
    }

    #[tokio::test]
    async fn upload_resumes_after_conflict_and_lands_in_the_thread_directory() {
        let fixture = Fixture::new().await;
        let bytes: Vec<u8> = (0..CHUNK_SIZE + 17).map(|i| (i % 251) as u8).collect();
        let (code, begun) = fixture
            .start("../report final.bin", bytes.len() as u64, "file")
            .await;
        assert_eq!(code, 200);
        assert_eq!(begun["chunkSize"], CHUNK_SIZE);
        let id = begun["uploadId"].as_str().unwrap().to_owned();
        let path = fixture.path(&id);
        let first = bytes[..CHUNK_SIZE].to_vec();
        let (code, answer) = fixture
            .call("PUT", &format!("{path}?offset=0"), first.clone())
            .await;
        assert_eq!(
            (code, answer["received"].as_u64()),
            (200, Some(CHUNK_SIZE as u64))
        );
        // The first copy of a repeated chunk landed; the repeat changes nothing.
        let (code, _) = fixture
            .call("PUT", &format!("{path}?offset=0"), first)
            .await;
        assert_eq!(code, 200);
        // A wrong offset is refused and says where to continue.
        let (code, answer) = fixture
            .call("PUT", &format!("{path}?offset=3"), b"x".to_vec())
            .await;
        assert_eq!(
            (code, answer["received"].as_u64()),
            (409, Some(CHUNK_SIZE as u64))
        );
        // Finishing early is refused.
        let finish = format!("{path}/complete");
        assert_eq!(fixture.call("POST", &finish, Vec::new()).await.0, 409);
        let (_, answer) = fixture.call("GET", &path, Vec::new()).await;
        assert_eq!(answer["received"].as_u64(), Some(CHUNK_SIZE as u64));
        fixture
            .call(
                "PUT",
                &format!("{path}?offset={CHUNK_SIZE}"),
                bytes[CHUNK_SIZE..].to_vec(),
            )
            .await;
        let (code, done) = fixture.call("POST", &finish, Vec::new()).await;
        assert_eq!(code, 200);
        let token = done["token"].as_str().unwrap();
        assert!(token.starts_with(&format!(
            "[FILE ./.temp/threads/{}/report-final-",
            fixture.thread
        )));
        let relative = token.trim_start_matches("[FILE ./").trim_end_matches(']');
        assert_eq!(fs::read(fixture.root.join(relative)).unwrap(), bytes);
        // The answer may have been lost: asking again gives the same one.
        let (code, again) = fixture.call("POST", &finish, Vec::new()).await;
        assert_eq!((code, again["token"].as_str()), (200, Some(token)));
        // The partial is gone, and the temp tree stays out of git.
        let threads = fixture.root.join(".temp/threads");
        assert_eq!(
            fs::read_to_string(threads.join(".gitignore")).unwrap(),
            "*\n"
        );
        let uploads = threads.join(&fixture.thread).join(".uploads");
        assert_eq!(fs::read_dir(uploads).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn limits_photos_threads_and_cancel() {
        let fixture = Fixture::new().await;
        assert_eq!(fixture.start("big.bin", MAX_SIZE + 1, "file").await.0, 413);
        // Exactly 1 GiB is allowed, unless the disk is too full to take it.
        let code = fixture.start("edge.bin", MAX_SIZE, "file").await.0;
        assert!(code == 200 || code == 507, "{code}");

        // A small photo goes into the prompt; one too large to read whole is linked.
        let (_, begun) = fixture.start("cat.png", 0, "photo").await;
        let id = begun["uploadId"].as_str().unwrap().to_owned();
        let finish = format!("{}/complete", fixture.path(&id));
        let (_, done) = fixture.call("POST", &finish, Vec::new()).await;
        assert!(done["token"].as_str().unwrap().starts_with("[PHOTO "));
        let (code, begun) = fixture.start("huge.png", MAX_PHOTO_SIZE + 1, "photo").await;
        if code == 200 {
            let id = begun["uploadId"].as_str().unwrap().to_owned();
            let part = fixture
                .root
                .join(".temp/threads")
                .join(&fixture.thread)
                .join(".uploads")
                .join(format!("{id}.part"));
            fs::File::options()
                .write(true)
                .open(part)
                .unwrap()
                .set_len(MAX_PHOTO_SIZE + 1)
                .unwrap();
            let finish = format!("{}/complete", fixture.path(&id));
            let (_, done) = fixture.call("POST", &finish, Vec::new()).await;
            assert!(done["token"].as_str().unwrap().starts_with("[FILE "));
        }

        // Another thread's path does not reach this upload, and too much is refused.
        let (_, begun) = fixture.start("a.bin", 3, "file").await;
        let id = begun["uploadId"].as_str().unwrap().to_owned();
        let other = format!("/api/threads/other/uploads/{id}");
        assert_eq!(fixture.call("GET", &other, Vec::new()).await.0, 404);
        let path = fixture.path(&id);
        let (code, _) = fixture
            .call("PUT", &format!("{path}?offset=0"), b"four".to_vec())
            .await;
        assert_eq!(code, 400);

        // Cancelling removes the partial and the record, and may be repeated.
        fixture
            .call("PUT", &format!("{path}?offset=0"), b"ab".to_vec())
            .await;
        let part = fixture
            .root
            .join(".temp/threads")
            .join(&fixture.thread)
            .join(".uploads")
            .join(format!("{id}.part"));
        assert!(part.exists());
        assert_eq!(fixture.call("DELETE", &path, Vec::new()).await.0, 200);
        assert!(!part.exists());
        assert_eq!(fixture.call("DELETE", &path, Vec::new()).await.0, 200);
        assert_eq!(fixture.call("GET", &path, Vec::new()).await.0, 404);
    }

    #[tokio::test]
    async fn a_worktree_thread_keeps_its_attachments_where_its_agent_runs() {
        let fixture = Fixture::new().await;
        let worktree = fixture._dir.path().join("worktree");
        fs::create_dir(&worktree).unwrap();
        fixture
            .state
            .db
            .with(|conn| {
                conn.execute(
                    "UPDATE threads SET worktree_path=?1 WHERE id=?2",
                    [worktree.to_str().unwrap(), fixture.thread.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        let (_, begun) = fixture.start("a.bin", 0, "file").await;
        let id = begun["uploadId"].as_str().unwrap().to_owned();
        let finish = format!("{}/complete", fixture.path(&id));
        let (_, done) = fixture.call("POST", &finish, Vec::new()).await;
        let relative = done["token"]
            .as_str()
            .unwrap()
            .trim_start_matches("[FILE ./")
            .trim_end_matches(']');
        assert!(worktree.join(relative).exists());
        assert!(!fixture.root.join(relative).exists());
    }

    #[tokio::test]
    async fn sweep_removes_expired_partials_and_keeps_finished_files() {
        let fixture = Fixture::new().await;
        let (_, open) = fixture.start("open.bin", 3, "file").await;
        let open = open["uploadId"].as_str().unwrap().to_owned();
        let (_, done) = fixture.start("done.bin", 0, "file").await;
        let done_id = done["uploadId"].as_str().unwrap().to_owned();
        let finish = format!("{}/complete", fixture.path(&done_id));
        let (_, finished) = fixture.call("POST", &finish, Vec::new()).await;
        let relative = finished["token"]
            .as_str()
            .unwrap()
            .trim_start_matches("[FILE ./")
            .trim_end_matches(']')
            .to_owned();
        for id in [&open, &done_id] {
            let mut upload = load(&fixture.state, &fixture.thread, id).unwrap_or_else(|_| panic!());
            upload.created_at -= UPLOAD_TTL_MS + 1;
            write_record(&fixture.state, id, &upload).unwrap();
        }
        sweep(&fixture.state).unwrap();
        assert!(!record_path(&fixture.state, &open).exists());
        assert!(!record_path(&fixture.state, &done_id).exists());
        let part = fixture
            .root
            .join(".temp/threads")
            .join(&fixture.thread)
            .join(".uploads")
            .join(format!("{open}.part"));
        assert!(!part.exists());
        assert!(fixture.root.join(relative).exists());
    }
}
