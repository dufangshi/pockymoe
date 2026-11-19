//! The update lock is a directory, so creating it is atomic on every platform.
//! Its owner record names the job and the process that took it. A later request
//! can then tell a running job from one whose process stopped before it wrote
//! a job status, which the status-based recovery in `job` cannot see.
use super::*;
use std::sync::OnceLock;

const OWNER: &str = "owner.json";
/// A lock with no readable owner is only that young between `create_dir` and
/// the owner write, or it comes from a release that wrote none.
const UNOWNED_GRACE_MS: u64 = 30_000;
/// Scheduling or a host-managed handoff writes its first status within seconds.
/// Past this, a status-less lock is stale even if its PID was reused.
const UNSCHEDULED_LIMIT_MS: u64 = 10 * 60_000;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Owner {
    job_id: String,
    pid: u32,
    /// A container restart can give the next Supervisor the same PID.
    instance: String,
    created_at: u64,
}

fn instance() -> &'static str {
    static INSTANCE: OnceLock<String> = OnceLock::new();
    INSTANCE.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

/// Take the lock for a new job and return its ID. A stale lock is removed once.
pub(super) fn acquire(lock: &Path, status_file: &Path) -> Result<String> {
    let job_id = uuid::Uuid::new_v4().to_string();
    for attempt in 0..2 {
        match std::fs::create_dir(lock) {
            Ok(()) => {
                let owner = Owner {
                    job_id: job_id.clone(),
                    pid: std::process::id(),
                    instance: instance().into(),
                    created_at: now(),
                };
                if let Err(error) = write(&lock.join(OWNER), &owner) {
                    remove(lock);
                    return Err(error);
                }
                return Ok(job_id);
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists
                    && attempt == 0
                    && stale(lock, status_file) =>
            {
                remove(lock);
            }
            Err(error) => {
                return Err(error).context(
                    "An update is already in progress; retry in a minute if no update is running",
                )
            }
        }
    }
    unreachable!()
}

/// Release a job's lock. A lock that another job has since taken stays.
pub(super) fn release(lock: &Path, job_id: &str) {
    match read::<Owner>(&lock.join(OWNER)) {
        Ok(owner) if !job_id.is_empty() && owner.job_id != job_id => {}
        _ => remove(lock),
    }
}

fn remove(lock: &Path) {
    let _ = std::fs::remove_file(lock.join(OWNER));
    let _ = std::fs::remove_dir(lock);
}

fn stale(lock: &Path, status_file: &Path) -> bool {
    let status = read::<Value>(status_file).unwrap_or(Value::Null);
    let Ok(owner) = read::<Owner>(&lock.join(OWNER)) else {
        let age = std::fs::metadata(lock)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .unwrap_or_default();
        return !active(&status) && age.as_millis() as u64 > UNOWNED_GRACE_MS;
    };
    if status["jobId"] == owner.job_id.as_str() {
        // `job` has already failed a stopped worker; a terminal status only
        // means its process exited before removing the lock.
        return !active(&status);
    }
    // The owner stopped before writing this job's first status.
    if now().saturating_sub(owner.created_at) > UNSCHEDULED_LIMIT_MS {
        return true;
    }
    if owner.pid == std::process::id() {
        owner.instance != instance()
    } else {
        !service::alive(owner.pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(root: &Path) -> (PathBuf, PathBuf) {
        let status = root.join("updates/supervisor-update.json");
        private_dir(status.parent().unwrap()).unwrap();
        (status.with_file_name("supervisor-update.lock"), status)
    }

    fn owner(lock: &Path, edit: impl FnOnce(&mut Owner)) {
        let mut owner: Owner = read(&lock.join(OWNER)).unwrap();
        edit(&mut owner);
        write(&lock.join(OWNER), &owner).unwrap();
    }

    #[test]
    fn a_lock_whose_process_stopped_before_scheduling_is_recovered() {
        let root = tempfile::tempdir().unwrap();
        let (lock, status) = paths(root.path());
        let first = acquire(&lock, &status).unwrap();
        // A live job in this same process keeps its lock.
        assert!(acquire(&lock, &status).is_err());
        // The previous Supervisor stopped: its PID no longer exists.
        owner(&lock, |o| o.pid = u32::MAX - 1);
        let second = acquire(&lock, &status).unwrap();
        assert_ne!(first, second);
        // A restarted container reuses the PID, but not the process instance.
        owner(&lock, |o| o.instance = "previous-process".into());
        let third = acquire(&lock, &status).unwrap();
        // A reused PID cannot keep a status-less lock forever.
        owner(&lock, |o| {
            o.pid = 1;
            o.created_at = now() - UNSCHEDULED_LIMIT_MS - 1;
        });
        let fourth = acquire(&lock, &status).unwrap();
        assert!(![first, second, third].contains(&fourth));
    }

    #[test]
    fn a_job_status_decides_once_it_is_written() {
        let root = tempfile::tempdir().unwrap();
        let (lock, status) = paths(root.path());
        let job_id = acquire(&lock, &status).unwrap();
        owner(&lock, |o| o.pid = u32::MAX - 1);
        // An active job keeps the lock even after the starting process stopped;
        // `job` fails it first when its worker stopped.
        write(&status, &json!({"phase":"installing","jobId":job_id})).unwrap();
        assert!(acquire(&lock, &status).is_err());
        // A finished job whose process exited before releasing the lock.
        write(&status, &json!({"phase":"completed","jobId":job_id})).unwrap();
        let next = acquire(&lock, &status).unwrap();
        // The finished job's late release leaves the new lock alone.
        release(&lock, &job_id);
        assert!(lock.exists());
        release(&lock, &next);
        assert!(!lock.exists());
    }

    #[cfg(unix)]
    #[test]
    fn an_unowned_lock_is_recovered_only_after_its_grace_period() {
        let root = tempfile::tempdir().unwrap();
        let (lock, status) = paths(root.path());
        std::fs::create_dir(&lock).unwrap();
        assert!(acquire(&lock, &status).is_err());
        let old = SystemTime::now() - Duration::from_millis(UNOWNED_GRACE_MS + 5_000);
        std::fs::File::open(&lock)
            .unwrap()
            .set_modified(old)
            .unwrap();
        acquire(&lock, &status).unwrap();
        assert!(lock.join(OWNER).exists());
    }
}
