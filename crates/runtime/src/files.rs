use std::ffi::OsString;
use std::fs::File;
use std::io::ErrorKind;
use std::io::{self, Read, Seek};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Result};
use pockymoe_protocol::{ThreadWorkspaceFilePreviewDto, ThreadWorkspaceTreeNodeDto};
use tempfile::NamedTempFile;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub const DIRECTORY_DOWNLOAD_MAX_FILES_EXCLUSIVE: usize = 1_000;
pub const DIRECTORY_DOWNLOAD_MAX_BYTES_EXCLUSIVE: u64 = 1_000_000_000;

/// Create an empty file exclusively: never truncate an existing path or create
/// missing directories. Unix parents are opened relative to directory handles,
/// without following symlinks, just like the conditional document API.
pub fn create_empty_file(root: &Path, rel: &str) -> Result<()> {
    anyhow::ensure!(
        !rel.is_empty()
            && rel.len() <= 4096
            && !rel.contains('\\')
            && !rel.chars().any(char::is_control)
            && !Path::new(rel).is_absolute()
            && rel
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
            && Path::new(rel)
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
        "invalidRelativePath"
    );
    #[cfg(unix)]
    {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::OpenOptionsExt},
        };
        let mut parent = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(root.canonicalize()?)?;
        let mut parts = Path::new(rel).components().peekable();
        while let Some(Component::Normal(part)) = parts.next() {
            let name = std::ffi::CString::new(part.as_bytes())?;
            let directory = parts.peek().is_some();
            let flags = libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | if directory {
                    libc::O_RDONLY | libc::O_DIRECTORY
                } else {
                    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL
                };
            let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags, 0o644) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let file = unsafe { File::from_raw_fd(fd) };
            if !directory {
                file.sync_all()?;
                parent.sync_all()?;
                return Ok(());
            }
            parent = file;
        }
        bail!("invalidRelativePath")
    }
    #[cfg(not(unix))]
    {
        let root = root.canonicalize()?;
        let path = assert_within(&root, Path::new(rel))?;
        // The parent must already exist; create_new preserves collision safety.
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalidRelativePath"))?;
        anyhow::ensure!(parent.is_dir(), "parentDirectoryMissing");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?
            .sync_all()?;
        Ok(())
    }
}

pub enum WorkspaceDownload {
    File {
        path: PathBuf,
        file: File,
    },
    DirectoryArchive {
        filename: String,
        archive: NamedTempFile,
    },
}

pub fn assert_within(root: &Path, candidate: &Path) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let normalized = normalize_path(&joined)?;
    let resolved = resolve_existing_ancestor(&normalized)?;
    if !resolved.starts_with(&root) {
        bail!("path is outside the workspace");
    }
    Ok(resolved)
}

fn normalize_path(path: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                if !out.pop() {
                    bail!("path is outside the workspace");
                }
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    Ok(out)
}

/// Mutation must never resolve a selected symlink into its target, or remove the
/// workspace itself. Parent resolution still rejects escapes for new names.
pub fn assert_mutation_within(root: &Path, candidate: &Path) -> Result<PathBuf> {
    let canonical_root = root.canonicalize()?;
    let lexical = normalize_path(&if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        canonical_root.join(candidate)
    })?;
    if std::fs::symlink_metadata(&lexical).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!("Symbolic links cannot be renamed or deleted from Explorer");
    }
    let resolved = assert_within(&canonical_root, candidate)?;
    if resolved == canonical_root {
        bail!("The workspace root cannot be renamed or deleted");
    }
    Ok(resolved)
}

