//! Adapted from CC Switch live/engine.rs: plan -> stage -> reread -> rename.
//! Copyright (c) 2025 Jason Young. MIT; see THIRD_PARTY_NOTICES.md.
//! External writers do not share our lock; comparison is optimistic, leaving
//! the same small compare/rename race as CC Switch.
use super::*;
pub const CONFLICT: &str = "Native configuration changed: reload before applying.";
pub fn stage(path: &Path, bytes: &[u8]) -> Result<tempfile::NamedTempFile> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Invalid configuration path"))?;
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    Ok(file)
}
pub fn commit(file: tempfile::NamedTempFile, path: &Path, expected: Option<&str>) -> Result<()> {
    if read(path)?.as_deref() != expected {
        bail!(CONFLICT);
    }
    replace(file, path)
}

/// Puts a staged file in place. Renaming over the target is atomic, but a file
/// mounted on its own (a container that shares one configuration file of its
/// host: `-v ~/.codex/config.toml:/home/node/.codex/config.toml`) cannot be
/// renamed over: Linux answers EBUSY, and applying an upstream failed with
/// "Device or resource busy (os error 16)". Such a file is rewritten in place
/// instead, which is the only way to change it; anything else stays atomic.
pub fn replace(file: tempfile::NamedTempFile, path: &Path) -> Result<()> {
    match file.persist(path) {
        Ok(_) => Ok(()),
        Err(error) if is_mount_point_busy(&error.error) => {
            let bytes = std::fs::read(error.file.path())?;
            rewrite_in_place(path, &bytes)
        }
        Err(error) => Err(error.error.into()),
    }
}

fn is_mount_point_busy(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::ResourceBusy || error.raw_os_error() == Some(16)
}

fn rewrite_in_place(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut target = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)?;
    target.write_all(bytes)?;
    target.sync_all()?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_target_that_cannot_be_renamed_over_is_rewritten_in_place() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        private_write(&path, b"model = \"before\"\n").unwrap();
        let inode = |p: &Path| std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(p).unwrap());
        let original = inode(&path);
        // A rename refused with EBUSY, as for a file mounted on its own.
        rewrite_in_place(&path, b"model = \"after\"\n").unwrap();
        assert_eq!(read(&path).unwrap().unwrap(), "model = \"after\"\n");
        assert_eq!(inode(&path), original, "the mounted file itself is changed");
        assert!(is_mount_point_busy(&std::io::Error::from_raw_os_error(16)));
        assert!(!is_mount_point_busy(&std::io::Error::from_raw_os_error(13)));
    }
    #[test]
    fn reread_before_rename_preserves_a_concurrent_claude_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        private_write(&path, b"{\"model\":\"before\"}").unwrap();
        let staged = stage(&path, b"{\"model\":\"ours\"}").unwrap();
        private_write(&path, b"{\"model\":\"external\",\"hooks\":{}}").unwrap();
        assert!(commit(staged, &path, Some("{\"model\":\"before\"}")).is_err());
        assert_eq!(
            read(&path).unwrap().unwrap(),
            "{\"model\":\"external\",\"hooks\":{}}"
        );
    }
}
