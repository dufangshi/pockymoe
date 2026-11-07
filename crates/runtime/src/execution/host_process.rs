//! A Host process guardian is not an isolation environment. It is a Linux
//! subreaper with a durable, metadata-only exit receipt, so an approved migration
//! can prove that pre-migration writers ended after a Supervisor crash.
use super::RunRequest;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    fs::File,
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
};

#[derive(Clone)]
pub struct HostProcessContext {
    pub directory: PathBuf,
    pub thread_id: String,
}
tokio::task_local! {static HOST:Option<HostProcessContext>;}
thread_local! {static BLOCKING:RefCell<Option<HostProcessContext>>=const{RefCell::new(None)};}
pub fn current() -> Option<HostProcessContext> {
    HOST.try_with(Clone::clone)
        .ok()
        .flatten()
        .or_else(|| BLOCKING.with(|c| c.borrow().clone()))
}
pub fn enabled_context() -> Option<HostProcessContext> {
    executable()?;
    current()
}
pub async fn scope<T>(
    context: Option<HostProcessContext>,
    future: impl std::future::Future<Output = T>,
) -> T {
    HOST.scope(context, future).await
}
pub fn blocking_scope<T>(context: Option<HostProcessContext>, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<HostProcessContext>);
    impl Drop for Restore {
        fn drop(&mut self) {
            BLOCKING.with(|c| {
                c.replace(self.0.take());
            });
        }
    }
    let _restore = Restore(BLOCKING.with(|c| c.replace(context)));
    action()
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    #[serde(alias = "thread_id")]
    thread_id: String,
    id: String,
    pid: Option<u32>,
    start: Option<String>,
    stopped: bool,
}

fn executable() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    (cfg!(target_os = "linux") && ENABLED.load(std::sync::atomic::Ordering::Relaxed)).then_some(exe)
}
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The CLI binary implements the internal guardian subcommand. Library hosts
/// must opt in only when their own executable supports that entry point.
pub fn enable() {
    ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
}
pub(crate) fn is_enabled() -> bool {
    executable().is_some()
}

#[cfg(unix)]
fn private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let meta = path.symlink_metadata()?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink() && meta.uid() == unsafe { libc::geteuid() },
        "Host guardian storage is not private owned storage"
    );
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    ensure!(
        path.canonicalize()? == path,
        "Host guardian storage cannot traverse symlinks"
    );
    Ok(())
}
#[cfg(not(unix))]
fn private_directory(_path: &Path) -> Result<()> {
    anyhow::bail!("Host process guardians require Linux");
}
fn persist(path: &Path, receipt: &Receipt) -> Result<()> {
    use std::io::Write;
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().context("Host receipt parent missing")?)?;
    file.write_all(&serde_json::to_vec(receipt)?)?;
    file.as_file_mut().sync_all()?;
    file.persist(path)
        .map_err(|_| anyhow::anyhow!("Host guardian receipt could not be persisted"))?;
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
pub(super) fn process_start(pid: u32) -> Result<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    Ok(stat
        .rsplit_once(") ")
        .context("invalid process stat")?
        .1
        .split_whitespace()
        .nth(19)
        .context("process start time missing")?
        .into())
}

impl HostProcessContext {
    fn prepare(&self, id: &str) -> Result<PathBuf> {
        private_directory(&self.directory)?;
        let path = self.directory.join(format!("{id}.json"));
        persist(
            &path,
            &Receipt {
                thread_id: self.thread_id.clone(),
                id: id.into(),
                pid: None,
                start: None,
                stopped: false,
            },
        )?;
        Ok(path)
    }

