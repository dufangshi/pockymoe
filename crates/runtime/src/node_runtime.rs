//! Node is an optional Agent/ACP dependency, never a Pockymoe bootstrap dependency.
use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};
use tokio::sync::Mutex;

const VERSION: &str = "v22.22.0";
pub(crate) fn bin_dir() -> PathBuf {
    let root = crate::config::home_dir().join(".local/share/remote-codex/agent-node");
    if cfg!(windows) {
        root
    } else {
        root.join("bin")
    }
}
/// Node `^22.19 || >=24`, the strictest range among managed harnesses (DSH).
fn node_ready(stdout: &[u8]) -> bool {
    let Some(version) = std::str::from_utf8(stdout)
        .ok()
        .and_then(|s| s.trim().strip_prefix('v'))
    else {
        return false;
    };
    let mut parts = version.split('.').map(|part| part.parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(22), Some(minor)) => minor >= 19,
        (Some(major), _) => major >= 24,
        _ => false,
    }
}
async fn ready() -> bool {
    let Some(node) = crate::acp::resolve_executable("node") else {
        return false;
    };
    if crate::acp::resolve_executable("npm").is_none() {
        return false;
    }
    let mut command = tokio::process::Command::new(node);
    crate::child_process::hide_tokio(&mut command);
    tokio::time::timeout(Duration::from_secs(5), command.arg("--version").output())
        .await
        .is_ok_and(|r| r.is_ok_and(|o| o.status.success() && node_ready(&o.stdout)))
}
fn archive_name() -> Result<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "win",
        _ => bail!("Unsupported Node platform"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => bail!("Unsupported Node architecture"),
    };
    Ok(format!(
        "node-{VERSION}-{os}-{arch}.{}",
        if cfg!(windows) { "zip" } else { "tar.gz" }
    ))
}
async fn download(
    client: &reqwest::Client,
    url: &str,
    target: &Path,
    expected: &str,
) -> Result<()> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    ensure!(
        response.content_length().unwrap_or(0) <= 128 * 1024 * 1024,
        "Node archive too large"
    );
    let mut file = std::fs::File::create(target)?;
    let mut hash = Sha256::new();
    let mut count = 0u64;
    while let Some(bytes) = response.chunk().await? {
        count += bytes.len() as u64;
        ensure!(count <= 128 * 1024 * 1024, "Node archive too large");
        hash.update(&bytes);
        file.write_all(&bytes)?;
    }
    file.sync_all()?;
    ensure!(
        hex::encode(hash.finalize()) == expected,
        "Agent Node checksum verification failed"
    );
    Ok(())
}
pub(crate) async fn ensure() -> Result<()> {
    static INSTALL: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = INSTALL.get_or_init(|| Mutex::new(())).lock().await;
    if ready().await {
        return Ok(());
    }
    tracing::info!("Preparing a private Node.js runtime for Agent/ACP dependencies");
    let name = archive_name()?;
    let base = format!("https://nodejs.org/dist/{VERSION}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(300))
        .build()?;
    let sums = client
        .get(format!("{base}/SHASUMS256.txt"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    ensure!(sums.len() < 64 * 1024, "Invalid Node checksum response");
    let matching: Vec<_> = sums
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next()?;
            let file = fields.next()?;
            (file == name
                && fields.next().is_none()
                && hash.len() == 64
                && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then_some(hash)
        })
        .collect();
    ensure!(
        matching.len() == 1,
        "Node release has no unique checksum for this platform"
    );
    let root = bin_dir();
    let root = if cfg!(windows) {
        root
    } else {
        root.parent().unwrap().to_path_buf()
    };
    std::fs::create_dir_all(root.parent().unwrap())?;
    let stage = tempfile::tempdir_in(root.parent().unwrap())?;
    let archive = stage.path().join(&name);
    download(&client, &format!("{base}/{name}"), &archive, matching[0])
        .await
        .context("Download Node for Agent dependencies; Pockymoe itself does not require Node")?;
    let extracted = stage.path().join("unpacked");
    std::fs::create_dir(&extracted)?;
    if cfg!(windows) {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&archive)?)?;
        ensure!(zip.len() < 20_000, "Invalid Node archive");
        let mut total = 0;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            let path = extracted.join(entry.enclosed_name().context("Invalid Node archive path")?);
            total += entry.size();
            ensure!(
                total <= 512 * 1024 * 1024,
                "Expanded Node archive too large"
            );
            ensure!(
                entry.unix_mode().unwrap_or(0) & 0o170000 != 0o120000,
                "Unsafe Node symlink"
            );
            if entry.is_dir() {
                std::fs::create_dir_all(path)?;
            } else {
                std::fs::create_dir_all(path.parent().unwrap())?;
                std::io::copy(&mut entry, &mut std::fs::File::create(path)?)?;
            }
        }
    } else {
        let out = tokio::process::Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&extracted)
            .output()
            .await?;
        ensure!(
            out.status.success(),
            "Cannot extract Agent Node archive: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let directory = name.trim_end_matches(".tar.gz").trim_end_matches(".zip");
    let candidate = extracted.join(directory);
    let node = candidate.join(if cfg!(windows) {
        "node.exe"
    } else {
        "bin/node"
    });
    let mut command = tokio::process::Command::new(node);
    crate::child_process::hide_tokio(&mut command);
    let out = command.arg("--version").output().await?;
    ensure!(
        out.status.success() && node_ready(&out.stdout),
        "Invalid Agent Node executable"
    );
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    std::fs::rename(candidate, root)?;
    ensure!(
        ready().await,
        "Agent Node/npm runtime is not executable after installation"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agent_node_version_gate() {
        assert!(node_ready(b"v22.22.0\n"));
        assert!(node_ready(b"v22.19.0"));
        assert!(node_ready(b"v24.1.0"));
        assert!(!node_ready(b"v22.18.1"));
        assert!(!node_ready(b"v23.11.0"));
        assert!(!node_ready(b"v20.1.0"));
        assert!(!node_ready(b"not-node"));
        assert!(archive_name().unwrap().starts_with("node-v22.22.0-"));
    }
}
