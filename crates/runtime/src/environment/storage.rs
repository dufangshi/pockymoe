use std::{
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};

use super::docker::{resolve_daemon_source, DockerHostInfo, EnvironmentDocker, LocalDockerContext};
use pockymoe_protocol::EnvironmentMountDto;

pub(super) fn canonical_directory(path: &Path) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "environment paths must be absolute");
    let path = path
        .canonicalize()
        .context("environment directory cannot be resolved")?;
    ensure!(path.is_dir(), "environment source is not a directory");
    ensure!(path.to_str().is_some(), "environment paths must be UTF-8");
    Ok(path)
}

pub(super) fn allowed_roots(roots: &[String]) -> Result<Vec<PathBuf>> {
    ensure!(
        !roots.is_empty() && roots.len() <= 64,
        "configure explicit project storage roots"
    );
    roots
        .iter()
        .map(|root| {
            let root = canonical_directory(Path::new(root))?;
            ensure!(root.components().count() >= 3, "storage root is too broad");
            ensure!(
                !["/etc", "/proc", "/sys", "/dev", "/run", "/var/run", "/root"]
                    .iter()
                    .any(|v| root.starts_with(v)),
                "system storage roots are forbidden"
            );
            Ok(root)
        })
        .collect()
}

fn authorized(path: &Path, roots: &[PathBuf]) -> Result<()> {
    ensure!(
        roots.iter().any(|root| path.starts_with(root)),
        "Git/storage closure includes a source outside allowedRoots"
    );
    Ok(())
}

fn small_text(path: &Path) -> Result<String> {
    ensure!(
        path.symlink_metadata()?.file_type().is_file(),
        "Git metadata pointer must be a regular file, not a symlink"
    );
    let bytes = std::fs::read(path).context("Git metadata pointer cannot be read")?;
    ensure!(
        bytes.len() <= 64 * 1024,
        "Git metadata pointer is too large"
    );
    String::from_utf8(bytes).context("Git metadata pointer is not UTF-8")
}

fn resolve_pointer(base: &Path, pointer: &str) -> Result<PathBuf> {
    let pointer = pointer.trim();
    ensure!(
        !pointer.is_empty() && !pointer.contains(['\0', '\n', '\r']),
        "invalid Git metadata pointer"
    );
    canonical_directory(&base.join(pointer))
}

/// Mount the selected checkout and its Git closure, never its containing project
/// merely because that project owns the worktree. Every external gitdir,
/// commondir and local object alternate needs independent root authorization.
pub(super) fn git_closure(
    workspace: &Path,
    working: &Path,
    roots: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    authorized(workspace, roots)?;
    authorized(working, roots)?;
    let mut result = BTreeSet::from([working.to_path_buf()]);
    let pointer = working.join(".git");
    let mut queue = Vec::new();
    if pointer.is_dir() {
        queue.push(canonical_directory(&pointer)?);
    } else if pointer.exists() {
        let content = small_text(&pointer)?;
        let path = content
            .trim()
            .strip_prefix("gitdir:")
            .ok_or_else(|| anyhow::anyhow!("unsupported .git pointer"))?;
        queue.push(resolve_pointer(working, path)?);
    }
    let mut visited = BTreeSet::new();
    while let Some(gitdir) = queue.pop() {
        ensure!(visited.len() < 128, "Git storage closure is too large");
        if !visited.insert(gitdir.clone()) {
            continue;
        }
        authorized(&gitdir, roots)?;
        result.insert(gitdir.clone());
        let common = gitdir.join("commondir");
        if common.exists() {
            queue.push(resolve_pointer(&gitdir, &small_text(&common)?)?);
        }
        let alternates = gitdir.join("objects/info/alternates");
        if alternates.exists() {
            for alternate in small_text(&alternates)?.lines().filter(|s| !s.is_empty()) {
                let objects = resolve_pointer(&gitdir.join("objects"), alternate)?;
                authorized(&objects, roots)?;
                result.insert(objects.clone());
                // Alternate entries reference object databases, so process their
                // own alternates without mounting the parent repository.
                collect_alternates(&objects, roots, &mut result, &mut BTreeSet::new())?;
            }
        }
        ensure!(
            !gitdir.join("objects/info/http-alternates").exists(),
            "HTTP Git alternates are unsupported in an offline environment"
        );
    }
    // Initialized submodules have a second private metadata closure. Fail closed
    // until that closure is explicitly represented, instead of silently mounting
    // an enclosing directory which may contain unrelated working copies.
    if working.join(".gitmodules").exists() {
        bail!("submodule working copies require a storage adapter; same-storage submodules are not supported");
    }
    let paths: Vec<_> = result.into_iter().collect();
    Ok(paths
        .iter()
        .filter(|path| {
            !paths
                .iter()
                .any(|ancestor| path != &ancestor && path.starts_with(ancestor))
        })
        .cloned()
        .collect())
}

