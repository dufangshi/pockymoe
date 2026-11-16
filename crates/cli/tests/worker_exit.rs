//! A Host guardian must be gone once it has stopped, while its parent still
//! holds its stdin: the Supervisor keeps that pipe for as long as it owns the
//! process. A guardian that outlived its stop stayed forever, and a busy device
//! collected thousands of them.
#![cfg(target_os = "linux")]
use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn start_time(pid: u32) -> String {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields = stat.rsplit_once(") ").unwrap().1;
    fields.split_whitespace().nth(19).unwrap().into()
}

struct Guardian {
    child: Child,
    receipt: std::path::PathBuf,
    directory: std::path::PathBuf,
}
impl Drop for Guardian {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// Starts a guardian as `host_process::spawn` does; `argv` is its launch frame.
fn guardian(argv: Option<&[&str]>) -> Guardian {
    let directory = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("pockymoe-guardian-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = directory.join(format!("{id}.json"));
    let write = |pid: Option<u32>| {
        let value = serde_json::json!({"threadId":"thread","id":id,"pid":pid,"start":pid.map(start_time),"stopped":false});
        let staged = receipt.with_extension("tmp");
        std::fs::write(&staged, value.to_string()).unwrap();
        std::fs::rename(&staged, &receipt).unwrap();
    };
    write(None);
    let mut child = Command::new(env!("CARGO_BIN_EXE_pockymoe"))
        .args(["environment-worker", "host-run", &id])
        .env("POCKYMOE_HOST_PROCESS_RECEIPT", &receipt)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    write(Some(child.id()));
    if let Some(argv) = argv {
        let frame =
            serde_json::json!({"id":id,"argv":argv,"cwd":"/tmp","env":[],"stdinClosed":true});
        let stdin = child.stdin.as_mut().unwrap();
        stdin.write_all(format!("{frame}\n").as_bytes()).unwrap();
        stdin.flush().unwrap();
    }
    Guardian {
        child,
        receipt,
        directory,
    }
}

/// Whether the guardian exits within the time, with its stdin still held open.
fn exits(guardian: &mut Guardian) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if guardian.child.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}
fn stopped(guardian: &Guardian) -> bool {
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&guardian.receipt).unwrap()).unwrap();
    receipt["stopped"] == true
}
fn terminate(guardian: &Guardian) {
    // Past registration and into its wait, as when the Supervisor stops it.
    std::thread::sleep(Duration::from_millis(700));
    let pid = guardian.child.id().to_string();
    assert!(Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap()
        .success());
}

#[test]
fn a_stopped_guardian_exits_while_its_parent_still_holds_its_stdin() {
    let mut running = guardian(Some(&["sleep", "100"]));
    terminate(&running);
    assert!(exits(&mut running), "stopped while its command ran");
    assert!(stopped(&running));

    let mut waiting = guardian(None);
    terminate(&waiting);
    assert!(exits(&mut waiting), "stopped before a launch frame");
    assert!(stopped(&waiting));
}

#[test]
fn a_guardian_exits_when_its_command_ends_or_cannot_start() {
    let mut finished = guardian(Some(&["true"]));
    assert!(exits(&mut finished));
    assert!(stopped(&finished));
    let mut missing = guardian(Some(&["/nonexistent-pockymoe-program"]));
    assert!(exits(&mut missing));
    assert!(stopped(&missing));
}
