//! Trusted local Docker reads are supplied by the Supervisor, never the worker.
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Result};
use async_trait::async_trait;
use pockymoe_protocol::{EnvironmentLimitsDto, EnvironmentMountDto};
use serde_json::Value;

pub const ENVIRONMENT_LABEL: &str = "ca.pockymoe.environment";
pub const GENERATION_LABEL: &str = "ca.pockymoe.generation";

#[derive(Debug, Clone)]
pub struct LocalDockerContext {
    /// Only a local Unix socket is accepted. None means the local default.
    pub docker_host: Option<String>,
    /// Numeric checkout owner for private, capability-free storage probes.
    pub probe_user: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DockerHostInfo {
    pub daemon_id: String,
    pub cpus: f64,
    pub memory_bytes: u64,
    /// The actual Supervisor container inspect, authenticated by the driver.
    pub supervisor_container: Option<Value>,
    /// True only when the driver proved the Supervisor is in the host namespace.
    /// An unknown container identity must not be represented as a host process.
    pub host_namespace_verified: bool,
}

#[async_trait]
pub trait EnvironmentDocker: Send + Sync {
    async fn host_info(&self, context: &LocalDockerContext) -> Result<DockerHostInfo>;

    /// SHA-256 of a regular file read through a temporary, read-only daemon bind.
    /// relative_path=None means source itself is the file (the worker executable).
    /// The probe must have network none, no capabilities, no sockets, and no
    /// secret environment. Never implement this by reading the Supervisor path.
    async fn bind_sha256(
        &self,
        context: &LocalDockerContext,
        daemon_source: &Path,
        relative_path: Option<&Path>,
    ) -> Result<String>;

    /// All containers, including stopped containers, with ENVIRONMENT_LABEL=id.
    /// Return full Engine inspect objects, not a filtered list of running ones.
    async fn inspect_environment(
        &self,
        context: &LocalDockerContext,
        id: &str,
    ) -> Result<Vec<Value>>;
}

/// Control-plane helper only. Worker commands never pass through this driver.
pub struct CliEnvironmentDocker {
    executable: PathBuf,
    probe_image: String,
}

impl Default for CliEnvironmentDocker {
    fn default() -> Self {
        Self {
            executable: which::which("docker").unwrap_or_else(|_| PathBuf::from("docker")),
            probe_image: "busybox:1.37.0".into(),
        }
    }
}

impl CliEnvironmentDocker {
    pub fn with_probe_image(mut self, image: impl Into<String>) -> Self {
        self.probe_image = image.into();
        self
    }

    async fn output(&self, context: &LocalDockerContext, args: &[String]) -> Result<Vec<u8>> {
        let endpoint = context
            .docker_host
            .as_deref()
            .unwrap_or("unix:///var/run/docker.sock");
        validate_docker_host(Some(endpoint))?;
        let config = tempfile::tempdir()?;
        let mut command = tokio::process::Command::new(&self.executable);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .arg("--config")
            .arg(config.path())
            .arg("--host")
            .arg(endpoint)
            .args(args)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null());
        let output = tokio::time::timeout(std::time::Duration::from_secs(45), command.output())
            .await
            .map_err(|_| anyhow::anyhow!("trusted local Docker read timed out"))?
            .map_err(|_| anyhow::anyhow!("trusted local Docker command could not start"))?;
        ensure!(
            output.status.success(),
            "trusted local Docker command failed"
        );
        ensure!(
            output.stdout.len() <= 16 * 1024 * 1024,
            "Docker read exceeds the size limit"
        );
        Ok(output.stdout)
    }

    async fn json(&self, context: &LocalDockerContext, args: &[String]) -> Result<Value> {
        serde_json::from_slice(&self.output(context, args).await?)
            .map_err(|_| anyhow::anyhow!("Docker inspect returned invalid JSON"))
    }
}

pub(super) fn validate_docker_host(value: Option<&str>) -> Result<()> {
    if let Some(endpoint) = value {
        let path = endpoint.strip_prefix("unix://").ok_or_else(|| anyhow::anyhow!("only a local Unix Docker socket is supported; remote storage requires an explicit adapter"))?;
        ensure!(
            Path::new(path).is_absolute() && !path.contains(['\0', '\n', '\r', '?', '#']),
            "invalid local Docker socket path"
        );
    }
    Ok(())
}

