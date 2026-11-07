//! Runs inside the worker, after owner-approved binds exist. It returns only
//! generic errors, never a credential body or a native CLI's auth diagnostics.
use super::RunRequest;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    fs::File,
    path::{Path, PathBuf},
};

pub(super) fn pointer_variable(name: &str) -> bool {
    matches!(
        name,
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
    )
}

fn read_json(path: &Path, message: &str) -> Result<Value> {
    let bytes = std::fs::read(path).map_err(|_| anyhow::anyhow!("{message}"))?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "credential file exceeds supported size"
    );
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("{message}"))
}
fn text(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.is_empty())
}

fn read_only_filesystem(path: &Path) -> Result<bool> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        ensure!(
            unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } == 0,
            "cannot inspect approved gh directory permissions"
        );
        Ok(unsafe { stat.assume_init() }.f_flag & libc::ST_RDONLY != 0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        anyhow::bail!("approved gh mounts require Linux")
    }
}

fn codex_config(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|_| anyhow::anyhow!("native Codex config is unreadable"))?;
    ensure!(
        raw.len() <= 2 * 1024 * 1024,
        "native Codex config exceeds supported size"
    );
    let value: toml::Value =
        toml::from_str(&raw).map_err(|_| anyhow::anyhow!("native Codex config is invalid"))?;
    let conflict = [
        "model_providers",
        "profiles",
        "profile",
        "openai_base_url",
        "chatgpt_base_url",
        "experimental_bearer_token",
        "http_headers",
        "env_http_headers",
    ]
    .iter()
    .any(|k| value.get(k).is_some());
    ensure!(!conflict && value.get("model_provider").is_none_or(|v|v.as_str()==Some("openai")),"unsupported_native_codex_config: custom providers, profiles, routes or headers conflict with subscription authentication; choose the read-only auth.json slot or review the original configuration");
    ensure!(value.get("cli_auth_credentials_store").is_none_or(|v|matches!(v.as_str(),Some("file"|"auto"))),"unsupported_native_codex_store: OS keyring/ephemeral credentials are not projected; approve an existing file store");
    ensure!(
        value
            .get("forced_login_method")
            .is_none_or(|v| v.as_str() == Some("chatgpt")),
        "unsupported_native_codex_login: the mapped config requires another billing/login mode"
    );
    Ok(())
}

fn native_lock(tool: &str, state: &Path) -> Result<File> {
    #[cfg(unix)]
    {
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(state.join(format!("{tool}-native-session.lock")))?;
        ensure!(unsafe{libc::flock(file.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)}==0,"native_credential_session_in_use: one native session per tool/config directory is supported in this environment; close the existing session before starting another (native in-process subagents remain supported)");
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = tool;
        anyhow::bail!("native credential locks require Linux");
    }
}

