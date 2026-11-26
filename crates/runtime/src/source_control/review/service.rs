//! Supervisor glue: which repositories this device knows, the device-wide PR
//! list, and the review actions the Web, the CLI and agents call.
use super::*;
use crate::source_control::hosts::Host;
use crate::Supervisor;
use futures_util::{stream, StreamExt};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// Who is acting. Drafts belong to their author: an agent never sees or sends
/// the owner's pending comments, and the owner's UI never shows an agent's.
#[derive(Clone, Copy, Debug)]
pub enum ReviewCaller<'a> {
    Owner,
    Agent(&'a str),
}
impl ReviewCaller<'_> {
    fn author(&self) -> String {
        match self {
            Self::Owner => "owner".into(),
            Self::Agent(thread) => format!("thread:{thread}"),
        }
    }
}

/// A repository this device can review, with its local checkout when it has one.
#[derive(Clone, Debug)]
pub struct KnownRepository {
    pub repository: Repository,
    pub root: Option<PathBuf>,
    pub remote: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_label: Option<String>,
}

fn same(a: &Repository, b: &Repository) -> bool {
    a.host == b.host
        && a.owner.eq_ignore_ascii_case(&b.owner)
        && a.name.eq_ignore_ascii_case(&b.name)
}

impl Supervisor {
    /// Repositories from every workspace's remotes (forks' upstreams included)
    /// and from linked PRs, with host-kind overrides and disabled hosts applied.
    pub fn review_repositories(&self) -> Result<Vec<KnownRepository>> {
        let settings = crate::source_control::load(self)?;
        let mut found: Vec<KnownRepository> = Vec::new();
        for workspace in self.list_workspaces()? {
            let root = PathBuf::from(&workspace.abs_path);
            let Ok(remotes) = crate::source_control::hosts::git(
                &root,
                &["config", "--get-regexp", "^remote\\..*\\.url$"],
            ) else {
                continue;
            };
            for line in remotes.lines() {
                let Some((key, url)) = line.split_once(' ') else {
                    continue;
                };
                let Ok(repository) = Repository::parse(url.trim()) else {
                    continue;
                };
                if found.iter().any(|k| same(&k.repository, &repository)) {
                    continue;
                }
                let remote = key
                    .strip_prefix("remote.")
                    .and_then(|k| k.strip_suffix(".url"))
                    .map(str::to_owned);
                found.push(KnownRepository {
                    repository,
                    root: Some(root.clone()),
                    remote,
                    workspace_id: Some(workspace.id.clone()),
                    workspace_label: Some(workspace.label.clone()),
                });
            }
        }
        let linked: Vec<String> = self.db.with(|c| {
            let mut q = c.prepare("SELECT DISTINCT item_json FROM thread_source_links")?;
            let rows = q
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(rows)
        })?;
        for raw in linked {
            if let Ok(link) = serde_json::from_str::<crate::source_control::ItemLink>(&raw) {
                if !found.iter().any(|k| same(&k.repository, &link.repository)) {
                    found.push(KnownRepository {
                        repository: link.repository,
                        root: None,
                        remote: None,
                        workspace_id: None,
                        workspace_label: None,
                    });
                }
            }
        }
        found.retain_mut(|known| match settings.hosts.get(&known.repository.host) {
            Some(host) => {
                known.repository.kind = host.kind;
                host.enabled
            }
            None => true,
        });
        Ok(found)
    }

    /// Resolve a PR URL to its repository, applying the device's host settings.
    pub fn review_target(&self, url: &str) -> Result<(Repository, u64)> {
        let link = crate::source_control::hosts::parse_link(url)?;
        ensure!(link.kind == "pr", "Not a pull request URL");
        let mut repo = link.repository;
        if let Some(host) = crate::source_control::load(self)?.hosts.get(&repo.host) {
            ensure!(host.enabled, "Source-control host is disabled");
            repo.kind = host.kind;
        }
        Ok((repo, link.number))
    }