#[async_trait]
impl EnvironmentDocker for CliEnvironmentDocker {
    async fn host_info(&self, context: &LocalDockerContext) -> Result<DockerHostInfo> {
        let info = self
            .json(
                context,
                &["info".into(), "--format".into(), "{{json .}}".into()],
            )
            .await?;
        validate_daemon(&info)?;
        let cgroup = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
        let containerized = Path::new("/.dockerenv").exists()
            || Path::new("/run/.containerenv").exists()
            || cgroup.contains("docker")
            || cgroup.contains("kubepods")
            || cgroup.contains("libpod");
        let supervisor_container = if containerized {
            let identity = hostname::get()?
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("Supervisor container identity is unavailable"))?;
            ensure!(
                identity.len() >= 12 && identity.bytes().all(|b| b.is_ascii_hexdigit()),
                "Supervisor container identity cannot be resolved safely"
            );
            let value = self
                .json(
                    context,
                    &["container".into(), "inspect".into(), identity.clone()],
                )
                .await?;
            let inspect = value
                .as_array()
                .and_then(|v| (v.len() == 1).then(|| &v[0]))
                .ok_or_else(|| anyhow::anyhow!("Supervisor container inspect is unavailable"))?;
            ensure!(
                inspect["Id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with(&identity))
                    && inspect["Config"]["Hostname"] == identity,
                "Supervisor container identity does not match the daemon"
            );
            Some(inspect.clone())
        } else {
            None
        };
        Ok(DockerHostInfo {
            daemon_id: info["ID"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("Docker daemon identity is unavailable"))?
                .into(),
            cpus: info["NCPU"]
                .as_f64()
                .filter(|n| *n > 0.0)
                .ok_or_else(|| anyhow::anyhow!("Docker CPU capacity is unavailable"))?,
            memory_bytes: info["MemTotal"]
                .as_u64()
                .filter(|n| *n > 0)
                .ok_or_else(|| anyhow::anyhow!("Docker memory capacity is unavailable"))?,
            supervisor_container,
            host_namespace_verified: !containerized,
        })
    }

    async fn bind_sha256(
        &self,
        context: &LocalDockerContext,
        source: &Path,
        relative_path: Option<&Path>,
    ) -> Result<String> {
        let user = context.probe_user.as_deref().ok_or_else(|| {
            anyhow::anyhow!("bind identity probe requires the non-root storage owner")
        })?;
        let (uid, gid) = user
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("invalid bind probe owner"))?;
        ensure!(
            uid.parse::<u32>().is_ok_and(|uid| uid != 0) && gid.parse::<u32>().is_ok(),
            "invalid bind probe owner"
        );
        let source = source
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("bind path must be UTF-8"))?;
        ensure!(
            Path::new(source).is_absolute() && !source.contains([',', '\0', '\n', '\r']),
            "bind source cannot be represented by the Docker probe"
        );
        let target = match relative_path {
            Some(relative) => {
                ensure!(
                    relative.components().count() == 1 && relative.file_name().is_some(),
                    "invalid bind probe relative path"
                );
                format!(
                    "/probe/{}",
                    relative
                        .to_str()
                        .ok_or_else(|| anyhow::anyhow!("probe path must be UTF-8"))?
                )
            }
            None => "/probe".to_owned(),
        };
        let name = format!("pockymoe-bind-probe-{}", uuid::Uuid::new_v4());
        let result = self
            .output(
                context,
                &[
                    "run".into(),
                    "--rm".into(),
                    "--name".into(),
                    name.clone(),
                    "--user".into(),
                    user.into(),
                    "--network".into(),
                    "none".into(),
                    "--read-only".into(),
                    "--cap-drop".into(),
                    "ALL".into(),
                    "--security-opt".into(),
                    "no-new-privileges:true".into(),
                    "--cpus".into(),
                    "0.1".into(),
                    "--memory".into(),
                    "33554432".into(),
                    "--memory-swap".into(),
                    "33554432".into(),
                    "--pids-limit".into(),
                    "16".into(),
                    "--mount".into(),
                    format!("type=bind,src={source},dst=/probe,readonly,bind-propagation=rprivate"),
                    "--entrypoint".into(),
                    "sha256sum".into(),
                    self.probe_image.clone(),
                    target,
                ],
            )
            .await;
        if result.is_err() {
            // A killed Docker client does not kill its container. Remove only
            // this random helper identity; never run daemon-wide cleanup.
            let _ = self
                .output(
                    context,
                    &["container".into(), "rm".into(), "--force".into(), name],
                )
                .await;
        }
        let output = result?;
        let output = std::str::from_utf8(&output)
            .map_err(|_| anyhow::anyhow!("invalid bind proof response"))?;
        let hash = output
            .split_whitespace()
            .next()
            .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| anyhow::anyhow!("invalid bind proof hash"))?;
        Ok(hash.to_ascii_lowercase())
    }

    async fn inspect_environment(
        &self,
        context: &LocalDockerContext,
        id: &str,
    ) -> Result<Vec<Value>> {
        let ids = self
            .output(
                context,
                &[
                    "container".into(),
                    "ls".into(),
                    "--all".into(),
                    "--quiet".into(),
                    "--no-trunc".into(),
                    "--filter".into(),
                    format!("label={ENVIRONMENT_LABEL}={id}"),
                ],
            )
            .await?;
        let ids = std::str::from_utf8(&ids)
            .map_err(|_| anyhow::anyhow!("Docker identities are invalid"))?;
        let ids: Vec<_> = ids.split_whitespace().collect();
        ensure!(
            ids.len() <= 16
                && ids
                    .iter()
                    .all(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())),
            "unexpected Docker identity list"
        );
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut args = vec!["container".into(), "inspect".into()];
        args.extend(ids.into_iter().map(str::to_owned));
        self.json(context, &args)
            .await?
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Docker container inspect is unavailable"))
    }
}

