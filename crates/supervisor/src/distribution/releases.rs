use super::*;
use anyhow::{bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    time::Duration,
};

pub const REPO: &str = "https://github.com/dufangshi/pockymoe";
pub const WEB: &str = "remote-codex-web.zip";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub version: String,
    pub executable: PathBuf,
    pub web_dist: PathBuf,
}
pub fn stable_version(raw: &str) -> Result<String> {
    let v = raw.trim().strip_prefix('v').unwrap_or(raw.trim());
    ensure!(
        v.split('.').count() == 3
            && v.split('.').all(|p| !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u32>().is_ok()),
        "Invalid stable runtime version"
    );
    Ok(v.into())
}
pub fn platform() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("remote-codex-linux-x64-gnu"),
        ("linux", "aarch64") => Ok("remote-codex-linux-arm64-gnu"),
        ("macos", "aarch64") => Ok("remote-codex-darwin-arm64"),
        ("windows", "x86_64") => Ok("remote-codex-win32-x64-msvc-cli.exe"),
        _ => bail!("Unsupported runtime platform"),
    }
}
fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("pockymoe-native-updater")
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(300))
        .build()?)
}
pub async fn latest() -> Result<String> {
    let response = client()?
        .get(format!(
            "{REPO}/releases/latest/download/runtime-version.txt"
        ))
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?;
    let bytes = response.bytes().await?;
    ensure!(bytes.len() <= 128, "Invalid release version response");
    stable_version(std::str::from_utf8(&bytes)?)
}
pub fn checksums(text: &str) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let parts: Vec<_> = line.split_whitespace().collect();
        ensure!(
            parts.len() == 2
                && parts[0].len() == 64
                && parts[0].bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid release checksum entry"
        );
        let name = parts[1].trim_start_matches('*');
        ensure!(
            !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != "..",
            "Invalid release asset name"
        );
        ensure!(
            result
                .insert(name.into(), parts[0].to_ascii_lowercase())
                .is_none(),
            "Duplicate release checksum"
        );
    }
    Ok(result)
}
async fn sums(repo: &str, version: &str) -> Result<BTreeMap<String, String>> {
    let text = client()?
        .get(format!("{repo}/releases/download/v{version}/SHA256SUMS"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    ensure!(text.len() < 64 * 1024, "Unexpected checksum file size");
    let sums = checksums(&text)?;
    for name in [
        "remote-codex-linux-x64-gnu",
        "remote-codex-linux-arm64-gnu",
        "remote-codex-darwin-arm64",
        "remote-codex-win32-x64-msvc-cli.exe",
        WEB,
    ] {
        ensure!(
            sums.contains_key(name),
            "Incomplete runtime release: missing {name}"
        );
    }
    Ok(sums)
}
async fn download(
    repo: &str,
    version: &str,
    name: &str,
    target: &Path,
    digest: &str,
) -> Result<()> {
    eprintln!("Downloading {name}…");
    let mut response = client()?
        .get(format!("{repo}/releases/download/v{version}/{name}"))
        .send()
        .await?
        .error_for_status()?;
    let total = response.content_length();
    ensure!(
        total.unwrap_or(0) <= 256 * 1024 * 1024,
        "Release asset too large"
    );
    let mut file = std::fs::File::create(target)?;
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut last = 0;
    while let Some(chunk) = response.chunk().await? {
        count += chunk.len() as u64;
        ensure!(count <= 256 * 1024 * 1024, "Release asset too large");
        hash.update(&chunk);
        file.write_all(&chunk)?;
        if let Some(total) = total.filter(|t| *t > 0) {
            let pct = count * 100 / total;
            if pct >= last + 10 {
                eprintln!("  {name}: {pct}%");
                last = pct;
            }
        }
    }
    file.sync_all()?;
    ensure!(
        hex::encode(hash.finalize()) == digest,
        "Checksum failed for {name}"
    );
    Ok(())
}
pub fn unpack_web(archive: &Path, target: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
    ensure!(zip.len() <= 5000, "Web archive has too many entries");
    private_dir(target)?;
    let mut size = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let relative = entry
            .enclosed_name()
            .context("Unsafe path in Web archive")?;
        ensure!(
            entry.unix_mode().unwrap_or(0) & 0o170000 != 0o120000,
            "Symlinks are not allowed in Web archive"
        );
        size = size
            .checked_add(entry.size())
            .context("Web archive size overflow")?;
        ensure!(size <= 256 * 1024 * 1024, "Expanded Web archive too large");
        let path = target.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&path)?;
        } else {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let mut out = std::fs::File::create(path)?;
            let limit = entry.size();
            let written = std::io::copy(&mut entry.by_ref().take(limit + 1), &mut out)?;
            ensure!(written == limit, "Web archive entry size mismatch");
        }
    }
    ensure!(
        target.join("index.html").is_file(),
        "Release Web bundle has no index.html"
    );
    Ok(())
}
pub async fn install(version: &str, bootstrap: Option<&Path>) -> Result<Installed> {
    install_from(&install_root().join("releases"), REPO, version, bootstrap).await
}
async fn install_from(
    root: &Path,
    repo: &str,
    version: &str,
    bootstrap: Option<&Path>,
) -> Result<Installed> {
    let version = stable_version(version)?;
    private_dir(&root)?;
    let target = root.join(&version);
    let metadata = target.join("installed.json");
    if let Ok(installed) = read::<Installed>(&metadata) {
        if installed.version == version
            && installed.executable.is_file()
            && installed.web_dist.join("index.html").is_file()
        {
            verify_binary(&installed.executable, &version).await?;
            return Ok(installed);
        }
    }
    ensure!(
        !target.exists(),
        "Incomplete staged release {}; remove it after checking the update log",
        target.display()
    );
    let stage = root.join(format!("stage-{}", uuid::Uuid::new_v4()));
    private_dir(&stage)?;
    let result = async {
        let sums = sums(repo, &version).await?;
        let binary = stage.join(binary_name());
        let asset = platform()?;
        if let Some(source) = bootstrap {
            std::fs::copy(source, &binary)?;
            ensure!(
                hex::encode(Sha256::digest(std::fs::read(&binary)?)) == sums[asset],
                "Bootstrap binary checksum mismatch"
            );
        } else {
            download(repo, &version, asset, &binary, &sums[asset]).await?;
        }
        executable(&binary)?;
        verify_binary(&binary, &version).await?;
        let archive = stage.join(WEB);
        download(repo, &version, WEB, &archive, &sums[WEB]).await?;
        unpack_web(&archive, &stage.join("web"))?;
        std::fs::remove_file(archive)?;
        let installed = Installed {
            version: version.clone(),
            executable: target.join(binary_name()),
            web_dist: target.join("web"),
        };
        write(&stage.join("installed.json"), &installed)?;
        std::fs::rename(&stage, &target)?;
        Ok(installed)
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_dir_all(stage);
    }
    result
}
async fn verify_binary(binary: &Path, version: &str) -> Result<()> {
    // A process forked elsewhere while the binary was being written can briefly
    // hold its write descriptor; Linux then refuses to execute it (ETXTBSY).
    let mut attempt = 0;
    let checked = loop {
        let mut command = tokio::process::Command::new(binary);
        command.arg("version").kill_on_drop(true);
        match tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .context("Runtime version check timed out")?
        {
            Err(error) if error.raw_os_error() == Some(26) && attempt < 20 => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            result => break result?,
        }
    };
    ensure!(
        checked.status.success() && String::from_utf8_lossy(&checked.stdout).trim() == version,
        "Downloaded runtime version mismatch"
    );
    Ok(())
}
pub fn current() -> Result<Installed> {
    read(&install_root().join("current.json"))
}
pub fn activate(installed: &Installed) -> Result<()> {
    write(&install_root().join("current.json"), installed)?;
    #[cfg(unix)]
    {
        // A stable native CLI entry follows subsequent updates. Never overwrite
        // an unrelated executable or link in the user's local bin directory.
        let current = install_root().join("current");
        let staged = install_root().join(format!(".current-{}", uuid::Uuid::new_v4()));
        std::os::unix::fs::symlink(
            installed
                .executable
                .parent()
                .context("Missing release directory")?,
            &staged,
        )?;
        std::fs::rename(staged, &current)?;
        let bin = home().join(".local/bin");
        std::fs::create_dir_all(&bin)?;
        // `remote-codex` is the pre-rename command that scripts and hooks use.
        for command in ["pockymoe", "remote-codex"] {
            let cli = bin.join(command);
            if std::fs::symlink_metadata(&cli).is_err() {
                std::os::unix::fs::symlink(current.join(binary_name()), cli)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_metadata_rejects_unstable_versions_and_ambiguous_checksums() {
        assert_eq!(stable_version("v0.12.75\n").unwrap(), "0.12.75");
        for invalid in ["", "1.2", "1.2.3-next", "../0.12.75", "1.2.3.4"] {
            assert!(stable_version(invalid).is_err());
        }
        let digest = "a".repeat(64);
        assert_eq!(
            checksums(&format!("{digest}  runtime\n")).unwrap()["runtime"],
            digest
        );
        for invalid in [
            format!("{digest}  ../runtime"),
            format!("{digest}  runtime\n{digest}  runtime"),
            "nohash runtime".into(),
        ] {
            assert!(checksums(&invalid).is_err());
        }
    }
    fn web_zip(name: &str, content: &[u8]) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        archive
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(content).unwrap();
        archive.finish().unwrap().into_inner()
    }
    #[test]
    fn native_web_archive_rejects_traversal_and_incomplete_bundles() {
        let root = tempfile::tempdir().unwrap();
        for (index, name) in ["../outside.html", "assets/only.js"].iter().enumerate() {
            let archive = root.path().join(format!("{index}.zip"));
            std::fs::write(&archive, web_zip(name, b"fixture")).unwrap();
            assert!(unpack_web(&archive, &root.path().join(index.to_string())).is_err());
        }
        assert!(!root.path().join("outside.html").exists());
        let archive = root.path().join("good.zip");
        std::fs::write(&archive, web_zip("index.html", b"<html>fixture</html>")).unwrap();
        unpack_web(&archive, &root.path().join("good")).unwrap();
        assert_eq!(
            std::fs::read(root.path().join("good/index.html")).unwrap(),
            b"<html>fixture</html>"
        );
    }
    #[cfg(unix)]
    async fn fixture(corrupt: bool, binary_version: &str) -> (String, tokio::task::JoinHandle<()>) {
        let binary = format!("#!/bin/sh\nprintf '%s\\n' '{binary_version}'\n").into_bytes();
        let web = web_zip("index.html", b"<html>fixture</html>");
        let hash = if corrupt {
            "a".repeat(64)
        } else {
            hex::encode(Sha256::digest(&binary))
        };
        let mut sums = [
            "remote-codex-linux-x64-gnu",
            "remote-codex-linux-arm64-gnu",
            "remote-codex-darwin-arm64",
            "remote-codex-win32-x64-msvc-cli.exe",
        ]
        .iter()
        .map(|name| format!("{hash}  {name}\n"))
        .collect::<String>();
        sums.push_str(&format!("{}  {WEB}\n", hex::encode(Sha256::digest(&web))));
        let files = BTreeMap::from([
            (
                format!("releases/download/v9.1.0/{}", platform().unwrap()),
                binary,
            ),
            (format!("releases/download/v9.1.0/{WEB}"), web),
            (
                "releases/download/v9.1.0/SHA256SUMS".into(),
                sums.into_bytes(),
            ),
        ]);
        async fn serve(
            axum::extract::State(files): axum::extract::State<BTreeMap<String, Vec<u8>>>,
            axum::extract::Path(path): axum::extract::Path<String>,
        ) -> (axum::http::StatusCode, Vec<u8>) {
            match files.get(&path) {
                Some(bytes) => (axum::http::StatusCode::OK, bytes.clone()),
                None => (axum::http::StatusCode::NOT_FOUND, Vec::new()),
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let router = axum::Router::new()
            .route("/{*path}", axum::routing::get(serve))
            .with_state(files);
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (address, task)
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn native_release_download_verifies_before_activation_and_reuses_complete_install() {
        let root = tempfile::tempdir().unwrap();
        let (repo, server) = fixture(false, "9.1.0").await;
        let installed = install_from(root.path(), &repo, "9.1.0", None)
            .await
            .unwrap();
        assert!(installed.executable.is_file());
        assert!(installed.web_dist.join("index.html").is_file());
        assert!(!root.path().join("current.json").exists());
        server.abort();
        let reused = install_from(root.path(), &repo, "9.1.0", None)
            .await
            .unwrap();
        assert_eq!(reused.executable, installed.executable);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn native_release_rejects_corrupt_or_wrong_version_without_partial_install() {
        for (corrupt, version) in [(true, "9.1.0"), (false, "9.9.9")] {
            let root = tempfile::tempdir().unwrap();
            let (repo, server) = fixture(corrupt, version).await;
            assert!(install_from(root.path(), &repo, "9.1.0", None)
                .await
                .is_err());
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
            server.abort();
        }
    }
}
