//! Chunked attachments and read-only workspace files between devices of one owner.
//! Contract: docs/cross-device-peer.zh.md.
use crate::{
    auth::PeerCaller,
    http::{err, map_err, ApiErr},
    peer_link::{self, PeerError, PeerResponse},
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use axum::{
    body::{to_bytes, Body},
    extract::{Path as UrlPath, Query, Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use pockymoe_runtime::{files, interaction::AgentRequester, Supervisor};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, Metadata, OpenOptions},
    future::Future,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use uuid::Uuid;

const CHUNK_SIZE: usize = 4 * 1024 * 1024;
const READ_SIZE: usize = 1024 * 1024;
const MAX_SIZE: u64 = 1024 * 1024 * 1024;
const UPLOAD_TTL: i64 = 24 * 60 * 60 * 1000;
const RECEIPT_TTL: i64 = 7 * UPLOAD_TTL;
static UPLOAD_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Upload {
    thread_id: String,
    name: String,
    size: u64,
    sha256: String,
    device_id: String,
    user_id: String,
    created_at: i64,
    #[serde(default)]
    committed: Option<Value>,
}

#[derive(Deserialize)]
pub(crate) struct OffsetQuery {
    offset: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReadQuery {
    workspace_id: String,
    path: String,
    offset: Option<u64>,
    length: Option<usize>,
}

fn require_peer(
    state: &Supervisor,
    caller: Option<Extension<PeerCaller>>,
) -> Result<PeerCaller, ApiErr> {
    let caller = caller.ok_or_else(|| {
        err(
            StatusCode::FORBIDDEN,
            "peer_forbidden",
            "A peer device identity is required",
        )
    })?;
    if !state.peer_access_enabled() {
        return Err(err(
            StatusCode::FORBIDDEN,
            "peer_access_disabled",
            "Peer access is disabled on this device",
        ));
    }
    state.ensure_peer_reaches_device().map_err(map_err)?;
    caller.0.ensure_source_private()?;
    Ok(caller.0)
}

fn require_local_access(state: &Supervisor) -> Result<()> {
    ensure!(
        state.peer_access_enabled(),
        "peer_access_disabled: Peer access is disabled on this device"
    );
    Ok(())
}

fn data_dir(state: &Supervisor) -> &Path {
    state
        .config
        .database_url
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn upload_lock(state: &Supervisor) -> Result<Arc<Mutex<()>>, ApiErr> {
    let mut locks = UPLOAD_LOCKS
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| map_err(anyhow!("upload lock failed")))?;
    locks.retain(|_, lock| lock.strong_count() > 0);
    let slot = locks.entry(state.config.database_url.clone()).or_default();
    let lock = slot.upgrade().unwrap_or_else(|| Arc::new(Mutex::new(())));
    *slot = Arc::downgrade(&lock);
    Ok(lock)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ApiErr> + Send + 'static,
) -> Result<T, ApiErr> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| map_err(error.into()))?
}

fn basename(name: &str) -> Result<String> {
    let name = name.rsplit(['/', '\\']).next().unwrap_or("");
    ensure!(
        !name.is_empty()
            && name != "."
            && name != ".."
            && !name.chars().any(char::is_control)
            && name.len() <= 255,
        "name must have a nonempty basename of at most 255 bytes"
    );
    Ok(name.to_owned())
}

fn sha256(value: &str) -> Result<String> {
    ensure!(
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "sha256 must contain 64 hexadecimal digits"
    );
    Ok(value.to_ascii_lowercase())
}

fn upload_id(value: &str) -> Result<String, ApiErr> {
    Uuid::parse_str(value)
        .map(|id| id.to_string())
        .map_err(|_| {
            err(
                StatusCode::BAD_REQUEST,
                "invalid_upload_id",
                "Invalid upload ID",
            )
        })
}

fn thread_cwd(state: &Supervisor, id: &str) -> Result<PathBuf> {
    let thread = state.get_thread(id)?;
    Ok(match thread.worktree_path {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(state.get_workspace(&thread.workspace_id)?.abs_path),
    })
}

fn incoming_path(state: &Supervisor, id: &str, upload: &Upload) -> Result<(PathBuf, PathBuf)> {
    let root = thread_cwd(state, &upload.thread_id)?.canonicalize()?;
    let relative = PathBuf::from(".temp/threads")
        .join(&upload.thread_id)
        .join("incoming")
        .join(id)
        .join(&upload.name);
    let path = root.join(&relative);
    // A symlink to another place inside the workspace is also outside incoming.
    ensure!(
        files::assert_within(&root, &path)? == path,
        "incoming path must not contain symbolic links"
    );
    Ok((path, relative))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn expired(upload: &Upload) -> bool {
    let ttl = if upload.committed.is_some() {
        RECEIPT_TTL
    } else {
        UPLOAD_TTL
    };
    Utc::now()
        .timestamp_millis()
        .saturating_sub(upload.created_at)
        >= ttl
}

fn cleanup_uploads(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("json") => {
                let upload: Upload = serde_json::from_slice(&fs::read(&path)?)?;
                if expired(&upload) {
                    let _ = fs::remove_file(path.with_extension("part"));
                    fs::remove_file(path)?;
                }
            }
            Some("part") if !path.with_extension("json").exists() => {
                let metadata = match fs::metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                };
                let modified: DateTime<Utc> = metadata.modified()?.into();
                if Utc::now().timestamp_millis() - modified.timestamp_millis() >= UPLOAD_TTL {
                    fs::remove_file(path)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn load_upload(state: &Supervisor, id: &str, caller: &PeerCaller) -> Result<Upload, ApiErr> {
    let dir = data_dir(state).join("peer-uploads");
    let bytes = fs::read(dir.join(format!("{id}.json"))).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            err(
                StatusCode::NOT_FOUND,
                "upload_not_found",
                "Upload not found",
            )
        } else {
            map_err(error.into())
        }
    })?;
    let upload: Upload = serde_json::from_slice(&bytes).map_err(|error| map_err(error.into()))?;
    if upload.device_id != caller.device_id || upload.user_id != caller.user_id {
        return Err(err(
            StatusCode::FORBIDDEN,
            "peer_forbidden",
            "Upload belongs to another peer device",
        ));
    }
    if expired(&upload) {
        let _ = fs::remove_file(dir.join(format!("{id}.part")));
        let _ = fs::remove_file(dir.join(format!("{id}.json")));
        return Err(err(
            StatusCode::NOT_FOUND,
            "upload_not_found",
            "Upload expired",
        ));
    }
    Ok(upload)
}

fn received(state: &Supervisor, id: &str, upload: &Upload) -> Result<u64, ApiErr> {
    if upload.committed.is_some() {
        return Ok(upload.size);
    }
    let part = data_dir(state)
        .join("peer-uploads")
        .join(format!("{id}.part"));
    match fs::metadata(part) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let (path, _) = incoming_path(state, id, upload).map_err(map_err)?;
            let (size, hash, _) = digest_file(&path).map_err(map_err)?;
            if size != upload.size || hash != upload.sha256 {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "upload_size_mismatch",
                    "Completed upload does not match",
                ));
            }
            Ok(size)
        }
        Err(error) => Err(map_err(error.into())),
    }
}

