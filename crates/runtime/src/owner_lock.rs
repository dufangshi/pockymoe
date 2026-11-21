//! Exclusive `flock` owner locks for the Supervisor database and its private
//! environment sidecar.
//!
//! A `flock` belongs to the open file description. A child forked by this
//! process shares it until the child execs (close-on-exec only closes it then),
//! so right after this process drops an owner lock, a fork that was in flight
//! can still hold it for a moment. Reopening the same store in this process
//! (tests do it constantly; a restart within one process would too) must wait
//! that out instead of reporting another Supervisor. Any other holder is
//! refused exactly as before.

use std::collections::HashMap;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long after a release this process may still see its own forked copy.
const RECENT_RELEASE: Duration = Duration::from_secs(5);
/// How long to wait for such a copy to go away.
const INHERITED_WAIT: Duration = Duration::from_secs(2);

static RELEASED: Mutex<Option<HashMap<PathBuf, Instant>>> = Mutex::new(None);

fn released_recently(path: &Path) -> bool {
    RELEASED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|released| released.get(path))
        .is_some_and(|at| at.elapsed() < RECENT_RELEASE)
}

/// An acquired owner lock; dropping it releases the lock and records that
/// this process released it.
pub(crate) struct OwnerLock {
    file: Option<File>,
    path: PathBuf,
}

impl OwnerLock {
    /// Try to take `file`'s exclusive lock without blocking, waiting briefly
    /// only when this process itself released `path` moments ago.
    pub(crate) fn try_acquire(file: File, path: &Path) -> Result<Self, (File, TryLockError)> {
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => {
                    return Ok(Self {
                        file: Some(file),
                        path: path.to_path_buf(),
                    })
                }
                Err(TryLockError::WouldBlock)
                    if started.elapsed() < INHERITED_WAIT && released_recently(path) =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err((file, error)),
            }
        }
    }
}

impl Drop for OwnerLock {
    fn drop(&mut self) {
        // Close (and so release) first, then record the release.
        drop(self.file.take());
        let mut released = RELEASED.lock().unwrap_or_else(|e| e.into_inner());
        let released = released.get_or_insert_with(HashMap::new);
        released.retain(|_, at| at.elapsed() < RECENT_RELEASE);
        released.insert(self.path.clone(), Instant::now());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn open(path: &Path) -> File {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .unwrap()
    }

    #[test]
    fn waits_out_an_inherited_copy_only_after_its_own_release() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner.lock");
        let first = OwnerLock::try_acquire(open(&path), &path).ok().unwrap();
        // A separate open file description stands in for a forked child's copy.
        let other = open(&path);
        // Held by someone else, never released here: refused at once.
        let started = Instant::now();
        assert!(OwnerLock::try_acquire(open(&path), &path).is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
        drop(first);
        other.try_lock().unwrap();
        let holder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            drop(other);
        });
        // Released here a moment ago and briefly held elsewhere: waited out.
        let reopened = OwnerLock::try_acquire(open(&path), &path);
        assert!(reopened.is_ok());
        holder.join().unwrap();
    }
}