pub(super) fn prepare(request: &mut RunRequest) -> Result<Vec<File>> {
    prepare_in(request, Path::new("/pockymoe/state"))
}
fn prepare_in(request: &mut RunRequest, state: &Path) -> Result<Vec<File>> {
    let mut locks = Vec::new();
    let get = |name: &str| {
        request
            .env
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    let codex = get("POCKYMOE_ENV_CODEX_NATIVE").as_deref() == Some("1");
    let claude = get("POCKYMOE_ENV_CLAUDE_NATIVE").as_deref() == Some("1");
    let codex_directory = get("CODEX_HOME");
    let claude_directory = get("CLAUDE_CONFIG_DIR");
    let github_directory = get("GH_CONFIG_DIR");
    if codex {
        locks.push(native_lock("codex", state)?);
        let directory =
            PathBuf::from(codex_directory.context("native Codex config directory is missing")?);
        let auth=read_json(&directory.join("auth.json"),"unsupported_native_codex_store: a readable file-backed ChatGPT login is required; OS keyring credentials are not mounted")?;
        ensure!(text(&auth["tokens"]["access_token"]) && text(&auth["tokens"]["refresh_token"]) && !text(&auth["OPENAI_API_KEY"]),"unsupported_native_codex_auth: require a native ChatGPT file login, not an API-key profile");
        codex_config(&directory.join("config.toml"))?;
        let mut cwd = Some(Path::new(&request.cwd));
        for _ in 0..32 {
            let Some(path) = cwd else { break };
            codex_config(&path.join(".codex/config.toml"))?;
            if path.join(".git").exists() {
                break;
            }
            cwd = path.parent();
        }
        request.env.retain(|(k, _)| {
            !matches!(
                k.as_str(),
                "OPENAI_API_KEY"
                    | "OPENAI_BASE_URL"
                    | "OPENAI_ORG_ID"
                    | "OPENAI_PROJECT_ID"
                    | "CODEX_API_KEY"
                    | "DEFAULT_AUTH_REQUEST"
                    | "CODEX_AUTH_REFRESH_URL_OVERRIDE"
                    | "CODEX_CHATGPT_BASE_URL"
            )
        });
    }
    if claude {
        locks.push(native_lock("claude", state)?);
        let directory =
            PathBuf::from(claude_directory.context("native Claude config directory is missing")?);
        let auth=read_json(&directory.join(".credentials.json"),"unsupported_native_claude_store: Linux .credentials.json is required; macOS Keychain is not projected")?;
        ensure!(
            text(&auth["claudeAiOauth"]["accessToken"]),
            "unsupported_native_claude_auth: require a native subscription file login"
        );
        request.env.retain(|(k, _)| {
            !matches!(
                k.as_str(),
                "ANTHROPIC_API_KEY"
                    | "ANTHROPIC_AUTH_TOKEN"
                    | "CLAUDE_CODE_OAUTH_TOKEN"
                    | "CLAUDE_CODE_OAUTH_TOKEN_FILE"
                    | "CLAUDE_CODE_OAUTH_REFRESH_TOKEN"
                    | "ANTHROPIC_CUSTOM_HEADERS"
                    | "ANTHROPIC_BASE_URL"
            )
        });
    }
    if let Some(directory) = github_directory {
        if read_only_filesystem(Path::new(&directory))? {
            let config = std::fs::read(Path::new(&directory).join("config.yml"))
                .ok()
                .and_then(|bytes| serde_yaml::from_slice::<serde_yaml::Value>(&bytes).ok());
            let migrated = config
                .as_ref()
                .and_then(|c| c.get("version"))
                .is_some_and(|v| v.as_str() == Some("1") || v.as_i64() == Some(1));
            ensure!(migrated, "unsupported_gh_read_only_schema: the pinned gh requires config.yml version 1; migrate the original configuration with gh on Host or request its whole directory read-write. Credentials are never copied to work around read-only permissions");
        }
        let bytes=std::fs::read(Path::new(&directory).join("hosts.yml")).map_err(|_|anyhow::anyhow!("unsupported_gh_store: readable hosts.yml file authentication is required; system credential stores are not projected"))?;
        ensure!(
            bytes.len() <= 1024 * 1024,
            "gh config exceeds supported size"
        );
        let hosts: serde_yaml::Value = serde_yaml::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("gh hosts.yml is invalid"))?;
        let hosts = hosts
            .as_mapping()
            .context("gh hosts.yml must name a file-backed host")?;
        ensure!(!hosts.is_empty(), "gh hosts.yml has no file-backed login");
        let mut config = String::from("[credential]\n\thelper =\n");
        for (host, record) in hosts {
            let host = host
                .as_str()
                .filter(|h| super::super::environment::requests::valid_egress(h))
                .context("unsupported gh host name")?;
            let token = record
                .get("oauth_token")
                .and_then(|v| v.as_str())
                .is_some_and(|v| !v.is_empty())
                || record
                    .get("users")
                    .and_then(|v| v.as_mapping())
                    .is_some_and(|users| {
                        users.values().any(|u| {
                            u.get("oauth_token")
                                .and_then(|v| v.as_str())
                                .is_some_and(|v| !v.is_empty())
                        })
                    });
            ensure!(token,"unsupported_gh_store: this hosts.yml refers to a system credential store; a configuration file alone is not authentication");
            ensure!(record.get("git_protocol").and_then(|v|v.as_str()).is_none_or(|v|v=="https"),"unsupported_gh_git_protocol: SSH requires a separate identity/network/known_hosts adapter");
            config.push_str(&format!(
                "[credential \"https://{host}\"]\n\thelper = !/usr/bin/gh auth git-credential\n"
            ));
        }
        let path = state.join("git-credential-config");
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new_in(state)?;
        file.write_all(config.as_bytes())?;
        file.persist(&path)
            .map_err(|_| anyhow::anyhow!("cannot prepare the isolated gh HTTPS helper"))?;
        request.env.retain(|(k, _)| {
            !matches!(
                k.as_str(),
                "GH_TOKEN"
                    | "GITHUB_TOKEN"
                    | "GH_ENTERPRISE_TOKEN"
                    | "GITHUB_ENTERPRISE_TOKEN"
                    | "GIT_CONFIG_GLOBAL"
                    | "GIT_CONFIG_NOSYSTEM"
            )
        });
        request.env.extend([
            (
                "GIT_CONFIG_GLOBAL".into(),
                path.to_string_lossy().into_owned(),
            ),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("GH_PROMPT_DISABLED".into(), "1".into()),
        ]);
    }
    Ok(locks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn command(directory: &Path, tool: &str) -> RunRequest {
        RunRequest {
            id: uuid::Uuid::new_v4().to_string(),
            argv: vec!["unused".into()],
            stdin_closed: false,
            cwd: directory.to_string_lossy().into(),
            env: vec![
                (
                    format!("POCKYMOE_ENV_{}_NATIVE", tool.to_ascii_uppercase()),
                    "1".into(),
                ),
                (
                    if tool == "codex" {
                        "CODEX_HOME"
                    } else {
                        "CLAUDE_CONFIG_DIR"
                    }
                    .into(),
                    directory.to_string_lossy().into(),
                ),
            ],
        }
    }
    #[test]
    fn native_file_modes_clear_api_precedence_and_keep_refresh_owned_by_the_native_cli() {
        let dir = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let private = uuid::Uuid::new_v4().to_string();
        std::fs::write(dir.path().join("auth.json"),json!({"tokens":{"access_token":private,"refresh_token":private},"OPENAI_API_KEY":null}).to_string()).unwrap();
        let mut codex = command(dir.path(), "codex");
        codex.env.extend([
            ("OPENAI_API_KEY".into(), private.clone()),
            ("OPENAI_BASE_URL".into(), "https://old-api.invalid".into()),
        ]);
        let locks = prepare_in(&mut codex, state.path()).unwrap();
        assert!(!codex
            .env
            .iter()
            .any(|(k, _)| k == "OPENAI_API_KEY" || k == "OPENAI_BASE_URL"));
        assert!(prepare_in(&mut command(dir.path(), "codex"), state.path()).is_err());
        drop(locks);
        let replacement = dir.path().join("replacement");
        std::fs::write(
            &replacement,
            json!({"tokens":{"access_token":private,"refresh_token":private}}).to_string(),
        )
        .unwrap();
        std::fs::rename(replacement, dir.path().join("auth.json")).unwrap();
        assert!(prepare_in(&mut codex, state.path()).is_ok());
        std::fs::write(
            dir.path().join(".credentials.json"),
            json!({"claudeAiOauth":{"accessToken":private}}).to_string(),
        )
        .unwrap();
        let mut claude = command(dir.path(), "claude");
        claude.env.extend([
            ("ANTHROPIC_API_KEY".into(), private.clone()),
            ("ANTHROPIC_AUTH_TOKEN".into(), private.clone()),
            ("CLAUDE_CODE_OAUTH_TOKEN".into(), private.clone()),
        ]);
        assert!(prepare_in(&mut claude, state.path()).is_ok());
        assert!(!claude.env.iter().any(|(_, v)| v == &private));
    }
    #[test]
    fn native_config_never_silently_substitutes_api_or_system_keyring_authentication() {
        let dir = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let private = uuid::Uuid::new_v4().to_string();
        let mut codex = command(dir.path(), "codex");
        let error = prepare_in(&mut codex, state.path())
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("file-backed"));
        std::fs::write(
            dir.path().join("auth.json"),
            json!({"OPENAI_API_KEY":private}).to_string(),
        )
        .unwrap();
        let error = prepare_in(&mut codex, state.path())
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains(&private));
        assert!(error.contains("native ChatGPT"));
        std::fs::write(
            dir.path().join("auth.json"),
            json!({"tokens":{"access_token":private,"refresh_token":private}}).to_string(),
        )
        .unwrap();
        for config in [
            "cli_auth_credentials_store = \"keyring\"",
            "model_provider = \"old-api\"",
            "[model_providers.old]\nbase_url = \"https://api.invalid\"",
            "[profiles.old]\nmodel_provider = \"old-api\"",
        ] {
            std::fs::write(dir.path().join("config.toml"), config).unwrap();
            assert!(prepare_in(&mut codex, state.path()).is_err());
        }
        let mut claude = command(dir.path(), "claude");
        assert!(prepare_in(&mut claude, state.path())
            .err()
            .unwrap()
            .to_string()
            .contains("Keychain"));
    }
    #[test]
    fn gh_requires_file_auth_and_installs_a_secret_free_https_helper_instead_of_forwarding_token_env(
    ) {
        let dir = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let private = uuid::Uuid::new_v4().to_string();
        let mut request = RunRequest {
            id: uuid::Uuid::new_v4().to_string(),
            argv: vec!["unused".into()],
            stdin_closed: false,
            cwd: dir.path().to_string_lossy().into(),
            env: vec![
                ("GH_CONFIG_DIR".into(), dir.path().to_string_lossy().into()),
                ("GH_TOKEN".into(), private.clone()),
            ],
        };
        std::fs::write(
            dir.path().join("hosts.yml"),
            "github.com:\n  user: fixture\n  git_protocol: https\n",
        )
        .unwrap();
        assert!(prepare_in(&mut request, state.path()).is_err());
        std::fs::write(
            dir.path().join("hosts.yml"),
            format!(
                "github.com:\n  user: fixture\n  oauth_token: {private}\n  git_protocol: https\n"
            ),
        )
        .unwrap();
        assert!(prepare_in(&mut request, state.path()).is_ok());
        let helper = std::fs::read_to_string(state.path().join("git-credential-config")).unwrap();
        assert!(!helper.contains(&private));
        assert!(helper.contains("!/usr/bin/gh auth git-credential"));
        assert!(!request.env.iter().any(|(k, _)| k == "GH_TOKEN"));
        std::fs::write(
            dir.path().join("hosts.yml"),
            format!("github.com:\n  oauth_token: {private}\n  git_protocol: ssh\n"),
        )
        .unwrap();
        assert!(prepare_in(&mut request, state.path()).is_err());
    }
}
