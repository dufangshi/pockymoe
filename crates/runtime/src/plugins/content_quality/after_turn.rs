//! Opt-in review of recorded edits and dirty human documents after completed turns.
use super::*;
use tokio::io::AsyncReadExt;

pub(super) fn command(root: &Path, arguments: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    command.env_clear();
    for name in ["PATH", "SYSTEMROOT", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .arg("--no-optional-locks")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
        ])
        .arg("-C")
        .arg(root)
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    command
}

async fn git(root: &Path, arguments: &[&str]) -> Result<Vec<u8>> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut child = command(root, arguments)
            .spawn()
            .map_err(|_| anyhow::anyhow!("cannot start document Git scan"))?;
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .context("Git scan has no output")?
            .take(65537)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| anyhow::anyhow!("cannot read document Git scan"))?;
        ensure!(bytes.len() <= 65536, "document Git scan exceeds 64 KiB");
        ensure!(
            child
                .wait()
                .await
                .map_err(|_| anyhow::anyhow!("document Git scan failed"))?
                .success(),
            "document Git scan unavailable for workspace"
        );
        Ok(bytes)
    })
    .await
    .map_err(|_| anyhow::anyhow!("document Git scan timed out"))?
}

pub(super) async fn documents(root: &Path) -> Result<(Vec<String>, bool)> {
    // Porcelain emits repository-relative paths; show-prefix makes a nested
    // workspace safe without reviewing sibling files outside it.
    let prefix = git(root, &["rev-parse", "--show-prefix"]).await?;
    let prefix = std::str::from_utf8(&prefix)
        .context("workspace Git prefix is not UTF-8")?
        .trim_end_matches('\n');
    let status = git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
            "--",
            ".",
        ],
    )
    .await?;
    let mut paths = Vec::new();
    for entry in status.split(|b| *b == 0) {
        if entry.len() < 4 || entry[2] != b' ' || entry[..2].contains(&b'D') {
            continue;
        }
        let Ok(path) = std::str::from_utf8(&entry[3..]) else {
            continue;
        };
        let Some(path) = path.strip_prefix(prefix) else {
            continue;
        };
        let Ok(resolved) = document::resolve(root, path) else {
            continue;
        };
        let Ok(source) = document::read(&resolved) else {
            continue;
        };
        if document::unsupported(&source, path) {
            continue;
        }
        paths.push(path.to_string());
    }
    paths.sort();
    paths.dedup();
    let truncated = paths.len() > 5;
    paths.truncate(5);
    Ok((paths, truncated))
}