    fn known_repository(&self, repo: &Repository) -> Option<KnownRepository> {
        self.review_repositories()
            .ok()?
            .into_iter()
            .find(|k| same(&k.repository, repo) && k.root.is_some())
    }

    /// Every PR in the device's repositories in one state. Filtering by author,
    /// review, checks and text happens in the page, on this one list.
    pub async fn pull_request_page(&self, state: &str) -> Result<Value> {
        let known = self.review_repositories()?;
        let results: Vec<(KnownRepository, Result<Vec<PullSummary>>)> = stream::iter(known)
            .map(|known| async move {
                let result = async {
                    let host = self.source_host(&known.repository)?;
                    tokio::time::timeout(
                        std::time::Duration::from_secs(45),
                        list(host.as_ref(), &known.repository, state),
                    )
                    .await
                    .context("Host did not answer in time")?
                }
                .await;
                (known, result)
            })
            .buffer_unordered(4)
            .collect()
            .await;
        let mut hosts: Vec<Repository> = Vec::new();
        let mut pulls = Vec::new();
        let mut repositories = Vec::new();
        for (known, result) in results {
            let entry = json!({
                "repository": known.repository,
                "workspaceId": known.workspace_id,
                "workspaceLabel": known.workspace_label,
            });
            match result {
                Ok(list) => {
                    if !hosts.iter().any(|h| h.host == known.repository.host) {
                        hosts.push(known.repository.clone());
                    }
                    let mut entry = entry;
                    entry["count"] = json!(list.len());
                    repositories.push(entry);
                    pulls.extend(list);
                }
                Err(error) => {
                    let mut entry = entry;
                    entry["error"] = json!(bounded(&error.to_string(), 300));
                    repositories.push(entry);
                }
            }
        }
        // Who "me" is on each host, for the author and review-requested filters.
        let viewers: serde_json::Map<String, Value> = stream::iter(hosts)
            .map(|repo| async move {
                let login = async {
                    let host = self.source_host(&repo).ok()?;
                    let value = host
                        .request("GET", &host.identity_path(), None)
                        .await
                        .ok()?;
                    opt_str(&value["login"])
                        .or(opt_str(&value["username"]))
                        .or(opt_str(&value["nickname"]))
                }
                .await;
                (repo.host, login.map(Value::String).unwrap_or(Value::Null))
            })
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect();
        stack::infer(&mut pulls);
        pulls.sort_by(|a: &PullSummary, b| b.updated_at.cmp(&a.updated_at));
        Ok(json!({
            "state": state,
            "pulls": pulls,
            "repositories": repositories,
            "viewers": viewers,
            "fetchedAt": pockymoe_protocol::now_rfc3339(),
        }))
    }

    /// A review action on a PR named by URL (the Web's device-wide surface).
    pub async fn pull_request_review(
        self: &Arc<Self>,
        url: &str,
        action: &str,
        input: &Value,
        caller: ReviewCaller<'_>,
    ) -> Result<Value> {
        let (repo, number) = self.review_target(url)?;
        self.pull_request_review_on(&repo, number, action, input, caller)
            .await
    }

