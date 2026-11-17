//! Confined removal of directories the runtime created itself: old native
//! releases, update staging directories, adapter installations
//! (docs/github-runtime.md, docs/managed-acp-adapters.md).
//!
//! Every removal is a direct child of a known parent. It is renamed to a
//! `.trash-*` sibling first and only then deleted, so a failure leaves either
//! the untouched directory or a trash directory that the next sweep removes.
//! Windows refuses to rename a directory whose executable is running, which
//! makes that case a harmless no-op.
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const TRASH_PREFIX: &str = ".trash-";

/// The executables and command lines of every process this user can see.
pub struct LiveProcesses {
    texts: Vec<String>,
}

impl LiveProcesses {
    /// Blocking. `None` when the process table cannot be read: callers must
    /// then keep whatever they cannot prove unused.
    pub fn collect() -> Option<Self> {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .without_tasks()
                .with_cmd(UpdateKind::Always)
                .with_exe(UpdateKind::Always),
        );
        let processes = system.processes();
        if processes.is_empty() {
            return None;
        }
        let mut texts = Vec::new();
        for process in processes.values() {
            if let Some(exe) = process.exe() {
                texts.push(exe.to_string_lossy().into_owned());
            }
            texts.extend(
                process
                    .cmd()
                    .iter()
                    .map(|part| part.to_string_lossy().into_owned()),
            );
        }
        Some(Self::from_texts(texts))
    }

    pub fn from_texts(texts: impl IntoIterator<Item = String>) -> Self {
        Self {
            texts: texts.into_iter().map(|text| normalize(&text)).collect(),
        }
    }

    /// Whether any process runs from, or names a path inside, `dir`. A false
    /// positive only keeps a directory longer.
    pub fn uses(&self, dir: &Path) -> bool {
        let mut needles = vec![normalize(&dir.to_string_lossy())];
        if let Ok(canonical) = std::fs::canonicalize(dir) {
            needles.push(normalize(&canonical.to_string_lossy()));
        }
        needles.retain(|needle| !needle.is_empty());
        self.texts
            .iter()
            .any(|text| needles.iter().any(|needle| contains_path(text, needle)))
    }
}

fn normalize(text: &str) -> String {
    let text = if cfg!(windows) {
        text.replace('/', "\\").to_lowercase()
    } else {
        text.to_string()
    };
    // `\\?\C:\...` from canonicalize on Windows.
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.trim_end_matches(['/', '\\']).to_string()
}

/// `dir` occurs in `text` as a whole path prefix: `/a/0.13.1` matches
/// `/a/0.13.1/remote-codex` but not `/a/0.13.10`.
fn contains_path(text: &str, dir: &str) -> bool {
    text.match_indices(dir).any(|(start, _)| {
        text[start + dir.len()..]
            .chars()
            .next()
            .is_none_or(|next| !(next.is_alphanumeric() || matches!(next, '.' | '-' | '_')))
    })
}

/// Latest modification time of `path` and its direct entries, without
/// following symlinks.
pub fn newest_mtime(path: &Path) -> Option<SystemTime> {
    let mut newest = std::fs::symlink_metadata(path).ok()?.modified().ok()?;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries {
            let modified = entry.ok()?.metadata().ok()?.modified().ok()?;
            newest = newest.max(modified);
        }
    }
    Some(newest)
}

/// True only when `path` and its direct entries are all older than `age`.
/// Unknown or future times count as recent.
pub fn older_than(path: &Path, age: Duration, now: SystemTime) -> bool {
    newest_mtime(path)
        .and_then(|newest| now.duration_since(newest).ok())
        .is_some_and(|elapsed| elapsed >= age)
}

/// Remove `child`, which must be a real directory directly inside `parent`.
/// Symlinks, files and anything outside `parent` are refused.
pub fn remove_child_dir(parent: &Path, child: &Path) -> io::Result<()> {
    let refused = |why: &str| io::Error::new(io::ErrorKind::InvalidInput, why.to_string());
    let parent = std::fs::canonicalize(parent)?;
    let name = child.file_name().ok_or_else(|| refused("no file name"))?;
    if Path::new(name).components().count() != 1
        || !matches!(
            Path::new(name).components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        return Err(refused("not a plain name"));
    }
    let child_parent = child.parent().ok_or_else(|| refused("no parent"))?;
    if std::fs::canonicalize(child_parent)? != parent {
        return Err(refused("outside the expected parent"));
    }
    let target = parent.join(name);
    let metadata = std::fs::symlink_metadata(&target)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(refused("not a directory"));
    }
    let trash = parent.join(format!("{TRASH_PREFIX}{}", uuid::Uuid::new_v4()));
    std::fs::rename(&target, &trash)?;
    if let Err(error) = std::fs::remove_dir_all(&trash) {
        // Out of sight already; `sweep_trash` finishes it next time.
        tracing::debug!(%error, path = %trash.display(), "partial cleanup; retried later");
    }
    Ok(())
}