    /// No metadata receipt is inferred from a missing PID. Only the guardian's
    /// receipt after it has reaped every descendant proves quiescence.
    pub async fn quiesce(&self) -> Result<usize> {
        self.quiesce_filter(None).await
    }
    pub async fn quiesce_process(&self, id: &str) -> Result<usize> {
        self.quiesce_filter(Some(id)).await
    }
    async fn quiesce_filter(&self, id: Option<&str>) -> Result<usize> {
        if !self.directory.exists() {
            return Ok(0);
        }
        private_directory(&self.directory)?;
        let mut count = 0;
        for entry in std::fs::read_dir(&self.directory)? {
            let path = entry?.path();
            if path.extension().is_none_or(|s| s != "json") {
                continue;
            }
            let bytes = std::fs::read(&path)?;
            ensure!(bytes.len() < 16384, "invalid Host guardian receipt");
            let mut receipt: Receipt = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("invalid Host guardian receipt"))?;
            if receipt.thread_id != self.thread_id || id.is_some_and(|id| receipt.id != id) {
                continue;
            }
            count += 1;
            if receipt.stopped {
                continue;
            }
            let Some(pid) = receipt.pid else {
                receipt.stopped = true;
                persist(&path, &receipt)?;
                continue;
            };
            if process_start(pid).ok() == receipt.start {
                let command = std::fs::read(format!("/proc/{pid}/cmdline"))?;
                let fields = command.split(|b| *b == 0).collect::<Vec<_>>();
                ensure!(
                    fields.len() >= 4
                        && fields[1] == b"environment-worker"
                        && matches!(fields[2], b"host-run" | b"host-shell")
                        && fields[3] == receipt.id.as_bytes(),
                    "Host guardian identity is uncertain; automatic migration is blocked"
                );
                signal(
                    pid,
                    receipt
                        .start
                        .as_deref()
                        .context("Host guardian start time missing")?,
                    15,
                )?;
            }
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let bytes = std::fs::read(&path)?;
                let current: Receipt = serde_json::from_slice(&bytes)
                    .map_err(|_| anyhow::anyhow!("invalid Host guardian receipt"))?;
                if current.stopped {
                    break;
                }
                ensure!(tokio::time::Instant::now()<deadline,"Host guardian did not prove all writers stopped; owner process recovery is required before migration");
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
        Ok(count)
    }
}

#[cfg(target_os = "linux")]
fn signal(pid: u32, start: &str, signal: i32) -> Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32 };
    if fd < 0 {
        ensure!(
            process_start(pid).is_err(),
            "Linux pidfd is required to safely signal a Host guardian"
        );
        return Ok(());
    }
    let fd = unsafe { File::from_raw_fd(fd) };
    if process_start(pid).ok().as_deref() != Some(start) {
        return Ok(());
    }
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    ensure!(
        result == 0 || process_start(pid).is_err(),
        "could not stop the owned Host guardian"
    );
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn signal(_pid: u32, _start: &str, _signal: i32) -> Result<()> {
    anyhow::bail!("Host process guardians require Linux");
}

/// Spawn an ACP/finite Host command without putting its environment or secrets
/// into argv, a receipt, or a file. The original environment stays on the pipe's
/// parent/guardian/child; the guardian strips its own metadata from the harness.
pub async fn spawn(
    command: &mut Command,
    id: &str,
    inherit: bool,
    stdin_closed: bool,
) -> Result<Child> {
    let (Some(context), Some(exe)) = (current(), executable()) else {
        return Ok(command.spawn()?);
    };
    let path = context.prepare(id)?;
    let original = command.as_std();
    let argv = std::iter::once(original.get_program())
        .chain(original.get_args())
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    let cwd = original
        .get_current_dir()
        .map(Path::to_path_buf)
        .unwrap_or(std::env::current_dir()?);
    let mut guardian = Command::new(exe);
    guardian
        .args(["environment-worker", "host-run", id])
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    #[cfg(unix)]
    guardian.process_group(0);
    if !inherit {
        guardian.env_clear();
    }
    for (key, value) in original.get_envs() {
        match value {
            Some(value) => {
                guardian.env(key, value);
            }
            None => {
                guardian.env_remove(key);
            }
        }
    }
    guardian.env("POCKYMOE_HOST_PROCESS_RECEIPT", &path);
    let mut child = guardian
        .spawn()
        .context("Host process guardian could not start")?;
    let pid = child.id().context("Host guardian PID missing")?;
    persist(
        &path,
        &Receipt {
            thread_id: context.thread_id,
            id: id.into(),
            pid: Some(pid),
            start: Some(process_start(pid)?),
            stopped: false,
        },
    )?;
    let request = RunRequest {
        id: id.into(),
        argv,
        cwd: cwd.to_string_lossy().into_owned(),
        env: Vec::new(),
        stdin_closed,
    };
    let stdin = child
        .stdin
        .as_mut()
        .context("Host guardian stdin missing")?;
    stdin.write_all(&serde_json::to_vec(&request)?).await?;
    stdin.write_all(b"\n").await?;
    Ok(child)
}

