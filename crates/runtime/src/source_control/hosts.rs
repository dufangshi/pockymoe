use super::*;
use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const LIMIT: u64 = 4 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostKind {
    Github,
    Gitlab,
    Forgejo,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub host: String,
    pub owner: String,
    pub name: String,
    pub kind: HostKind,
}
pub fn validate_host(host: &str) -> Result<()> {
    let hostname = if let Some((name, port)) = host.rsplit_once(':') {
        ensure!(port.parse::<u16>().is_ok(), "Invalid host port");
        name
    } else {
        host
    };
    ensure!(
        !hostname.is_empty()
            && hostname.len() < 254
            && hostname
                .split('.')
                .all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')),
        "Invalid source-control hostname"
    );
    Ok(())
}
fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
impl Repository {
    pub fn parse(remote: &str) -> Result<Self> {
        let url = if let Some((host, path)) =
            remote.strip_prefix("git@").and_then(|s| s.split_once(':'))
        {
            format!("https://{host}/{path}")
        } else {
            remote.to_owned()
        };
        let url = url::Url::parse(&url).context("Unsupported Git remote URL")?;
        let loopback = url.host_str().is_some_and(|h| {
            h == "localhost"
                || h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        ensure!(
            (matches!(url.scheme(), "https" | "ssh") || (url.scheme() == "http" && loopback))
                && url.password().is_none()
                && (url.scheme() == "ssh" || url.username().is_empty()),
            "Use an HTTPS or SSH remote without embedded credentials"
        );
        let hostname = url
            .host_str()
            .context("Remote hostname required")?
            .to_lowercase();
        let host = if let Some(port) = url.port() {
            format!("{hostname}:{port}")
        } else {
            hostname
        };
        validate_host(&host)?;
        let parts: Vec<_> = url
            .path()
            .trim_matches('/')
            .trim_end_matches(".git")
            .split('/')
            .collect();
        ensure!(
            parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && *p != ".."),
            "Remote must name an owner and repository"
        );
        // Bitbucket and Azure DevOps are not supported; never guess another kind for them.
        ensure!(
            !(host == "bitbucket.org"
                || host == "dev.azure.com"
                || host == "ssh.dev.azure.com"
                || host.ends_with(".visualstudio.com")),
            "Bitbucket and Azure DevOps are not supported"
        );
        let kind = if host == "github.com" {
            HostKind::Github
        } else if host.contains("gitlab") {
            HostKind::Gitlab
        } else {
            known_hosts()
                .into_iter()
                .find(|(name, _)| name == &host)
                .map(|(_, kind)| kind)
                .unwrap_or(HostKind::Forgejo)
        };
        Ok(Self {
            host,
            owner: parts[..parts.len() - 1].join("/"),
            name: parts[parts.len() - 1].into(),
            kind,
        })
    }
    pub fn pull_request_url(&self, number: u64) -> String {
        let marker = match self.kind {
            HostKind::Gitlab => "-/merge_requests",
            HostKind::Forgejo => "pulls",
            HostKind::Github => "pull",
        };
        let scheme = if self.host.starts_with("127.0.0.1:") || self.host.starts_with("localhost:") {
            "http"
        } else {
            "https"
        };
        format!(
            "{scheme}://{}/{}/{}/{marker}/{number}",
            self.host, self.owner, self.name
        )
    }
    fn project(&self) -> String {
        enc(&format!("{}/{}", self.owner, self.name))
    }
}
pub fn parse_link(url: &str) -> Result<ItemLink> {
    let parsed = url::Url::parse(url)?;
    let loopback = parsed.host_str().is_some_and(|h| {
        h == "localhost"
            || h.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    ensure!(
        (parsed.scheme() == "https" || (parsed.scheme() == "http" && loopback))
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && parsed.username().is_empty(),
        "Expected a clean HTTPS PR or issue URL"
    );
    let path: Vec<_> = parsed.path().trim_matches('/').split('/').collect();
    ensure!(path.len() >= 4, "Invalid PR or issue URL");
    let number = path.last().unwrap().parse::<u64>()?;
    ensure!(number > 0, "Invalid item number");
    let marker = path[path.len() - 2];
    let kind = match marker {
        "pull" | "pulls" | "pull-requests" | "merge_requests" | "pullrequest" => "pr",
        "issues" => "issue",
        _ => bail!("Unsupported item URL"),
    };
    let end = if path.get(path.len() - 3) == Some(&"-") {
        path.len() - 3
    } else {
        path.len() - 2
    };
    let base = format!(
        "{}://{}/{}",
        parsed.scheme(),
        parsed.authority(),
        path[..end].join("/")
    );
    let mut repository = Repository::parse(&base)?;
    if marker == "merge_requests" {
        repository.kind = HostKind::Gitlab;
    }
    Ok(ItemLink {
        url: url.trim_end_matches('/').into(),
        number,
        kind: kind.into(),
        repository,
    })
}
pub fn guard_head(expected: &str, actual: &str) -> Result<()> {
    ensure!(
        !expected.is_empty() && expected == actual,
        "conflict: PR head moved; reload before merging"
    );
    Ok(())
}
pub fn head_oid(kind: HostKind, v: &Value) -> String {
    match kind {
        HostKind::Github | HostKind::Forgejo => v["head"]["sha"].as_str(),
        HostKind::Gitlab => v["sha"].as_str(),
    }
    .unwrap_or("")
    .into()
}
pub fn item_url(kind: HostKind, v: &Value) -> Option<&str> {
    match kind {
        HostKind::Github | HostKind::Forgejo => v["html_url"].as_str(),
        HostKind::Gitlab => v["web_url"].as_str(),
    }
}

pub struct Operation {
    pub method: String,
    pub path: String,
    pub body: Option<Value>,
}
fn op(method: &str, path: String, body: Option<Value>) -> Result<Operation> {
    Ok(Operation {
        method: method.into(),
        path,
        body,
    })
}
#[async_trait]
pub trait Host: Send + Sync {
    fn identity_path(&self) -> String;
    fn operation(
        &self,
        action: &str,
        issue: bool,
        number: Option<u64>,
        input: &Value,
    ) -> Result<Operation>;
    async fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value>;
    /// A plain-text response (raw diffs). Hosts without one refuse.
    async fn request_text(&self, _path: &str) -> Result<String> {
        bail!("Raw host responses are unavailable for this host")
    }
}
struct Adapter {
    repo: Repository,
    token: String,
}
pub fn adapter(repo: Repository, token: String) -> Result<Box<dyn Host>> {
    let token = if token.is_empty() {
        let key = match repo.kind {
            HostKind::Github if repo.host == "github.com" => "GH_TOKEN",
            HostKind::Github => "GH_ENTERPRISE_TOKEN",
            HostKind::Gitlab => "GITLAB_TOKEN",
            HostKind::Forgejo => "GITEA_TOKEN",
        };
        std::env::var(key).unwrap_or_default()
    } else {
        token
    };
    Ok(Box::new(Adapter { repo, token }))
}
#[async_trait]
impl Host for Adapter {
    fn identity_path(&self) -> String {
        "user".into()
    }

    fn operation(
        &self,
        action: &str,
        issue: bool,
        number: Option<u64>,
        input: &Value,
    ) -> Result<Operation> {
        let r = &self.repo;
        let n = number.unwrap_or(0);
        if !matches!(action, "list" | "create") {
            ensure!(n > 0, "Item number required");
        }
        let method = input["method"].as_str().unwrap_or("squash");
        ensure!(
            ["merge", "squash", "rebase"].contains(&method),
            "Invalid merge method"
        );
        let title = input["title"].as_str().unwrap_or("");
        let body = input["body"].as_str().unwrap_or("");
        let head = input["head"].as_str().unwrap_or("");
        let base = input["base"].as_str().unwrap_or("main");
        let draft = input["draft"].as_bool().unwrap_or(false);
        if action == "create" {
            ensure!(!title.trim().is_empty(), "Title required");
            if !issue {
                ensure!(!head.is_empty(), "Head branch required");
            }
        }
        match r.kind {
            HostKind::Github | HostKind::Forgejo => {
                let root = format!(
                    "repos/{}/{}/{}",
                    enc(&r.owner),
                    enc(&r.name),
                    if issue { "issues" } else { "pulls" }
                );
                let item = format!("{root}/{n}");
                match action {
                    "list"
                        if r.kind == HostKind::Github
                            && input["search"].as_str().is_some_and(|s| !s.is_empty()) =>
                    {
                        op(
                            "GET",
                            format!(
                                "search/issues?q={}",
                                enc(&format!(
                                    "repo:{}/{} is:{} {}",
                                    r.owner,
                                    r.name,
                                    if issue { "issue" } else { "pr" },
                                    input["search"].as_str().unwrap_or("")
                                ))
                            ),
                            None,
                        )
                    }
                    "list" => op("GET", format!("{root}?state=open&per_page=100"), None),
                    "show" => op("GET", item, None),
                    "checks" if r.kind == HostKind::Forgejo => {
                        let sha = input["headSha"]
                            .as_str()
                            .context("headSha required for checks")?;
                        op(
                            "GET",
                            format!(
                                "repos/{}/{}/commits/{}/status",
                                enc(&r.owner),
                                enc(&r.name),
                                enc(sha)
                            ),
                            None,
                        )
                    }
                    "checks" => {
                        let sha = input["headSha"]
                            .as_str()
                            .context("headSha required for checks")?;
                        op(
                            "GET",
                            format!(
                                "repos/{}/{}/commits/{}/check-runs",
                                enc(&r.owner),
                                enc(&r.name),
                                enc(sha)
                            ),
                            None,
                        )
                    }
                    "comments" => op(
                        "GET",
                        format!(
                            "repos/{}/{}/issues/{n}/comments",
                            enc(&r.owner),
                            enc(&r.name)
                        ),
                        None,
                    ),
                    "reviews" | "commits" | "files" => {
                        op("GET", format!("{item}/{action}?per_page=100"), None)
                    }
                    "create" => op(
                        "POST",
                        root,
                        Some(if issue {
                            json!({"title":title,"body":body})
                        } else {
                            json!({"title":if r.kind==HostKind::Forgejo&&draft {format!("WIP: {title}")}else{title.into()},"body":body,"head":head,"base":base,"draft":draft})
                        }),
                    ),
                    "update" => {
                        let mut fields = json!({});
                        for k in ["title", "body"] {
                            if input.get(k).is_some() {
                                fields[k] = input[k].clone();
                            }
                        }
                        op("PATCH", item, Some(fields))
                    }
                    "comment" => {
                        ensure!(!body.trim().is_empty(), "Comment required");
                        op(
                            "POST",
                            format!(
                                "repos/{}/{}/issues/{n}/comments",
                                enc(&r.owner),
                                enc(&r.name)
                            ),
                            Some(json!({"body":body})),
                        )
                    }
                    "ready" if r.kind == HostKind::Forgejo => op(
                        "PATCH",
                        item,
                        Some(
                            json!({"title":title.trim_start_matches("WIP: ").trim_start_matches("[WIP] ")}),
                        ),
                    ),
                    "autoMerge" if r.kind == HostKind::Forgejo => op(
                        "POST",
                        format!("{item}/merge"),
                        Some(
                            json!({"Do":method,"head_commit_id":input["expectedHead"],"merge_when_checks_succeed":true}),
                        ),
                    ),
                    "merge" if r.kind == HostKind::Forgejo => op(
                        "POST",
                        format!("{item}/merge"),
                        Some(json!({"Do":method,"head_commit_id":input["expectedHead"]})),
                    ),
                    "merge" => op(
                        "PUT",
                        format!("{item}/merge"),
                        Some(json!({"merge_method":method,"sha":input["expectedHead"]})),
                    ),
                    "ready" | "autoMerge" if r.kind == HostKind::Github => {
                        let node = input["nodeId"]
                            .as_str()
                            .context("nodeId required from PR detail")?;
                        let (mutation, fields) = if action == "ready" {
                            (
                                "markPullRequestReadyForReview",
                                format!("pullRequestId: {}", serde_json::to_string(node)?),
                            )
                        } else {
                            (
                                "enablePullRequestAutoMerge",
                                format!(
                                    "pullRequestId: {}, mergeMethod: {}",
                                    serde_json::to_string(node)?,
                                    method.to_uppercase()
                                ),
                            )
                        };
                        op(
                            "POST",
                            "graphql".into(),
                            Some(
                                json!({"query":format!("mutation {{ {mutation}(input: {{ {fields} }}) {{ clientMutationId }} }}")}),
                            ),
                        )
                    }
                    _ => bail!("Operation unavailable for this host"),
                }
            }
            HostKind::Gitlab => {
                ensure!(
                    !matches!(action, "merge" | "autoMerge") || method != "rebase",
                    "GitLab merge strategy follows project settings; choose merge or squash"
                );
                let root = format!(
                    "projects/{}/{}",
                    r.project(),
                    if issue { "issues" } else { "merge_requests" }
                );
                let item = format!("{root}/{n}");
                match action {
                    "list" => op(
                        "GET",
                        format!(
                            "{root}?state=opened&per_page=100&search={}",
                            enc(input["search"].as_str().unwrap_or(""))
                        ),
                        None,
                    ),
                    "show" => op("GET", item, None),
                    "comments" => op("GET", format!("{item}/notes?per_page=100"), None),
                    "checks" => op("GET", format!("{item}/pipelines"), None),
                    "reviews" => op("GET", format!("{item}/approvals"), None),
                    "files" => op("GET", format!("{item}/diffs"), None),
                    "commits" => op("GET", format!("{item}/commits"), None),
                    "create" => op(
                        "POST",
                        root,
                        Some(if issue {
                            json!({"title":title,"description":body})
                        } else {
                            json!({"title":if draft{format!("Draft: {title}")}else{title.into()},"description":body,"source_branch":head,"target_branch":base})
                        }),
                    ),
                    "update" | "ready" => {
                        let mut fields = json!({});
                        if let Some(t) = input["title"].as_str() {
                            fields["title"] = json!(if action == "ready" {
                                t.trim_start_matches("Draft: ")
                            } else {
                                t
                            });
                        }
                        if input.get("body").is_some() {
                            fields["description"] = input["body"].clone();
                        }
                        op("PUT", item, Some(fields))
                    }
                    "comment" => {
                        ensure!(!body.trim().is_empty(), "Comment required");
                        op("POST", format!("{item}/notes"), Some(json!({"body":body})))
                    }
                    "merge" | "autoMerge" => op(
                        "PUT",
                        format!("{item}/merge"),
                        Some(
                            json!({"sha":input["expectedHead"],"squash":method=="squash","auto_merge":action=="autoMerge"}),
                        ),
                    ),
                    _ => bail!("Operation unavailable for GitLab"),
                }
            }
        }
    }
    async fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let text = self.raw(method, path, body).await?;
        if text.trim().is_empty() {
            Ok(json!({}))
        } else {
            Ok(serde_json::from_str(&text)?)
        }
    }
    async fn request_text(&self, path: &str) -> Result<String> {
        self.raw("GET", path, None).await
    }
}
impl Adapter {
    async fn raw(&self, method: &str, path: &str, body: Option<Value>) -> Result<String> {
        ensure!(
            !path.starts_with('/') && !path.contains("://"),
            "Invalid host API path"
        );
        if self.token.is_empty() && matches!(self.repo.kind, HostKind::Github | HostKind::Gitlab) {
            let (program, args) = if self.repo.kind == HostKind::Github {
                (
                    "gh",
                    vec![
                        "api",
                        "--hostname",
                        self.repo.host.as_str(),
                        "--method",
                        method,
                        path,
                    ],
                )
            } else {
                (
                    "glab",
                    vec![
                        "api",
                        "--hostname",
                        self.repo.host.as_str(),
                        "--method",
                        method,
                        path,
                    ],
                )
            };
            let mut args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
            if body.is_some() {
                args.extend(["--input".into(), "-".into()]);
            }
            return command(
                program,
                &args,
                Path::new("/"),
                body.map(|b| serde_json::to_vec(&b)).transpose()?,
            )
            .await;
        }
        ensure!(
            !self.token.is_empty(),
            "Sign in to the source-control host in Settings"
        );
        // Loopback hosts (throwaway local Forgejo or GitLab) speak plain HTTP.
        let scheme = if self.repo.host.split(':').next().is_some_and(|h| {
            h == "localhost"
                || h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }) {
            "http"
        } else {
            "https"
        };
        let base = match self.repo.kind {
            HostKind::Github if self.repo.host == "github.com" => "https://api.github.com".into(),
            HostKind::Github if path == "graphql" => format!("https://{}/api", self.repo.host),
            HostKind::Github => format!("https://{}/api/v3", self.repo.host),
            HostKind::Gitlab if path == "graphql" => format!("{scheme}://{}/api", self.repo.host),
            HostKind::Gitlab => format!("{scheme}://{}/api/v4", self.repo.host),
            HostKind::Forgejo => format!("{scheme}://{}/api/v1", self.repo.host),
        };
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let mut req = client
            .request(method.parse()?, format!("{base}/{path}"))
            .header("User-Agent", "Pockymoe")
            .bearer_auth(&self.token);
        if self.repo.kind == HostKind::Gitlab {
            req = req.header("PRIVATE-TOKEN", &self.token);
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let mut response = req
            .send()
            .await
            .context("Source-control host request failed")?;
        ensure!(
            response.status().is_success(),
            "Source-control host returned HTTP {}",
            response.status().as_u16()
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= LIMIT as usize,
                "Host response too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}
/// No shell interpolation, no credential prompts, bounded streams and deadline.
pub async fn command(
    program: &str,
    args: &[String],
    root: &Path,
    input: Option<Vec<u8>>,
) -> Result<String> {
    let (status, out, _) = command_capture(program, args, root, input).await?;
    // Host error bodies can echo tokens or private URLs. Return the status only.
    ensure!(
        status.success(),
        "Source-control command failed ({status}); check host sign-in and repository permissions"
    );
    Ok(out)
}
/// Repository setup explicitly returns its command logs to its owner. Host API
/// callers use `command`, which keeps error bodies out of responses.
pub(super) async fn command_capture(
    program: &str,
    args: &[String],
    root: &Path,
    input: Option<Vec<u8>>,
) -> Result<(std::process::ExitStatus, String, String)> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for key in crate::child_process::inherited_host_identity() {
        cmd.env_remove(key);
    }
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
    ] {
        cmd.env_remove(key);
    }
    let mut child = cmd.spawn().context("Source-control command unavailable")?;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(120), async move {
        let write = async move {
            if let Some(input) = input {
                stdin.write_all(&input).await?;
            }
            drop(stdin);
            Ok::<_, std::io::Error>(())
        };
        let read = async move {
            let mut data = Vec::new();
            stdout.take(LIMIT + 1).read_to_end(&mut data).await?;
            Ok::<_, std::io::Error>(data)
        };
        let err = async move {
            let mut data = Vec::new();
            stderr.take(LIMIT + 1).read_to_end(&mut data).await?;
            Ok::<_, std::io::Error>(data)
        };
        let (_, out, err) = tokio::try_join!(write, read, err)?;
        ensure!(
            out.len() <= LIMIT as usize && err.len() <= LIMIT as usize,
            "Source-control output too large"
        );
        let status = child.wait().await?;
        Ok((
            status,
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
        ))
    })
    .await
    .context("Source-control command timed out")?
}
pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    // Called from blocking worktree APIs as well as async operations. A dedicated
    // worker avoids nesting a Tokio runtime on a Supervisor executor thread.
    let root = root.to_owned();
    let args = [
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.hooksPath=",
        "-c",
        "core.pager=cat",
    ]
    .into_iter()
    .chain(args.iter().copied())
    .map(str::to_owned)
    .collect::<Vec<_>>();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(command("git", &args, &root, None))
    })
    .join()
    .map_err(|_| anyhow::anyhow!("Git worker failed"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_remotes_and_links() {
        for (url, kind, owner) in [
            (
                "git@github.com:dufangshi/pockymoe.git",
                HostKind::Github,
                "dufangshi",
            ),
            (
                "https://gitlab.com/group/sub/repo.git",
                HostKind::Gitlab,
                "group/sub",
            ),
            (
                "https://codeberg.org/owner/repo",
                HostKind::Forgejo,
                "owner",
            ),
        ] {
            let r = Repository::parse(url).unwrap();
            assert_eq!(r.kind, kind);
            assert_eq!(r.owner, owner);
        }
        let l =
            parse_link("https://gitlab.example.com/group/sub/repo/-/merge_requests/42").unwrap();
        assert_eq!(l.repository.owner, "group/sub");
        assert_eq!(l.number, 42);
        for url in [
            "file:///tmp/repo",
            "https://secret@github.com/a/b/pull/1",
            "http://github.com/a/b/pull/1",
            "https://github.com/a/b/pull/0",
            "https://bitbucket.org/team/repo/pull-requests/1",
            "https://dev.azure.com/org/project/_git/repo/pullrequest/1",
        ] {
            assert!(parse_link(url).is_err(), "{url}");
        }
        // Unsupported hosts are refused, never guessed as another kind.
        for remote in [
            "https://bitbucket.org/team/repo",
            "git@ssh.dev.azure.com:v3/org/project/repo",
            "https://dev.azure.com/org/project/_git/repo",
            "https://org.visualstudio.com/project/_git/repo",
        ] {
            assert!(Repository::parse(remote).is_err(), "{remote}");
        }
    }
    #[test]
    fn forgejo_drafts_and_guards() {
        let host = adapter(
            Repository {
                host: "forgejo.example.com".into(),
                owner: "o".into(),
                name: "r".into(),
                kind: HostKind::Forgejo,
            },
            String::new(),
        )
        .unwrap();
        let create = host
            .operation(
                "create",
                false,
                None,
                &json!({"title":"Change","head":"feature","draft":true}),
            )
            .unwrap();
        assert_eq!(create.body.unwrap()["title"], "WIP: Change");
        let auto = host
            .operation(
                "autoMerge",
                false,
                Some(42),
                &json!({"expectedHead":"reviewed"}),
            )
            .unwrap();
        assert_eq!(auto.body.unwrap()["head_commit_id"], "reviewed");
    }
    #[test]
    fn operations_guard_merges_and_preserve_host_fields() {
        assert!(guard_head("abc", "def").is_err());
        assert!(guard_head("abc", "abc").is_ok());
        assert!(guard_head("", "").is_err());
        for (kind, json, oid) in [
            (HostKind::Github, json!({"head":{"sha":"a"}}), "a"),
            (HostKind::Gitlab, json!({"sha":"b"}), "b"),
            (HostKind::Forgejo, json!({"head":{"sha":"c"}}), "c"),
        ] {
            assert_eq!(head_oid(kind, &json), oid);
        }
        for kind in [HostKind::Github, HostKind::Gitlab, HostKind::Forgejo] {
            let host = adapter(
                Repository {
                    host: "example.com".into(),
                    owner: "o".into(),
                    name: "r".into(),
                    kind,
                },
                String::new(),
            )
            .unwrap();
            let op = host
                .operation(
                    "merge",
                    false,
                    Some(1),
                    &json!({"expectedHead":"abc","method":"squash"}),
                )
                .unwrap();
            let body = op.body.unwrap();
            assert_eq!(
                body[if kind == HostKind::Forgejo {
                    "head_commit_id"
                } else {
                    "sha"
                }],
                "abc"
            );
            assert!(host.operation("merge", false, None, &json!({})).is_err());
        }
    }
}

pub fn known_hosts() -> Vec<(String, HostKind)> {
    let mut hosts = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        for (path, kind, nested) in [
            (".config/gh/hosts.yml", HostKind::Github, false),
            (".config/glab-cli/config.yml", HostKind::Gitlab, true),
        ] {
            if let Ok(bytes) = std::fs::read(Path::new(&home).join(path)) {
                if bytes.len() > 1024 * 1024 {
                    continue;
                }
                if let Ok(mut value) = serde_yaml::from_slice::<serde_yaml::Value>(&bytes) {
                    if nested {
                        value = value.get("hosts").cloned().unwrap_or_default();
                    }
                    if let Some(map) = value.as_mapping() {
                        for key in map.keys().filter_map(|k| k.as_str()) {
                            if validate_host(key).is_ok() {
                                hosts.push((key.into(), kind));
                            }
                        }
                    }
                }
            }
        }
    }
    if let Ok(host) = std::env::var("GH_HOST") {
        if validate_host(&host).is_ok() {
            hosts.push((host, HostKind::Github));
        }
    }
    hosts
}