fn validate_daemon(info: &Value) -> Result<()> {
    ensure!(
        info["OSType"] == "linux",
        "execution environments require a Linux Docker daemon"
    );
    for capability in [
        "MemoryLimit",
        "SwapLimit",
        "PidsLimit",
        "CpuCfsQuota",
        "CpuCfsPeriod",
    ] {
        ensure!(
            info[capability] == true,
            "Docker daemon cannot enforce required resource limits"
        );
    }
    ensure!(
        info["SecurityOptions"]
            .as_array()
            .is_some_and(
                |options| !options.iter().any(|option| option
                    .as_str()
                    .is_some_and(|value| value.starts_with("name=rootless")
                        || value.starts_with("name=userns")))
            ),
        "Docker rootless/UID remapping requires a storage ownership adapter"
    );
    Ok(())
}

/// Resolve the most specific Supervisor mount, preserving the container path
/// as the eventual sandbox target. A local socket does not imply equal paths.
pub fn resolve_daemon_source(path: &Path, host: &DockerHostInfo) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "bind path must be absolute");
    if let Some(container) = &host.supervisor_container {
        let mounts = container["Mounts"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Supervisor mount namespace is unavailable"))?;
        let matched = mounts
            .iter()
            .filter_map(|mount| {
                let target = Path::new(mount["Destination"].as_str()?);
                path.strip_prefix(target)
                    .ok()
                    .map(|suffix| (mount, target, suffix))
            })
            .max_by_key(|(_, target, _)| target.components().count());
        let Some((mount, _, suffix)) = matched else {
            bail!("source is in the Supervisor container layer, not shared storage");
        };
        ensure!(
            matches!(mount["Type"].as_str(), Some("bind" | "volume")),
            "Supervisor source is not persistent bind or volume storage"
        );
        let source = mount["Source"]
            .as_str()
            .filter(|source| Path::new(source).is_absolute())
            .ok_or_else(|| anyhow::anyhow!("Supervisor daemon source is unavailable"))?;
        Ok(Path::new(source).join(suffix))
    } else {
        ensure!(
            host.host_namespace_verified,
            "Supervisor storage namespace is unverified"
        );
        Ok(path.to_path_buf())
    }
}

pub(super) struct ExpectedContainer<'a> {
    pub id: &'a str,
    pub generation: u64,
    pub uid_gid: &'a str,
    pub image: &'a str,
    pub mounts: &'a [EnvironmentMountDto],
    pub limits: &'a EnvironmentLimitsDto,
}