/// Delete `.trash-*` directories a previous removal left behind.
pub fn sweep_trash(parent: &Path) {
    for path in child_dirs(parent) {
        let is_trash = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(TRASH_PREFIX));
        if is_trash {
            if let Err(error) = std::fs::remove_dir_all(&path) {
                tracing::debug!(%error, path = %path.display(), "trash not removed");
            }
        }
    }
}

/// Real directories (not symlinks) directly inside `parent`.
pub fn child_dirs(parent: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry.file_type().ok()?.is_dir().then(|| entry.path())
        })
        .collect()
}

/// Remove each path, logging instead of failing.
pub fn remove_all(parent: &Path, paths: &[PathBuf], what: &str) -> usize {
    let mut removed = 0;
    for path in paths {
        match remove_child_dir(parent, path) {
            Ok(()) => {
                removed += 1;
                tracing::info!(path = %path.display(), "removed old {what}");
            }
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "could not remove old {what}")
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_is_confined_to_real_child_directories() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("parent");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(parent.join("old/nested")).unwrap();
        std::fs::write(parent.join("old/nested/file"), "x").unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep"), "x").unwrap();
        std::fs::write(parent.join("file"), "x").unwrap();

        assert!(remove_child_dir(&parent, &outside).is_err());
        assert!(remove_child_dir(&parent, &parent.join("..")).is_err());
        assert!(remove_child_dir(&parent, &parent.join("old/nested")).is_err());
        assert!(remove_child_dir(&parent, &parent.join("file")).is_err());
        assert!(remove_child_dir(&parent, &parent.join("missing")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, parent.join("link")).unwrap();
            assert!(remove_child_dir(&parent, &parent.join("link")).is_err());
            assert!(parent.join("link").exists());
        }
        assert!(outside.join("keep").is_file());
        assert!(parent.join("old/nested/file").is_file());

        remove_child_dir(&parent, &parent.join("old")).unwrap();
        assert!(!parent.join("old").exists());
        assert!(parent.join("file").is_file());
        assert!(std::fs::read_dir(&parent).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(TRASH_PREFIX)));
    }

    #[cfg(unix)]
    #[test]
    fn removing_a_directory_never_follows_a_symlink_inside_it() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("parent");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(parent.join("old")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, parent.join("old/escape")).unwrap();
        remove_child_dir(&parent, &parent.join("old")).unwrap();
        assert!(outside.join("keep").is_file());
    }

    #[test]
    fn sweep_removes_only_trash() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".trash-1/inner")).unwrap();
        std::fs::create_dir_all(root.path().join("keep")).unwrap();
        sweep_trash(root.path());
        assert!(!root.path().join(".trash-1").exists());
        assert!(root.path().join("keep").is_dir());
    }

    #[test]
    fn live_paths_match_whole_components_only() {
        let live = LiveProcesses::from_texts([
            "/home/u/native/releases/0.13.10/remote-codex".to_string(),
            "node".to_string(),
            "--cache=/home/u/adapters/cache".to_string(),
        ]);
        assert!(live.uses(Path::new("/home/u/native/releases/0.13.10")));
        assert!(!live.uses(Path::new("/home/u/native/releases/0.13.1")));
        assert!(live.uses(Path::new("/home/u/adapters/cache")));
        assert!(!live.uses(Path::new("/home/u/adapters/cac")));
    }

    #[test]
    fn the_process_table_shows_this_test_running_from_its_directory() {
        let live = LiveProcesses::collect().expect("process table");
        let exe = std::env::current_exe().unwrap();
        assert!(live.uses(exe.parent().unwrap()));
        let unused = tempfile::tempdir().unwrap();
        assert!(!live.uses(unused.path()));
    }

    #[cfg(unix)]
    #[test]
    fn the_process_table_shows_a_path_named_in_arguments() {
        let named = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 30"])
            .arg(named.path().join("bin/adapter"))
            .spawn()
            .unwrap();
        let seen = LiveProcesses::collect().unwrap().uses(named.path());
        let _ = child.kill();
        let _ = child.wait();
        assert!(seen);
    }

    #[test]
    fn age_counts_the_newest_entry() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("log"), "x").unwrap();
        let now = SystemTime::now();
        assert!(!older_than(root.path(), Duration::from_secs(3600), now));
        assert!(older_than(
            root.path(),
            Duration::from_secs(3600),
            now + Duration::from_secs(7200)
        ));
        assert!(!older_than(
            &root.path().join("missing"),
            Duration::ZERO,
            now
        ));
    }
}