fn eligible(root: &Path, raw: &str) -> Option<String> {
    if raw.is_empty() || raw.len() > 4096 || raw.chars().any(char::is_control) {
        return None;
    }
    let candidate = Path::new(raw);
    // Keep the lexical relative path for resolve()'s per-component symlink and
    // traversal checks, in addition to checking canonical absolute containment.
    let relative = if candidate.is_absolute() {
        let canonical = candidate.canonicalize().ok()?;
        if !canonical.starts_with(root) {
            return None;
        }
        candidate.strip_prefix(root).ok()?.to_str()?
    } else {
        raw
    };
    let resolved = document::resolve(root, relative).ok()?;
    let source = document::read(&resolved).ok()?;
    if document::unsupported(&source, relative) {
        return None;
    }
    Some(resolved.strip_prefix(root).ok()?.to_str()?.to_string())
}
fn failed_or_deleted(value: &Value) -> bool {
    if value["deleted"] == true || value["success"] == false {
        return true;
    }
    if value
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|v| !matches!(v, "completed" | "success" | "succeeded" | "done"))
    {
        return true;
    }
    [
        value.get("type"),
        value.get("kind"),
        value.pointer("/kind/type"),
        value.get("operation"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .any(|v| matches!(v, "delete" | "deleted" | "remove" | "removed" | "failed"))
}

pub(super) fn recorded_documents(root: &Path, items: &[Value]) -> Result<(Vec<String>, bool)> {
    let root = root
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("cannot resolve workspace"))?;
    let mut raw_paths = Vec::new();
    let mut budget = 256usize;
    fn structural(value: &Value, depth: usize, budget: &mut usize, paths: &mut Vec<String>) {
        if *budget == 0 || depth > 3 || paths.len() >= 128 {
            return;
        }
        *budget -= 1;
        if let Some(path) = value.as_str() {
            if path.len() <= 4096 {
                paths.push(path.to_string());
            }
            return;
        }
        if failed_or_deleted(value) {
            return;
        }
        for key in ["path", "filePath"] {
            if let Some(path) = value
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| s.len() <= 4096)
            {
                paths.push(path.to_string());
            }
        }
        for key in ["changes", "files"] {
            if let Some(values) = value.get(key).and_then(Value::as_array) {
                for child in values.iter().take(129) {
                    structural(child, depth + 1, budget, paths);
                }
            }
        }
    }
    let mut record_count = 0;
    for item in items.iter().filter(|item| item["kind"] == "fileChange") {
        record_count += 1;
        if record_count > 256 || budget == 0 || raw_paths.len() >= 128 {
            break;
        }
        if failed_or_deleted(item) {
            continue;
        }
        if ["path", "filePath", "changes", "files"]
            .iter()
            .any(|key| item.get(key).is_some())
        {
            structural(item, 0, &mut budget, &mut raw_paths);
        } else if let Some(text) = item["text"]
            .as_str()
            .filter(|s| s.len() <= 16384 && !s.chars().any(char::is_control))
        {
            // ACP's normalized text is an exact path or comma-space path list.
            // No shell commands, tool details, diffs or arbitrary object keys.
            if eligible(&root, text).is_some() {
                raw_paths.push(text.to_string());
            } else {
                raw_paths.extend(
                    text.split(", ")
                        .take(128usize.saturating_sub(raw_paths.len()))
                        .map(str::to_string),
                );
            }
        }
    }
    let mut paths = raw_paths
        .iter()
        .filter_map(|raw| eligible(&root, raw))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    let truncated = record_count > 256 || budget == 0 || raw_paths.len() >= 128 || paths.len() > 5;
    paths.truncate(5);
    Ok((paths, truncated))
}
pub(super) async fn candidates(
    root: &Path,
    items: &[Value],
) -> Result<(Vec<String>, bool, Option<String>)> {
    let (mut paths, mut truncated) = recorded_documents(root, items)?;
    let mut warning = None;
    match documents(root).await {
        Ok((dirty, more)) => {
            truncated |= more;
            for path in dirty {
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
        Err(error) => warning = Some(truncate(&error.to_string(), 240)),
    }
    truncated |= paths.len() > 5;
    paths.truncate(5);
    Ok((paths, truncated, warning))
}

pub(super) async fn run(context: &PluginActionContext, settings: &Settings) -> Result<Value> {
    let turn_id = reply_turn_id(context)?;
    let turn = stored_turn(&context.supervisor, &context.thread_id, &turn_id)?;
    if turn.status != "completed" {
        return Ok(json!({"status":"skipped","reason":"turnNotCompleted","turnId":turn_id}));
    }
    let reply = if settings.auto_replies {
        match tokio::time::timeout(Duration::from_secs(65), rewrite_reply(context, settings)).await
        {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => json!({"status":"failed","error":truncate(&error.to_string(),240)}),
            Err(_) => json!({"status":"failed","error":"reply editing timed out"}),
        }
    } else {
        json!({"status":"skipped","reason":"autoRepliesDisabled"})
    };
    let mut reports = Vec::new();
    let mut truncated = false;
    let mut scan_error = None;
    if settings.auto_documents {
        let scan: Result<_> = async {
            let thread = context.supervisor.get_thread(&context.thread_id)?;
            let root = context
                .supervisor
                .session_cwd(&thread)
                .context("thread has no workspace")?;
            candidates(Path::new(&root), &turn.items).await
        }
        .await;
        match scan {
            Err(error) => scan_error = Some(truncate(&error.to_string(), 240)),
            Ok((paths, more, warning)) => {
                truncated = more;
                scan_error = warning;
                for path in paths {
                    let child = PluginActionContext {
                        supervisor: context.supervisor.clone(),
                        run_id: context.run_id.clone(),
                        thread_id: context.thread_id.clone(),
                        settings: context.settings.clone(),
                        input: json!({"path":path,"mode":"review"}),
                        event: context.event.clone(),
                    };
                    let report = match tokio::time::timeout(
                        Duration::from_secs(40),
                        review_document(&child, settings),
                    )
                    .await
                    {
                        Ok(Ok(value)) => {
                            json!({"path":path,"status":value["status"],"reason":value.get("reason"),"sourceHash":value.get("sourceHash"),"written":false,"findings":value.get("findings"),"diff":truncate(value["diff"].as_str().unwrap_or(""),2000),"diffTruncated":value["diffTruncated"]==true || value["diff"].as_str().unwrap_or("").chars().count()>2000})
                        }
                        Ok(Err(error)) => {
                            json!({"path":path,"status":"failed","error":truncate(&error.to_string(),240)})
                        }
                        Err(_) => {
                            json!({"path":path,"status":"failed","error":"document review timed out"})
                        }
                    };
                    reports.push(report);
                }
            }
        }
    }
    let output = json!({"status":"completed","turnId":turn_id,"reply":reply,"documents":reports,"documentsTruncated":truncated,"documentScanError":scan_error,"documentMode":"review"});
    ensure!(
        serde_json::to_vec(&output)?.len() <= 240 * 1024,
        "after-turn report exceeds size limit"
    );
    Ok(output)
}
