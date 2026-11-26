//! Owner-authorized source-control hosts. Host tokens never enter DTOs or prompts.
mod hosts;
pub mod setup;
mod watch;
use crate::Supervisor;
use anyhow::{bail, ensure, Context, Result};
pub use hosts::{HostKind, Repository};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
pub use watch::{WatchSnapshot, WatchState};
pub(crate) fn migrate(c: &rusqlite::Connection) -> Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS thread_source_links (thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE, url TEXT NOT NULL, item_json TEXT NOT NULL, PRIMARY KEY(thread_id,url));")?;
    Ok(())
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    hosts: BTreeMap<String, HostSettings>,
    #[serde(default)]
    writing_instructions: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostSettings {
    kind: HostKind,
    token: String,
    enabled: bool,
}
fn settings_path(s: &Supervisor) -> PathBuf {
    crate::upstreams::directory(&s.config.database_url).join("source-control.json")
}
fn load(s: &Supervisor) -> Result<Settings> {
    let path = settings_path(s);
    if !path.exists() {
        return Ok(Settings::default());
    }
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
fn save(s: &Supervisor, value: &Settings) -> Result<()> {
    crate::upstreams::private_write(&settings_path(s), &serde_json::to_vec(value)?)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ItemLink {
    pub url: String,
    pub number: u64,
    pub kind: String,
    pub repository: Repository,
}

impl Supervisor {
    pub fn source_control_settings(&self) -> Result<Value> {
        let settings = load(self)?;
        Ok(
            json!({"hosts":settings.hosts.iter().map(|(host,s)|json!({"host":host,"kind":s.kind,"enabled":s.enabled,"hasToken":!s.token.is_empty()})).collect::<Vec<_>>(),"writingInstructions":settings.writing_instructions}),
        )
    }
    pub fn save_source_control_settings(&self, input: &Value) -> Result<Value> {
        let mut settings = load(self)?;
        if let Some(instructions) = input["writingInstructions"].as_str() {
            ensure!(
                instructions.len() <= 16_000,
                "Writing instructions too long"
            );
            settings.writing_instructions = instructions.into();
        }
        if let Some(host) = input["host"].as_str() {
            hosts::validate_host(host)?;
            let kind: HostKind = serde_json::from_value(input["kind"].clone())?;
            let entry = settings
                .hosts
                .entry(host.to_owned())
                .or_insert(HostSettings {
                    kind,
                    token: String::new(),
                    enabled: true,
                });
            entry.kind = kind;
            if let Some(token) = input["token"].as_str() {
                ensure!(!token.contains(['\n', '\r']), "Invalid token");
                entry.token = token.into();
            }
            if let Some(enabled) = input["enabled"].as_bool() {
                entry.enabled = enabled;
            }
        }
        save(self, &settings)?;
        self.source_control_settings()
    }
    pub fn source_repository(
        &self,
        thread: &str,
        copy: Option<&str>,
    ) -> Result<(PathBuf, Repository)> {
        let t = self.get_thread(thread)?;
        ensure!(
            self.execution_context(thread)?.is_none(),
            "Source-control host access is unavailable inside an isolated environment"
        );
        let root = if let Some(copy) = copy {
            self.worktree_copy(thread, copy)?.path
        } else {
            self.session_cwd(&t)
                .unwrap_or(self.get_workspace(&t.workspace_id)?.abs_path)
        };
        let root = PathBuf::from(root);
        let remote = hosts::git(&root, &["remote", "get-url", "origin"])?;
        let mut repo = Repository::parse(remote.trim())?;
        if let Some(config) = load(self)?.hosts.get(&repo.host) {
            repo.kind = config.kind;
            ensure!(config.enabled, "Source-control host is disabled");
        }
        Ok((root, repo))
    }
    pub async fn source_control_scan(&self, thread: &str) -> Result<Value> {
        let (_, repo) = self.source_repository(thread, None)?;
        let host = self.source_host(&repo)?;
        let authenticated = host
            .request("GET", host.identity_path(), None)
            .await
            .is_ok();
        Ok(
            json!({"repository":repo,"authenticated":authenticated,"settings":self.source_control_settings()?}),
        )
    }
    fn source_host(&self, repo: &Repository) -> Result<Box<dyn hosts::Host>> {
        let settings = load(self)?;
        let token = settings
            .hosts
            .get(&repo.host)
            .filter(|s| s.enabled)
            .map(|s| s.token.clone())
            .unwrap_or_default();
        hosts::adapter(repo.clone(), token)
    }
    pub fn source_links(&self, thread: &str) -> Result<Vec<ItemLink>> {
        self.get_thread(thread)?;
        self.db.with(|c| {
            let mut q = c.prepare(
                "SELECT item_json FROM thread_source_links WHERE thread_id=?1 ORDER BY url",
            )?;
            let rows = q
                .query_map([thread], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(|r| Ok(serde_json::from_str(&r)?))
                .collect()
        })
    }
    pub fn source_link(&self, thread: &str, url: &str, unlink: bool) -> Result<Value> {
        self.get_thread(thread)?;
        let link = hosts::parse_link(url)?;
        self.db.with(|c| {
            if unlink {c.execute("DELETE FROM thread_source_links WHERE thread_id=?1 AND url=?2",rusqlite::params![thread,link.url])?;}
            else {c.execute("INSERT OR REPLACE INTO thread_source_links(thread_id,url,item_json) VALUES(?1,?2,?3)",rusqlite::params![thread,link.url,serde_json::to_string(&link)?])?;}
            Ok(())
        })?;
        Ok(json!({"links":self.source_links(thread)?}))
    }
    /// Parse only output from PR creation tools, never arbitrary transcript prose.
    pub fn source_auto_link(&self, thread: &str, command: &str, output: &str) -> Result<()> {
        if !(command.contains("gh pr create") || command.contains("glab mr create")) {
            return Ok(());
        }
        for word in output.split_whitespace() {
            let word = word.trim_matches(|c: char| matches!(c, '\'' | '"' | '(' | ')' | ','));
            if hosts::parse_link(word).is_ok() {
                self.source_link(thread, word, false)?;
            }
        }
        Ok(())
    }
    pub async fn source_control_action(
        &self,
        thread: &str,
        action: &str,
        mut input: Value,
    ) -> Result<Value> {
        self.get_thread(thread)?;
        match action {
            "publish" => return self.publish_source_repository(thread, &input).await,
            "cleanup" => return self.source_cleanup_suggestions(thread),
            "links" => return Ok(json!({"links":self.source_links(thread)?})),
            "link" | "unlink" => {
                return self.source_link(
                    thread,
                    input["url"].as_str().context("URL required")?,
                    action == "unlink",
                )
            }
            "scan" => return self.source_control_scan(thread).await,
            "unwatch" => return self.source_unwatch(thread),
            "watchStatus" => return self.source_watch_status(thread),
            _ => {}
        }
        let (root, mut repo) = self.source_repository(thread, input["workingCopyId"].as_str())?;
        if let Some(url) = input["url"].as_str() {
            let link = self
                .source_links(thread)?
                .into_iter()
                .find(|l| l.url == url)
                .context("Item is not linked to this thread")?;
            repo = link.repository;
        }
        if let Some(config) = load(self)?.hosts.get(&repo.host) {
            ensure!(config.enabled, "Source-control host is disabled");
            repo.kind = config.kind;
        }
        let host = self.source_host(&repo)?;
        let issue = input["kind"].as_str() == Some("issue");
        let number = input["number"].as_u64();
        if action == "watch" {
            return self
                .source_watch(thread, &repo, number.context("PR number required")?)
                .await;
        }
        if action == "watchStatus" {
            return self.source_watch_status(thread);
        }
        if action == "workOn" {
            return self
                .source_work_on(
                    thread,
                    &root,
                    &repo,
                    number.context("PR number required")?,
                    &input,
                )
                .await;
        }
        if action == "checks" && input["headSha"].as_str().is_none() {
            let detail = host.operation("show", false, number, &json!({}))?;
            let detail = host.request(&detail.method, &detail.path, None).await?;
            input["headSha"] = json!(hosts::head_oid(repo.kind, &detail));
        }
        if action == "create" && !issue && input["head"].as_str().is_none_or(str::is_empty) {
            input["head"] = json!(hosts::git(&root, &["symbolic-ref", "--short", "HEAD"])?.trim());
        }
        if action == "ready" && repo.kind == HostKind::Gitlab && input["title"].as_str().is_none() {
            let op = host.operation("show", false, number, &json!({}))?;
            input["title"] = host.request(&op.method, &op.path, None).await?["title"].clone();
        }
        let operation = host.operation(action, issue, number, &input)?;
        if action == "merge" || action == "autoMerge" {
            let expected = input["expectedHead"]
                .as_str()
                .context("expectedHead required; reload the PR before merging")?;
            let detail = host.operation("show", false, number, &json!({}))?;
            let current = host
                .request(&detail.method, &detail.path, detail.body)
                .await?;
            hosts::guard_head(expected, &hosts::head_oid(repo.kind, &current))?;
        }
        if action == "create" && !issue {
            // API tokens do not replace Git's own transport credentials. A failed
            // push prevents creation and linking, and is returned to the caller.
            if hosts::git(&root, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_err() {
                let branch = hosts::git(&root, &["symbolic-ref", "--short", "HEAD"])?;
                hosts::git(&root, &["push", "--set-upstream", "origin", branch.trim()])?;
            }
        }
        let result = host
            .request(&operation.method, &operation.path, operation.body)
            .await?;
        if action == "create" {
            if let Some(url) = hosts::item_url(repo.kind, &result) {
                self.source_link(thread, url, false)?;
            }
        }
        Ok(json!({"repository":repo,"data":result}))
    }
    async fn source_work_on(
        &self,
        thread: &str,
        root: &Path,
        repo: &Repository,
        number: u64,
        input: &Value,
    ) -> Result<Value> {
        ensure!(
            repo.kind == HostKind::Github
                || repo.kind == HostKind::Gitlab
                || repo.kind == HostKind::Forgejo,
            "Work on PR is unavailable for this host"
        );
        let reference = if repo.kind == HostKind::Gitlab {
            format!("refs/merge-requests/{number}/head")
        } else {
            format!("refs/pull/{number}/head")
        };
        let target = format!("refs/pockymoe/pull-requests/{number}");
        hosts::git(root, &["fetch", "origin", &format!("{reference}:{target}")])?;
        let name = input["name"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("pr-{number}"));
        let created=self.worktree_create(thread,&json!({"name":name,"base":target,"cwd":root,"clientRequestId":input["clientRequestId"]}))?;
        let source = self.get_thread(thread)?;
        let new = self
            .create_thread(pockymoe_protocol::CreateThreadInput {
                workspace_id: source.workspace_id,
                title: Some(format!("PR #{number}")),
                provider: Some(source.provider),
                agent_id: source.agent_id,
                model: source.model.unwrap_or_default(),
                reasoning_effort: source.reasoning_effort,
                approval_mode: source.approval_mode,
                parent_thread_id: None,
            })
            .await?;
        let path = created["workingCopy"]["path"]
            .as_str()
            .context("Created worktree path missing")?;
        let copy = self.worktree_adopt(&new.id, path)?;
        self.worktree_use(&new.id, &copy.id).await?;
        let marker = if repo.kind == HostKind::Gitlab {
            "-/merge_requests"
        } else if repo.kind == HostKind::Forgejo {
            "pulls"
        } else {
            "pull"
        };
        let url = format!(
            "https://{}/{}/{}/{marker}/{number}",
            repo.host, repo.owner, repo.name
        );
        self.source_link(&new.id, &url, false)?;
        Ok(json!({"thread":self.get_thread(&new.id)?,"workingCopy":copy}))
    }
}

impl Supervisor {
    pub async fn rescan_source_control(&self) -> Result<Value> {
        let settings = load(self)?;
        let mut hosts = settings.hosts.keys().cloned().collect::<Vec<_>>();
        for (hostname, _) in hosts::known_hosts() {
            if !hosts.contains(&hostname) {
                hosts.push(hostname);
            }
        }
        for default in ["github.com", "gitlab.com"] {
            if !hosts.iter().any(|h| h == default) {
                hosts.push(default.into());
            }
        }
        let mut found = Vec::new();
        for hostname in hosts {
            let kind = settings
                .hosts
                .get(&hostname)
                .map(|h| h.kind)
                .unwrap_or_else(|| {
                    hosts::known_hosts()
                        .into_iter()
                        .find(|(h, _)| h == &hostname)
                        .map(|(_, k)| k)
                        .unwrap_or(if hostname == "github.com" {
                            HostKind::Github
                        } else {
                            HostKind::Gitlab
                        })
                });
            let enabled = settings.hosts.get(&hostname).is_none_or(|h| h.enabled);
            let repo = Repository {
                host: hostname.clone(),
                owner: String::new(),
                name: String::new(),
                kind,
            };
            let authenticated = if enabled {
                let host = self.source_host(&repo)?;
                host.request("GET", host.identity_path(), None)
                    .await
                    .is_ok()
            } else {
                false
            };
            found.push(json!({"host":hostname,"kind":kind,"enabled":enabled,"authenticated":authenticated,"hasToken":settings.hosts.get(&hostname).is_some_and(|h|!h.token.is_empty())}));
        }
        Ok(json!({"hosts":found,"writingInstructions":settings.writing_instructions}))
    }
    pub fn source_cleanup_suggestions(&self, thread: &str) -> Result<Value> {
        let context = self.worktree_context(thread)?;
        let mut suggestions = Vec::new();
        for copy in context["workingCopies"].as_array().into_iter().flatten() {
            if copy["managed"] != true {
                continue;
            }
            let path = copy["path"].as_str().context("Working-copy path missing")?;
            let branch = copy["branch"].as_str().unwrap_or("");
            if branch.is_empty() {
                continue;
            }
            let users: i64 = self.db.with(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM threads WHERE worktree_path=?1 AND closed_at IS NULL",
                    [path],
                    |r| r.get(0),
                )?)
            })?;
            if users > 0 {
                continue;
            }
            let merged = hosts::git(
                Path::new(path),
                &[
                    "branch",
                    "--merged",
                    "origin/HEAD",
                    "--format=%(refname:short)",
                ],
            )
            .unwrap_or_default();
            if merged.lines().any(|b| b == branch) {
                suggestions.push(json!({"workingCopyId":copy["id"],"path":path,"branch":branch,"reason":"merged"}));
            }
        }
        Ok(json!({"suggestions":suggestions}))
    }
    pub async fn clone_source_workspace(&self, url: &str, label: Option<String>) -> Result<Value> {
        let repo = Repository::parse(url)?;
        crate::interaction::validate_name(&repo.name)?;
        let dev = PathBuf::from(self.workspace_settings().dev_home);
        std::fs::create_dir_all(&dev)?;
        let target = dev.join(&repo.name);
        ensure!(
            !target.exists() && !target.is_symlink(),
            "Clone target already exists"
        );
        // Clone to a private staging directory; failed SSH attempts never leave
        // a half-populated destination that a retry would overwrite.
        let stage = tempfile::tempdir_in(&dev)?;
        let checkout = stage.path().join("checkout");
        let args = vec![
            "clone".into(),
            "--".into(),
            url.into(),
            checkout.to_string_lossy().into_owned(),
        ];
        if hosts::command("git", &args, &dev, None).await.is_err() {
            if checkout.exists() {
                std::fs::remove_dir_all(&checkout)?;
            }
            let https = format!("https://{}/{}/{}.git", repo.host, repo.owner, repo.name);
            hosts::command(
                "git",
                &[
                    "clone".into(),
                    "--".into(),
                    https,
                    checkout.to_string_lossy().into_owned(),
                ],
                &dev,
                None,
            )
            .await?;
        }
        std::fs::rename(checkout, &target)?;
        let workspace = self.create_workspace(pockymoe_protocol::CreateWorkspaceInput {
            abs_path: Some(target.to_string_lossy().into_owned()),
            git_url: None,
            label,
        })?;
        Ok(json!({"workspace":workspace}))
    }
    async fn publish_source_repository(&self, thread: &str, input: &Value) -> Result<Value> {
        let t = self.get_thread(thread)?;
        ensure!(
            self.execution_context(thread)?.is_none(),
            "Publishing is unavailable in an isolated environment"
        );
        let root = PathBuf::from(
            self.session_cwd(&t)
                .unwrap_or(self.get_workspace(&t.workspace_id)?.abs_path),
        );
        ensure!(
            hosts::git(&root, &["remote"])?.trim().is_empty(),
            "Repository already has a remote"
        );
        let hostname = input["host"].as_str().unwrap_or("github.com");
        hosts::validate_host(hostname)?;
        let kind = load(self)?
            .hosts
            .get(hostname)
            .map(|h| h.kind)
            .unwrap_or(HostKind::Github);
        let name = input["name"].as_str().context("Repository name required")?;
        crate::interaction::validate_name(name)?;
        let repo = Repository {
            host: hostname.into(),
            owner: String::new(),
            name: name.into(),
            kind,
        };
        let host = self.source_host(&repo)?;
        let (path, body) = match kind {
            HostKind::Github | HostKind::Forgejo => (
                "user/repos",
                json!({"name":name,"private":input["private"].as_bool().unwrap_or(true)}),
            ),
            HostKind::Gitlab => (
                "projects",
                json!({"name":name,"visibility":if input["private"]==false{"public"}else{"private"}}),
            ),
            _ => bail!("Publishing is unavailable for this host"),
        };
        let result = host.request("POST", path, Some(body)).await?;
        let remote = result["ssh_url"]
            .as_str()
            .or(result["ssh_url_to_repo"].as_str())
            .or(result["clone_url"].as_str())
            .context("Host did not return a clone URL")?;
        Repository::parse(remote)?;
        hosts::git(&root, &["remote", "add", "origin", remote])?;
        let branch = hosts::git(&root, &["symbolic-ref", "--short", "HEAD"])?;
        hosts::git(&root, &["push", "--set-upstream", "origin", branch.trim()])?;
        Ok(json!({"repository":result}))
    }
}