/// Treat a successful Dokploy deployment as insufficient: enforce the actual
/// Engine configuration before exposing any execution target.
pub(super) fn verify_container(
    value: &Value,
    expected: &ExpectedContainer<'_>,
) -> Result<EnvironmentLimitsDto> {
    let config = &value["Config"];
    let host = &value["HostConfig"];
    ensure!(
        value["State"]["Running"] == true,
        "environment container is not running"
    );
    ensure!(
        config["Labels"][ENVIRONMENT_LABEL] == expected.id,
        "container ownership label differs"
    );
    ensure!(
        config["Labels"][GENERATION_LABEL] == expected.generation.to_string(),
        "container generation differs"
    );
    ensure!(config["User"] == expected.uid_gid, "container user differs");
    ensure!(config["Image"] == expected.image, "container image differs");
    ensure!(
        host["Privileged"] == false,
        "privileged environment rejected"
    );
    ensure!(
        host["NetworkMode"] == "none",
        "environment networking is not disabled"
    );
    ensure!(
        host["PidMode"].as_str().is_some_and(str::is_empty),
        "shared PID namespace rejected"
    );
    ensure!(
        matches!(host["IpcMode"].as_str(), Some("private")),
        "shared IPC namespace rejected"
    );
    ensure!(host["Init"] == true, "environment init/reaper is missing");
    ensure!(
        host["ReadonlyRootfs"] == true,
        "environment root filesystem is writable"
    );
    let caps = host["CapDrop"].as_array();
    ensure!(
        caps.is_some_and(|v| v
            .iter()
            .any(|v| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("all")))),
        "container capabilities are not dropped"
    );
    ensure!(
        host["CapAdd"].as_array().is_none_or(Vec::is_empty),
        "added capabilities rejected"
    );
    ensure!(
        host["GroupAdd"].as_array().is_none_or(Vec::is_empty),
        "supplementary container groups rejected"
    );
    ensure!(
        host["UsernsMode"].as_str().is_none_or(|v| v != "host"),
        "host user namespace override rejected"
    );
    ensure!(
        host["Devices"].as_array().is_none_or(Vec::is_empty),
        "host devices rejected"
    );
    ensure!(
        host["DeviceRequests"].as_array().is_none_or(Vec::is_empty),
        "device requests rejected"
    );
    ensure!(
        host["SecurityOpt"].as_array().is_some_and(|v| !v.is_empty()
            && v.iter().all(|v| matches!(
                v.as_str(),
                Some("no-new-privileges:true" | "no-new-privileges")
            ))),
        "container security options differ from no-new-privileges"
    );
    ensure!(
        host["PortBindings"]
            .as_object()
            .is_none_or(|v| v.is_empty()),
        "published environment ports rejected"
    );

    let mounts = value["Mounts"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("container mount inspection unavailable"))?;
    let mut actual_binds = 0;
    for actual in mounts {
        match actual["Type"].as_str() {
            Some("bind") => {
                actual_binds += 1;
                ensure!(
                    expected.mounts.iter().any(|mount| {
                        actual["Source"] == mount.source
                            && actual["Destination"] == mount.target
                            && actual["RW"].as_bool() == Some(!mount.read_only)
                            && actual["Propagation"]
                                .as_str()
                                .is_some_and(|s| matches!(s, "rprivate" | "private"))
                    }),
                    "container has an unexpected bind mount"
                );
            }
            Some("tmpfs") => ensure!(
                matches!(actual["Destination"].as_str(), Some("/tmp" | "/run")),
                "unexpected temporary mount"
            ),
            _ => bail!("unexpected persistent container mount"),
        }
    }
    ensure!(
        actual_binds == expected.mounts.len(),
        "container bind mount count differs"
    );
    let cpu = host["NanoCpus"]
        .as_u64()
        .filter(|v| *v > 0)
        .map(|v| v as f64 / 1_000_000_000.0)
        .or_else(|| {
            let quota = host["CpuQuota"].as_i64()?;
            let period = host["CpuPeriod"].as_i64()?;
            (quota > 0 && period > 0).then_some(quota as f64 / period as f64)
        })
        .ok_or_else(|| anyhow::anyhow!("container CPU hard limit is missing"))?;
    let memory = host["Memory"]
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or_else(|| anyhow::anyhow!("container memory hard limit is missing"))?;
    ensure!(
        host["MemorySwap"].as_u64() == Some(memory),
        "container swap is not disabled"
    );
    let pids = host["PidsLimit"]
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or_else(|| anyhow::anyhow!("container PID limit is missing"))?;
    let nofile = host["Ulimits"]
        .as_array()
        .and_then(|v| v.iter().find(|v| v["Name"] == "nofile"))
        .filter(|v| v["Soft"] == v["Hard"])
        .and_then(|v| v["Hard"].as_u64())
        .filter(|v| *v > 0)
        .ok_or_else(|| anyhow::anyhow!("container nofile hard limit is missing"))?;
    let observed = EnvironmentLimitsDto {
        cpu,
        memory_bytes: memory,
        pids: u32::try_from(pids)?,
        nofile: u32::try_from(nofile)?,
    };
    ensure!(
        (observed.cpu - expected.limits.cpu).abs() < 0.000001
            && observed.memory_bytes == expected.limits.memory_bytes
            && observed.pids == expected.limits.pids
            && observed.nofile == expected.limits.nofile,
        "observed container hard limits differ"
    );
    Ok(observed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn container_namespace_uses_longest_bind_and_volume_prefix() {
        let host = DockerHostInfo {
            daemon_id: "fixture".into(),
            cpus: 4.0,
            memory_bytes: 1,
            supervisor_container: Some(json!({"Mounts":[
                {"Type":"bind","Source":"/srv/projects","Destination":"/work","RW":true},
                {"Type":"volume","Source":"/var/lib/docker/volumes/private/_data","Destination":"/work/state","RW":true},
                {"Type":"bind","Source":"/srv/worker","Destination":"/opt/worker","RW":false}
            ]})),
            host_namespace_verified: false,
        };
        assert_eq!(
            resolve_daemon_source(Path::new("/work/repo"), &host).unwrap(),
            Path::new("/srv/projects/repo")
        );
        assert_eq!(
            resolve_daemon_source(Path::new("/work/state/env"), &host).unwrap(),
            Path::new("/var/lib/docker/volumes/private/_data/env")
        );
        assert!(resolve_daemon_source(Path::new("/worktree/repo"), &host).is_err());
        assert!(resolve_daemon_source(Path::new("/bin/pockymoe"), &host).is_err());
        assert_eq!(
            resolve_daemon_source(Path::new("/opt/worker"), &host).unwrap(),
            Path::new("/srv/worker")
        );
    }

    #[test]
    fn unknown_namespace_cannot_assume_equal_host_paths() {
        let host = DockerHostInfo {
            daemon_id: "fixture".into(),
            cpus: 4.0,
            memory_bytes: 1,
            supervisor_container: None,
            host_namespace_verified: false,
        };
        assert!(resolve_daemon_source(Path::new("/work"), &host).is_err());
    }

    #[test]
    fn daemon_requires_enforceable_limits_and_native_storage_uid_mapping() {
        let info = json!({"OSType":"linux","MemoryLimit":true,"SwapLimit":true,"PidsLimit":true,"CpuCfsQuota":true,"CpuCfsPeriod":true,"SecurityOptions":["name=seccomp,profile=builtin"]});
        validate_daemon(&info).unwrap();
        for field in [
            "MemoryLimit",
            "SwapLimit",
            "PidsLimit",
            "CpuCfsQuota",
            "CpuCfsPeriod",
        ] {
            let mut invalid = info.clone();
            invalid[field] = json!(false);
            assert!(validate_daemon(&invalid).is_err());
        }
        for option in ["name=rootless", "name=userns"] {
            let mut invalid = info.clone();
            invalid["SecurityOptions"] = json!([option]);
            assert!(validate_daemon(&invalid).is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cli_proof_has_the_storage_owner_and_no_inherited_credentials() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("docker-fixture");
        std::fs::write(&executable,r#"#!/usr/bin/python3
import hashlib, os, pathlib, sys
args=sys.argv[1:]
if set(os.environ)-{'PATH','LC_CTYPE'}: sys.exit(10)
required=['--user','--network','--read-only','--cap-drop','--security-opt','--memory','--pids-limit','--host','--config']
if not all(value in args for value in required): sys.exit(11)
if args[args.index('--user')+1]!='1000:1000': sys.exit(12)
if args[args.index('--network')+1]!='none': sys.exit(13)
if args[args.index('--cap-drop')+1]!='ALL': sys.exit(14)
mount=args[args.index('--mount')+1]
source=next(value[4:] for value in mount.split(',') if value.startswith('src='))
if 'readonly' not in mount.split(','): sys.exit(15)
relative=args[-1].removeprefix('/probe').lstrip('/')
path=pathlib.Path(source)/relative if relative else pathlib.Path(source)
print(hashlib.sha256(path.read_bytes()).hexdigest())
"#).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let proof = root.path().join("proof");
        std::fs::write(&proof, b"same-storage-fixture").unwrap();
        let driver = CliEnvironmentDocker {
            executable,
            probe_image: "fixture-probe:1".into(),
        };
        let context = LocalDockerContext {
            docker_host: Some("unix:///fixture/docker.sock".into()),
            probe_user: Some("1000:1000".into()),
        };
        let actual = driver
            .bind_sha256(&context, root.path(), Some(Path::new("proof")))
            .await
            .unwrap();
        assert_eq!(actual, super::super::storage::file_sha256(&proof).unwrap());
    }
}
