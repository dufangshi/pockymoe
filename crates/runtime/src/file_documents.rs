//! Bounded UTF-8 documents and durable conditional-save receipts.
//! The process gate covers managed writers. Native shell/IDE writers do not
//! cooperate: the last version check + rename is deliberately NOT an OS CAS.
use crate::Supervisor;
use anyhow::{bail, ensure, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path},
    sync::{LazyLock, Mutex, MutexGuard},
};

pub const MAX_BYTES: usize = 50 * 1024;
pub const MAX_LINES: usize = 1000;
static MUTATIONS: Mutex<()> = Mutex::new(());
type OperationKey = (std::path::PathBuf, String, String, String, String);
static ACTIVE_OPERATIONS: LazyLock<Mutex<HashMap<OperationKey, (usize, Value)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Live request tracking is separate from durable uncertain intent. Dropping a
/// process never converts an intent into an automatically replayable operation.
pub struct SaveOperationGuard(OperationKey);
impl Drop for SaveOperationGuard {
    fn drop(&mut self) {
        let mut active = ACTIVE_OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((count, _)) = active.get_mut(&self.0) {
            *count -= 1;
            if *count == 0 {
                active.remove(&self.0);
            }
        }
    }
}
// A single conservative gate also serializes directory mutations and workspace
// aliases. No per-path lock ordering, no independent coordinator.
pub fn mutation_guard() -> MutexGuard<'static, ()> {
    MUTATIONS.lock().unwrap_or_else(|e| e.into_inner())
}
pub fn hash(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveDocument {
    pub path: String,
    pub workspace_revision: String,
    pub file_identity: String,
    pub expected_hash: String,
    pub content: String,
    pub draft_revision: u64,
    pub operation_id: String,
    pub operation_created_at: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub path: String,
    pub name: String,
    pub language: String,
    pub workspace_revision: String,
    pub file_identity: String,
    pub content_hash: Option<String>,
    pub content: Option<String>,
    pub size: u64,
    pub encoding: String,
    pub bom: bool,
    pub eol: String,
    pub read_only_reason: Option<String>,
    pub truncated: bool,
}

fn identity(meta: &Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash(
            format!(
                "{}:{}:{}:{}",
                meta.dev(),
                meta.ino(),
                meta.ctime(),
                meta.ctime_nsec()
            )
            .as_bytes(),
        )
    }
    #[cfg(not(unix))]
    {
        hash(format!("{:?}:{:?}", meta.created(), meta.modified()).as_bytes())
    }
}
fn root_revision(root: &Path) -> Result<String> {
    let root = root.canonicalize()?;
    let meta = root.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(hash(
            format!(
                "{}:{}:{}:{:?}",
                root.display(),
                meta.dev(),
                meta.ino(),
                meta.created()
            )
            .as_bytes(),
        ))
    }
    #[cfg(not(unix))]
    {
        Ok(hash(
            format!("{}:{:?}", root.display(), meta.created()).as_bytes(),
        ))
    }
}
fn relative_path(rel: &str) -> Result<()> {
    ensure!(
        !rel.is_empty()
            && rel.len() <= 4096
            && !rel.contains(['\\', '\0'])
            && !Path::new(rel).is_absolute(),
        "invalidRelativePath"
    );
    ensure!(
        Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_))),
        "invalidRelativePath"
    );
    Ok(())
}

