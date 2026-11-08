//! Internal sandbox worker. This entry point never boots a Supervisor, opens a
//! device database, discovers host credentials, or registers a Relay device.
use super::RunRequest;
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{path::Path, process::Stdio};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

pub async fn run(mode: &str, args: &[String]) -> Result<()> {
    ensure!(
        cfg!(target_os = "linux"),
        "unsupported_environment_platform"
    );
    if matches!(mode, "host-run" | "host-shell") {
        let path = super::host_process::receipt_path()?;
        let id = args.first().context("Host process identity required")?;
        #[cfg(unix)]
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        #[cfg(unix)]
        let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
        super::host_process::await_registered(&path, id).await?;
        let action = async {
            if mode == "host-run" {
                process(id, Some(path.clone())).await
            } else {
                let program = args.get(1).context("Host shell required")?;
                execute(
                    RunRequest {
                        id: id.clone(),
                        argv: vec![program.clone()],
                        cwd: std::env::current_dir()?.to_string_lossy().into(),
                        env: Vec::new(),
                        stdin_closed: false,
                    },
                    None,
                    Some(path.clone()),
                )
                .await
            }
        };
        #[cfg(unix)]
        let result = tokio::select! {
            result=action=>result,
            _=terminate.recv()=>Err(anyhow::anyhow!("Host process launch cancelled")),
            _=hangup.recv()=>Err(anyhow::anyhow!("Host process transport closed")),
        };
        #[cfg(not(unix))]
        let result = action.await;
        // Errors/EOF before a launch frame can also be proved child-free.
        // Never infer this from a dead guardian PID: the registered guardian
        // itself reaps descendants and signs the receipt while still alive.
        terminate_descendants(std::process::id()).await?;
        super::host_process::stopped(&path)?;
        return result;
    }
    ensure!(
        std::env::var_os("POCKYMOE_ENVIRONMENT_WORKER").is_some(),
        "worker_requires_managed_environment"
    );
    match mode {
        "serve" => serve().await,
        "run" => process(args.first().context("process identity required")?, None).await,
        "request" => {
            let mut input = Vec::new();
            tokio::io::stdin()
                .take(64 * 1024 * 1024 + 1)
                .read_to_end(&mut input)
                .await?;
            ensure!(input.len() <= 64 * 1024 * 1024, "worker input too large");
            let result = operation(serde_json::from_slice(&input)?);
            let response = match result {
                Ok(value) => json!({"ok":true,"value":value}),
                Err(error) => json!({"ok":false,"error":error.to_string()}),
            };
            tokio::io::stdout()
                .write_all(&serde_json::to_vec(&response)?)
                .await?;
            Ok(())
        }
        "kill" => kill(args.first().context("process identity required")?),
        "shell" => {
            let id = args.first().context("process identity required")?.clone();
            let env = std::env::vars()
                .filter(|(k, _)| {
                    super::credentials::pointer_variable(k)
                        || matches!(
                            k.as_str(),
                            "POCKYMOE_TOKEN"
                                | "POCKYMOE_THREAD_ID"
                                | "POCKYMOE_URL"
                                | "HTTPS_PROXY"
                                | "https_proxy"
                                | "NO_PROXY"
                        )
                })
                .collect();
            execute(
                RunRequest {
                    id,
                    argv: vec!["/bin/sh".into()],
                    cwd: std::env::current_dir()?.to_string_lossy().into(),
                    env,
                    stdin_closed: false,
                },
                None,
                None,
            )
            .await
        }
        "tcp" => {
            let port: u16 = args.first().context("port required")?.parse()?;
            ensure!(port > 0, "invalid port");
            let socket =
                tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await?;
            socket.set_nodelay(true)?;
            let (mut read, mut write) = socket.into_split();
            let mut stdin = tokio::io::stdin();
            let mut stdout = tokio::io::stdout();
            let upload = async {
                tokio::io::copy(&mut stdin, &mut write).await?;
                write.shutdown().await
            };
            let download = async {
                let mut bytes = [0u8; 16384];
                loop {
                    let n = read.read(&mut bytes).await?;
                    if n == 0 {
                        return Ok::<(), std::io::Error>(());
                    }
                    stdout.write_all(&bytes[..n]).await?;
                    stdout.flush().await?;
                }
            };
            tokio::pin!(upload, download);
            let result = tokio::select! {
                result = &mut upload => match result {
                    Ok(()) => download.await,
                    Err(error) => Err(error),
                },
                result = &mut download => result,
            };
            // Tokio's stdin uses an uncancellable blocking read. This internal
            // one-shot worker must exit when the upstream closes, even if the
            // client keeps stdin open. Every response chunk is already flushed.
            // Returning would make runtime shutdown wait indefinitely for stdin.
            if result.is_err() {
                eprintln!("environment TCP transport interrupted");
            }
            std::process::exit(if result.is_ok() { 0 } else { 1 })
        }
        _ => bail!("unsupported worker mode"),
    }
}
#[cfg(unix)]
async fn serve() -> Result<()> {
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:45681").await?;
    tokio::spawn(async move {
        while let Ok((mut input, _)) = proxy.accept().await {
            tokio::spawn(async move {
                if let Ok(mut output) =
                    tokio::net::UnixStream::connect("/pockymoe/bridge/egress.sock").await
                {
                    let _ = tokio::io::copy_bidirectional(&mut input, &mut output).await;
                }
            });
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:45680").await?;
    loop {
        let (mut input, _) = listener.accept().await?;
        tokio::spawn(async move {
            if let Ok(mut output) =
                tokio::net::UnixStream::connect("/pockymoe/bridge/gateway.sock").await
            {
                let _ = tokio::io::copy_bidirectional(&mut input, &mut output).await;
            }
        });
    }
}
async fn process(expected: &str, host: Option<std::path::PathBuf>) -> Result<()> {
    let mut input = BufReader::new(tokio::io::stdin());
    let mut line = String::new();
    input.read_line(&mut line).await?;
    ensure!(line.len() < 1024 * 1024, "worker launch too large");
    let request: RunRequest = serde_json::from_str(&line)?;
    ensure!(request.id == expected, "worker process identity differs");
    ensure!(
        uuid::Uuid::parse_str(&request.id).is_ok(),
        "invalid process identity"
    );
    ensure!(!request.argv.is_empty(), "empty command");
    execute(request, Some(input), host).await
}
async fn execute(
    request: RunRequest,
    input: Option<BufReader<tokio::io::Stdin>>,
    host: Option<std::path::PathBuf>,
) -> Result<()> {
    ensure!(
        uuid::Uuid::parse_str(&request.id).is_ok() && !request.argv.is_empty(),
        "invalid process request"
    );
    #[cfg(target_os = "linux")]
    unsafe {
        ensure!(
            libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) == 0,
            "subreaper unavailable"
        );
        ensure!(
            libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) == 0,
            "worker trace protection unavailable"
        );
    }
    let mut request = request;
    let _native_credential_locks = if host.is_none() {
        super::credentials::prepare(&mut request)?
    } else {
        Vec::new()
    };
    if host.is_none()
        && request
            .env
            .iter()
            .any(|(k, v)| k == "POCKYMOE_ENV_CODEX" && v == "1")
    {
        std::fs::create_dir_all("/pockymoe/state/codex")?;
        std::fs::write("/pockymoe/state/codex/config.toml", "model_provider = \"pockymoe_environment\"\n[model_providers.pockymoe_environment]\nname = \"Pockymoe Environment\"\nbase_url = \"http://127.0.0.1:45680/model/codex\"\nenv_key = \"OPENAI_API_KEY\"\nwire_api = \"responses\"\nrequires_openai_auth = false\nsupports_websockets = false\n")?;
    }
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(unix)]
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    let is_terminal = input.is_none();
    let mut command = tokio::process::Command::new(&request.argv[0]);
    command.args(&request.argv[1..]).current_dir(&request.cwd);
    if host.is_none() {
        command
            .env_clear()
            .env("PATH", "/opt/pockymoe/bin:/usr/local/bin:/usr/bin:/bin")
            .env("HOME", "/pockymoe/state")
            .env("CODEX_HOME", "/pockymoe/state/codex")
            .env("CLAUDE_CONFIG_DIR", "/pockymoe/state/claude")
            .env("TMPDIR", "/tmp")
            .env("LANG", "C.UTF-8")
            .env("TERM", "xterm-256color")
            .env("DISABLE_AUTOUPDATER", "1")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env("POCKYMOE_ENVIRONMENT_WORKER", "1");
    }
    command
        .env_remove("POCKYMOE_HOST_PROCESS_RECEIPT")
        .envs(request.env)
        .stdin(if request.stdin_closed {
            Stdio::null()
        } else if is_terminal {
            Stdio::inherit()
        } else {
            Stdio::piped()
        })
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().context("sandbox command unavailable")?;
    let pid = child.id().context("missing sandbox pid")?;
    #[cfg(unix)]
    if is_terminal {
        unsafe {
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(0, pid as i32);
        }
    }
    let record = if host.is_none() {
        std::fs::create_dir_all("/tmp/pockymoe-processes")?;
        Some(Path::new("/tmp/pockymoe-processes").join(&request.id))
    } else {
        None
    };
    let self_pid = std::process::id();
    if let Some(record) = &record {
        std::fs::write(record, format!("{} {}", self_pid, process_start(self_pid)?))?;
    }
    let mut stdin = child.stdin.take();
    let transport = async move {
        if let Some(mut input) = input {
            if let Some(mut stdin) = stdin.take() {
                let _ = tokio::io::copy(&mut input, &mut stdin).await;
            } else {
                // The command sees EOF, but its parent's live transport still
                // owns the process domain and must cancel it on disconnect.
                let _ = tokio::io::copy(&mut input, &mut tokio::io::sink()).await;
            }
        } else {
            std::future::pending::<()>().await
        }
    };
    let stopped = async move {
        #[cfg(unix)]
        {
            tokio::select! {_=terminate.recv()=>{},_=hangup.recv()=>{},_=tokio::signal::ctrl_c()=>{}}
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    };
    let code = tokio::select! {
        result=child.wait()=>result?.code().unwrap_or(128),
        _=transport=>130,
        _=stopped=>130,
    };
    // A subreaper retains double-forked / setsid descendants. The worker lives
    // until its entire descendant tree is killed, rather than just the CLI pipe.
    // No unverified negative process-group signal: the initial child may have
    // exited and its PID/group could already have been reused.
    let cleanup = terminate_descendants(self_pid).await;
    if let Some(record) = record {
        let _ = std::fs::remove_file(record);
    }
    if let Some(path) = host {
        if cleanup.is_ok() {
            super::host_process::stopped(&path)?;
        } else {
            eprintln!("Host process cleanup could not prove quiescence");
            std::process::exit(1);
        }
    }
    std::process::exit(code)
}
fn process_start(pid: u32) -> Result<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    Ok(stat
        .rsplit_once(") ")
        .context("invalid process stat")?
        .1
        .split_whitespace()
        .nth(19)
        .context("missing process start time")?
        .into())
}
async fn terminate_descendants(parent: u32) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let mut all = Vec::<(u32, u32, String)>::new();
            if let Ok(entries) = std::fs::read_dir("/proc") {
                for entry in entries.flatten() {
                    let Some(pid) = entry
                        .file_name()
                        .to_str()
                        .and_then(|n| n.parse::<u32>().ok())
                    else {
                        continue;
                    };
                    if let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) {
                        if let Some(ppid) = stat
                            .rsplit_once(") ")
                            .and_then(|(_, v)| v.split_whitespace().nth(1))
                            .and_then(|v| v.parse().ok())
                        {
                            if let Ok(start) = process_start(pid) {
                                all.push((pid, ppid, start));
                            }
                        }
                    }
                }
            }
            let mut descendants = std::collections::HashSet::from([parent]);
            loop {
                let before = descendants.len();
                for (pid, ppid, _) in &all {
                    if descendants.contains(ppid) {
                        descendants.insert(*pid);
                    }
                }
                if before == descendants.len() {
                    break;
                }
            }
            for (pid, _, start) in all
                .iter()
                .filter(|(pid, _, _)| *pid != parent && descendants.contains(pid))
            {
                let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, *pid, 0) as i32 };
                if fd < 0 {
                    continue;
                }
                let fd = unsafe { std::fs::File::from_raw_fd(fd) };
                if process_start(*pid).ok().as_ref() == Some(start) {
                    unsafe {
                        libc::syscall(
                            libc::SYS_pidfd_send_signal,
                            fd.as_raw_fd(),
                            libc::SIGKILL,
                            std::ptr::null::<libc::siginfo_t>(),
                            0,
                        );
                    }
                }
            }
            // Reap adopted children too. ECHILD, rather than an empty proc snapshot,
            // proves there is no child left that can fork or retain a file writer.
            loop {
                let result = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
                if result > 0 {
                    continue;
                }
                if result < 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD)
                {
                    return Ok(());
                }
                break;
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "process descendants did not quiesce"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
    #[cfg(not(target_os = "linux"))]
    anyhow::bail!("process cleanup requires Linux");
}
fn kill(id: &str) -> Result<()> {
    use std::io::Read;
    ensure!(
        uuid::Uuid::parse_str(id).is_ok(),
        "invalid process identity"
    );
    let file = Path::new("/tmp/pockymoe-processes").join(id);
    // Match the immutable launch identity in procfs too: project code shares
    // this container UID and may remove or replace a writable tmp record.
    let mut targets = Vec::new();
    for entry in std::fs::read_dir("/proc")?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|v| v.parse::<u32>().ok())
        else {
            continue;
        };
        if pid <= 1 || pid == std::process::id() {
            continue;
        }
        let Ok(mut command_line) = std::fs::File::open(entry.path().join("cmdline")) else {
            continue;
        };
        let mut bytes = Vec::new();
        if command_line
            .by_ref()
            .take(16384)
            .read_to_end(&mut bytes)
            .is_err()
        {
            continue;
        }
        let fields = bytes.split(|b| *b == 0).collect::<Vec<_>>();
        if fields.len() >= 4
            && fields[0] == b"/opt/pockymoe/bin/pockymoe"
            && fields[1] == b"environment-worker"
            && matches!(fields[2], b"run" | b"shell")
            && fields[3] == id.as_bytes()
        {
            if let Ok(start) = process_start(pid) {
                targets.push((pid, start));
            }
        }
    }
    if targets.is_empty() {
        ensure!(
            !file.exists(),
            "sandbox worker disappeared; process domain outcome is uncertain"
        );
        return Ok(());
    }
    for (pid, _) in &targets {
        #[cfg(unix)]
        unsafe {
            libc::kill(*pid as i32, libc::SIGTERM);
        }
    }
    // The worker removes its record only after reaping the command domain.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let alive = || {
        targets
            .iter()
            .any(|(pid, start)| process_start(*pid).ok().as_ref() == Some(start))
    };
    while alive() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    ensure!(
        !alive() && !file.exists(),
        "sandbox process domain cancellation timed out"
    );
    Ok(())
}
fn operation(input: Value) -> Result<Value> {
    let op = input["op"].as_str().context("operation required")?;
    let root = Path::new(input["root"].as_str().context("root required")?);
    let path = input["path"].as_str().unwrap_or("");
    match op {
        "readText" => {
            let p = crate::files::assert_within(root, Path::new(path))?;
            Ok(json!({"content":std::fs::read_to_string(p)?}))
        }
        "diffBytes" => {
            use std::io::Read;
            let target = root.join(path);
            let parent = target
                .parent()
                .context("file has no parent")?
                .canonicalize()?;
            ensure!(
                parent.starts_with(root.canonicalize()?),
                "file resolves outside working copy"
            );
            let metadata = target.symlink_metadata()?;
            let limit = input["limit"]
                .as_u64()
                .unwrap_or(512 * 1024)
                .min(512 * 1024);
            let bytes = if metadata.file_type().is_symlink() {
                std::fs::read_link(&target)?
                    .to_string_lossy()
                    .as_bytes()
                    .to_vec()
            } else {
                ensure!(metadata.is_file(), "diff target is not a regular file");
                let mut bytes = Vec::new();
                std::fs::File::open(target)?
                    .take(limit + 1)
                    .read_to_end(&mut bytes)?;
                bytes
            };
            Ok(json!({"bytes":STANDARD.encode(bytes)}))
        }
        "readBytes" => {
            let (p, bytes) = crate::files::read_bytes(root, path)?;
            Ok(json!({"path":p,"bytes":STANDARD.encode(bytes)}))
        }
        "readThreadImage" => {
            let (bytes, _) = crate::files::read_thread_image(
                root,
                input["threadId"]
                    .as_str()
                    .context("thread identity required")?,
                path,
            )?;
            Ok(json!({"bytes":STANDARD.encode(bytes)}))
        }
        "writeText" => {
            crate::files::write_file(
                root,
                path,
                input["content"].as_str().context("content required")?,
            )?;
            Ok(Value::Null)
        }
        "writeBytes" => {
            let p = crate::files::assert_mutation_within(root, Path::new(path))?;
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let bytes = STANDARD.decode(input["bytes"].as_str().context("bytes required")?)?;
            std::fs::write(p, bytes)?;
            Ok(Value::Null)
        }
        "appendChunk" => {
            let bytes = STANDARD.decode(input["bytes"].as_str().context("bytes required")?)?;
            let offset = input["offset"].as_u64().context("offset required")?;
            let received = crate::files::append_chunk(root, Path::new(path), offset, &bytes)?;
            Ok(json!({"received":received}))
        }
        "createFile" => {
            crate::files::create_empty_file(root, path)?;
            Ok(Value::Null)
        }
        "tree" => Ok(serde_json::to_value(crate::files::list_tree(root, path)?)?),
        "preview" => Ok(serde_json::to_value(crate::files::preview_file(
            root,
            path,
            input["limit"].as_u64().unwrap_or(65536) as usize,
        )?)?),
        "delete" => {
            let p = crate::files::assert_mutation_within(root, Path::new(path))?;
            ensure!(p != root.canonicalize()?, "cannot delete workspace root");
            if p.is_dir() {
                std::fs::remove_dir_all(p)?;
            } else {
                std::fs::remove_file(p)?;
            }
            Ok(Value::Null)
        }
        "move" => {
            let from = crate::files::assert_mutation_within(root, Path::new(path))?;
            let to = crate::files::assert_mutation_within(
                root,
                Path::new(input["to"].as_str().context("destination required")?),
            )?;
            ensure!(
                from != root.canonicalize()? && to != root.canonicalize()?,
                "cannot move workspace root"
            );
            ensure!(!to.exists(), "destination exists");
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(from, to)?;
            Ok(Value::Null)
        }
        "copyFile" => {
            let from = crate::files::assert_within(root, Path::new(path))?;
            let to = crate::files::assert_mutation_within(
                root,
                Path::new(input["to"].as_str().context("destination required")?),
            )?;
            ensure!(from.is_file(), "only file copies are supported");
            ensure!(!to.exists(), "destination exists");
            let bytes = std::fs::copy(from, &to)?;
            Ok(json!({"path":to,"bytes":bytes}))
        }
        "stat" => {
            let p = crate::files::assert_within(root, Path::new(path))?;
            let m = std::fs::metadata(p)?;
            Ok(json!({"isDir":m.is_dir(),"isFile":m.is_file(),"size":m.len()}))
        }
        "document" => Ok(serde_json::to_value(
            crate::file_documents::read_document(root, path)?.0,
        )?),
        "fileCapabilities" => Ok(
            json!({"workspaceRevision":crate::file_documents::root_revision(root)?,"conditionalSave":true,"documentRead":true,"textRangeRead":false,"maxEditableBytes":1048576,"maxEditableLines":20000}),
        ),
        "saveDocument" => {
            let input: crate::file_documents::SaveDocument =
                serde_json::from_value(input["input"].clone())?;
            ensure!(
                crate::file_documents::root_revision(root)? == input.workspace_revision,
                "workspaceChanged"
            );
            crate::file_documents::commit_document_io(root, &input, |_, _| Ok(()))
        }
        "git" => {
            let args: Vec<String> = serde_json::from_value(input["args"].clone())?;
            let out = std::process::Command::new("git")
                .current_dir(root)
                .args([
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.pager=cat",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "--literal-pathspecs",
                ])
                .args(args)
                .env_clear()
                .env("PATH", "/usr/local/bin:/usr/bin:/bin")
                .env("HOME", "/pockymoe/state")
                .env("GIT_TERMINAL_PROMPT", "0")
                .stdin(Stdio::null())
                .output()?;
            ensure!(out.stdout.len() < 512 * 1024, "Git output exceeds limit");
            ensure!(out.status.success(), "sandbox Git operation failed");
            Ok(json!({"bytes":STANDARD.encode(out.stdout),"truncated":false}))
        }
        _ => bail!("unsupported sandbox file operation"),
    }
}

#[cfg(not(unix))]
async fn serve() -> Result<()> {
    bail!("unsupported_environment_platform")
}