    fn linked_threads(&self, url: &str) -> Result<Vec<Value>> {
        let rows: Vec<(String, String)> = self.db.with(|c| {
            let mut q = c.prepare("SELECT l.thread_id, t.title FROM thread_source_links l JOIN threads t ON t.id=l.thread_id WHERE l.url=?1 AND t.closed_at IS NULL ORDER BY t.updated_at DESC")?;
            let rows = q
                .query_map([url.trim_end_matches('/')], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })?;
        Ok(rows
            .into_iter()
            .map(|(id, title)| json!({"id":id,"title":title}))
            .collect())
    }

    pub async fn pull_request_review_on(
        self: &Arc<Self>,
        repo: &Repository,
        number: u64,
        action: &str,
        input: &Value,
        caller: ReviewCaller<'_>,
    ) -> Result<Value> {
        ensure!(number > 0, "PR number required");
        let key = pr_key(repo, number);
        let author = caller.author();
        // Local-only actions first: no host call, no host sign-in needed.
        match action {
            "drafts" => return Ok(json!({"drafts":store::drafts(&self.db, &key, &author)?})),
            "draftSave" => {
                let draft = store::save_draft(&self.db, &key, &author, input)?;
                return Ok(json!({"draft":draft,"drafts":store::drafts(&self.db, &key, &author)?}));
            }
            "draftDelete" => {
                if input["all"] == true {
                    store::delete_all_drafts(&self.db, &key, &author)?;
                } else {
                    let ids: Vec<String> = input["ids"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(Into::into))
                                .collect()
                        })
                        .or_else(|| input["id"].as_str().map(|id| vec![id.to_owned()]))
                        .context("Draft id required")?;
                    store::delete_drafts(&self.db, &key, &author, &ids)?;
                }
                return Ok(json!({"drafts":store::drafts(&self.db, &key, &author)?}));
            }
            "guideShow" => {
                let head = input["headSha"].as_str().unwrap_or("");
                return guide::state(self, &key, head);
            }
            _ => {}
        }
        let host = self.source_host(repo)?;
        let host = host.as_ref();
        match action {
            "head" => Ok(json!({"headSha": head(host, repo, number).await?})),
            "detail" => {
                let detail = detail(host, repo, number).await?;
                let local = self.known_repository(repo);
                Ok(json!({
                    "guide": guide::state(self, &key, &detail.summary.head_sha)?,
                    "drafts": store::drafts(&self.db, &key, &author)?,
                    "linkedThreads": self.linked_threads(&detail.summary.url)?,
                    "workspace": local.as_ref().map(|k| json!({"id":k.workspace_id,"label":k.workspace_label})),
                    "removeAgentCredits": crate::source_control::load(self)?.remove_agent_credits,
                    "stack": stack::read(host, repo, number).await.ok().flatten(),
                    "detail": detail,
                }))
            }
            "files" => {
                let (head, files, on_host) = self.review_files(host, repo, number, &key).await?;
                Ok(json!({"headSha":head,"files":files,"viewedOnHost":on_host}))
            }
            "viewed" => {
                let paths: Vec<String> = input["paths"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(Into::into))
                            .collect()
                    })
                    .or_else(|| input["path"].as_str().map(|p| vec![p.to_owned()]))
                    .context("Path required")?;
                ensure!(
                    !paths.is_empty() && paths.len() <= 500,
                    "Name 1 to 500 files"
                );
                let viewed = input["viewed"].as_bool().unwrap_or(true);
                if repo.kind == HostKind::Github {
                    let detail = detail(host, repo, number).await?;
                    for path in &paths {
                        set_viewed_native(host, &detail, path, viewed).await?;
                    }
                } else {
                    // Agents and the CLI may name files without revisions: take
                    // each file's current one, so a later push shows it changed.
                    let current = if viewed && !input["revisions"].is_object() {
                        self.review_files(host, repo, number, &key).await?.1
                    } else {
                        Vec::new()
                    };
                    for path in &paths {
                        let revision = input["revisions"][path].as_str().or_else(|| {
                            current
                                .iter()
                                .find(|f| &f.path == path)
                                .map(|f| f.digest.as_str())
                        });
                        ensure!(
                            !viewed || revision.is_some_and(|r| !r.is_empty() && r.len() <= 128),
                            "Revision required to mark a file viewed"
                        );
                        store::set_viewed(&self.db, &key, path, revision.filter(|_| viewed))?;
                    }
                }
                Ok(json!({"ok":true}))
            }
            "submit" => {
                let event = review_event(input["event"].as_str().unwrap_or("comment"))?;
                let expected = input["expectedHead"]
                    .as_str()
                    .context("expectedHead required; load the PR before submitting")?;
                let all = store::drafts(&self.db, &key, &author)?;
                let chosen: Vec<Draft> = match input["draftIds"].as_array() {
                    Some(ids) => all
                        .iter()
                        .filter(|d| d.kind != "summary" && ids.iter().any(|i| i == d.id.as_str()))
                        .cloned()
                        .collect(),
                    None => all
                        .iter()
                        .filter(|d| d.kind != "summary")
                        .cloned()
                        .collect(),
                };
                let summary = all.iter().find(|d| d.kind == "summary");
                let body = input["body"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| summary.map(|d| d.body.clone()))
                    .unwrap_or_default();
                ensure!(body.len() <= store::MAX_BODY, "Review text too long");
                let detail = detail(host, repo, number).await?;
                let submission = Submission {
                    event,
                    body: &body,
                    comments: &chosen,
                    head: expected,
                };
                let review = submit(host, repo, &detail, &submission).await?;
                let mut sent = chosen.clone();
                if let Some(summary) = summary.filter(|d| d.body == body) {
                    sent.push(summary.clone());
                }
                store::clear_submitted(&self.db, &key, &author, &sent)?;
                Ok(json!({"review":review,"drafts":store::drafts(&self.db, &key, &author)?}))
            }
            "reply" | "resolve" => {
                let thread = input["threadId"].as_str().context("threadId required")?;
                let detail = detail(host, repo, number).await?;
                if action == "reply" {
                    let body = input["body"].as_str().unwrap_or("");
                    ensure!(body.len() <= store::MAX_BODY, "Reply too long");
                    reply(host, repo, &detail, thread, body).await
                } else {
                    resolve(host, repo, &detail, thread, input["resolved"] != false).await
                }
            }
            "comment" | "update" => {
                if action == "comment" {
                    let body = input["body"].as_str().unwrap_or("");
                    ensure!(!body.trim().is_empty(), "Comment required");
                    ensure!(body.len() <= store::MAX_BODY, "Comment too long");
                }
                let op = host.operation(action, false, Some(number), input)?;
                host.request(&op.method, &op.path, op.body).await
            }
            "reviewers" => {
                let names = |key: &str| -> Vec<String> {
                    input[key]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| {
                            v.as_str()
                                .map(|s| s.trim().trim_start_matches('@').to_owned())
                        })
                        .filter(|s| !s.is_empty())
                        .collect()
                };
                let (add, remove) = (names("add"), names("remove"));
                ensure!(
                    add.len() + remove.len() <= 25,
                    "At most 25 reviewers per change"
                );
                let detail = detail(host, repo, number).await?;
                reviewers(host, repo, &detail, &add, &remove).await
            }
            "state" => {
                let detail = detail(host, repo, number).await?;
                set_state(host, repo, &detail, input["state"].as_str().unwrap_or("")).await
            }
            "merge" | "autoMerge" | "mergeMessage" => {
                let detail = detail(host, repo, number).await?;
                let method = input["method"]
                    .as_str()
                    .or(detail.merge_methods.first().map(String::as_str))
                    .unwrap_or("squash")
                    .to_owned();
                if action == "mergeMessage" {
                    let (title, body) = merge_message(host, repo, &detail, &method).await?;
                    return Ok(json!({"title":title,"body":body}));
                }
                let expected = input["expectedHead"].as_str().unwrap_or("");
                let merge_input = MergeInput {
                    method: &method,
                    expected_head: expected,
                    remove_credits: input["removeAgentCredits"].as_bool().unwrap_or(false),
                };
                if action == "merge" {
                    merge(host, repo, &detail, &merge_input).await
                } else {
                    auto_merge(host, repo, &detail, &merge_input, input["enable"] != false).await
                }
            }
            "stack" => Ok(json!({"stack": stack::read(host, repo, number).await?})),
            "mergeStack" | "rebaseStack" => {
                let current = stack::read(host, repo, number)
                    .await?
                    .context("This PR is not in a stack")?;
                let expected: Vec<stack::StackHead> = serde_json::from_value(
                    input["expectedHeads"].clone(),
                )
                .context("expectedHeads required: [{number, headSha}] for each open layer")?;
                let local = self.known_repository(repo);
                if action == "rebaseStack" {
                    let local = local.context(
                        "Add a workspace with this repository to this device to rebase a stack",
                    )?;
                    stack::rebase_stack(host, repo, &current, &expected, &local).await
                } else {
                    let method = input["method"].as_str().unwrap_or("merge").to_owned();
                    stack::merge_stack(
                        host,
                        repo,
                        &stack::MergeStack {
                            stack: &current,
                            target: number,
                            method: &method,
                            expected: &expected,
                            local: local.as_ref(),
                        },
                    )
                    .await
                }
            }
            "rerun" => {
                let detail = detail(host, repo, number).await?;
                rerun(host, repo, &detail, input["token"].as_str().unwrap_or("")).await
            }
            "revert" => {
                let detail = detail(host, repo, number).await?;
                revert(host, repo, &detail).await
            }
            "guide" => {
                let detail = detail(host, repo, number).await?;
                let local = self.known_repository(repo).context(
                    "Add a workspace with this repository to this device to write a guide",
                )?;
                guide::start(self, &key, repo, detail, local)
            }
            "link" => {
                let ReviewCaller::Owner = caller else {
                    bail!("Agents link PRs with `pockymoe pr link`");
                };
                let thread = input["threadId"].as_str().context("threadId required")?;
                self.source_link(thread, &repo.pull_request_url(number), false)?;
                Ok(json!({"linkedThreads":self.linked_threads(&repo.pull_request_url(number))?}))
            }
            "workOn" => {
                let ReviewCaller::Owner = caller else {
                    bail!("Agents start PR worktrees with `pockymoe pr work-on`");
                };
                self.pull_request_work_on(repo, number, input).await
            }
            _ => bail!("Unknown pull request action"),
        }
    }

    /// The changed files with viewed state, and the head they belong to. Raw-diff
    /// hosts refresh a PR's diff some time after a push; with a local checkout
    /// the diff is computed from the head itself. Revisions here are the ones
    /// viewed marks are stored with, whichever path produced them.
    async fn review_files(
        &self,
        host: &dyn Host,
        repo: &Repository,
        number: u64,
        key: &str,
    ) -> Result<(String, Vec<PullFile>, bool)> {
        if repo.kind == HostKind::Forgejo {
            if let Some(known) = self.known_repository(repo) {
                let detail = detail(host, repo, number).await?;
                let local = fetch_review_refs(&known, repo, &detail, &format!("files-{number}"))
                    .and_then(|refs| {
                        let diff = git(
                            &refs.root,
                            &[
                                "diff",
                                "--no-ext-diff",
                                "--no-textconv",
                                "--full-index",
                                "-M",
                                &refs.base,
                                &refs.head,
                                "--",
                            ],
                        )?;
                        Ok((refs.head.clone(), files_from_diff(&diff)))
                    });
                if let Ok((head, mut files)) = local {
                    store::apply_viewed(&self.db, key, &mut files)?;
                    return Ok((head, files, false));
                }
            }
        }
        let mut files = files(host, repo, number).await?;
        let on_host = repo.kind == HostKind::Github;
        if !on_host {
            store::apply_viewed(&self.db, key, &mut files)?;
        }
        let head = head(host, repo, number).await.unwrap_or_default();
        Ok((head, files, on_host))
    }

    /// "Work on PR" from the Pull requests page: a new idle thread in the
    /// repository's workspace, on a managed worktree at the PR head.
    async fn pull_request_work_on(
        &self,
        repo: &Repository,
        number: u64,
        input: &Value,
    ) -> Result<Value> {
        let known = self
            .known_repository(repo)
            .context("Add a workspace with this repository to this device first")?;
        let workspace_id = input["workspaceId"]
            .as_str()
            .map(str::to_owned)
            .or(known.workspace_id.clone())
            .context("Workspace required")?;
        let workspace = self.get_workspace(&workspace_id)?;
        let root = PathBuf::from(&workspace.abs_path);
        // The thread copies the workspace's most recent root thread's harness and
        // model, like starting from that workspace's new-thread button.
        let template = self
            .list_threads(Some(&workspace_id), false)?
            .into_iter()
            .find(|t| t.parent_thread_id.is_none());
        let thread = self
            .create_thread(pockymoe_protocol::CreateThreadInput {
                workspace_id: workspace_id.clone(),
                title: Some(format!("PR #{number}")),
                provider: template.as_ref().map(|t| t.provider),
                agent_id: template.as_ref().and_then(|t| t.agent_id.clone()),
                model: template
                    .as_ref()
                    .and_then(|t| t.model.clone())
                    .unwrap_or_default(),
                reasoning_effort: template.as_ref().and_then(|t| t.reasoning_effort.clone()),
                approval_mode: template
                    .as_ref()
                    .map(|t| t.approval_mode.clone())
                    .unwrap_or_else(|| "guarded".into()),
                parent_thread_id: None,
            })
            .await?;
        let result = async {
            let target = self
                .fetch_pull_request_ref(
                    &root,
                    repo,
                    number,
                    &format!("refs/pockymoe/pull-requests/{number}"),
                )
                .await?;
            let name = input["name"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("pr-{number}"));
            let created = self.worktree_create(
                &thread.id,
                &json!({"name":name,"base":target,"cwd":root,"clientRequestId":input["clientRequestId"]}),
            )?;
            let copy = created["workingCopy"]["id"]
                .as_str()
                .context("Created worktree missing")?
                .to_owned();
            self.worktree_use(&thread.id, &copy).await?;
            let url = repo.pull_request_url(number);
            self.source_link(&thread.id, &url, false)?;
            self.db.set_kv(
                &format!("source-worktree:{copy}"),
                &serde_json::to_string(&crate::source_control::hosts::parse_link(&url)?)?,
            )?;
            Ok::<_, anyhow::Error>(created)
        }
        .await;
        match result {
            Ok(created) => Ok(
                json!({"thread":self.get_thread(&thread.id)?,"workingCopy":created["workingCopy"],"setup":created["setup"]}),
            ),
            Err(error) => {
                // Nothing usable was made: do not leave an empty thread behind.
                let _ = self.delete_thread(&thread.id);
                Err(error)
            }
        }
    }

    /// `pockymoe pr review …` for a thread: the PR belongs to the thread's
    /// repository or is linked to the thread, like every other `pr` command.
    pub async fn source_review_action(
        self: &Arc<Self>,
        thread: &str,
        input: &Value,
    ) -> Result<Value> {
        let (_, mut repo) = self.source_repository(thread, input["workingCopyId"].as_str())?;
        let number = input["number"].as_u64().context("PR number required")?;
        if let Some(url) = input["url"].as_str() {
            let link = self
                .source_links(thread)?
                .into_iter()
                .find(|l| l.url == url)
                .context("Item is not linked to this thread")?;
            ensure!(link.kind == "pr", "Not a pull request");
            repo = link.repository;
            if let Some(host) = crate::source_control::load(self)?.hosts.get(&repo.host) {
                ensure!(host.enabled, "Source-control host is disabled");
                repo.kind = host.kind;
            }
        }
        let action = input["reviewAction"]
            .as_str()
            .context("Review action required")?;
        ensure!(
            !matches!(action, "link" | "workOn"),
            "Use `pockymoe pr link` or `pockymoe pr work-on`"
        );
        let caller = if input["agentCaller"] == true {
            ReviewCaller::Agent(thread)
        } else {
            ReviewCaller::Owner
        };
        self.pull_request_review_on(&repo, number, action, input, caller)
            .await
    }

    pub fn pull_request_views(&self) -> Result<Value> {
        Ok(json!({"views":store::views(&self.db)?}))
    }
    pub fn save_pull_request_views(&self, input: &Value) -> Result<Value> {
        Ok(json!({"views":store::save_views(&self.db, &input["views"])?}))
    }
}

