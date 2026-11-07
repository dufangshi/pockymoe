//! Durable metadata-only proposals. An owner decision is deliberately outside
//! this module: the HTTP layer must prove the owner, never trust tool approval.
use super::*;
use sha2::{Digest, Sha256};

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct EnvironmentProfile {
    pub authentication: EnvironmentAuthentication,
    pub credentials: Vec<ApprovedCredential>,
    pub egress_hosts: Option<Vec<String>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ApprovedCredential {
    pub proposal: CredentialMountProposal,
    identity: SourceIdentity,
}
impl ApprovedCredential {
    pub(super) fn identity_is_directory(&self) -> bool {
        self.identity.directory
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceIdentity {
    device: u64,
    inode: u64,
    directory: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct RequestRecord {
    pub dto: EnvironmentRequestDto,
    profile: EnvironmentProfile,
    client_request_id: Option<String>,
    input_digest: String,
    #[serde(default)]
    host_fenced: bool,
    #[serde(default)]
    continuation: Option<String>,
}

/// Obtained from the authenticated thread and Supervisor records, not the spec.
pub struct EnvironmentRequestContext {
    pub thread_id: String,
    pub workspace_id: String,
    pub workspace_path: PathBuf,
    pub working_path: PathBuf,
    pub harness: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

fn digest(value: &impl Serialize) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

fn source_identity(path: &Path) -> Result<SourceIdentity> {
    ensure!(path.is_absolute(), "credential source must be absolute");
    // canonicalize checks all ancestors without reading a credential body. A
    // symlink may not silently turn a reviewed path into another source.
    ensure!(
        path.canonicalize().ok().as_deref() == Some(path),
        "credential source must exist and contain no symlink or non-normal component"
    );
    let meta = path
        .symlink_metadata()
        .context("credential source metadata is unavailable")?;
    ensure!(
        meta.is_file() || meta.is_dir(),
        "credential sockets, devices and special files are unsupported"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(SourceIdentity {
            device: meta.dev(),
            inode: meta.ino(),
            directory: meta.is_dir(),
        })
    }
    #[cfg(not(unix))]
    {
        bail!("credential binds require Linux file identity");
    }
}

fn normal_absolute(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    ensure!(
        value.len() <= 4096
            && path.is_absolute()
            && value != "/"
            && !value.contains(['\0', '\n', '\r'])
            && path.components().all(|c| matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )),
        "invalid absolute credential path"
    );
    let normalized: PathBuf = path.components().collect();
    ensure!(
        normalized.to_string_lossy() == value && !value.ends_with('/'),
        "credential path must be normalized"
    );
    Ok(path.to_owned())
}

fn overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

// Only metadata is inspected before approval. A bind of a directory also binds
// its Unix sockets/hardlinks; those must not become an alternate control-plane
// entry point hidden below an otherwise innocent configuration directory.
fn codex_helper_alias(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let parts = relative
        .iter()
        .filter_map(|p| p.to_str())
        .collect::<Vec<_>>();
    if parts.len() != 4
        || parts[0] != "tmp"
        || !matches!(parts[1], "arg0" | "arg0-linux")
        || !parts[2].starts_with("codex-arg0")
        || !parts[2]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        || !matches!(
            parts[3],
            "apply_patch" | "applypatch" | "codex-linux-sandbox" | "codex-execve-wrapper"
        )
    {
        return false;
    }
    // Codex creates these nonsecret dispatch aliases itself. They resolve in
    // the container's mount namespace, not the source's Host namespace. Do not
    // follow one during metadata review or allow aliases to any other file.
    let Ok(target) = std::fs::read_link(path) else {
        return false;
    };
    target.file_name().is_some_and(|n| n == "codex")
        && target.to_str().is_some_and(|v| normal_absolute(v).is_ok())
}

fn inspect_credential_tree(root: &Path, purpose: CredentialMountPurpose) -> Result<()> {
    let mut pending = vec![root.to_owned()];
    let mut count = 0usize;
    while let Some(path) = pending.pop() {
        count += 1;
        ensure!(count<=100_000,"credential directory exceeds supported metadata review size; propose a narrower source");
        let metadata = path
            .symlink_metadata()
            .context("credential directory metadata is unreadable")?;
        if metadata.file_type().is_symlink()
            && purpose == CredentialMountPurpose::Codex
            && codex_helper_alias(root, &path)
        {
            continue;
        }
        ensure!(!metadata.file_type().is_symlink() && (metadata.is_dir() || metadata.is_file()),"credential directories cannot contain symlinks, sockets or special files; propose a narrower source");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            ensure!(
                !metadata.is_file() || metadata.nlink() == 1,
                "hardlinked credential files cannot be granted"
            );
        }
        if metadata.is_dir() {
            for entry in
                std::fs::read_dir(&path).context("credential directory metadata is unreadable")?
            {
                pending.push(entry?.path());
            }
        }
    }
    Ok(())
}

pub(crate) fn valid_egress(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.contains('.')
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        && !host.starts_with('.')
        && !host.ends_with('.')
        && host
            .split('.')
            .all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-'))
        && host.parse::<std::net::IpAddr>().is_err()
}

impl EnvironmentManager {
    pub fn list_requests(&self, thread: Option<&str>) -> Result<Vec<EnvironmentRequestDto>> {
        self.read(|s| {
            Ok(s.requests
                .values()
                .filter(|r| thread.is_none_or(|t| r.dto.thread_id == t))
                .map(|r| r.dto.clone())
                .collect())
        })
    }

    pub fn get_request(&self, id: &str) -> Result<EnvironmentRequestDto> {
        self.read(|s| {
            Ok(s.requests
                .get(id)
                .context("environment request not found")?
                .dto
                .clone())
        })
    }

    pub fn propose(
        &self,
        input: &ProposeEnvironmentRequest,
        context: EnvironmentRequestContext,
    ) -> Result<EnvironmentRequestDto> {
        ensure!(
            input.handoff.chars().count() <= 12_000,
            "handoff exceeds 12000 characters; never include secrets"
        );
        ensure!(
            input.credential_mounts.len() <= 16,
            "at most 16 credential mounts may be proposed"
        );
        ensure!(
            input
                .client_request_id
                .as_ref()
                .is_none_or(|id| !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))),
            "invalid clientRequestId"
        );
        ensure!(
            matches!(context.harness.as_str(), "codex" | "claude" | "custom"),
            "unsupported_isolated_harness"
        );
        let input_digest = digest(input)?;
        if let Some(key) = &input.client_request_id {
            if let Some(previous) = self.read(|s| {
                Ok(s.requests
                    .values()
                    .find(|r| {
                        r.dto.thread_id == context.thread_id
                            && r.client_request_id.as_ref() == Some(key)
                    })
                    .cloned())
            })? {
                ensure!(
                    previous.input_digest == input_digest,
                    "conflict: clientRequestId already names another immutable proposal"
                );
                return Ok(previous.dto);
            }
        }
        let connection = self.get_connection(&input.connection_id)?;
        let workspace = storage::canonical_directory(&context.workspace_path)?;
        let working = storage::canonical_directory(&context.working_path)?;
        let roots = storage::allowed_roots(&connection.allowed_roots)?;
        let projects = storage::git_closure(&workspace, &working, &roots)?;
        for path in &projects {
            self.validate_project_source(path)?;
        }
        let limits = input.limits.clone().unwrap_or(EnvironmentLimitsDto {
            cpu: connection.cpu_limit,
            memory_bytes: connection.memory_limit_bytes,
            pids: connection.pids_limit,
            nofile: connection.nofile_limit,
        });
        validate_limits(&limits)?;
        ensure!(
            limits.cpu <= connection.cpu_limit
                && limits.memory_bytes <= connection.memory_limit_bytes
                && limits.pids <= connection.pids_limit
                && limits.nofile <= connection.nofile_limit,
            "requested limits exceed the connection ceiling"
        );
        let mut egress = input
            .egress_hosts
            .iter()
            .map(|h| h.to_ascii_lowercase())
            .collect::<Vec<_>>();
        egress.sort();
        egress.dedup();
        ensure!(
            egress.len() <= 128 && egress.iter().all(|h| valid_egress(h)),
            "egress must name exact HTTPS DNS hosts; IPs, wildcards, ports and SSH are unsupported"
        );
        let mut profile = EnvironmentProfile {
            authentication: input.authentication.clone(),
            credentials: Vec::new(),
            egress_hosts: Some(egress.clone()),
        };
        for mount in &input.credential_mounts {
            let source = normal_absolute(&mount.source)?;
            let target = normal_absolute(&mount.target)?;
            self.validate_credential_source_policy(&source)?;
            let identity = source_identity(&source)?;
            inspect_credential_tree(&source, mount.purpose)?;
            let native_file = !identity.directory
                && match mount.purpose {
                    CredentialMountPurpose::Codex => {
                        target == Path::new("/pockymoe/state/codex/auth.json")
                            && source.file_name().is_some_and(|n| n == "auth.json")
                    }
                    CredentialMountPurpose::Claude => {
                        target == Path::new("/pockymoe/state/claude/.credentials.json")
                            && source.file_name().is_some_and(|n| n == ".credentials.json")
                    }
                    _ => false,
                };
            ensure!(target.starts_with("/credentials") && target!=Path::new("/credentials") || native_file,"credential target must be below /credentials; native read-only file slots are the only state exception");
            ensure!(!native_file || mount.read_only,"native refresh requires approval of a whole writable directory, not a writable single-file bind");
            ensure!(
                !projects.iter().any(|p| overlap(p, &target)),
                "credential target overlaps project/Git storage"
            );
            ensure!(
                !profile
                    .credentials
                    .iter()
                    .any(|p| overlap(Path::new(&p.proposal.target), &target)),
                "credential targets may not overlap or nest"
            );
            ensure!(
                !profile
                    .credentials
                    .iter()
                    .any(|p| (!p.proposal.read_only || !mount.read_only)
                        && (overlap(Path::new(&p.proposal.source), &source)
                            || (p.identity.device == identity.device
                                && p.identity.inode == identity.inode))),
                "writable credential sources may not overlap within one proposal"
            );
            for (key, value) in &mount.environment {
                // Values are pointers only. Never accept arbitrary environment
                // values, API/auth overrides, PATH, proxy or Pockymoe identities.
                ensure!(
                    matches!(
                        key.as_str(),
                        "CODEX_HOME"
                            | "CLAUDE_CONFIG_DIR"
                            | "GH_CONFIG_DIR"
                            | "AWS_SHARED_CREDENTIALS_FILE"
                            | "AWS_CONFIG_FILE"
                            | "GOOGLE_APPLICATION_CREDENTIALS"
                            | "AZURE_CONFIG_DIR"
                            | "KUBECONFIG"
                            | "NPM_CONFIG_USERCONFIG"
                            | "DOCKER_CONFIG"
                            | "SSL_CERT_FILE"
                            | "NODE_EXTRA_CA_CERTS"
                            | "CURL_CA_BUNDLE"
                    ),
                    "unsupported credential configuration variable"
                );
                let value_path = normal_absolute(value)?;
                ensure!(
                    (identity.directory && value_path.starts_with(&target))
                        || (!identity.directory && value_path == target)
                        || (native_file && value_path == target.parent().unwrap()),
                    "credential environment value must point inside its approved target"
                );
                ensure!(
                    !profile
                        .credentials
                        .iter()
                        .any(|m| m.proposal.environment.contains_key(key)),
                    "duplicate credential configuration variable"
                );
                ensure!(
                    match key.as_str() {
                        "CODEX_HOME" => mount.purpose == CredentialMountPurpose::Codex,
                        "CLAUDE_CONFIG_DIR" => mount.purpose == CredentialMountPurpose::Claude,
                        "GH_CONFIG_DIR" => mount.purpose == CredentialMountPurpose::Github,
                        _ => mount.purpose == CredentialMountPurpose::Generic,
                    },
                    "credential purpose does not match its configuration variable"
                );
            }
            if let Some(key) = match mount.purpose {
                CredentialMountPurpose::Codex => Some("CODEX_HOME"),
                CredentialMountPurpose::Claude => Some("CLAUDE_CONFIG_DIR"),
                CredentialMountPurpose::Github => Some("GH_CONFIG_DIR"),
                _ => None,
            } {
                ensure!(
                    mount.environment.contains_key(key),
                    "native/gh mapping requires its explicit tool configuration directory pointer"
                );
                ensure!(
                    identity.directory || native_file,
                    "this tool requires a directory or its supported native read-only file slot"
                );
                ensure!(!identity.directory || mount.purpose==CredentialMountPurpose::Github || !mount.read_only,"native Codex/Claude need a writable config directory for state/refresh; use the explicit read-only single-file slot for a read-only login");
                let expected = if identity.directory {
                    &target
                } else {
                    target.parent().unwrap()
                };
                ensure!(
                    Path::new(&mount.environment[key]) == expected,
                    "tool config directory must match the exact credential target"
                );
                ensure!(
                    !profile
                        .credentials
                        .iter()
                        .any(|m| m.proposal.purpose == mount.purpose),
                    "only one mapping per native tool is supported"
                );
            }
            profile.credentials.push(ApprovedCredential {
                proposal: mount.clone(),
                identity,
            });
        }
        for (mode, purpose) in [
            (profile.authentication.codex, CredentialMountPurpose::Codex),
            (
                profile.authentication.claude,
                CredentialMountPurpose::Claude,
            ),
        ] {
            let mapped = profile
                .credentials
                .iter()
                .any(|m| m.proposal.purpose == purpose);
            ensure!(mapped==(mode==EnvironmentAuthMode::NativeFile),"native file authentication and its explicit credential mapping must be selected together");
        }
        if profile.authentication.codex == EnvironmentAuthMode::NativeFile {
            ensure!(
                ["auth.openai.com", "chatgpt.com"]
                    .iter()
                    .all(|h| egress.iter().any(|e| e == h)),
                "native Codex requires explicit HTTPS approval for auth.openai.com and chatgpt.com"
            );
        }
        if profile.authentication.claude == EnvironmentAuthMode::NativeFile {
            ensure!(["api.anthropic.com","platform.claude.com"].iter().all(|h|egress.iter().any(|e|e==h)),"native Claude requires explicit HTTPS approval for api.anthropic.com and platform.claude.com");
        }
        let name = input
            .name
            .as_deref()
            .unwrap_or("Agent environment")
            .trim()
            .to_owned();
        ensure!(
            !name.is_empty() && name.len() <= 200 && !name.contains(['\0', '\n', '\r']),
            "invalid environment name"
        );
        let spec = EnvironmentRequestSpec {
            connection,
            thread_id: context.thread_id.clone(),
            workspace_id: context.workspace_id,
            workspace_path: workspace.to_string_lossy().into_owned(),
            working_path: working.to_string_lossy().into_owned(),
            harness: context.harness,
            model: context.model,
            reasoning_effort: context.reasoning_effort,
            name,
            limits,
            egress_hosts: egress,
            authentication: profile.authentication.clone(),
            credential_mounts: profile
                .credentials
                .iter()
                .map(|c| c.proposal.clone())
                .collect(),
            handoff: input.handoff.clone(),
            migration: "sameThreadFreshSession".into(),
            credential_mount_kinds: Vec::new(),
        };
        let mut spec = spec;
        spec.credential_mount_kinds = profile
            .credentials
            .iter()
            .map(|m| {
                if m.identity.directory {
                    "directory"
                } else {
                    "file"
                }
                .into()
            })
            .collect();
        let now = now_rfc3339();
        // Bind the digest to source identities too. Replacing the source requires
        // a new proposal even if its human-readable path has stayed identical.
        let review_digest = digest(&(&spec, &profile))?;
        let dto = EnvironmentRequestDto {
            id: uuid::Uuid::new_v4().to_string(),
            thread_id: context.thread_id,
            spec,
            digest: review_digest,
            state: "pending".into(),
            environment_id: None,
            last_error: None,
            approved_at: None,
            continuation_queued: false,
            created_at: now.clone(),
            updated_at: now,
        };
        self.write(|s| {
            ensure!(
                s.requests.len() < 4096,
                "environment request retention limit reached"
            );
            ensure!(
                !s.requests.values().any(|r| r.dto.thread_id == dto.thread_id
                    && matches!(
                        r.dto.state.as_str(),
                        "pending" | "waitingIdle" | "provisioning"
                    )),
                "thread already has an outstanding environment request"
            );
            s.requests.insert(
                dto.id.clone(),
                RequestRecord {
                    dto: dto.clone(),
                    profile,
                    client_request_id: input.client_request_id.clone(),
                    input_digest,
                    host_fenced: false,
                    continuation: None,
                },
            );
            Ok(dto)
        })
    }

    fn validate_credential_source_policy(&self, source: &Path) -> Result<()> {
        let database = PathBuf::from(
            self.directory
                .to_string_lossy()
                .trim_end_matches(".environments"),
        );
        let executable = std::env::current_exe()?.canonicalize()?;
        let mut denied = vec![
            self.directory.clone(),
            database.clone(),
            database.with_extension("cli.json"),
            crate::upstreams::directory(&database),
            executable,
            PathBuf::from("/var/run/docker.sock"),
            PathBuf::from("/run/docker.sock"),
            PathBuf::from("/proc"),
            PathBuf::from("/sys"),
            PathBuf::from("/dev"),
            PathBuf::from("/run"),
            PathBuf::from("/etc"),
            PathBuf::from("/root"),
        ];
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            ensure!(
                source != home && !home.starts_with(source),
                "an entire host home or its parent cannot be granted"
            );
            for name in [
                ".ssh",
                ".remote-codex",
                ".config/pockymoe",
                ".config/remote-codex",
                ".local/share/pockymoe",
                ".local/share/remote-codex",
                ".docker",
            ] {
                denied.push(home.join(name));
            }
        }
        ensure!(!denied.iter().any(|p|overlap(p,source)),"credential source exposes a private control plane, system directory, socket or unsupported SSH store");
        ensure!(
            !source.file_name().is_some_and(|s| s == "docker.sock"),
            "Docker sockets cannot be granted"
        );
        // The database WAL/SHM and management/CLI sidecars share this filename
        // prefix; granting the database parent is already caught by overlap.
        ensure!(
            !source
                .to_string_lossy()
                .starts_with(database.to_string_lossy().as_ref()),
            "Supervisor database and identity sidecars cannot be granted"
        );
        Ok(())
    }

    pub(super) fn validate_credential_sources(&self, profile: &EnvironmentProfile) -> Result<()> {
        for grant in &profile.credentials {
            let source = Path::new(&grant.proposal.source);
            self.validate_credential_source_policy(source)?;
            ensure!(
                source_identity(source)? == grant.identity,
                "credential source identity changed; submit a new proposal for owner review"
            );
        }
        Ok(())
    }

    /// The HTTP owner proof is required before invoking this method.
    pub fn decide_request(
        &self,
        id: &str,
        digest: &str,
        approve: bool,
    ) -> Result<EnvironmentRequestDto> {
        self.write(|s| {
            let request=s.requests.get(id).context("environment request not found")?;
            ensure!(request.dto.digest==digest,"conflict: approval digest does not match the immutable proposal");
            if approve && request.dto.approved_at.is_some() { return Ok(request.dto.clone()); }
            ensure!(request.dto.state=="pending","conflict: request is no longer pending");
            if approve {
                self.validate_credential_sources(&request.profile)?;
                for grant in &request.profile.credentials {inspect_credential_tree(Path::new(&grant.proposal.source), grant.proposal.purpose)?;}
                for other in s.requests.values().filter(|r|r.dto.id!=id && r.dto.approved_at.is_some() && !matches!(r.dto.state.as_str(),"cancelled"|"rejected")) {
                    let released=other.dto.environment_id.as_ref().and_then(|e|s.environments.get(e)).is_some_and(|e|e.dto.state=="destroyed");
                    if released {continue;}
                    ensure!(!request.profile.credentials.iter().any(|a|other.profile.credentials.iter().any(|b|(!a.proposal.read_only || !b.proposal.read_only) && (overlap(Path::new(&a.proposal.source),Path::new(&b.proposal.source)) || (a.identity.device==b.identity.device && a.identity.inode==b.identity.inode)))),"credential source has an exclusive writable grant in another environment; destroy it before reassigning");
                }
            }
            let r=s.requests.get_mut(id).unwrap();
            r.dto.state=if approve {"waitingIdle"} else {"rejected"}.into();
            if approve { r.dto.approved_at=Some(now_rfc3339()); }
            r.dto.updated_at=now_rfc3339(); Ok(r.dto.clone())
        })
    }

    pub fn cancel_request(&self, id: &str, thread: &str) -> Result<EnvironmentRequestDto> {
        self.write(|s|{let r=s.requests.get_mut(id).context("environment request not found")?;ensure!(r.dto.thread_id==thread,"forbidden: request belongs to another thread"); if r.dto.state=="cancelled" {return Ok(r.dto.clone());} ensure!(matches!(r.dto.state.as_str(),"pending"|"waitingIdle") && !r.host_fenced,"conflict: provisioning already fenced Host; only the owner can recover this request");r.dto.state="cancelled".into();r.dto.updated_at=now_rfc3339();Ok(r.dto.clone())})
    }

    pub fn retry_request(&self, id: &str, digest: &str) -> Result<EnvironmentRequestDto> {
        self.write(|s| {
            let r = s
                .requests
                .get_mut(id)
                .context("environment request not found")?;
            ensure!(
                r.dto.digest == digest && r.dto.approved_at.is_some(),
                "approved immutable request required"
            );
            if matches!(
                r.dto.state.as_str(),
                "waitingIdle" | "provisioning" | "ready"
            ) {
                return Ok(r.dto.clone());
            }
            ensure!(
                r.dto.state == "failed",
                "only a failed request can be retried"
            );
            r.dto.state = "waitingIdle".into();
            r.dto.last_error = None;
            r.dto.updated_at = now_rfc3339();
            Ok(r.dto.clone())
        })
    }

    pub fn complete_host_recovery(&self, id: &str, digest: &str) -> Result<EnvironmentRequestDto> {
        self.write(|s| {
            let r = s
                .requests
                .get(id)
                .context("environment request not found")?;
            ensure!(
                r.dto.digest == digest && r.dto.approved_at.is_some(),
                "approved immutable request required"
            );
            if r.dto.state == "cancelled" && !r.host_fenced {
                return Ok(r.dto.clone());
            }
            ensure!(
                r.dto.state == "failed",
                "only a failed migration can recover to Host"
            );
            ensure!(
                r.dto.environment_id.as_ref().is_none_or(|id| s
                    .environments
                    .get(id)
                    .is_some_and(|e| e.dto.state == "destroyed")),
                "destroy the old computation before restoring Host"
            );
            let thread = r.dto.thread_id.clone();
            s.bindings.remove(&thread);
            let r = s.requests.get_mut(id).unwrap();
            r.dto.state = "cancelled".into();
            r.dto.updated_at = now_rfc3339();
            r.host_fenced = false;
            Ok(r.dto.clone())
        })
    }

    pub fn request_blocks_turn(&self, thread: &str) -> Result<bool> {
        self.read(|s| {
            Ok(s.requests.values().any(|r| {
                r.dto.thread_id == thread
                    && (matches!(r.dto.state.as_str(), "waitingIdle" | "provisioning")
                        || (r.host_fenced && r.dto.state == "failed"))
            }))
        })
    }

    pub fn require_fenced_binding(&self, thread: &str) -> Result<()> {
        self.read(|s|{ensure!(!s.requests.values().any(|r|r.dto.thread_id==thread && r.host_fenced && !matches!(r.dto.state.as_str(),"cancelled"|"rejected")) || s.bindings.contains_key(thread),"environment_migration_pending: Host execution is fenced; owner recovery is required");Ok(())})
    }

    pub fn begin_request(&self, id: &str, continuation: &str) -> Result<EnvironmentRequestDto> {
        self.write(|s| {
            let r = s
                .requests
                .get_mut(id)
                .context("environment request not found")?;
            ensure!(
                r.dto.approved_at.is_some()
                    && matches!(r.dto.state.as_str(), "waitingIdle" | "provisioning"),
                "request has no active owner authorization"
            );
            r.host_fenced = true;
            r.dto.state = "provisioning".into();
            if r.continuation.is_none() {
                r.continuation = Some(continuation.into());
            }
            r.dto.updated_at = now_rfc3339();
            Ok(r.dto.clone())
        })
    }

    pub fn request_continuation(&self, id: &str) -> Result<String> {
        self.read(|s| {
            s.requests
                .get(id)
                .context("environment request not found")?
                .continuation
                .clone()
                .context("migration continuation is not prepared")
        })
    }

    pub async fn provision_request(&self, id: &str) -> Result<EnvironmentDto> {
        let record = self.read(|s| {
            s.requests
                .get(id)
                .cloned()
                .context("environment request not found")
        })?;
        ensure!(
            record.dto.approved_at.is_some()
                && record.dto.state == "provisioning"
                && record.host_fenced,
            "approved fenced request required"
        );
        let spec = &record.dto.spec;
        ensure!(
            self.get_connection(&spec.connection.id)? == spec.connection,
            "connection changed since review; submit a new proposal"
        );
        self.validate_credential_sources(&record.profile)?;
        let env = self
            .create_configured(
                &CreateEnvironmentRequest {
                    connection_id: spec.connection.id.clone(),
                    thread_id: spec.thread_id.clone(),
                    name: Some(spec.name.clone()),
                },
                Path::new(&spec.workspace_path),
                Path::new(&spec.working_path),
                record.profile,
                Some(spec.limits.clone()),
                Some(id.into()),
            )
            .await?;
        self.write(|s| {
            s.requests.get_mut(id).unwrap().dto.environment_id = Some(env.id.clone());
            Ok(())
        })?;
        match env.state.as_str() {"running"=>Ok(env),"stopped"=>self.action(&env.id,EnvironmentAction::Start).await,_=>bail!("environment is not safely startable; the owner must stop/rebuild its retained generation before retry")}
    }

    pub fn fail_request(&self, id: &str, message: &str) -> Result<()> {
        self.write(|s| {
            let r = s
                .requests
                .get_mut(id)
                .context("environment request not found")?;
            if matches!(r.dto.state.as_str(), "cancelled" | "rejected") {
                return Ok(());
            }
            r.host_fenced |= r.dto.approved_at.is_some();
            r.dto.state = "failed".into();
            r.dto.last_error = Some(message.into());
            r.dto.updated_at = now_rfc3339();
            Ok(())
        })
    }
    pub fn finish_request(&self, id: &str) -> Result<EnvironmentRequestDto> {
        self.write(|s| {
            let r = s
                .requests
                .get_mut(id)
                .context("environment request not found")?;
            ensure!(
                r.dto.state == "provisioning" && r.dto.environment_id.is_some(),
                "provisioned request required"
            );
            r.dto.state = "ready".into();
            r.dto.continuation_queued = true;
            r.dto.last_error = None;
            r.dto.updated_at = now_rfc3339();
            Ok(r.dto.clone())
        })
    }

    pub fn authentication(&self, id: &str) -> Result<EnvironmentAuthentication> {
        Ok(self.record(id)?.profile.authentication)
    }
    pub fn check_credential_sources(&self, id: &str) -> Result<()> {
        self.validate_credential_sources(&self.record(id)?.profile)
    }
    pub(super) async fn credential_mount(
        &self,
        grant: &ApprovedCredential,
        docker: &dyn EnvironmentDocker,
        context: &LocalDockerContext,
        host: &DockerHostInfo,
    ) -> Result<EnvironmentMountDto> {
        inspect_credential_tree(Path::new(&grant.proposal.source), grant.proposal.purpose)?;
        let source = docker::resolve_daemon_source(Path::new(&grant.proposal.source), host)?;
        let (device, inode, directory) =
            docker.bind_identity(context, &source).await.map_err(|_| {
                anyhow::anyhow!("credential source namespace identity could not be verified")
            })?;
        ensure!(
            SourceIdentity {
                device,
                inode,
                directory
            } == grant.identity,
            "credential daemon source is not the approved original storage"
        );
        Ok(EnvironmentMountDto {
            source: source
                .to_str()
                .context("credential daemon source is not UTF-8")?
                .into(),
            target: grant.proposal.target.clone(),
            read_only: grant.proposal.read_only,
        })
    }
    pub(super) async fn verify_running_credentials(
        &self,
        profile: &EnvironmentProfile,
        docker: &dyn EnvironmentDocker,
        context: &LocalDockerContext,
        container: &str,
    ) -> Result<()> {
        self.validate_credential_sources(profile)?;
        for grant in &profile.credentials {
            let (device, inode, directory) = docker
                .container_bind_identity(context, container, &grant.proposal.target)
                .await?;
            ensure!(SourceIdentity{device,inode,directory}==grant.identity,"running credential mount is not the approved source; computation must be stopped before native initialization");
        }
        Ok(())
    }
    pub fn credential_environment(&self, id: &str) -> Result<Vec<(String, String)>> {
        Ok(self
            .record(id)?
            .profile
            .credentials
            .into_iter()
            .flat_map(|m| m.proposal.environment)
            .collect())
    }
    pub fn egress_hosts(&self, id: &str) -> Result<Vec<String>> {
        let r = self.record(id)?;
        Ok(r.profile.egress_hosts.unwrap_or(
            self.get_connection(&r.dto.connection_id)?
                .allowed_egress_hosts,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Lab {
        root: tempfile::TempDir,
        manager: EnvironmentManager,
        project: PathBuf,
    }
    impl Lab {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let project = root.path().join("project");
            std::fs::create_dir(&project).unwrap();
            let manager = EnvironmentManager::new(&root.path().join("supervisor.sqlite"));
            manager
                .write(|s| {
                    s.connections.insert(
                        "fixture".into(),
                        ConnectionRecord {
                            api_key: uuid::Uuid::new_v4().to_string(),
                            public: EnvironmentConnectionDto {
                                id: "fixture".into(),
                                name: "Fixture".into(),
                                base_url: "http://127.0.0.1:1".into(),
                                configured: true,
                                dokploy_environment_id: "fixture".into(),
                                image: "fixture:1".into(),
                                storage_node: "local".into(),
                                docker_host: None,
                                allowed_roots: vec![project.to_string_lossy().into_owned()],
                                allowed_egress_hosts: vec![],
                                cpu_limit: 1.0,
                                memory_limit_bytes: 536870912,
                                pids_limit: 128,
                                nofile_limit: 1024,
                            },
                        },
                    );
                    Ok(())
                })
                .unwrap();
            Self {
                root,
                manager,
                project,
            }
        }
        fn context(&self, id: &str) -> EnvironmentRequestContext {
            EnvironmentRequestContext {
                thread_id: id.into(),
                workspace_id: "workspace".into(),
                workspace_path: self.project.clone(),
                working_path: self.project.clone(),
                harness: "codex".into(),
                model: Some("preserve-model".into()),
                reasoning_effort: Some("max".into()),
            }
        }
        fn input(&self) -> ProposeEnvironmentRequest {
            ProposeEnvironmentRequest{connection_id:"fixture".into(),name:None,limits:None,egress_hosts:vec![],authentication:Default::default(),credential_mounts:vec![],handoff:"Keep the original constraints; inspect unfinished tools instead of replaying them".into(),client_request_id:Some("stable".into())}
        }
        fn mount(&self, name: &str, read_only: bool) -> CredentialMountProposal {
            let source = self.root.path().join(name);
            std::fs::create_dir(&source).unwrap();
            CredentialMountProposal {
                source: source.to_string_lossy().into_owned(),
                target: format!("/credentials/{name}"),
                read_only,
                purpose: CredentialMountPurpose::Generic,
                environment: BTreeMap::new(),
            }
        }
    }

    #[test]
    fn proposal_is_metadata_only_immutable_and_idempotent_without_provisioning() {
        let lab = Lab::new();
        let mut input = lab.input();
        let mount = lab.mount("secret-store", false);
        let canary = uuid::Uuid::new_v4().to_string();
        std::fs::write(Path::new(&mount.source).join("private"), &canary).unwrap();
        input.credential_mounts.push(mount.clone());
        let request = lab.manager.propose(&input, lab.context("thread")).unwrap();
        assert_eq!(request.state, "pending");
        assert_eq!(request.digest.len(), 64);
        assert_eq!(request.spec.model.as_deref(), Some("preserve-model"));
        assert!(!serde_json::to_string(&request).unwrap().contains(&canary));
        assert_eq!(std::fs::read_dir(&mount.source).unwrap().count(), 1);
        assert!(lab
            .manager
            .read(|s| Ok(s.environments.is_empty() && s.bindings.is_empty()))
            .unwrap());
        assert_eq!(
            lab.manager
                .propose(&input, lab.context("thread"))
                .unwrap()
                .id,
            request.id
        );
        input.handoff.push_str(" changed");
        assert!(lab.manager.propose(&input, lab.context("thread")).is_err());
        assert!(lab
            .manager
            .decide_request(&request.id, "wrong", true)
            .is_err());
        assert_eq!(
            lab.manager.get_request(&request.id).unwrap().state,
            "pending"
        );
        assert!(lab
            .manager
            .cancel_request(&request.id, "another-thread")
            .is_err());
    }

    #[test]
    fn approval_survives_restart_but_never_creates_an_environment_or_replays_a_prompt() {
        let lab = Lab::new();
        let request = lab
            .manager
            .propose(&lab.input(), lab.context("thread"))
            .unwrap();
        let approved = lab
            .manager
            .decide_request(&request.id, &request.digest, true)
            .unwrap();
        assert_eq!(approved.state, "waitingIdle");
        assert_eq!(
            lab.manager
                .decide_request(&request.id, &request.digest, true)
                .unwrap()
                .approved_at,
            approved.approved_at
        );
        assert!(lab.manager.request_blocks_turn("thread").unwrap());
        let database = lab.root.path().join("supervisor.sqlite");
        drop(lab.manager);
        let manager = EnvironmentManager::new(&database);
        let restored = manager.get_request(&request.id).unwrap();
        assert_eq!(restored.state, "waitingIdle");
        assert!(!restored.continuation_queued);
        assert!(manager.read(|s| Ok(s.environments.is_empty())).unwrap());
        let cancelled = manager.cancel_request(&request.id, "thread").unwrap();
        assert_eq!(cancelled.state, "cancelled");
        assert!(!manager.request_blocks_turn("thread").unwrap());
        assert_eq!(
            manager.cancel_request(&request.id, "thread").unwrap().state,
            "cancelled"
        );
    }

    #[test]
    fn exact_source_identity_and_writable_grants_are_enforced() {
        let lab = Lab::new();
        let mut input = lab.input();
        input.credential_mounts.push(lab.mount("oauth", false));
        let first = lab.manager.propose(&input, lab.context("one")).unwrap();
        let second = lab.manager.propose(&input, lab.context("two")).unwrap();
        lab.manager
            .decide_request(&first.id, &first.digest, true)
            .unwrap();
        assert!(lab
            .manager
            .decide_request(&second.id, &second.digest, true)
            .is_err());
        lab.manager.cancel_request(&first.id, "one").unwrap();
        lab.manager
            .decide_request(&second.id, &second.digest, true)
            .unwrap();
        let source = Path::new(&input.credential_mounts[0].source);
        std::fs::write(source.join("auth.json"), uuid::Uuid::new_v4().as_bytes()).unwrap();
        let replacement = source.join("auth.new");
        std::fs::write(&replacement, uuid::Uuid::new_v4().as_bytes()).unwrap();
        std::fs::rename(replacement, source.join("auth.json")).unwrap();
        let profile = lab
            .manager
            .read(|s| Ok(s.requests[&second.id].profile.clone()))
            .unwrap();
        assert!(lab.manager.validate_credential_sources(&profile).is_ok());
        std::fs::rename(source, source.with_extension("retired")).unwrap();
        std::fs::create_dir(source).unwrap();
        assert!(lab.manager.validate_credential_sources(&profile).is_err());
    }

    #[test]
    fn native_codex_generated_helper_aliases_survive_reapproval_without_following_host_links() {
        let root = tempfile::tempdir().unwrap();
        let helper = root.path().join("tmp/arg0-linux/codex-arg0Fixture");
        std::fs::create_dir_all(&helper).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                "/opt/pockymoe/harnesses/vendor/codex",
                helper.join("apply_patch"),
            )
            .unwrap();
            assert!(inspect_credential_tree(root.path(), CredentialMountPurpose::Codex).is_ok());
            assert!(inspect_credential_tree(root.path(), CredentialMountPurpose::Generic).is_err());
            std::os::unix::fs::symlink("/proc/self/environ", helper.join("codex-linux-sandbox"))
                .unwrap();
            assert!(inspect_credential_tree(root.path(), CredentialMountPurpose::Codex).is_err());
        }
    }

    #[test]
    fn pending_requests_reject_control_plane_symlinks_nested_targets_and_secret_env() {
        let lab = Lab::new();
        let base = lab.mount("store", true);
        for (source, target, key, value) in [
            (
                lab.manager.directory.to_string_lossy().into_owned(),
                "/credentials/store",
                "",
                "",
            ),
            (base.source.clone(), "/opt/pockymoe/bin/pockymoe", "", ""),
            (base.source.clone(), "/pockymoe/bridge", "", ""),
            (
                base.source.clone(),
                "/credentials/store",
                "OPENAI_API_KEY",
                "not-a-value",
            ),
            (
                base.source.clone(),
                "/credentials/store",
                "GH_CONFIG_DIR",
                "/outside",
            ),
            (base.source.clone(), "/credentials//store", "", ""),
        ] {
            let mut input = lab.input();
            let mut mount = base.clone();
            mount.source = source;
            mount.target = target.into();
            if !key.is_empty() {
                mount.environment.insert(key.into(), value.into());
            }
            input.credential_mounts.push(mount);
            assert!(lab.manager.propose(&input, lab.context("thread")).is_err());
        }
        let mut input = lab.input();
        input.credential_mounts = vec![
            base.clone(),
            CredentialMountProposal {
                target: "/credentials/store/nested".into(),
                ..base.clone()
            },
        ];
        assert!(lab.manager.propose(&input, lab.context("thread")).is_err());
        #[cfg(unix)]
        {
            let alias = lab.root.path().join("alias");
            std::os::unix::fs::symlink(&base.source, &alias).unwrap();
            input.credential_mounts = vec![CredentialMountProposal {
                source: alias.to_string_lossy().into_owned(),
                ..base
            }];
            assert!(lab.manager.propose(&input, lab.context("thread")).is_err());
        }
    }

    #[test]
    fn native_single_file_is_read_only_and_cannot_claim_refresh_or_widen_scope() {
        let lab = Lab::new();
        let source = lab.root.path().join("auth.json");
        std::fs::write(&source, uuid::Uuid::new_v4().as_bytes()).unwrap();
        let mut input = lab.input();
        input.authentication.codex = EnvironmentAuthMode::NativeFile;
        input.egress_hosts = vec!["auth.openai.com".into(), "chatgpt.com".into()];
        input.credential_mounts.push(CredentialMountProposal {
            source: source.to_string_lossy().into_owned(),
            target: "/pockymoe/state/codex/auth.json".into(),
            read_only: false,
            purpose: CredentialMountPurpose::Codex,
            environment: BTreeMap::from([("CODEX_HOME".into(), "/pockymoe/state/codex".into())]),
        });
        assert!(lab.manager.propose(&input, lab.context("thread")).is_err());
        input.credential_mounts[0].read_only = true;
        let request = lab.manager.propose(&input, lab.context("thread")).unwrap();
        assert_eq!(request.spec.credential_mount_kinds, vec!["file"]);
        let replacement = source.with_extension("new");
        std::fs::write(&replacement, uuid::Uuid::new_v4().as_bytes()).unwrap();
        std::fs::rename(replacement, &source).unwrap();
        assert!(lab
            .manager
            .decide_request(&request.id, &request.digest, true)
            .is_err());
    }

    #[test]
    fn a_failed_approved_migration_keeps_host_fenced_until_explicit_owner_recovery() {
        let lab = Lab::new();
        let request = lab
            .manager
            .propose(&lab.input(), lab.context("thread"))
            .unwrap();
        lab.manager
            .decide_request(&request.id, &request.digest, true)
            .unwrap();
        lab.manager
            .begin_request(
                &request.id,
                "bounded original journal and untrusted handoff",
            )
            .unwrap();
        lab.manager
            .fail_request(&request.id, "fixture failure")
            .unwrap();
        assert!(lab.manager.require_fenced_binding("thread").is_err());
        assert!(lab.manager.cancel_request(&request.id, "thread").is_err());
        assert!(lab.manager.request_blocks_turn("thread").unwrap());
        let retry = lab
            .manager
            .retry_request(&request.id, &request.digest)
            .unwrap();
        assert_eq!(retry.state, "waitingIdle");
        assert_eq!(
            lab.manager
                .retry_request(&request.id, &request.digest)
                .unwrap()
                .updated_at,
            retry.updated_at
        );
        lab.manager
            .fail_request(&request.id, "fixture repeated failure")
            .unwrap();
        assert!(lab
            .manager
            .complete_host_recovery(&request.id, "wrong")
            .is_err());
        lab.manager
            .complete_host_recovery(&request.id, &request.digest)
            .unwrap();
        assert!(lab.manager.require_fenced_binding("thread").is_ok());
        assert!(!lab.manager.request_blocks_turn("thread").unwrap());
    }
}