pub(crate) async fn upload_begin(
    State(state): State<Arc<Supervisor>>,
    caller: Option<Extension<PeerCaller>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    let caller = require_peer(&state, caller)?;
    let lock = upload_lock(&state)?;
    blocking(move || {
        let _guard = lock.lock().map_err(|_| map_err(anyhow!("upload lock failed")))?;
        let thread_id = input["threadId"].as_str().context("threadId required").map_err(map_err)?;
        state
            .ensure_agent_reaches_thread(AgentRequester::Peer, thread_id)
            .map_err(map_err)?;
        thread_cwd(&state, thread_id).map_err(map_err)?;
        let name = basename(input["name"].as_str().context("name required").map_err(map_err)?).map_err(map_err)?;
        let size = input["size"].as_u64().context("size must be a nonnegative integer").map_err(map_err)?;
        if size > MAX_SIZE { return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", "Upload is limited to 1 GiB")); }
        let hash = sha256(input["sha256"].as_str().context("sha256 required").map_err(map_err)?).map_err(map_err)?;
        let dir = data_dir(&state).join("peer-uploads");
        fs::create_dir_all(&dir).map_err(|error| map_err(error.into()))?;
        cleanup_uploads(&dir).map_err(map_err)?;
        let (id, upload) = if let Some(id) = input.get("uploadId").filter(|value| !value.is_null()) {
            let id = upload_id(id.as_str().context("uploadId must be a string").map_err(map_err)?)?;
            let upload = load_upload(&state, &id, &caller)?;
            if upload.thread_id != thread_id || upload.name != name || upload.size != size || upload.sha256 != hash {
                return Err(err(StatusCode::CONFLICT, "upload_mismatch", "Upload metadata does not match"));
            }
            (id, upload)
        } else {
            let id = Uuid::new_v4().to_string();
            let upload = Upload { thread_id:thread_id.to_owned(), name, size, sha256:hash, device_id:caller.device_id, user_id:caller.user_id, created_at:Utc::now().timestamp_millis(), committed:None };
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
            options.open(dir.join(format!("{id}.part"))).map_err(|error| map_err(error.into()))?;
            write_json(&dir.join(format!("{id}.json")), &upload).map_err(map_err)?;
            (id, upload)
        };
        Ok(Json(json!({"uploadId":id, "received":received(&state, &id, &upload)?, "chunkSize":CHUNK_SIZE})))
    }).await
}

pub(crate) async fn upload_chunk(
    State(state): State<Arc<Supervisor>>,
    caller: Option<Extension<PeerCaller>>,
    UrlPath(id): UrlPath<String>,
    Query(query): Query<OffsetQuery>,
    request: Request,
) -> Result<Response, ApiErr> {
    let caller = require_peer(&state, caller)?;
    let id = upload_id(&id)?;
    let body = to_bytes(request.into_body(), CHUNK_SIZE)
        .await
        .map_err(|_| {
            err(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "Upload chunk is limited to 4 MiB",
            )
        })?;
    let lock = upload_lock(&state)?;
    blocking(move || {
        let _guard = lock.lock().map_err(|_| map_err(anyhow!("upload lock failed")))?;
        let upload = load_upload(&state, &id, &caller)?;
        let current = received(&state, &id, &upload)?;
        if query.offset != current || upload.committed.is_some() {
            return Ok((StatusCode::CONFLICT, Json(json!({"code":"offset_conflict", "message":"Upload offset does not match", "received":current}))).into_response());
        }
        let next = current.checked_add(body.len() as u64).filter(|next| *next <= upload.size).ok_or_else(|| err(StatusCode::BAD_REQUEST, "upload_size_mismatch", "Chunk exceeds declared upload size"))?;
        let mut file = OpenOptions::new().append(true).open(data_dir(&state).join("peer-uploads").join(format!("{id}.part"))).map_err(|error| map_err(error.into()))?;
        file.write_all(&body).map_err(|error| map_err(error.into()))?;
        file.sync_all().map_err(|error| map_err(error.into()))?;
        Ok(Json(json!({"uploadId":id, "received":next})).into_response())
    }).await
}

fn digest_file(path: &Path) -> Result<(u64, String, Metadata)> {
    ensure!(
        fs::metadata(path)?.is_file(),
        "path must point to a regular file"
    );
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = Read::read(&mut file, &mut buffer)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        digest.update(&buffer[..count]);
    }
    Ok((size, hex::encode(digest.finalize()), metadata))
}

pub(crate) async fn upload_commit(
    State(state): State<Arc<Supervisor>>,
    caller: Option<Extension<PeerCaller>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Value>, ApiErr> {
    let caller = require_peer(&state, caller)?;
    let id = upload_id(&id)?;
    let lock = upload_lock(&state)?;
    blocking(move || {
        let _guard = lock.lock().map_err(|_| map_err(anyhow!("upload lock failed")))?;
        let mut upload = load_upload(&state, &id, &caller)?;
        if let Some(receipt) = upload.committed { return Ok(Json(receipt)); }
        state.ensure_agent_reaches_thread(AgentRequester::Peer, &upload.thread_id).map_err(map_err)?;
        let dir = data_dir(&state).join("peer-uploads");
        let part = dir.join(format!("{id}.part"));
        let (path, relative) = incoming_path(&state, &id, &upload).map_err(map_err)?;
        // A crash after the rename can leave the completed file without a receipt.
        let source = if part.exists() { &part } else { &path };
        let (size, hash, _) = digest_file(source).map_err(map_err)?;
        if size != upload.size { return Err(err(StatusCode::BAD_REQUEST, "upload_size_mismatch", "Upload size does not match")); }
        if hash != upload.sha256 { return Err(err(StatusCode::BAD_REQUEST, "hash_mismatch", "Upload SHA-256 does not match")); }
        if source == &part {
            fs::create_dir_all(path.parent().unwrap()).map_err(|error| map_err(error.into()))?;
            incoming_path(&state, &id, &upload).map_err(map_err)?;
            match fs::rename(&part, &path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                    let mut file = OpenOptions::new().write(true).create_new(true).open(&path).map_err(|error| map_err(error.into()))?;
                    if let Err(error) = std::io::copy(&mut File::open(&part).map_err(|error| map_err(error.into()))?, &mut file).and_then(|_| file.sync_all()) {
                        let _ = fs::remove_file(&path);
                        return Err(map_err(error.into()));
                    }
                    fs::remove_file(&part).map_err(|error| map_err(error.into()))?;
                }
                Err(error) => return Err(map_err(error.into())),
            }
        }
        let receipt = json!({"name":upload.name, "size":size, "sha256":hash, "path":path, "relativePath":relative.to_string_lossy().replace('\\', "/")});
        upload.committed = Some(receipt.clone());
        write_json(&dir.join(format!("{id}.json")), &upload).map_err(map_err)?;
        Ok(Json(receipt))
    }).await
}

/// A path in a workspace that another device asked for.
fn workspace_path(state: &Supervisor, id: &str, path: &str) -> Result<(PathBuf, PathBuf)> {
    state.ensure_agent_reaches_workspace(AgentRequester::Peer, id)?;
    let root = PathBuf::from(state.get_workspace(id)?.abs_path).canonicalize()?;
    let path = files::assert_within(&root, Path::new(path))?;
    Ok((root, path))
}

fn modified_at(metadata: &Metadata) -> Option<String> {
    metadata
        .modified()
        .ok()
        .map(|time| DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Millis, true))
}

pub(crate) async fn list(
    State(state): State<Arc<Supervisor>>,
    caller: Option<Extension<PeerCaller>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    require_peer(&state, caller)?;
    blocking(move || {
        let id = input["workspaceId"].as_str().context("workspaceId required").map_err(map_err)?;
        let (root, path) = workspace_path(&state, id, input["path"].as_str().unwrap_or(".")).map_err(map_err)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(path).map_err(|error| map_err(error.into()))? {
            let entry = entry.map_err(|error| map_err(error.into()))?;
            if files::assert_within(&root, &entry.path()).is_err() { continue; }
            let metadata = entry.metadata().map_err(|error| map_err(error.into()))?;
            if !metadata.is_dir() && !metadata.is_file() { continue; }
            let mut value = json!({"name":entry.file_name().to_string_lossy(), "path":entry.path().strip_prefix(&root).map_err(|error| map_err(error.into()))?.to_string_lossy().replace('\\', "/"), "kind":if metadata.is_dir() { "directory" } else { "file" }});
            if metadata.is_file() { value["size"] = json!(metadata.len()); }
            if let Some(modified) = modified_at(&metadata) { value["modifiedAt"] = json!(modified); }
            entries.push(value);
        }
        entries.sort_by_key(|entry| (entry["kind"].as_str().unwrap_or("").to_owned(), entry["name"].as_str().unwrap_or("").to_lowercase()));
        Ok(Json(json!({"entries":entries})))
    }).await
}