fn collect_alternates(
    objects: &Path,
    roots: &[PathBuf],
    result: &mut BTreeSet<PathBuf>,
    visited: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    if !visited.insert(objects.to_path_buf()) {
        return Ok(());
    }
    ensure!(
        visited.len() <= 64,
        "Git object alternate closure is too large"
    );
    ensure!(
        !objects.join("info/http-alternates").exists(),
        "HTTP Git alternates are unsupported"
    );
    let pointer = objects.join("info/alternates");
    if pointer.exists() {
        for line in small_text(&pointer)?.lines().filter(|s| !s.is_empty()) {
            let next = resolve_pointer(objects, line)?;
            authorized(&next, roots)?;
            result.insert(next.clone());
            collect_alternates(&next, roots, result, visited)?;
        }
    }
    Ok(())
}

pub(super) fn file_sha256(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).context("bind identity file cannot be read")?;
    ensure!(
        file.metadata()?.is_file(),
        "bind identity source is not a regular file"
    );
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hex::encode(hash.finalize()))
}

pub(super) async fn verified_mount(
    docker: &dyn EnvironmentDocker,
    context: &LocalDockerContext,
    host: &DockerHostInfo,
    local: &Path,
    target: &str,
    read_only: bool,
) -> Result<EnvironmentMountDto> {
    let source = resolve_daemon_source(local, host)?;
    // A challenge written into the actual source proves same storage even when
    // the Supervisor and daemon both have a directory with the same spelling.
    // Tempfile cleanup also runs when the driver fails or a future is cancelled.
    let (probe, relative, expected) = if local.is_dir() {
        use std::io::Write;
        let mut probe = tempfile::Builder::new()
            .prefix(".pockymoe-storage-proof-")
            .tempfile_in(local)
            .context("cannot create same-storage identity challenge")?;
        probe.write_all(uuid::Uuid::new_v4().as_bytes())?;
        probe.write_all(uuid::Uuid::new_v4().as_bytes())?;
        probe.as_file_mut().sync_all()?;
        let hash = file_sha256(probe.path())?;
        let relative = probe.path().file_name().map(PathBuf::from);
        (Some(probe), relative, hash)
    } else {
        (None, None, file_sha256(local)?)
    };
    let actual = docker
        .bind_sha256(context, &source, relative.as_deref())
        .await
        .map_err(|_| anyhow::anyhow!("Docker bind identity probe failed"))?;
    ensure!(
        actual.trim().eq_ignore_ascii_case(&expected),
        "Docker bind source is not the Supervisor storage"
    );
    drop(probe);
    Ok(EnvironmentMountDto {
        source: source
            .to_str()
            .context("daemon bind path must be UTF-8")?
            .to_owned(),
        target: target.to_owned(),
        read_only,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_worktree_requires_private_and_common_gitdir_authorization() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("project");
        let working = root.path().join("external");
        let private = workspace.join(".git/worktrees/external");
        std::fs::create_dir_all(&private).unwrap();
        std::fs::create_dir_all(&working).unwrap();
        std::fs::write(
            working.join(".git"),
            format!("gitdir: {}\n", private.display()),
        )
        .unwrap();
        std::fs::write(private.join("commondir"), "../..\n").unwrap();
        assert!(git_closure(&workspace, &working, std::slice::from_ref(&working)).is_err());
        let paths =
            git_closure(&workspace, &working, &[workspace.clone(), working.clone()]).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&workspace.join(".git")));
        assert!(paths.contains(&working));
        assert!(!paths.contains(&workspace));
    }

    #[test]
    fn symlink_escape_and_external_alternates_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let external = root.path().join("external");
        std::fs::create_dir_all(project.join(".git/objects/info")).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        std::fs::write(
            project.join(".git/objects/info/alternates"),
            external.to_str().unwrap(),
        )
        .unwrap();
        assert!(git_closure(&project, &project, std::slice::from_ref(&project)).is_err());
        let paths = git_closure(&project, &project, &[project.clone(), external.clone()]).unwrap();
        assert!(paths.contains(&external));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&external, project.join("escape")).unwrap();
            assert!(
                authorized(&project.join("escape").canonicalize().unwrap(), &[project]).is_err()
            );
        }
    }
}