/// The PTY guardian gets a metadata receipt, never a credential or command file.
pub fn terminal(id: &str) -> Result<Option<(PathBuf, PathBuf)>> {
    let (Some(context), Some(exe)) = (current(), executable()) else {
        return Ok(None);
    };
    Ok(Some((exe, context.prepare(id)?)))
}
pub fn register_terminal(path: &Path, pid: u32) -> Result<()> {
    let mut receipt: Receipt = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        receipt.pid.is_none() && !receipt.stopped,
        "Host PTY receipt already used"
    );
    receipt.pid = Some(pid);
    receipt.start = Some(process_start(pid)?);
    persist(path, &receipt)
}
/// `Command` cannot expose its env_clear/stdin settings. The caller declares
/// environment inheritance explicitly; this finite-command path closes stdin.
pub fn spawn_sync(
    command: &mut std::process::Command,
    id: &str,
    inherit: bool,
) -> Result<std::process::Child> {
    use std::io::Write;
    let (Some(context), Some(exe)) = (current(), executable()) else {
        return Ok(command.spawn()?);
    };
    let path = context.prepare(id)?;
    let argv = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    let cwd = command
        .get_current_dir()
        .map(Path::to_path_buf)
        .unwrap_or(std::env::current_dir()?);
    let mut guardian = std::process::Command::new(exe);
    guardian
        .args(["environment-worker", "host-run", id])
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !inherit {
        guardian.env_clear();
    }
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => {
                guardian.env(key, value);
            }
            None => {
                guardian.env_remove(key);
            }
        }
    }
    guardian.env("POCKYMOE_HOST_PROCESS_RECEIPT", &path);
    let mut child = guardian.spawn()?;
    register_terminal(&path, child.id())?;
    let request = RunRequest {
        id: id.into(),
        argv,
        cwd: cwd.to_string_lossy().into_owned(),
        env: Vec::new(),
        stdin_closed: true,
    };
    let stdin = child
        .stdin
        .as_mut()
        .context("Host guardian stdin missing")?;
    stdin.write_all(&serde_json::to_vec(&request)?)?;
    stdin.write_all(b"\n")?;
    Ok(child)
}
impl HostProcessContext {
    pub fn stop_process_sync(&self, id: &str) -> Result<()> {
        stop_terminal(&self.directory.join(format!("{id}.json")))
    }
}
pub fn stop_terminal(path: &Path) -> Result<()> {
    let receipt: Receipt = serde_json::from_slice(&std::fs::read(path)?)?;
    if receipt.stopped {
        return Ok(());
    }
    let pid = receipt
        .pid
        .context("Host terminal has no registered process")?;
    signal(
        pid,
        receipt
            .start
            .as_deref()
            .context("Host terminal start time missing")?,
        15,
    )?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let receipt: Receipt = serde_json::from_slice(&std::fs::read(path)?)?;
        if receipt.stopped {
            return Ok(());
        }
        ensure!(
            std::time::Instant::now() < deadline,
            "Host terminal cleanup is not confirmed"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

pub(super) async fn await_registered(path: &Path, id: &str) -> Result<()> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let receipt: Receipt = serde_json::from_slice(&std::fs::read(path)?)?;
        ensure!(
            receipt.id == id && !receipt.stopped,
            "Host process launch is no longer authorized"
        );
        if receipt.pid == Some(std::process::id())
            && receipt.start.as_deref() == Some(process_start(std::process::id())?.as_str())
        {
            return Ok(());
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "Host process receipt was not registered"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

pub(super) fn receipt_path() -> Result<PathBuf> {
    let path = std::env::var_os("POCKYMOE_HOST_PROCESS_RECEIPT")
        .map(PathBuf::from)
        .context("Host guardian receipt required")?;
    private_directory(path.parent().context("Host receipt parent missing")?)?;
    Ok(path)
}
pub(super) fn stopped(path: &Path) -> Result<()> {
    let mut receipt: Receipt = serde_json::from_slice(&std::fs::read(path)?)
        .map_err(|_| anyhow::anyhow!("invalid Host guardian receipt"))?;
    ensure!(
        receipt.pid == Some(std::process::id())
            && receipt.start.as_deref() == Some(process_start(std::process::id())?.as_str()),
        "Host guardian receipt identity changed"
    );
    receipt.stopped = true;
    persist(path, &receipt)
}