pub(crate) async fn stat(
    State(state): State<Arc<Supervisor>>,
    caller: Option<Extension<PeerCaller>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiErr> {
    require_peer(&state, caller)?;
    blocking(move || {
        let id = input["workspaceId"]
            .as_str()
            .context("workspaceId required")
            .map_err(map_err)?;
        let path = input["path"]
            .as_str()
            .context("path required")
            .map_err(map_err)?;
        let (_, path) = workspace_path(&state, id, path).map_err(map_err)?;
        let (size, hash, metadata) = digest_file(&path).map_err(map_err)?;
        Ok(Json(
            json!({"kind":"file", "size":size, "sha256":hash, "modifiedAt":modified_at(&metadata)}),
        ))
    })
    .await
}

pub(crate) async fn read(
    State(state): State<Arc<Supervisor>>,
    caller: Option<Extension<PeerCaller>>,
    Query(query): Query<ReadQuery>,
) -> Result<Response, ApiErr> {
    require_peer(&state, caller)?;
    blocking(move || {
        let (_, path) =
            workspace_path(&state, &query.workspace_id, &query.path).map_err(map_err)?;
        let offset = query.offset.unwrap_or(0);
        let length = query.length.unwrap_or(READ_SIZE).min(READ_SIZE);
        if length == 0 {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "invalid_length",
                "length must be positive",
            ));
        }
        if !fs::metadata(&path)
            .map_err(|error| map_err(error.into()))?
            .is_file()
        {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "not_a_file",
                "path must point to a regular file",
            ));
        }
        let mut file = File::open(path).map_err(|error| map_err(error.into()))?;
        let size = file
            .metadata()
            .map_err(|error| map_err(error.into()))?
            .len();
        if offset > size || (offset == size && size > 0) {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{size}"))],
                Body::empty(),
            )
                .into_response());
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| map_err(error.into()))?;
        let mut bytes = Vec::new();
        Read::take(file, (length as u64).min(size - offset))
            .read_to_end(&mut bytes)
            .map_err(|error| map_err(error.into()))?;
        let range = if bytes.is_empty() {
            format!("bytes */{size}")
        } else {
            format!("bytes {offset}-{}/{size}", offset + bytes.len() as u64 - 1)
        };
        Ok((
            if bytes.is_empty() {
                StatusCode::OK
            } else {
                StatusCode::PARTIAL_CONTENT
            },
            [
                (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
                (header::CONTENT_RANGE, range),
                (header::CACHE_CONTROL, "private, no-store".to_owned()),
            ],
            bytes,
        )
            .into_response())
    })
    .await
}

/// Snapshot files, reusing workspace directory downloads for zip archives.
pub(crate) fn stage(paths: &[PathBuf], dir: &Path) -> Result<Vec<PathBuf>> {
    ensure!(paths.len() <= 20, "at most 20 attachments are allowed");
    fs::create_dir_all(dir)?;
    let destination_root = dir.canonicalize()?;
    let mut staged = Vec::new();
    for (index, input) in paths.iter().enumerate() {
        let path = input.canonicalize()?;
        ensure!(
            !path.is_dir() || !destination_root.starts_with(&path),
            "staging directory must be outside the attachment directory"
        );
        let root = path.parent().unwrap_or(&path);
        let relative = path
            .strip_prefix(root)?
            .to_str()
            .context("attachment path must be UTF-8")?;
        let download =
            files::prepare_download(root, if relative.is_empty() { "." } else { relative })?;
        let (name, mut source): (String, Box<dyn Read>) = match download {
            files::WorkspaceDownload::File { file, .. } => {
                ensure!(
                    file.metadata()?.len() <= MAX_SIZE,
                    "attachment is limited to 1 GiB"
                );
                (
                    basename(
                        input
                            .file_name()
                            .and_then(|name| name.to_str())
                            .context("attachment name must be UTF-8")?,
                    )?,
                    Box::new(file),
                )
            }
            files::WorkspaceDownload::DirectoryArchive {
                filename,
                mut archive,
            } => {
                archive.as_file_mut().seek(SeekFrom::Start(0))?;
                (basename(&filename)?, Box::new(archive))
            }
        };
        let folder = destination_root.join(index.to_string());
        fs::create_dir_all(&folder)?;
        let path = files::assert_within(&destination_root, &folder.join(name))?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let size = std::io::copy(
            &mut Read::by_ref(&mut source).take(MAX_SIZE + 1),
            &mut output,
        )?;
        if size > MAX_SIZE {
            drop(output);
            fs::remove_file(&path)?;
            bail!("attachment is limited to 1 GiB");
        }
        output.sync_all()?;
        staged.push(path);
    }
    Ok(staged)
}

struct FileRequest {
    method: &'static str,
    path: String,
    content_type: Option<&'static str>,
    body: Vec<u8>,
}
trait FilePeer: Sync {
    fn request(&self, request: FileRequest) -> impl Future<Output = Result<PeerResponse>> + Send;
}
struct LinkedPeer<'a> {
    state: &'a Supervisor,
    device_id: &'a str,
}
impl FilePeer for LinkedPeer<'_> {
    async fn request(&self, request: FileRequest) -> Result<PeerResponse> {
        peer_link::request(
            self.state,
            self.device_id,
            request.method,
            &request.path,
            request.content_type,
            request.body,
        )
        .await
    }
}

fn checked(response: PeerResponse) -> Result<PeerResponse> {
    if !(200..300).contains(&response.status) {
        return Err(PeerError::from_reply(
            response.status,
            &response.body,
            "Peer file request failed",
        )
        .into());
    }
    Ok(response)
}

async fn request_json(peer: &impl FilePeer, path: &str, body: Value) -> Result<Value> {
    let response = checked(
        peer.request(FileRequest {
            method: "POST",
            path: path.to_owned(),
            content_type: Some("application/json"),
            body: serde_json::to_vec(&body)?,
        })
        .await?,
    )?;
    Ok(serde_json::from_slice(&response.body)?)
}

fn resume_path(path: &Path, device_id: &str, thread_id: &str, hash: &str) -> Result<PathBuf> {
    let key = hex::encode(Sha256::digest(serde_json::to_vec(&json!([
        device_id, thread_id, path, hash
    ]))?));
    Ok(path
        .parent()
        .context("staged attachment needs a parent directory")?
        .join(format!(".peer-upload-{key}.json")))
}

/// Upload staged files; the server's received offset is authoritative on retry.
pub(crate) async fn upload(
    state: &Supervisor,
    device_id: &str,
    thread_id: &str,
    paths: &[PathBuf],
) -> Result<Vec<Value>> {
    require_local_access(state)?;
    upload_with(
        &LinkedPeer { state, device_id },
        device_id,
        thread_id,
        paths,
    )
    .await
}