/// A PR's head and merge base fetched into hidden refs of the local checkout;
/// the refs go when this is dropped (the objects stay for the next fetch).
pub(crate) struct ReviewRefs {
    pub root: PathBuf,
    pub head: String,
    pub base: String,
    refs: Vec<String>,
}
impl Drop for ReviewRefs {
    fn drop(&mut self) {
        for reference in &self.refs {
            let _ = git(&self.root, &["update-ref", "-d", reference]);
        }
    }
}

pub(crate) fn valid_ref_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.contains("..")
        && !name.contains(|c: char| {
            c.is_whitespace() || matches!(c, ':' | '~' | '^' | '?' | '*' | '[' | '\\')
        })
}

/// Fetch the reviewed head and the base into the local checkout and check the
/// head is the one the host reports. Hosts that refresh their PR refs after a
/// push (Forgejo) are read from the head branch itself, in
/// its own repository for forks, so the diff is never a push behind.
pub(crate) fn fetch_review_refs(
    known: &KnownRepository,
    repo: &Repository,
    detail: &PullDetail,
    tag: &str,
) -> Result<ReviewRefs> {
    let root = known
        .root
        .clone()
        .context("No local checkout of this repository")?;
    let remote = known
        .remote
        .clone()
        .unwrap_or_else(|| format!("https://{}/{}/{}.git", repo.host, repo.owner, repo.name));
    let s = &detail.summary;
    let (head_source, head_ref) = match repo.kind {
        HostKind::Github => (remote.clone(), format!("refs/pull/{}/head", s.number)),
        HostKind::Gitlab => (
            remote.clone(),
            format!("refs/merge-requests/{}/head", s.number),
        ),
        _ => (
            detail.extra["headClone"]
                .as_str()
                .filter(|url| Repository::parse(url).is_ok())
                .map(str::to_owned)
                .unwrap_or_else(|| remote.clone()),
            format!("refs/heads/{}", s.head_ref),
        ),
    };
    ensure!(
        valid_ref_name(&head_ref) && valid_ref_name(&s.base_ref),
        "Unusual branch names; review this PR on the host"
    );
    let head_target = format!("refs/pockymoe/review/{tag}/head");
    let base_target = format!("refs/pockymoe/review/{tag}/base");
    let mut refs = ReviewRefs {
        root: root.clone(),
        head: String::new(),
        base: String::new(),
        refs: vec![head_target.clone(), base_target.clone()],
    };
    git(
        &root,
        &[
            "fetch",
            "--no-tags",
            "--quiet",
            &head_source,
            &format!("+{head_ref}:{head_target}"),
        ],
    )?;
    git(
        &root,
        &[
            "fetch",
            "--no-tags",
            "--quiet",
            &remote,
            &format!("+refs/heads/{}:{base_target}", s.base_ref),
        ],
    )?;
    refs.head = git(
        &root,
        &[
            "rev-parse",
            "--verify",
            &format!("{head_target}^{{commit}}"),
        ],
    )?
    .trim()
    .to_owned();
    ensure!(
        refs.head == s.head_sha,
        "The host is still updating this PR after a push; try again in a moment"
    );
    refs.base = git(&root, &["merge-base", &base_target, &head_target])?
        .trim()
        .to_owned();
    Ok(refs)
}

/// Git in a review checkout: same bounded, prompt-free runner as host commands.
pub(crate) fn git(root: &Path, args: &[&str]) -> Result<String> {
    crate::source_control::hosts::git(root, args)
}
