// Compile with rustc, not cargo test --release: this exercises the exact
// production launcher without linking the entire Supervisor test binary.
#[path = "../crates/supervisor/src/distribution/windows_launcher.rs"]
mod windows_launcher;
use std::{
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn json(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
fn stop(pid: &str) {
    let _ = Command::new("taskkill.exe")
        .args(["/PID", pid, "/T", "/F"])
        .output();
}
struct OwnedWorker {
    root: std::path::PathBuf,
    launcher: String,
}
impl Drop for OwnedWorker {
    fn drop(&mut self) {
        if let Ok(pid) = fs::read_to_string(self.root.join("started")) {
            stop(pid.trim());
        }
        stop(&self.launcher);
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(marker) = std::env::args().nth(1) {
        eprintln!("INFO ordinary native stderr must not terminate this worker");
        thread::sleep(Duration::from_millis(500));
        fs::write(marker, std::process::id().to_string())?;
        thread::sleep(Duration::from_secs(60));
        return Ok(());
    }
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir()
        .join(format!("pockymoe-worker-smoke-{unique}"))
        .join("space folder");
    fs::create_dir_all(&root)?;
    let marker = root.join("started");
    let log = root.join("worker.log");
    let payload = root.join("payload.json");
    let launcher = root.join("launch.ps1");
    let executable = std::env::current_exe()?;
    fs::write(
        &payload,
        format!(
            "{{\"binary\":{},\"args\":[{}],\"environment\":{{}},\"log\":{}}}",
            json(&executable.to_string_lossy()),
            json(&marker.to_string_lossy()),
            json(&log.to_string_lossy())
        ),
    )?;
    fs::write(&launcher, windows_launcher::source(Path::new(&payload)))?;
    let command_line = format!("powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{}\"", launcher.display());
    let script = format!("$r=Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{{CommandLine='{}'}}; if($r.ReturnValue -ne 0){{exit 1}}; Write-Output $r.ProcessId", command_line.replace('\'', "''"));
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()?;
    if !output.status.success() {
        return Err("Disposable WMI launcher could not start".into());
    }
    let launcher_pid = String::from_utf8(output.stdout)?.trim().to_string();
    if !launcher_pid.chars().all(|c| c.is_ascii_digit()) || launcher_pid.is_empty() {
        return Err("Invalid owned launcher PID".into());
    }
    let owned = OwnedWorker {
        root,
        launcher: launcher_pid,
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    if !marker.exists() {
        return Err("Normal stderr terminated the disposable worker".into());
    }
    let pid = fs::read_to_string(marker)?;
    stop(pid.trim());
    let output = Command::new("tasklist.exe")
        .args([
            "/FI",
            &format!("PID eq {}", pid.trim()),
            "/FO",
            "CSV",
            "/NH",
        ])
        .output()?;
    if String::from_utf8_lossy(&output.stdout).contains(&format!("\"{}\"", pid.trim())) {
        return Err("Owned worker did not stop".into());
    }
    let bytes = fs::read(log)?;
    let text = if bytes.starts_with(&[0xff, 0xfe]) {
        String::from_utf16(
            &bytes[2..]
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>(),
        )?
    } else {
        String::from_utf8(bytes)?
    };
    if !text.contains("ordinary native stderr") {
        return Err("Disposable worker stderr was not retained".into());
    }
    drop(owned);
    println!("Production Windows launcher: native stderr survived and the owned worker stopped");
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