async fn upload_with(
    peer: &impl FilePeer,
    device_id: &str,
    thread_id: &str,
    paths: &[PathBuf],
) -> Result<Vec<Value>> {
    ensure!(paths.len() <= 20, "at most 20 attachments are allowed");
    let mut receipts = Vec::new();
    for path in paths {
        let name = basename(
            path.file_name()
                .and_then(|name| name.to_str())
                .context("attachment name must be UTF-8")?,
        )?;
        let hashed_path = path.clone();
        let (size, hash, _) =
            tokio::task::spawn_blocking(move || digest_file(&hashed_path)).await??;
        ensure!(size <= MAX_SIZE, "attachment is limited to 1 GiB");
        let marker = resume_path(path, device_id, thread_id, &hash)?;
        let mut input = json!({"threadId":thread_id, "name":name, "size":size, "sha256":hash});
        if let Ok(bytes) = fs::read(&marker) {
            input["uploadId"] = serde_json::from_slice::<Value>(&bytes)?["uploadId"].clone();
        }
        let begun = match request_json(peer, "/api/peer/files/uploads", input.clone()).await {
            Err(error)
                if matches!(
                    error.downcast_ref::<PeerError>(),
                    Some(PeerError::Remote { status: 404, .. })
                ) && input.get("uploadId").is_some() =>
            {
                input.as_object_mut().unwrap().remove("uploadId");
                request_json(peer, "/api/peer/files/uploads", input).await?
            }
            result => result?,
        };
        let id = begun["uploadId"]
            .as_str()
            .context("peer omitted uploadId")?;
        Uuid::parse_str(id).context("peer returned invalid uploadId")?;
        write_json(&marker, &json!({"uploadId":id}))?;
        let chunk_size = begun["chunkSize"]
            .as_u64()
            .context("peer omitted chunkSize")?;
        ensure!(
            (1..=CHUNK_SIZE as u64).contains(&chunk_size),
            "peer returned invalid chunkSize"
        );
        let mut offset = begun["received"]
            .as_u64()
            .context("peer omitted received offset")?;
        ensure!(offset <= size, "peer returned invalid received offset");
        let mut file = tokio::fs::File::open(path).await?;
        let mut conflicts = 0;
        while offset < size {
            file.seek(SeekFrom::Start(offset)).await?;
            let mut bytes = vec![0u8; (size - offset).min(chunk_size) as usize];
            file.read_exact(&mut bytes).await?;
            let expected = offset + bytes.len() as u64;
            let response = peer
                .request(FileRequest {
                    method: "PUT",
                    path: format!("/api/peer/files/uploads/{id}?offset={offset}"),
                    content_type: Some("application/octet-stream"),
                    body: bytes,
                })
                .await?;
            if response.status == 409 {
                conflicts += 1;
                ensure!(conflicts <= 3, "peer upload offset repeatedly conflicted");
                let value: Value = serde_json::from_slice(&response.body)?;
                offset = value["received"]
                    .as_u64()
                    .context("peer omitted conflict offset")?;
                ensure!(offset <= size, "peer returned invalid conflict offset");
            } else {
                let value: Value = serde_json::from_slice(&checked(response)?.body)?;
                offset = value["received"]
                    .as_u64()
                    .context("peer omitted received offset")?;
                ensure!(
                    offset == expected,
                    "peer returned unexpected received offset"
                );
            }
        }
        let receipt = request_json(
            peer,
            &format!("/api/peer/files/uploads/{id}/commit"),
            json!({}),
        )
        .await?;
        ensure!(
            receipt["name"] == name
                && receipt["size"] == size
                && receipt["sha256"] == hash
                && receipt["path"].is_string()
                && receipt["relativePath"].is_string(),
            "peer returned invalid upload receipt"
        );
        receipts.push(receipt);
    }
    Ok(receipts)
}

pub(crate) async fn fs_list(
    state: &Supervisor,
    device_id: &str,
    workspace_id: &str,
    path: &str,
) -> Result<Value> {
    require_local_access(state)?;
    request_json(
        &LinkedPeer { state, device_id },
        "/api/peer/files/list",
        json!({"workspaceId":workspace_id, "path":path}),
    )
    .await
}

fn download_path(
    state: &Supervisor,
    device_name: &str,
    name: &str,
    out: Option<PathBuf>,
    caller: Option<&str>,
) -> Result<PathBuf> {
    let root = match caller {
        Some(id) => thread_cwd(state, id)?,
        None => state.config.workspace_root.clone(),
    }
    .canonicalize()?;
    if let Some(out) = out {
        let out = if out.is_absolute() {
            out
        } else {
            root.join(out)
        };
        return Ok(if out.is_dir() { out.join(name) } else { out });
    }
    let folder = match caller {
        Some(id) => PathBuf::from(".temp/threads").join(id).join("downloads"),
        None => PathBuf::from(".temp/downloads"),
    };
    let path = root.join(folder).join(basename(device_name)?).join(name);
    ensure!(
        files::assert_within(&root, &path)? == path,
        "downloads path must not contain symbolic links"
    );
    Ok(path)
}