// Linux currently has the tested replacement/metadata contract. Other platforms
// remain readable; capabilities truthfully advertise no conditional editing.
#[cfg(unix)]
fn open_document(root: &Path, rel: &str) -> Result<(File, File, std::ffi::CString)> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::OpenOptionsExt,
    };
    relative_path(rel)?;
    let mut parent = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root.canonicalize()?)?;
    let mut parts = Path::new(rel).components().peekable();
    while let Some(Component::Normal(part)) = parts.next() {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(part.as_bytes())?;
        let directory = parts.peek().is_some();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if directory { libc::O_DIRECTORY } else { 0 };
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            // O_NOFOLLOW refuses a symbolic link: ELOOP for the file itself,
            // ENOTDIR for a linked directory on the way.
            let mut stat: libc::stat = unsafe { std::mem::zeroed() };
            let is_link = unsafe {
                libc::fstatat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    &mut stat,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } == 0
                && stat.st_mode & libc::S_IFMT == libc::S_IFLNK;
            ensure!(!is_link, "symbolicLink");
            return Err(error.into());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        if !directory {
            ensure!(file.metadata()?.is_file(), "notRegularFile");
            return Ok((file, parent, name));
        }
        parent = file;
    }
    bail!("invalidRelativePath")
}
#[cfg(not(unix))]
fn open_document(root: &Path, rel: &str) -> Result<(File, File, std::ffi::CString)> {
    relative_path(rel)?;
    let path = crate::files::assert_within(root, Path::new(rel))?;
    let file = File::open(&path)?;
    ensure!(file.metadata()?.is_file(), "notRegularFile");
    let placeholder = file.try_clone()?;
    Ok((
        file,
        placeholder,
        std::ffi::CString::new(path.file_name().unwrap().to_string_lossy().as_bytes())?,
    ))
}
fn read_document(root: &Path, rel: &str) -> Result<(Document, Vec<u8>, File)> {
    let (mut file, _, _) = open_document(root, rel)?;
    let meta = file.metadata()?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    let mut reason = if bytes.len() > MAX_BYTES || meta.len() > MAX_BYTES as u64 {
        Some("fileTooLarge")
    } else {
        None
    };
    let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    let utf8 = std::str::from_utf8(if bom { &bytes[3..] } else { &bytes });
    let mut content = None;
    let mut eol = "lf";
    if reason.is_none() {
        match utf8 {
            Err(_) => reason = Some("unsupportedEncoding"),
            Ok(text) => {
                let crlf = text.matches("\r\n").count();
                let cr = text.matches('\r').count();
                let lf = text.matches('\n').count();
                if cr > crlf {
                    eol = "cr";
                    reason = Some("unsupportedEol");
                } else if crlf > 0 && lf > crlf {
                    eol = "mixed";
                    reason = Some("unsupportedEol");
                } else if crlf > 0 {
                    eol = "crlf";
                }
                if text.contains('\0') {
                    reason = Some("binaryFile");
                }
                if lf + 1 > MAX_LINES {
                    reason = Some("tooManyLines");
                }
                content = Some(text.replace("\r\n", "\n"));
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
        if reason.is_none()
            && (meta.nlink() != 1
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o7000 != 0)
        {
            reason = Some("unsupportedMetadata");
        }
        // Reject extended ACLs/xattrs rather than silently removing them.
        if reason.is_none()
            && unsafe { libc::flistxattr(file.as_raw_fd(), std::ptr::null_mut(), 0) } != 0
        {
            reason = Some("unsupportedMetadata");
        }
        let mut fs = std::mem::MaybeUninit::<libc::statfs>::uninit();
        if reason.is_none()
            && (unsafe { libc::fstatfs(file.as_raw_fd(), fs.as_mut_ptr()) } != 0
                || !matches!(
                    unsafe { fs.assume_init() }.f_type,
                    0xef53 | 0x58465342 | 0x9123683e | 0x01021994 | 0x794c7630
                ))
        {
            reason = Some("unsupportedFilesystem");
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        if reason.is_none() {
            reason = Some("unsupportedPlatform");
        }
    }
    ensure!(
        identity(&file.metadata()?) == identity(&meta),
        "fileChangedDuringRead"
    );
    let name = Path::new(rel)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let doc = Document {
        path: rel.into(),
        language: crate::files::language_for(&name),
        name,
        workspace_revision: root_revision(root)?,
        file_identity: identity(&meta),
        content_hash: (bytes.len() <= MAX_BYTES && meta.len() <= MAX_BYTES as u64)
            .then(|| hash(&bytes)),
        content,
        size: meta.len(),
        encoding: if bytes.len() <= MAX_BYTES && utf8.is_ok() {
            "utf-8"
        } else {
            "unknown"
        }
        .into(),
        bom,
        eol: eol.into(),
        read_only_reason: reason.map(str::to_owned),
        truncated: bytes.len() > MAX_BYTES,
    };
    Ok((doc, bytes, file))
}

impl Supervisor {
    fn operation_key(
        &self,
        actor: &str,
        workspace: &str,
        revision: &str,
        operation: &str,
    ) -> OperationKey {
        (
            self.config.database_url.clone(),
            actor.into(),
            workspace.into(),
            revision.into(),
            operation.into(),
        )
    }
    // HTTP registers before spawn_blocking, so an accepted request waiting for
    // a blocking worker or mutation gate is also reported as pending.
    pub fn track_file_save(
        &self,
        actor: &str,
        workspace: &str,
        input: &SaveDocument,
    ) -> SaveOperationGuard {
        let key = self.operation_key(
            actor,
            workspace,
            &input.workspace_revision,
            &input.operation_id,
        );
        let pending = json!({"status":"pending","operationId":input.operation_id,"draftRevision":input.draft_revision,"path":input.path});
        let mut active = ACTIVE_OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
        active
            .entry(key.clone())
            .and_modify(|(count, _)| *count += 1)
            .or_insert((1, pending));
        SaveOperationGuard(key)
    }
    pub fn file_capabilities(&self, workspace: &str) -> Result<Value> {
        let ws = self.get_workspace(workspace)?;
        Ok(
            json!({"workspaceRevision":root_revision(Path::new(&ws.abs_path))?,"conditionalSave":cfg!(target_os="linux"),"documentRead":cfg!(unix),"textRangeRead":false,"maxEditableBytes":MAX_BYTES,"maxEditableLines":MAX_LINES}),
        )
    }
    pub fn file_document(&self, workspace: &str, path: &str) -> Result<Document> {
        ensure!(cfg!(unix), "unsupportedPlatform");
        let ws = self.get_workspace(workspace)?;
        Ok(read_document(Path::new(&ws.abs_path), path)?.0)
    }
    pub fn file_operation(&self, actor: &str, workspace: &str, operation: &str) -> Result<Value> {
        let ws = self.get_workspace(workspace)?;
        let revision = root_revision(Path::new(&ws.abs_path))?;
        if let Some((_, pending)) = ACTIVE_OPERATIONS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&self.operation_key(actor, workspace, &revision, operation))
        {
            return Ok(pending.clone());
        }
        self.db.with(|conn| {
            let result: Option<String> = conn.query_row("SELECT result FROM file_save_operations WHERE actor=?1 AND workspace_id=?2 AND workspace_revision=?3 AND operation_id=?4 AND created_at >= unixepoch()-86400",params![actor,workspace,revision,operation], |r| r.get(0)).optional()?;
            result.map(|s| serde_json::from_str(&s).map_err(Into::into)).unwrap_or_else(|| Err(anyhow::anyhow!("operationExpired")))
        })
    }
    pub fn file_save(&self, actor: &str, workspace: &str, input: SaveDocument) -> Result<Value> {
        ensure!(cfg!(target_os = "linux"), "unsupportedPlatform");
        let _operation = self.track_file_save(actor, workspace, &input);
        let _gate = mutation_guard();
        let ws = self.get_workspace(workspace)?;
        let root = Path::new(&ws.abs_path);
        ensure!(
            uuid::Uuid::parse_str(&input.operation_id).is_ok(),
            "invalidOperationId"
        );
        ensure!(
            input.content.len() <= MAX_BYTES
                && input.content.split('\n').count() <= MAX_LINES
                && !input.content.contains(['\r', '\0']),
            "invalidDocumentContent"
        );
        let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
        ensure!(
            input.operation_created_at <= now + 300_000
                && now.saturating_sub(input.operation_created_at) < 86_400_000,
            "operationExpired"
        );
        relative_path(&input.path)?;
        let digest = hash(&serde_json::to_vec(&input)?);
        let revision = root_revision(root)?;
        ensure!(revision == input.workspace_revision, "workspaceChanged");
        if let Some((old_digest,result)) = self.db.with(|conn| {
            Ok(conn.query_row("SELECT input_digest,result FROM file_save_operations WHERE actor=?1 AND workspace_id=?2 AND operation_id=?3", params![actor,workspace,input.operation_id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()?)
        })? { ensure!(old_digest == digest, "operationIdReuse"); return Ok(serde_json::from_str(&result)?); }
        let uncertain = json!({"status":"uncertain","operationId":input.operation_id,"draftRevision":input.draft_revision,"path":input.path});
        self.db.with(|conn| {
            conn.execute("DELETE FROM file_save_operations WHERE created_at < unixepoch()-86400 AND json_extract(result,'$.status') != 'uncertain'", [])?;
            let count: i64 = conn.query_row("SELECT COUNT(*) FROM file_save_operations WHERE actor=?1 AND workspace_id=?2",params![actor,workspace],|r| r.get(0))?;
            let bytes: i64 = conn.query_row("SELECT COALESCE(SUM(length(CAST(result AS BLOB))+COALESCE(length(before_bytes),0)),0) FROM file_save_operations",[],|r| r.get(0))?;
            // Reserve worst-case JSON escaping, duplicate bounded paths and before bytes.
            ensure!(count < 256 && bytes + ((MAX_BYTES * 10) as i64) < 32*1024*1024, "saveJournalFull");
            conn.execute("INSERT INTO file_save_operations(actor,workspace_id,workspace_revision,operation_id,input_digest,result,created_at) VALUES(?1,?2,?3,?4,?5,?6,unixepoch())",params![actor,workspace,revision,input.operation_id,digest,uncertain.to_string()])?; Ok(())
        })?;
        let attempt = self.commit_document(root, &input, actor, workspace);
        let result = match attempt {
            Ok(v) => v,
            Err(e) => {
                json!({"status":"failedBeforeWrite","code":"documentRejected","message":e.to_string(),"operationId":input.operation_id,"draftRevision":input.draft_revision,"path":input.path})
            }
        };
        // If receipt persistence fails after replacement, leave durable uncertain intent.
        if self.db.with(|conn| {conn.execute("UPDATE file_save_operations SET result=?1 WHERE actor=?2 AND workspace_id=?3 AND operation_id=?4",params![result.to_string(),actor,workspace,input.operation_id])?; Ok(())}).is_err() { return Ok(uncertain); }
        Ok(result)
    }
    fn commit_document(
        &self,
        root: &Path,
        input: &SaveDocument,
        actor: &str,
        workspace: &str,
    ) -> Result<Value> {
        let (doc, before, _) = match read_document(root, &input.path) {
            Ok(v) => v,
            Err(e) => {
                return Ok(
                    json!({"status":"conflict","code":"fileMissingOrUnavailable","message":e.to_string(),"operationId":input.operation_id,"draftRevision":input.draft_revision,"path":input.path}),
                )
            }
        };
        let conflict = |doc: &Document| json!({"status":"conflict","code":"fileConflict","operationId":input.operation_id,"draftRevision":input.draft_revision,"path":input.path,"snapshot":doc});
        if doc.content_hash.as_deref() != Some(&input.expected_hash)
            || doc.file_identity != input.file_identity
        {
            return Ok(conflict(&doc));
        }
        ensure!(
            doc.read_only_reason.is_none(),
            "{}",
            doc.read_only_reason.as_deref().unwrap_or("readOnly")
        );
        let text = if doc.eol == "crlf" {
            input.content.replace('\n', "\r\n")
        } else {
            input.content.clone()
        };
        let mut bytes = if doc.bom {
            vec![0xef, 0xbb, 0xbf]
        } else {
            vec![]
        };
        bytes.extend(text.as_bytes());
        ensure!(bytes.len() <= MAX_BYTES, "fileTooLarge");
        self.db.with(|conn| {conn.execute("UPDATE file_save_operations SET before_bytes=?1,intended_hash=?2 WHERE actor=?3 AND workspace_id=?4 AND operation_id=?5",params![before,hash(&bytes),actor,workspace,input.operation_id])?; Ok(())})?;
        #[cfg(target_os = "linux")]
        {
            use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
            let (file, parent, name) = open_document(root, &input.path)?;
            let mut temp =
                tempfile::NamedTempFile::new_in(format!("/proc/self/fd/{}", parent.as_raw_fd()))?;
            let meta = file.metadata()?;
            ensure!(
                unsafe { libc::fchown(temp.as_raw_fd(), meta.uid(), meta.gid()) } == 0,
                "cannotPreserveOwner"
            );
            temp.as_file().set_permissions(meta.permissions())?;
            ensure!(
                unsafe { libc::flistxattr(temp.as_raw_fd(), std::ptr::null_mut(), 0) } == 0,
                "cannotPreserveMetadata"
            );
            temp.write_all(&bytes)?;
            temp.as_file().sync_all()?;
            let (latest, _, _) = read_document(root, &input.path)?;
            if latest.content_hash != doc.content_hash
                || latest.file_identity != doc.file_identity
                || latest.workspace_revision != doc.workspace_revision
            {
                return Ok(conflict(&latest));
            }
            let (_, fresh_parent, _) = open_document(root, &input.path)?;
            let pm = parent.metadata()?;
            let fm = fresh_parent.metadata()?;
            ensure!(
                pm.dev() == fm.dev() && pm.ino() == fm.ino(),
                "parentChanged"
            );
            let tmp_name = std::ffi::CString::new(
                temp.path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .as_bytes(),
            )?;
            if unsafe {
                libc::renameat(
                    parent.as_raw_fd(),
                    tmp_name.as_ptr(),
                    parent.as_raw_fd(),
                    name.as_ptr(),
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            // Everything after this point is uncertain on failure, never failed-before-write.
            let saved = (|| -> Result<Value> {
                parent.sync_all()?;
                temp.as_file_mut().seek(SeekFrom::Start(0))?;
                let mut verified = Vec::new();
                Read::by_ref(temp.as_file_mut())
                    .take((MAX_BYTES + 1) as u64)
                    .read_to_end(&mut verified)?;
                ensure!(verified == bytes, "postCommitFileChanged");
                let receipt_meta = temp.as_file().metadata()?;
                Ok(
                    json!({"status":"saved","operationId":input.operation_id,"draftRevision":input.draft_revision,"path":input.path,"workspaceRevision":doc.workspace_revision,"contentHash":hash(&bytes),"fileIdentity":identity(&receipt_meta),"size":bytes.len(),"bom":doc.bom,"eol":doc.eol,"encoding":"utf-8"}),
                )
            })();
            return Ok(saved.unwrap_or_else(|e| json!({"status":"uncertain","operationId":input.operation_id,"draftRevision":input.draft_revision,"path":input.path,"message":e.to_string()})));
        }
        #[cfg(not(target_os = "linux"))]
        bail!("unsupportedPlatform")
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn symbolic_links_are_reported_instead_of_followed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::fs::write(dir.path().join("real/a.txt"), "a").unwrap();
        std::os::unix::fs::symlink("real", dir.path().join("alias")).unwrap();
        std::os::unix::fs::symlink("loop", dir.path().join("loop")).unwrap();
        for rel in ["loop", "alias/a.txt"] {
            let error = read_document(dir.path(), rel).map(|_| ()).unwrap_err();
            assert_eq!(error.to_string(), "symbolicLink", "{rel}");
        }
        assert!(read_document(dir.path(), "real/a.txt").is_ok());
    }
}