fn resolve_existing_ancestor(path: &Path) -> Result<PathBuf> {
    let mut ancestor = path.to_path_buf();
    let mut suffix = Vec::<OsString>::new();

    loop {
        match std::fs::symlink_metadata(&ancestor) {
            Ok(_) => {
                let mut resolved = ancestor.canonicalize()?;
                for component in suffix.into_iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let Some(component) = ancestor.file_name().map(OsString::from) else {
                    return Err(error.into());
                };
                suffix.push(component);
                if !ancestor.pop() {
                    return Err(error.into());
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub fn list_tree(root: &Path, rel: &str) -> Result<Vec<ThreadWorkspaceTreeNodeDto>> {
    let dir = assert_within(root, &PathBuf::from(rel))?;
    if !dir.is_dir() {
        bail!("not a directory");
    }
    let mut nodes = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = PathBuf::from(rel).join(&name);
        let mut meta = entry.metadata()?;
        // List a symbolic link as what it points to when that stays inside the
        // workspace; others (outside, missing or looping) stay plain entries.
        if meta.file_type().is_symlink() {
            if let Ok(target) = assert_within(root, &path) {
                if let Ok(resolved) = std::fs::metadata(&target) {
                    meta = resolved;
                }
            }
        }
        let kind = if meta.is_dir() { "directory" } else { "file" };
        nodes.push(ThreadWorkspaceTreeNodeDto {
            name,
            path: path.to_string_lossy().replace('\\', "/"),
            kind: kind.into(),
            size: if meta.is_file() {
                Some(meta.len())
            } else {
                None
            },
            has_children: Some(meta.is_dir()),
            children_loaded: Some(false),
            children: None,
        });
    }
    nodes.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(nodes)
}

pub fn preview_file(root: &Path, rel: &str, limit: usize) -> Result<ThreadWorkspaceFilePreviewDto> {
    let path = assert_within(root, &PathBuf::from(rel))?;
    anyhow::ensure!(std::fs::metadata(&path)?.is_file(), "not a regular file");
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = options.open(&path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(metadata.is_file(), "not a regular file");
    let limit = limit.min(64 * 1024);
    let mut bytes = Vec::new();
    file.by_ref()
        .take((limit + 4) as u64)
        .read_to_end(&mut bytes)?;
    let truncated = metadata.len() > limit as u64;
    let mut end = bytes.len().min(limit);
    // Only a partial final UTF-8 sequence may be trimmed; never lossy decode.
    let content = loop {
        match std::str::from_utf8(&bytes[..end]) {
            Ok(text) if !text.contains('\0') => break text.to_owned(),
            Err(e) if truncated && e.error_len().is_none() => end = e.valid_up_to(),
            _ => bail!("unsupportedEncodingOrBinaryFile"),
        }
    };
    let slice = &bytes[..end];
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| rel.to_string());
    Ok(ThreadWorkspaceFilePreviewDto {
        path: rel.replace('\\', "/"),
        name: name.clone(),
        content,
        language: language_for(&name),
        size: metadata.len(),
        truncated,
        next_offset: slice.len() as u64,
    })
}

pub fn read_bytes(root: &Path, rel: &str) -> Result<(PathBuf, Vec<u8>)> {
    let path = assert_within(root, Path::new(rel))?;
    let bytes = std::fs::read(&path)?;
    Ok((path, bytes))
}

pub fn prepare_download(root: &Path, rel: &str) -> Result<WorkspaceDownload> {
    let path = assert_within(root, Path::new(rel))?;
    if path.is_file() {
        let file = File::open(&path)?;
        return Ok(WorkspaceDownload::File { path, file });
    }
    if !path.is_dir() {
        bail!("Workspace download path must point to a file or directory.");
    }

    let archive_root = path
        .file_name()
        .filter(|name| !name.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("workspace"));
    let mut entries = Vec::new();
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;

    for entry in WalkDir::new(&path).follow_links(false).sort_by_file_name() {
        let entry = entry?;
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_file() {
            file_count += 1;
            if file_count >= DIRECTORY_DOWNLOAD_MAX_FILES_EXCLUSIVE {
                bail!(
                    "Directory download is limited to fewer than 1,000 files; `{rel}` contains 1,000 files or more."
                );
            }
            total_bytes = total_bytes
                .checked_add(entry.metadata()?.len())
                .ok_or_else(|| anyhow::anyhow!("Directory download size overflowed."))?;
            if total_bytes >= DIRECTORY_DOWNLOAD_MAX_BYTES_EXCLUSIVE {
                bail!(
                    "Directory download is limited to less than 1 GB (1,000,000,000 bytes); `{rel}` contains 1 GB or more."
                );
            }
        }
        if file_type.is_dir() || file_type.is_file() {
            entries.push(entry.into_path());
        }
    }

    let mut archive = NamedTempFile::new()?;
    {
        let mut writer = ZipWriter::new(archive.as_file_mut());
        let file_options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o644);
        let directory_options = SimpleFileOptions::default().unix_permissions(0o755);
        let mut archived_bytes = 0u64;
        for entry in entries {
            let relative = entry.strip_prefix(&path)?;
            let archive_path = archive_root.join(relative);
            if entry.is_dir() {
                writer.add_directory_from_path(&archive_path, directory_options)?;
            } else {
                writer.start_file_from_path(&archive_path, file_options)?;
                let input = File::open(&entry)?;
                let remaining =
                    (DIRECTORY_DOWNLOAD_MAX_BYTES_EXCLUSIVE - 1).saturating_sub(archived_bytes);
                let copied = io::copy(&mut input.take(remaining + 1), &mut writer)?;
                if copied > remaining {
                    bail!(
                        "Directory download is limited to less than 1 GB (1,000,000,000 bytes); `{rel}` contains 1 GB or more."
                    );
                }
                archived_bytes += copied;
            }
        }
        writer.finish()?;
    }

    Ok(WorkspaceDownload::DirectoryArchive {
        filename: format!("{}.zip", archive_root.to_string_lossy()),
        archive,
    })
}

/// Only raster signatures are accepted. Extensions and user-provided MIME types
/// are not proof that an attachment is safe to serve from the application origin.
pub fn raster_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.get(4..8) == Some(b"ftyp")
        && matches!(
            bytes.get(8..12),
            Some(b"heic" | b"heix" | b"hevc" | b"hevx")
        )
    {
        Some("image/heic")
    } else if bytes.get(4..8) == Some(b"ftyp")
        && matches!(bytes.get(8..12), Some(b"mif1" | b"msf1"))
    {
        Some("image/heif")
    } else {
        None
    }
}

pub fn read_thread_image(
    root: &Path,
    thread_id: &str,
    rel: &str,
) -> Result<(Vec<u8>, &'static str)> {
    let mut image = open_thread_image(root, thread_id, rel)?;
    Ok((image.read_all()?, image.mime))
}

const THREAD_IMAGE_MAX_BYTES: u64 = 25 * 1024 * 1024;

/// A validated, opened thread image attachment. The checks are done once, on
/// the open handle, so serving the original, a thumbnail or a 304 all share them.
#[derive(Debug)]
pub struct ThreadImageFile {
    file: File,
    /// Canonical path, inside `<workspace>/.temp/threads/<thread>/`.
    pub path: PathBuf,
    pub len: u64,
    pub modified: Option<std::time::SystemTime>,
    pub mime: &'static str,
    /// The file name carries the random suffix given to uploads, directly in
    /// the thread's attachment directory, so its content never changes.
    pub immutable_name: bool,
}

impl ThreadImageFile {
    pub fn read_all(&mut self) -> Result<Vec<u8>> {
        self.file.seek(std::io::SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        self.file
            .by_ref()
            .take(THREAD_IMAGE_MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > THREAD_IMAGE_MAX_BYTES {
            bail!("image attachment is too large");
        }
        if raster_image_mime(&bytes) != Some(self.mime) {
            bail!("attachment changed while it was read");
        }
        Ok(bytes)
    }
}

pub fn open_thread_image(root: &Path, thread_id: &str, rel: &str) -> Result<ThreadImageFile> {
    let path = assert_within(root, Path::new(rel))?;
    // Historical uploads already use this per-thread namespace. Resolve symlinks
    // before checking ownership, so a link to another thread/private file fails.
    let attachment_root = root.canonicalize()?.join(".temp/threads").join(thread_id);
    if assert_within(root, &attachment_root)? != attachment_root {
        bail!("attachment directory must not be a symbolic link");
    }
    if !path.starts_with(&attachment_root) || path == attachment_root {
        bail!("image is not an attachment of this thread");
    }
    let mut file = File::open(&path)?;
    let metadata = file.metadata()?;
    if metadata.len() > THREAD_IMAGE_MAX_BYTES {
        bail!("image attachment is too large");
    }
    // Every signature raster_image_mime knows fits in the first 12 bytes.
    let mut head = Vec::with_capacity(16);
    file.by_ref().take(16).read_to_end(&mut head)?;
    let mime = raster_image_mime(&head)
        .ok_or_else(|| anyhow::anyhow!("attachment is not a supported raster image"))?;
    let immutable_name = path.parent() == Some(attachment_root.as_path())
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(has_upload_suffix);
    Ok(ThreadImageFile {
        file,
        len: metadata.len(),
        modified: metadata.modified().ok(),
        mime,
        immutable_name,
        path,
    })
}

/// `sanitize_file_name` in service.rs names uploads `<stem>-<8 hex>[.<ext>]`.
fn has_upload_suffix(name: &str) -> bool {
    let stem = match name.rsplit_once('.') {
        Some((stem, extension))
            if !extension.is_empty() && extension.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            stem
        }
        _ => name,
    };
    stem.len() > 9
        && stem.rsplit_once('-').is_some_and(|(prefix, suffix)| {
            !prefix.is_empty()
                && suffix.len() == 8
                && suffix
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteScope {
    Workspace,
    Unrestricted,
}

pub fn write_file(root: &Path, rel: &str, content: &str) -> Result<()> {
    write_file_with_scope(root, rel, content, WriteScope::Workspace)
}

pub fn write_file_with_scope(
    root: &Path,
    rel: &str,
    content: &str,
    scope: WriteScope,
) -> Result<()> {
    let _gate = crate::file_documents::mutation_guard();
    let path = if scope == WriteScope::Unrestricted {
        if Path::new(rel).is_absolute() {
            PathBuf::from(rel)
        } else {
            root.join(rel)
        }
    } else {
        assert_within(root, Path::new(rel))?
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

pub fn language_for(name: &str) -> String {
    match Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
    {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "py" => "python",
        "md" => "markdown",
        "json" => "json",
        "toml" => "toml",
        "yml" | "yaml" => "yaml",
        "sh" => "shell",
        "css" => "css",
        "html" => "html",
        "sql" => "sql",
        _ => "text",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn create_file_is_exclusive_and_requires_a_valid_existing_parent() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("docs")).unwrap();
        create_empty_file(dir.path(), "docs/新文件.md").unwrap();
        fs::write(dir.path().join("docs/新文件.md"), "keep").unwrap();
        let error = create_empty_file(dir.path(), "docs/新文件.md").unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            ErrorKind::AlreadyExists
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("docs/新文件.md")).unwrap(),
            "keep"
        );
        for invalid in [
            "",
            ".",
            "../outside.txt",
            "/absolute.txt",
            "docs/../../outside",
            "docs\\bad.txt",
            "bad\nname",
            "docs//double.txt",
            "trailing/",
            "docs/./dot.txt",
        ] {
            assert!(create_empty_file(dir.path(), invalid).is_err(), "{invalid}");
        }
        assert!(create_empty_file(dir.path(), "missing/file.txt").is_err());
        assert!(!dir.path().join("missing").exists());
        assert!(create_empty_file(dir.path(), "docs").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn create_file_rejects_symlink_parents_and_existing_symlink_targets() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("missing.txt"),
            dir.path().join("dangling.txt"),
        )
        .unwrap();
        assert!(create_empty_file(dir.path(), "link/outside.txt").is_err());
        assert!(create_empty_file(dir.path(), "dangling.txt").is_err());
        assert!(!outside.path().join("outside.txt").exists());
        assert!(!outside.path().join("missing.txt").exists());
    }

    #[test]
    fn workspace_write_rejects_escape() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("ws");
        fs::create_dir_all(&root).unwrap();
        let err = write_file(&root, "../escape.txt", "no").unwrap_err();
        assert!(err.to_string().contains("outside"));
    }

    #[test]
    fn workspace_mutations_reject_root_and_escape_without_touching_contents() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("ws");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("keep.txt"), "keep").unwrap();
        assert!(assert_mutation_within(&root, Path::new(".")).is_err());
        assert!(assert_mutation_within(&root, Path::new("../outside")).is_err());
        assert_eq!(
            assert_mutation_within(&root, Path::new("renamed.txt")).unwrap(),
            root.canonicalize().unwrap().join("renamed.txt")
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("keep.txt"), root.join("alias.txt")).unwrap();
            assert!(assert_mutation_within(&root, Path::new("alias.txt")).is_err());
        }
        assert_eq!(fs::read_to_string(root.join("keep.txt")).unwrap(), "keep");
    }

    #[cfg(unix)]
    #[test]
    fn listing_shows_symlinks_as_their_target_inside_the_workspace() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path().join("ws");
        fs::create_dir_all(root.join("real")).unwrap();
        symlink("real", root.join("alias")).unwrap();
        symlink("loop", root.join("loop")).unwrap();
        symlink(dir.path(), root.join("outside")).unwrap();

        let nodes = list_tree(&root, "").unwrap();
        let kind = |name: &str| nodes.iter().find(|n| n.name == name).unwrap().kind.clone();
        assert_eq!(kind("alias"), "directory");
        assert_eq!(kind("loop"), "file");
        assert_eq!(kind("outside"), "file");
    }

    #[cfg(unix)]
    #[test]
    fn workspace_reads_reject_symlink_escape() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path().join("ws");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "secret").unwrap();
        symlink(&outside, root.join("linked")).unwrap();

        let error = read_bytes(&root, "linked/secret.txt").unwrap_err();

        assert!(error.to_string().contains("outside"));
    }

    #[cfg(unix)]
    #[test]
    fn workspace_writes_reject_symlink_escape_for_missing_file() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path().join("ws");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("linked")).unwrap();

        let error = write_file(&root, "linked/new.txt", "no").unwrap_err();

        assert!(error.to_string().contains("outside"));
        assert!(!outside.join("new.txt").exists());
    }
}