struct PartialDownload(PathBuf);
impl Drop for PartialDownload {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Download into a temporary file so an interrupted or corrupt read cannot replace out.
pub(crate) async fn fs_get(
    state: &Supervisor,
    device_id: &str,
    workspace_id: &str,
    path: &str,
    out: Option<PathBuf>,
    caller_thread: Option<&str>,
) -> Result<Value> {
    require_local_access(state)?;
    let device_name = if out.is_none() {
        peer_link::directory(state)
            .await?
            .into_iter()
            .find(|entry| entry["deviceId"] == device_id)
            .and_then(|entry| entry["name"].as_str().map(str::to_owned))
            .unwrap_or_else(|| device_id.to_owned())
    } else {
        device_id.to_owned()
    };
    fs_get_with(
        &LinkedPeer { state, device_id },
        state,
        &device_name,
        workspace_id,
        path,
        out,
        caller_thread,
    )
    .await
}

async fn fs_get_with(
    peer: &impl FilePeer,
    state: &Supervisor,
    device_name: &str,
    workspace_id: &str,
    path: &str,
    out: Option<PathBuf>,
    caller_thread: Option<&str>,
) -> Result<Value> {
    let value = request_json(
        peer,
        "/api/peer/files/stat",
        json!({"workspaceId":workspace_id, "path":path}),
    )
    .await?;
    ensure!(value["kind"] == "file", "peer path must be a regular file");
    let size = value["size"].as_u64().context("peer omitted file size")?;
    let hash = sha256(value["sha256"].as_str().context("peer omitted SHA-256")?)?;
    let name = basename(path)?;
    let output = download_path(state, device_name, &name, out, caller_thread)?;
    let parent = output
        .parent()
        .context("download destination needs a parent directory")?;
    tokio::fs::create_dir_all(parent).await?;
    let partial = PartialDownload(parent.join(format!(".peer-download-{}.part", Uuid::new_v4())));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial.0)
        .await?;
    let mut offset = 0u64;
    let mut digest = Sha256::new();
    while offset < size {
        let length = (size - offset).min(READ_SIZE as u64);
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("workspaceId", workspace_id)
            .append_pair("path", path)
            .append_pair("offset", &offset.to_string())
            .append_pair("length", &length.to_string())
            .finish();
        let response = checked(
            peer.request(FileRequest {
                method: "GET",
                path: format!("/api/peer/files/read?{query}"),
                content_type: None,
                body: Vec::new(),
            })
            .await?,
        )?;
        ensure!(
            !response.body.is_empty() && response.body.len() as u64 <= length,
            "peer returned invalid read length"
        );
        let next = offset + response.body.len() as u64;
        let expected = format!("bytes {offset}-{}/{size}", next - 1);
        let range = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-range"))
            .and_then(|(_, value)| value.as_str());
        ensure!(
            range == Some(expected.as_str()),
            "peer returned invalid content-range"
        );
        file.write_all(&response.body).await?;
        digest.update(&response.body);
        offset = next;
    }
    let actual = hex::encode(digest.finalize());
    ensure!(actual == hash, "Downloaded SHA-256 does not match");
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&partial.0, &output).await?;
    Ok(json!({"path":output, "size":size, "sha256":actual}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{auth::TrustedRelayForward, router};
    use axum::{http::Request as HttpRequest, Router};
    use pockymoe_protocol::{CreateThreadInput, CreateWorkspaceInput, Mode, Provider};
    use pockymoe_runtime::{
        fake::FakeRuntime, local_sessions::LocalSessionHomes, Database, RuntimeConfig,
    };
    use std::sync::atomic::{AtomicU8, Ordering};
    use tempfile::{tempdir, TempDir};
    use tower::ServiceExt;

    struct Fixture {
        dir: TempDir,
        state: Arc<Supervisor>,
        root: PathBuf,
        workspace: String,
        thread: String,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempdir().unwrap();
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
                app_name: "peer-files-test".into(),
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
                    workspace_id: workspace.clone(),
                    title: Some("file transfer".into()),
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
            state.set_peer_access(true).unwrap();
            Self {
                dir,
                state,
                root,
                workspace,
                thread,
            }
        }

        fn worktree(&self) -> PathBuf {
            let path = self.dir.path().join("worktree");
            fs::create_dir(&path).unwrap();
            self.state
                .db
                .with(|conn| {
                    conn.execute(
                        "UPDATE threads SET worktree_path=?1 WHERE id=?2",
                        [path.to_str().unwrap(), self.thread.as_str()],
                    )?;
                    Ok(())
                })
                .unwrap();
            path
        }

        fn begin(&self, name: &str, bytes: &[u8]) -> Value {
            json!({"threadId":self.thread, "name":name, "size":bytes.len(), "sha256":hex::encode(Sha256::digest(bytes))})
        }
    }

    fn caller() -> PeerCaller {
        PeerCaller {
            device_id: "source-device".into(),
            device_name: "Source Desk".into(),
            user_id: "owner".into(),
            shared_control: None,
        }
    }

    async fn dispatch(
        router: Router,
        method: &str,
        path: &str,
        bytes: Vec<u8>,
        peer: Option<PeerCaller>,
    ) -> PeerResponse {
        let mut request = HttpRequest::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(bytes))
            .unwrap();
        request.extensions_mut().insert(TrustedRelayForward);
        if let Some(peer) = peer {
            request.extensions_mut().insert(peer);
        }
        let response = router.oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(key, value)| (key.as_str().to_owned(), json!(value.to_str().unwrap())))
            .collect();
        let body = to_bytes(response.into_body(), CHUNK_SIZE + READ_SIZE)
            .await
            .unwrap()
            .to_vec();
        PeerResponse {
            status,
            headers,
            body,
        }
    }

    async fn post(fixture: &Fixture, path: &str, input: Value) -> (u16, Value) {
        let response = dispatch(
            router(fixture.state.clone()),
            "POST",
            path,
            serde_json::to_vec(&input).unwrap(),
            Some(caller()),
        )
        .await;
        (
            response.status,
            serde_json::from_slice(&response.body).unwrap(),
        )
    }

    async fn chunk(fixture: &Fixture, id: &str, offset: u64, bytes: &[u8]) -> (u16, Value) {
        let response = dispatch(
            router(fixture.state.clone()),
            "PUT",
            &format!("/api/peer/files/uploads/{id}?offset={offset}"),
            bytes.to_vec(),
            Some(caller()),
        )
        .await;
        (
            response.status,
            serde_json::from_slice(&response.body).unwrap(),
        )
    }

    async fn commit(fixture: &Fixture, id: &str) -> (u16, Value) {
        post(
            fixture,
            &format!("/api/peer/files/uploads/{id}/commit"),
            json!({}),
        )
        .await
    }

    #[tokio::test]
    async fn peer_files_handlers_require_identity_and_opt_in() {
        let fixture = Fixture::new().await;
        let id = Uuid::new_v4().to_string();
        let routes = [
            (
                "POST",
                "/api/peer/files/uploads".to_owned(),
                fixture.begin("a.bin", b"a"),
            ),
            (
                "PUT",
                format!("/api/peer/files/uploads/{id}?offset=0"),
                json!(null),
            ),
            (
                "POST",
                format!("/api/peer/files/uploads/{id}/commit"),
                json!({}),
            ),
            (
                "POST",
                "/api/peer/files/list".into(),
                json!({"workspaceId":fixture.workspace, "path":"."}),
            ),
            (
                "POST",
                "/api/peer/files/stat".into(),
                json!({"workspaceId":fixture.workspace, "path":"a.bin"}),
            ),
            (
                "GET",
                format!(
                    "/api/peer/files/read?workspaceId={}&path=a.bin",
                    fixture.workspace
                ),
                json!(null),
            ),
        ];
        for (method, path, input) in &routes {
            let response = dispatch(
                router(fixture.state.clone()),
                method,
                path,
                serde_json::to_vec(input).unwrap(),
                None,
            )
            .await;
            assert_eq!(response.status, 403, "{method} {path}");
        }
        fixture.state.set_peer_access(false).unwrap();
        for (method, path, input) in &routes {
            let response = dispatch(
                router(fixture.state.clone()),
                method,
                path,
                serde_json::to_vec(input).unwrap(),
                Some(caller()),
            )
            .await;
            assert_eq!(response.status, 403, "{method} {path}");
            assert_eq!(
                serde_json::from_slice::<Value>(&response.body).unwrap()["code"],
                "peer_access_disabled"
            );
        }
        assert!(!fixture.dir.path().join("peer-uploads").exists());
    }

    #[tokio::test]
    async fn peer_files_upload_chunks_resume_conflict_and_incoming() {
        let fixture = Fixture::new().await;
        let bytes: Vec<u8> = (0..CHUNK_SIZE + 17)
            .map(|index| (index % 251) as u8)
            .collect();
        let mut input = fixture.begin("../../folder\\report.bin", &bytes);
        input["sha256"] = json!(input["sha256"].as_str().unwrap().to_ascii_uppercase());
        let (status, begun) = post(&fixture, "/api/peer/files/uploads", input.clone()).await;
        assert_eq!(status, 200);
        assert_eq!(begun["received"], 0);
        assert_eq!(begun["chunkSize"], CHUNK_SIZE);
        let id = begun["uploadId"].as_str().unwrap();
        assert_eq!(
            chunk(&fixture, id, 0, &bytes[..CHUNK_SIZE]).await,
            (200, json!({"uploadId":id, "received":CHUNK_SIZE}))
        );
        let (status, conflict) = chunk(&fixture, id, 0, b"duplicate").await;
        assert_eq!(status, 409);
        assert_eq!(conflict["received"], CHUNK_SIZE);
        input["uploadId"] = json!(id);
        let (status, resumed) = post(&fixture, "/api/peer/files/uploads", input.clone()).await;
        assert_eq!(status, 200);
        assert_eq!(resumed["received"], CHUNK_SIZE);
        let mut wrong = input;
        wrong["name"] = json!("different.bin");
        assert_eq!(
            post(&fixture, "/api/peer/files/uploads", wrong).await.0,
            409
        );
        assert_eq!(commit(&fixture, id).await.1["code"], "upload_size_mismatch");
        assert_eq!(
            chunk(&fixture, id, CHUNK_SIZE as u64, &[0; 18]).await.1["code"],
            "upload_size_mismatch"
        );
        assert_eq!(
            chunk(&fixture, id, CHUNK_SIZE as u64, &bytes[CHUNK_SIZE..])
                .await
                .0,
            200
        );
        let (status, receipt) = commit(&fixture, id).await;
        assert_eq!(status, 200);
        let relative = format!(".temp/threads/{}/incoming/{id}/report.bin", fixture.thread);
        assert_eq!(receipt["relativePath"], relative);
        assert_eq!(
            receipt["path"],
            fixture.root.join(&relative).to_str().unwrap()
        );
        assert_eq!(receipt["name"], "report.bin");
        assert_eq!(receipt["size"], bytes.len());
        assert_eq!(receipt["sha256"], hex::encode(Sha256::digest(&bytes)));
        assert_eq!(fs::read(fixture.root.join(relative)).unwrap(), bytes);
        assert!(!fixture
            .dir
            .path()
            .join(format!("peer-uploads/{id}.part"))
            .exists());
        assert_eq!(commit(&fixture, id).await, (200, receipt));
    }

    #[tokio::test]
    async fn peer_files_upload_hash_validation_limits_and_names() {
        let fixture = Fixture::new().await;
        for name in ["", "/", "..", "a/", "bad\0name", &"x".repeat(256)] {
            assert_eq!(
                post(
                    &fixture,
                    "/api/peer/files/uploads",
                    fixture.begin(name, b"a")
                )
                .await
                .0,
                400,
                "{name:?}"
            );
        }
        for hash in ["a".repeat(63), "g".repeat(64)] {
            let mut input = fixture.begin("a.bin", b"a");
            input["sha256"] = json!(hash);
            assert_eq!(
                post(&fixture, "/api/peer/files/uploads", input).await.0,
                400
            );
        }
        let mut too_big = fixture.begin("a.bin", b"a");
        too_big["size"] = json!(MAX_SIZE + 1);
        assert_eq!(
            post(&fixture, "/api/peer/files/uploads", too_big).await.0,
            413
        );
        let mut input = fixture.begin("a.bin", b"wrong");
        input["sha256"] = json!(hex::encode(Sha256::digest(b"right")));
        let (_, begun) = post(&fixture, "/api/peer/files/uploads", input).await;
        let id = begun["uploadId"].as_str().unwrap();
        assert_eq!(
            chunk(&fixture, id, 0, &vec![0; CHUNK_SIZE + 1]).await.0,
            413
        );
        assert_eq!(chunk(&fixture, id, 0, b"wrong").await.0, 200);
        let (status, error) = commit(&fixture, id).await;
        assert_eq!(status, 400);
        assert_eq!(error["code"], "hash_mismatch");
        assert!(fixture
            .dir
            .path()
            .join(format!("peer-uploads/{id}.part"))
            .exists());
        assert!(!fixture
            .root
            .join(format!(".temp/threads/{}/incoming/{id}", fixture.thread))
            .exists());
    }

    #[tokio::test]
    async fn peer_files_upload_peer_binding_and_expiry() {
        let fixture = Fixture::new().await;
        let mut input = fixture.begin("a.bin", b"a");
        let (_, begun) = post(&fixture, "/api/peer/files/uploads", input.clone()).await;
        let id = begun["uploadId"].as_str().unwrap();
        input["uploadId"] = json!(id);
        for peer in [
            PeerCaller {
                device_id: "other-device".into(),
                ..caller()
            },
            PeerCaller {
                user_id: "other-owner".into(),
                ..caller()
            },
        ] {
            for (method, path, bytes) in [
                (
                    "POST",
                    "/api/peer/files/uploads".to_owned(),
                    serde_json::to_vec(&input).unwrap(),
                ),
                (
                    "PUT",
                    format!("/api/peer/files/uploads/{id}?offset=0"),
                    b"a".to_vec(),
                ),
                (
                    "POST",
                    format!("/api/peer/files/uploads/{id}/commit"),
                    b"{}".to_vec(),
                ),
            ] {
                assert_eq!(
                    dispatch(
                        router(fixture.state.clone()),
                        method,
                        &path,
                        bytes,
                        Some(peer.clone())
                    )
                    .await
                    .status,
                    403
                );
            }
        }
        let metadata = fixture.dir.path().join(format!("peer-uploads/{id}.json"));
        let mut upload: Upload = serde_json::from_slice(&fs::read(&metadata).unwrap()).unwrap();
        upload.created_at = Utc::now().timestamp_millis() - UPLOAD_TTL - 1;
        write_json(&metadata, &upload).unwrap();
        assert_eq!(
            post(&fixture, "/api/peer/files/uploads", input).await.0,
            404
        );
        assert!(!metadata.exists());
        assert!(!metadata.with_extension("part").exists());
    }

    #[tokio::test]
    async fn peer_files_upload_concurrent_offsets_and_worktree() {
        let fixture = Fixture::new().await;
        let worktree = fixture.worktree();
        let (_, begun) = post(
            &fixture,
            "/api/peer/files/uploads",
            fixture.begin("a.bin", b"abc"),
        )
        .await;
        let id = begun["uploadId"].as_str().unwrap();
        let (first, second) = tokio::join!(
            chunk(&fixture, id, 0, b"abc"),
            chunk(&fixture, id, 0, b"abc")
        );
        let mut statuses = [first.0, second.0];
        statuses.sort();
        assert_eq!(statuses, [200, 409]);
        let (status, receipt) = commit(&fixture, id).await;
        assert_eq!(status, 200);
        assert_eq!(
            receipt["path"],
            worktree
                .join(format!(
                    ".temp/threads/{}/incoming/{id}/a.bin",
                    fixture.thread
                ))
                .to_str()
                .unwrap()
        );
        assert!(!fixture.root.join(".temp").exists());
        // Resume still works if a crash happened between the move and receipt write.
        let metadata = fixture.dir.path().join(format!("peer-uploads/{id}.json"));
        let mut upload: Upload = serde_json::from_slice(&fs::read(&metadata).unwrap()).unwrap();
        upload.committed = None;
        write_json(&metadata, &upload).unwrap();
        let mut input = fixture.begin("a.bin", b"abc");
        input["uploadId"] = json!(id);
        assert_eq!(
            post(&fixture, "/api/peer/files/uploads", input).await.1["received"],
            3
        );
        assert_eq!(commit(&fixture, id).await, (200, receipt));
    }

    #[tokio::test]
    async fn peer_files_workspace_list_stat_read_ranges_and_escapes() {
        let fixture = Fixture::new().await;
        let bytes: Vec<u8> = (0..READ_SIZE + 29)
            .map(|index| (index % 251) as u8)
            .collect();
        fs::write(fixture.root.join("data.bin"), &bytes).unwrap();
        fs::write(fixture.root.join("empty.bin"), []).unwrap();
        fs::create_dir(fixture.root.join("subdir")).unwrap();
        let (status, listing) = post(
            &fixture,
            "/api/peer/files/list",
            json!({"workspaceId":fixture.workspace, "path":"."}),
        )
        .await;
        assert_eq!(status, 200);
        let entry = listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == "data.bin")
            .unwrap();
        assert_eq!(entry["path"], "data.bin");
        assert_eq!(entry["kind"], "file");
        assert_eq!(entry["size"], bytes.len());
        assert!(entry["modifiedAt"].is_string());
        let (status, info) = post(
            &fixture,
            "/api/peer/files/stat",
            json!({"workspaceId":fixture.workspace, "path":"data.bin"}),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(info["sha256"], hex::encode(Sha256::digest(&bytes)));
        assert_eq!(info["size"], bytes.len());
        assert!(info["modifiedAt"].is_string());
        assert_eq!(
            post(
                &fixture,
                "/api/peer/files/stat",
                json!({"workspaceId":fixture.workspace, "path":"subdir"})
            )
            .await
            .0,
            400
        );
        let base = format!(
            "/api/peer/files/read?workspaceId={}&path=data.bin",
            fixture.workspace
        );
        let response = dispatch(
            router(fixture.state.clone()),
            "GET",
            &format!("{base}&offset=7&length=5"),
            vec![],
            Some(caller()),
        )
        .await;
        assert_eq!(response.status, 206);
        assert_eq!(response.body, bytes[7..12]);
        assert_eq!(
            response.headers["content-range"],
            format!("bytes 7-11/{}", bytes.len())
        );
        let capped = dispatch(
            router(fixture.state.clone()),
            "GET",
            &format!("{base}&length={}", READ_SIZE + 99),
            vec![],
            Some(caller()),
        )
        .await;
        assert_eq!(capped.body.len(), READ_SIZE);
        let tail = dispatch(
            router(fixture.state.clone()),
            "GET",
            &format!("{base}&offset={}&length=1000", READ_SIZE),
            vec![],
            Some(caller()),
        )
        .await;
        assert_eq!(tail.body, bytes[READ_SIZE..]);
        assert_eq!(
            tail.headers["content-range"],
            format!("bytes {}-{}/{}", READ_SIZE, bytes.len() - 1, bytes.len())
        );
        let past = dispatch(
            router(fixture.state.clone()),
            "GET",
            &format!("{base}&offset={}", bytes.len()),
            vec![],
            Some(caller()),
        )
        .await;
        assert_eq!(past.status, 416);
        assert_eq!(
            past.headers["content-range"],
            format!("bytes */{}", bytes.len())
        );
        let empty = dispatch(
            router(fixture.state.clone()),
            "GET",
            &format!(
                "/api/peer/files/read?workspaceId={}&path=empty.bin",
                fixture.workspace
            ),
            vec![],
            Some(caller()),
        )
        .await;
        assert_eq!(empty.status, 200);
        assert!(empty.body.is_empty());
        assert_eq!(empty.headers["content-range"], "bytes */0");
        for path in [
            "../outside".to_owned(),
            fixture
                .dir
                .path()
                .join("outside")
                .to_string_lossy()
                .into_owned(),
        ] {
            for route in ["list", "stat"] {
                assert_eq!(
                    post(
                        &fixture,
                        &format!("/api/peer/files/{route}"),
                        json!({"workspaceId":fixture.workspace, "path":path})
                    )
                    .await
                    .0,
                    400
                );
            }
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("workspaceId", &fixture.workspace)
                .append_pair("path", &path)
                .finish();
            assert_eq!(
                dispatch(
                    router(fixture.state.clone()),
                    "GET",
                    &format!("/api/peer/files/read?{query}"),
                    vec![],
                    Some(caller())
                )
                .await
                .status,
                400
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn peer_files_symlinks_cannot_escape_workspace_or_redirect_incoming() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new().await;
        let outside = fixture.dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret"), b"secret").unwrap();
        symlink(&outside, fixture.root.join("link")).unwrap();
        for (route, path) in [("list", "link"), ("stat", "link/secret")] {
            assert_eq!(
                post(
                    &fixture,
                    &format!("/api/peer/files/{route}"),
                    json!({"workspaceId":fixture.workspace, "path":path})
                )
                .await
                .0,
                400
            );
        }
        assert_eq!(
            dispatch(
                router(fixture.state.clone()),
                "GET",
                &format!(
                    "/api/peer/files/read?workspaceId={}&path=link/secret",
                    fixture.workspace
                ),
                vec![],
                Some(caller())
            )
            .await
            .status,
            400
        );
        let (_, listing) = post(
            &fixture,
            "/api/peer/files/list",
            json!({"workspaceId":fixture.workspace, "path":"."}),
        )
        .await;
        assert!(listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["name"] != "link"));
        let (_, begun) = post(
            &fixture,
            "/api/peer/files/uploads",
            fixture.begin("a.bin", b"a"),
        )
        .await;
        let id = begun["uploadId"].as_str().unwrap();
        chunk(&fixture, id, 0, b"a").await;
        let thread_dir = fixture
            .root
            .join(format!(".temp/threads/{}", fixture.thread));
        fs::create_dir_all(&thread_dir).unwrap();
        // Also reject a redirect to an unrelated directory within this workspace.
        let redirected = fixture.root.join("redirected");
        fs::create_dir(&redirected).unwrap();
        symlink(&redirected, thread_dir.join("incoming")).unwrap();
        assert_eq!(commit(&fixture, id).await.0, 400);
        assert_eq!(fs::read_dir(redirected).unwrap().count(), 0);
        assert!(fixture
            .dir
            .path()
            .join(format!("peer-uploads/{id}.part"))
            .exists());
    }

    #[test]
    fn peer_files_stage_copies_files_and_zips_directories() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("one/file.txt");
        let second = dir.path().join("two/file.txt");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, b"one").unwrap();
        fs::write(&second, b"two").unwrap();
        let folder = dir.path().join("bundle");
        fs::create_dir_all(folder.join("nested/empty")).unwrap();
        fs::write(folder.join("nested/data.txt"), b"snapshot").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&first, folder.join("link.txt")).unwrap();
        let staged = stage(
            &[first.clone(), folder.clone(), second],
            &dir.path().join("staged"),
        )
        .unwrap();
        fs::write(first, b"changed").unwrap();
        fs::write(folder.join("nested/data.txt"), b"changed").unwrap();
        assert_eq!(fs::read(&staged[0]).unwrap(), b"one");
        assert_eq!(fs::read(&staged[2]).unwrap(), b"two");
        assert_eq!(staged[0].file_name().unwrap(), "file.txt");
        assert_eq!(staged[1].file_name().unwrap(), "bundle.zip");
        let mut archive = zip::ZipArchive::new(File::open(&staged[1]).unwrap()).unwrap();
        let mut text = String::new();
        archive
            .by_name("bundle/nested/data.txt")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "snapshot");
        assert!(archive.by_name("bundle/nested/empty/").unwrap().is_dir());
        assert!(archive.by_name("bundle/link.txt").is_err());
    }

    #[test]
    fn peer_files_stage_rejects_limits_and_recursive_destination() {
        let dir = tempdir().unwrap();
        let large = dir.path().join("large.bin");
        File::create(&large).unwrap().set_len(MAX_SIZE + 1).unwrap();
        assert!(stage(&[large], &dir.path().join("staged"))
            .unwrap_err()
            .to_string()
            .contains("1 GiB"));
        assert!(stage(
            &vec![dir.path().to_path_buf(); 21],
            &dir.path().join("many")
        )
        .is_err());
        assert!(
            stage(&[dir.path().to_path_buf()], &dir.path().join("recursive"))
                .unwrap_err()
                .to_string()
                .contains("outside")
        );
    }

    // Faults happen after the target answered: its durable offset may have advanced.
    struct RouterPeer {
        router: Router,
        fault: AtomicU8,
        calls: Mutex<Vec<Value>>,
    }
    impl RouterPeer {
        fn new(fixture: &Fixture) -> Self {
            Self {
                router: router(fixture.state.clone()),
                fault: AtomicU8::new(0),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn take_fault(&self, value: u8) -> bool {
            self.fault
                .compare_exchange(value, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        }
    }
    impl FilePeer for RouterPeer {
        async fn request(&self, request: FileRequest) -> Result<PeerResponse> {
            let put = request.method == "PUT";
            let commit = request.path.ends_with("/commit");
            let read = request.path.starts_with("/api/peer/files/read?");
            self.calls.lock().unwrap().push(json!({"method":request.method, "path":request.path, "size":request.body.len(), "input":if request.content_type == Some("application/json") { serde_json::from_slice::<Value>(&request.body).ok() } else { None }}));
            let mut response = dispatch(
                self.router.clone(),
                request.method,
                &request.path,
                request.body,
                Some(caller()),
            )
            .await;
            if (put && self.take_fault(1))
                || (commit && self.take_fault(2))
                || (read && self.take_fault(5))
            {
                return Err(PeerError::Offline("target".into()).into());
            }
            if put && self.take_fault(3) {
                response.status = 409;
            }
            if read && self.take_fault(4) {
                response.body[0] ^= 1;
            }
            if read && self.take_fault(6) {
                response
                    .headers
                    .insert("content-range".into(), json!("bytes 1-2/3"));
            }
            Ok(response)
        }
    }

    #[tokio::test]
    async fn peer_files_caller_upload_resumes_after_lost_chunk_and_commit_replies() {
        let target = Fixture::new().await;
        let peer = RouterPeer::new(&target);
        let source = tempdir().unwrap();
        let path = source.path().join("data.bin");
        let bytes: Vec<u8> = (0..CHUNK_SIZE + 13)
            .map(|index| (index % 251) as u8)
            .collect();
        fs::write(&path, &bytes).unwrap();
        peer.fault.store(1, Ordering::SeqCst);
        let error = upload_with(&peer, "target", &target.thread, std::slice::from_ref(&path))
            .await
            .unwrap_err();
        assert!(error.downcast_ref::<PeerError>().unwrap().retryable());
        peer.fault.store(2, Ordering::SeqCst);
        assert!(
            upload_with(&peer, "target", &target.thread, std::slice::from_ref(&path))
                .await
                .is_err()
        );
        let receipts = upload_with(&peer, "target", &target.thread, std::slice::from_ref(&path))
            .await
            .unwrap();
        assert_eq!(
            fs::read(receipts[0]["path"].as_str().unwrap()).unwrap(),
            bytes
        );
        let calls = peer.calls.lock().unwrap();
        let puts: Vec<_> = calls
            .iter()
            .filter(|call| call["method"] == "PUT")
            .collect();
        assert_eq!(puts.len(), 2);
        assert_eq!(puts[0]["size"], CHUNK_SIZE);
        assert_eq!(puts[1]["size"], 13);
        assert!(puts[1]["path"]
            .as_str()
            .unwrap()
            .ends_with(&format!("offset={CHUNK_SIZE}")));
        let begins: Vec<_> = calls
            .iter()
            .filter(|call| call["path"] == "/api/peer/files/uploads")
            .collect();
        assert_eq!(begins.len(), 3);
        assert_eq!(
            begins[1]["input"]["uploadId"],
            begins[2]["input"]["uploadId"]
        );
        assert_eq!(
            fs::read_dir(target.dir.path().join("peer-uploads"))
                .unwrap()
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn peer_files_caller_upload_handles_conflict_and_expired_resume() {
        let target = Fixture::new().await;
        let peer = RouterPeer::new(&target);
        let source = tempdir().unwrap();
        let path = source.path().join("data.bin");
        let bytes = vec![19; CHUNK_SIZE + 7];
        fs::write(&path, &bytes).unwrap();
        let hash = hex::encode(Sha256::digest(&bytes));
        let marker = resume_path(&path, "target", &target.thread, &hash).unwrap();
        let missing = Uuid::new_v4().to_string();
        write_json(&marker, &json!({"uploadId":missing})).unwrap();
        peer.fault.store(3, Ordering::SeqCst);
        let receipts = upload_with(&peer, "target", &target.thread, &[path])
            .await
            .unwrap();
        assert_eq!(
            fs::read(receipts[0]["path"].as_str().unwrap()).unwrap(),
            bytes
        );
        let calls = peer.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call["path"] == "/api/peer/files/uploads")
                .count(),
            2
        );
        assert_eq!(
            calls.iter().filter(|call| call["method"] == "PUT").count(),
            2
        );
        assert_ne!(
            serde_json::from_slice::<Value>(&fs::read(marker).unwrap()).unwrap()["uploadId"],
            missing
        );
    }

    #[tokio::test]
    async fn peer_files_caller_fs_get_chunks_checksums_and_default_destinations() {
        let target = Fixture::new().await;
        let local = Fixture::new().await;
        let worktree = local.worktree();
        let peer = RouterPeer::new(&target);
        let path = "nested/space & 文#.bin";
        fs::create_dir(target.root.join("nested")).unwrap();
        let bytes: Vec<u8> = (0..2 * READ_SIZE + 19)
            .map(|index| (index % 251) as u8)
            .collect();
        fs::write(target.root.join(path), &bytes).unwrap();
        let listing = request_json(
            &peer,
            "/api/peer/files/list",
            json!({"workspaceId":target.workspace, "path":"nested"}),
        )
        .await
        .unwrap();
        assert_eq!(listing["entries"][0]["path"], path);
        let value = fs_get_with(
            &peer,
            &local.state,
            "Remote Desk",
            &target.workspace,
            path,
            None,
            Some(&local.thread),
        )
        .await
        .unwrap();
        let expected = worktree.join(format!(
            ".temp/threads/{}/downloads/Remote Desk/space & 文#.bin",
            local.thread
        ));
        assert_eq!(value["path"], expected.to_str().unwrap());
        assert_eq!(value["sha256"], hex::encode(Sha256::digest(&bytes)));
        assert_eq!(value["size"], bytes.len());
        assert_eq!(fs::read(expected).unwrap(), bytes);
        let calls = peer.calls.lock().unwrap();
        let reads: Vec<_> = calls
            .iter()
            .filter(|call| call["method"] == "GET")
            .collect();
        assert_eq!(reads.len(), 3);
        for (index, call) in reads.iter().enumerate() {
            let uri =
                url::Url::parse(&format!("http://peer{}", call["path"].as_str().unwrap())).unwrap();
            let query: HashMap<_, _> = uri.query_pairs().into_owned().collect();
            assert_eq!(query["path"], path);
            assert_eq!(query["offset"], (index * READ_SIZE).to_string());
            assert!(query["length"].parse::<usize>().unwrap() <= READ_SIZE);
        }
        drop(calls);
        fs::write(target.root.join("empty.bin"), []).unwrap();
        let empty = fs_get_with(
            &peer,
            &local.state,
            "Remote Desk",
            &target.workspace,
            "empty.bin",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            empty["path"],
            local
                .root
                .join(".temp/downloads/Remote Desk/empty.bin")
                .to_str()
                .unwrap()
        );
        assert_eq!(empty["sha256"], hex::encode(Sha256::digest([])));
        assert_eq!(fs::read(empty["path"].as_str().unwrap()).unwrap(), b"");
    }

    #[tokio::test]
    async fn peer_files_caller_fs_get_rejects_corruption_and_preserves_out() {
        let target = Fixture::new().await;
        let local = Fixture::new().await;
        let peer = RouterPeer::new(&target);
        fs::write(target.root.join("data.bin"), b"download").unwrap();
        let output = local.root.join("keep.bin");
        for (fault, expected_error) in [(4, "SHA-256"), (5, "offline"), (6, "content-range")] {
            fs::write(&output, b"original").unwrap();
            peer.fault.store(fault, Ordering::SeqCst);
            let error = fs_get_with(
                &peer,
                &local.state,
                "Remote Desk",
                &target.workspace,
                "data.bin",
                Some(output.clone()),
                None,
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains(expected_error), "{error}");
            assert_eq!(fs::read(&output).unwrap(), b"original");
            assert!(fs::read_dir(&local.root).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".peer-download-")));
        }
        let result = fs_get_with(
            &peer,
            &local.state,
            "Remote Desk",
            &target.workspace,
            "data.bin",
            Some(PathBuf::from("keep.bin")),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result["path"], output.to_str().unwrap());
        assert_eq!(fs::read(output).unwrap(), b"download");
    }

    #[tokio::test]
    async fn peer_files_callers_enforce_local_opt_in() {
        let fixture = Fixture::new().await;
        fixture.state.set_peer_access(false).unwrap();
        assert!(upload(&fixture.state, "target", &fixture.thread, &[])
            .await
            .unwrap_err()
            .to_string()
            .contains("peer_access_disabled"));
        assert!(fs_list(&fixture.state, "target", &fixture.workspace, ".")
            .await
            .unwrap_err()
            .to_string()
            .contains("peer_access_disabled"));
        assert!(fs_get(
            &fixture.state,
            "target",
            &fixture.workspace,
            "a.bin",
            None,
            None
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("peer_access_disabled"));
    }
}
